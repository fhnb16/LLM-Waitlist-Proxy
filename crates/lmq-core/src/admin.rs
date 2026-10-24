//! Сервисный слой панели. Используется admin HTTP API (веб-панель) и нативным окном
//! напрямую, без HTTP.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::app::{App, BoundAddrs, VERSION};
use crate::config::{self, ClientCfg, Settings, hash_key};
use crate::crypto;
use crate::engines::EngineStatus;
use crate::gpu::GpuState;
use crate::metrics::esc;
use crate::queue::RunningJob;
use crate::routing::HealthSnapshot;
use crate::schedule::ScheduleNow;
use crate::stats::Point;
use crate::submit::{self, SubmitOpts, Submitted};
use crate::util::{now_ms, random_token};

/// Маска секретов в ответах панели. Если UI присылает её обратно — значение не меняется.
pub const MASK: &str = "••••••••";

#[derive(Clone, Debug, Serialize)]
pub struct UpstreamOverview {
    pub id: String,
    pub url: String,
    pub target_model: String,
    pub slots: u32,
    pub busy: u32,
    pub engine: Option<String>,
    pub engine_status: Option<EngineStatus>,
    pub usable: bool,
    pub health: HealthSnapshot,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelOverview {
    pub name: String,
    pub description: String,
    pub queued: usize,
    pub running: u32,
    pub slots: u32,
    pub busy: u32,
    pub available: bool,
    pub fallback: Vec<String>,
    pub aliases: Vec<String>,
    pub upstreams: Vec<UpstreamOverview>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EngineBrief {
    pub id: String,
    pub name: String,
    pub status: EngineStatus,
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Overview {
    pub version: &'static str,
    pub started_at: i64,
    pub uptime_s: i64,
    pub now: i64,
    pub paused: bool,
    pub profile: String,
    pub queued: usize,
    pub running: usize,
    pub counts: BTreeMap<String, i64>,
    pub models: Vec<ModelOverview>,
    pub running_jobs: Vec<RunningJob>,
    pub engines: Vec<EngineBrief>,
    pub gpu: GpuState,
    pub schedule: ScheduleNow,
    pub bound: BoundAddrs,
    pub clients: usize,
    pub db_ok: bool,
    pub last_5m: Point,
    pub last_1h: Point,
    /// Реле Telegram, если включено.
    pub tg_relay: Option<RelayOverview>,
    /// SOCKS5-прокси, если включён.
    pub socks: Option<SocksOverview>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SocksOverview {
    /// Что вписать программам: `socks5h://адрес:порт`.
    pub url: Option<String>,
    pub route: String,
    pub auth: bool,
    #[serde(flatten)]
    pub stats: crate::socks::Snapshot,
}

#[derive(Clone, Debug, Serialize)]
pub struct RelayOverview {
    /// Что вписать боту в «Сервер Bot API».
    pub url: Option<String>,
    pub route: String,
    #[serde(flatten)]
    pub stats: crate::tgrelay::Snapshot,
}

pub async fn overview(app: &Arc<App>) -> Overview {
    let routing = app.routing();
    let load = app.queue.load();
    let mut models = Vec::new();
    for (name, m) in &routing.models {
        let l = load.get(name).cloned().unwrap_or_default();
        let ups: Vec<UpstreamOverview> = m
            .upstreams
            .iter()
            .map(|u| UpstreamOverview {
                id: u.id.clone(),
                url: u.url.clone(),
                target_model: u.target_model.clone(),
                slots: u.slots,
                busy: u.busy(),
                engine: u.engine.clone(),
                engine_status: u.engine.as_ref().and_then(|e| app.engines.info(e)).map(|i| i.status),
                usable: crate::dispatcher::upstream_usable(app, u),
                health: u.health.snapshot(),
            })
            .collect();
        models.push(ModelOverview {
            name: name.clone(),
            description: m.cfg.description.clone(),
            queued: l.queued,
            running: l.running,
            slots: m.slots(),
            busy: ups.iter().map(|u| u.busy).sum(),
            available: submit::model_available(app, m),
            fallback: m.cfg.fallback.clone(),
            aliases: routing.aliases.iter().filter(|(_, t)| *t == name).map(|(a, _)| a.clone()).collect(),
            upstreams: ups,
        });
    }
    let counts: BTreeMap<String, i64> = app.store.count_by_state().await.unwrap_or_default().into_iter().collect();
    let q5 = crate::stats::Query { range: Some(crate::stats::Range::M5), ..Default::default() };
    let q60 = crate::stats::Query { range: Some(crate::stats::Range::H1), ..Default::default() };
    let last_5m = app.stats.query(&q5).await.map(|s| s.totals).unwrap_or_default();
    let last_1h = app.stats.query(&q60).await.map(|s| s.totals).unwrap_or_default();
    let db_ok = app.store.ping().await;
    let bound = app.bound.lock().clone();
    let now = now_ms();
    let s = app.settings();
    let tg_relay = s.tg_relay.enabled.then(|| RelayOverview {
        url: tg_relay_url(app),
        route: crate::net::describe(&crate::tgrelay::route(&s)),
        stats: app.tg_relay.snapshot(),
    });
    let socks = s.socks.enabled.then(|| socks_overview(app));
    Overview {
        version: VERSION,
        started_at: app.started_at,
        uptime_s: (now - app.started_at) / 1000,
        now,
        paused: app.paused.load(Ordering::Relaxed),
        profile: app.effective().profile.clone(),
        queued: app.queue.total_queued(),
        running: app.queue.total_running(),
        counts,
        models,
        running_jobs: app.queue.running(),
        engines: app.engines.list().into_iter().map(|e| EngineBrief { id: e.id, name: e.name, status: e.status, pid: e.pid }).collect(),
        gpu: app.gpu.snapshot(),
        schedule: (*app.schedule()).clone(),
        bound,
        clients: app.settings().clients.iter().filter(|c| c.enabled).count(),
        db_ok,
        last_5m,
        last_1h,
        tg_relay,
        socks,
    }
}

/// Текст `/metrics`.
pub async fn prometheus(app: &Arc<App>) -> String {
    let mut out = String::new();
    out.push_str("# HELP queue_jobs Jobs by state.\n# TYPE queue_jobs gauge\n");
    let mut counts: BTreeMap<String, i64> = app.store.count_by_state().await.unwrap_or_default().into_iter().collect();
    counts.insert("queued".into(), app.queue.total_queued() as i64);
    counts.insert("running".into(), app.queue.total_running() as i64);
    for (s, n) in &counts {
        let _ = writeln!(out, "queue_jobs{{state=\"{}\"}} {n}", esc(s));
    }
    app.metrics.render_into(&mut out);
    out.push_str("# HELP upstream_up Upstream is usable (breaker closed, engine ready).\n# TYPE upstream_up gauge\n");
    out.push_str("# HELP upstream_busy_slots Busy slots of an upstream.\n# TYPE upstream_busy_slots gauge\n");
    for u in app.routing().upstreams() {
        let up = crate::dispatcher::upstream_usable(app, u) as u8;
        let _ = writeln!(out, "upstream_up{{model=\"{}\",upstream=\"{}\"}} {up}", esc(&u.model), esc(&u.id));
        let _ = writeln!(out, "upstream_busy_slots{{model=\"{}\",upstream=\"{}\"}} {}", esc(&u.model), esc(&u.id), u.busy());
    }
    out.push_str("# HELP engine_up Local engine is ready.\n# TYPE engine_up gauge\n");
    for e in app.engines.list() {
        let _ = writeln!(out, "engine_up{{engine=\"{}\"}} {}", esc(&e.id), (e.status == EngineStatus::Ready) as u8);
    }
    let r = app.tg_relay.snapshot();
    out.push_str("# HELP tg_relay_requests_total Telegram relay requests by outcome since start.\n# TYPE tg_relay_requests_total counter\n");
    for (outcome, n) in [("ok", r.ok), ("telegram_error", r.tg_errors), ("relay_error", r.relay_errors), ("rejected", r.rejected)] {
        let _ = writeln!(out, "tg_relay_requests_total{{outcome=\"{outcome}\"}} {n}");
    }
    out.push_str("# HELP tg_relay_bytes_total Telegram relay traffic.\n# TYPE tg_relay_bytes_total counter\n");
    let _ = writeln!(out, "tg_relay_bytes_total{{direction=\"up\"}} {}", r.bytes_up);
    let _ = writeln!(out, "tg_relay_bytes_total{{direction=\"down\"}} {}", r.bytes_down);
    out.push_str("# HELP tg_relay_cache_hits_total Files served from the relay cache.\n# TYPE tg_relay_cache_hits_total counter\n");
    let _ = writeln!(out, "tg_relay_cache_hits_total {}", r.cache_hits);
    let sx = app.socks.snapshot();
    out.push_str("# HELP socks_up SOCKS5 proxy is listening.\n# TYPE socks_up gauge\n");
    let _ = writeln!(out, "socks_up {}", sx.listening.is_some() as u8);
    out.push_str("# HELP socks_active_connections Open SOCKS5 connections.\n# TYPE socks_active_connections gauge\n");
    let _ = writeln!(out, "socks_active_connections {}", sx.active);
    out.push_str("# HELP socks_connections_total SOCKS5 connections by outcome since start.\n# TYPE socks_connections_total counter\n");
    for (outcome, n) in [("ok", sx.connections), ("failed", sx.failed), ("rejected", sx.rejected)] {
        let _ = writeln!(out, "socks_connections_total{{outcome=\"{outcome}\"}} {n}");
    }
    out.push_str("# HELP socks_bytes_total SOCKS5 traffic of closed connections.\n# TYPE socks_bytes_total counter\n");
    let _ = writeln!(out, "socks_bytes_total{{direction=\"up\"}} {}", sx.bytes_up);
    let _ = writeln!(out, "socks_bytes_total{{direction=\"down\"}} {}", sx.bytes_down);
    let gpu = app.gpu.snapshot();
    if gpu.available {
        out.push_str("# TYPE gpu_utilization_percent gauge\n# TYPE gpu_memory_used_bytes gauge\n# TYPE gpu_temperature_celsius gauge\n");
        for d in &gpu.devices {
            let l = format!("gpu=\"{}\",name=\"{}\"", d.index, esc(&d.name));
            let _ = writeln!(out, "gpu_utilization_percent{{{l}}} {}", d.util);
            let _ = writeln!(out, "gpu_memory_used_bytes{{{l}}} {}", d.mem_used_mb * 1_048_576);
            let _ = writeln!(out, "gpu_memory_total_bytes{{{l}}} {}", d.mem_total_mb * 1_048_576);
            if let Some(t) = d.temp_c {
                let _ = writeln!(out, "gpu_temperature_celsius{{{l}}} {t}");
            }
            if let Some(p) = d.power_w {
                let _ = writeln!(out, "gpu_power_watts{{{l}}} {p}");
            }
        }
    }
    out
}

// ------------------------------------------------------------------ настройки

pub fn settings_view(app: &App) -> Value {
    let eff = app.effective();
    let mut file = eff.file.clone();
    config::mask_secrets(&mut file, MASK);
    let mut effective = eff.settings.clone();
    config::mask_secrets(&mut effective, MASK);
    json!({
        "file": file,
        "effective": effective,
        "locked": eff.locked,
        "profile": eff.profile,
        "mask": MASK,
        "secret_key_from_env": false,
        "paths": {
            "config_dir": app.paths.config_dir,
            "data_dir": app.paths.data_dir,
            "log_dir": app.paths.log_dir,
            "settings_file": eff.settings_path,
            "db": app.db_path(),
            "portable": app.paths.portable,
        },
    })
}

/// Сохраняет настройки из панели. Замаскированные секреты остаются прежними.
pub fn update_settings(app: &App, mut new: Settings) -> Result<()> {
    let old = app.effective().file.clone();
    config::restore_masked(&mut new, &old, MASK);
    // Клиентов панель меняет отдельными вызовами — их список берётся из файла.
    new.clients = old.clients.clone();
    app.apply_file_settings(new)
}

/// Частичное обновление: JSON-объект сливается с текущим файлом настроек.
pub fn patch_settings(app: &App, patch: Value) -> Result<()> {
    let mut cur = serde_json::to_value(&app.effective().file)?;
    merge_json(&mut cur, patch);
    let new: Settings = serde_json::from_value(cur).context("invalid settings")?;
    update_settings(app, new)
}

fn merge_json(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(&k) {
                    Some(existing) if existing.is_object() && v.is_object() => merge_json(existing, v),
                    _ => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, p) => *b = p,
    }
}

pub fn validate_settings(app: &App, new: &Settings) -> Vec<String> {
    let mut s = new.clone();
    let env_fn = |k: &str| app.env_var(k);
    if let Err(e) = config::apply_env(&mut s, &env_fn) {
        return vec![e.to_string()];
    }
    config::validate(&s)
}

// -------------------------------------------------------------------- клиенты

#[derive(Serialize)]
pub struct ClientView {
    pub name: String,
    pub enabled: bool,
    pub created_at: i64,
    pub note: String,
    pub from_env: bool,
}

pub fn clients(app: &App) -> Vec<ClientView> {
    let file: Vec<String> = app.effective().file.clients.iter().map(|c| c.name.clone()).collect();
    app.settings()
        .clients
        .iter()
        .map(|c| ClientView { name: c.name.clone(), enabled: c.enabled, created_at: c.created_at, note: c.note.clone(), from_env: !file.contains(&c.name) })
        .collect()
}

/// Новый клиент. Ключ возвращается один раз и больше нигде не хранится.
pub fn create_client(app: &App, name: &str, note: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        bail!("имя клиента: от 1 до 64 символов");
    }
    let mut file = app.effective().file.clone();
    if file.clients.iter().any(|c| c.name == name) || app.settings().clients.iter().any(|c| c.name == name) {
        bail!("клиент '{name}' уже есть");
    }
    let key = format!("lmq_{}", random_token(24));
    file.clients.push(ClientCfg { name: name.into(), key_hash: hash_key(&key), enabled: true, created_at: now_ms(), note: note.into() });
    app.apply_file_settings(file)?;
    tracing::info!(client = name, "client created");
    Ok(key)
}

/// Новый ключ для существующего клиента.
pub fn rotate_client(app: &App, name: &str) -> Result<String> {
    let mut file = app.effective().file.clone();
    let c = file.clients.iter_mut().find(|c| c.name == name).ok_or_else(|| anyhow!("клиент '{name}' не найден в файле настроек"))?;
    let key = format!("lmq_{}", random_token(24));
    c.key_hash = hash_key(&key);
    app.apply_file_settings(file)?;
    tracing::info!(client = name, "client key rotated");
    Ok(key)
}

pub fn set_client(app: &App, name: &str, enabled: Option<bool>, note: Option<String>) -> Result<()> {
    let mut file = app.effective().file.clone();
    let c = file.clients.iter_mut().find(|c| c.name == name).ok_or_else(|| anyhow!("клиент '{name}' не найден в файле настроек"))?;
    if let Some(e) = enabled {
        c.enabled = e;
    }
    if let Some(n) = note {
        c.note = n;
    }
    app.apply_file_settings(file)
}

pub fn delete_client(app: &App, name: &str) -> Result<()> {
    let mut file = app.effective().file.clone();
    let before = file.clients.len();
    file.clients.retain(|c| c.name != name);
    if file.clients.len() == before {
        bail!("клиент '{name}' не найден в файле настроек");
    }
    app.apply_file_settings(file)?;
    tracing::info!(client = name, "client deleted");
    Ok(())
}

// -------------------------------------------------------------------- задания

/// Подробности задания для панели — без текстов сообщений и ответа.
pub async fn job_detail(app: &App, job_id: &str) -> Result<Option<Value>> {
    let Some(j) = app.store.get_job(job_id).await? else { return Ok(None) };
    let payload: Option<Map<String, Value>> = j.payload.as_deref().and_then(|p| serde_json::from_str(p).ok());
    let params: Option<Map<String, Value>> =
        payload.as_ref().map(|p| p.iter().filter(|(k, _)| *k != "messages").map(|(k, v)| (k.clone(), v.clone())).collect());
    let messages = payload.as_ref().and_then(|p| p.get("messages")).map(messages_summary);
    let mut v = serde_json::to_value(&j)?;
    v["public_state"] = json!(j.public_state());
    v["position"] = json!(app.queue.position(job_id).map(|p| p + 1));
    v["running"] = json!(app.queue.is_running(job_id));
    v["params"] = json!(params);
    v["messages"] = json!(messages);
    v["has_result"] = json!(j.result_text.is_some());
    v["result_chars"] = json!(j.result_text.as_ref().map(|t| t.chars().count()));
    Ok(Some(v))
}

/// Строение сообщений без текстов: роль, вид содержимого, длина; у частей — тип и
/// для картинок источник (`data:image/jpeg`, хост ссылки) и размер.
pub fn messages_summary(messages: &Value) -> Value {
    let Some(list) = messages.as_array() else { return json!([]) };
    Value::Array(
        list.iter()
            .map(|x| {
                let role = x.get("role").and_then(Value::as_str).unwrap_or("?");
                match x.get("content") {
                    Some(Value::String(s)) => json!({ "role": role, "kind": "text", "size": s.chars().count() }),
                    Some(Value::Array(parts)) => json!({
                        "role": role,
                        "kind": "parts",
                        "size": parts.len(),
                        "parts": parts.iter().map(part_summary).collect::<Vec<_>>(),
                    }),
                    Some(Value::Null) | None => json!({ "role": role, "kind": "none", "size": 0 }),
                    Some(other) => json!({ "role": role, "kind": "other", "type": json_type(other) }),
                }
            })
            .collect(),
    )
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn part_summary(p: &Value) -> Value {
    let ty = p.get("type").and_then(Value::as_str).unwrap_or("?");
    match ty {
        "text" => json!({ "type": "text", "size": p.get("text").and_then(Value::as_str).map(|s| s.chars().count()).unwrap_or(0) }),
        "image_url" | "input_image" => {
            let url = p.get("image_url").and_then(|u| u.get("url").or(Some(u))).and_then(Value::as_str).unwrap_or("");
            json!({ "type": ty, "source": image_source(url), "bytes": url.len(), "detail": p.get("image_url").and_then(|u| u.get("detail")) })
        }
        other => json!({ "type": other, "keys": p.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()) }),
    }
}

/// Модели, которые отдаёт сам апстрим (`GET {base}/models` с его ключом и маршрутом):
/// варианты для поля «Имя у апстрима».
pub async fn upstream_models(app: &App, model: &str, index: usize) -> Result<Vec<String>> {
    let routing = app.routing();
    let up = routing.models.get(model).and_then(|m| m.upstreams.get(index)).cloned().ok_or_else(|| anyhow!("апстрим не найден — сохраните модель"))?;
    let http = app.upstream_client(&up).map_err(|e| anyhow!(e))?;
    let mut req = http.get(format!("{}/models", up.base_url())).timeout(std::time::Duration::from_secs(10));
    if !up.api_key.is_empty() {
        req = req.bearer_auth(&up.api_key);
    }
    let resp = req.send().await.map_err(|e| anyhow!(crate::netdiag::error_chain(&e)))?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("HTTP {}: {}", status.as_u16(), body["error"]["message"].as_str().unwrap_or(""));
    }
    // OpenAI: {"data": [{"id"}]}; Ollama: {"models": [{"name"}]}.
    let list = body["data"].as_array().or(body["models"].as_array()).cloned().unwrap_or_default();
    let mut ids: Vec<String> = list.iter().filter_map(|m| m["id"].as_str().or(m["name"].as_str()).map(String::from)).collect();
    ids.sort();
    Ok(ids)
}

// ---------------------------------------------------------------------- сеть

/// Интерфейсы, найденные локальные прокси и действующие маршруты (для полей `network.*`).
pub async fn network_view(app: &App) -> Value {
    let proxies = crate::net::detect_local_proxies().await;
    let s = app.settings();
    let routing = app.routing();
    let ups: Vec<Value> = routing.upstreams().map(|u| json!({ "id": u.id, "url": u.url, "route": crate::net::describe(&u.route) })).collect();
    json!({
        "interfaces": crate::net::interfaces(),
        "proxies": proxies,
        "routes": {
            "upstream": crate::net::describe(&s.network.upstream),
            "callback": crate::net::describe(&s.network.callback),
            "telegram": crate::net::describe(&s.network.telegram),
            "tg_relay": crate::net::describe(&crate::tgrelay::route(&s)),
        },
        "upstreams": ups,
    })
}

/// Проверка всех маршрутов: бот (адрес колбэка), каждый апстрим, Telegram (если включён
/// протокол 1). Запросы — `GET` без данных. `custom` — свой адрес и маршрут.
pub async fn network_check(app: &Arc<App>, custom: Option<(String, crate::config::RouteCfg)>) -> Vec<Value> {
    let timeout = std::time::Duration::from_secs(8);
    let s = app.settings();
    let mut targets: Vec<(String, String, crate::config::RouteCfg)> = Vec::new();
    if let Some((url, route)) = custom {
        targets.push(("custom".into(), url, route));
    } else {
        let mut cb = s.callback.url.clone();
        if cb.trim().is_empty() {
            let recent = app.store.list_jobs(&crate::store::JobFilter { limit: 50, ..Default::default() }).await.map(|r| r.0).unwrap_or_default();
            cb = recent.iter().find_map(|j| j.callback_url.clone().filter(|u| !u.is_empty())).unwrap_or_default();
        }
        if !cb.is_empty() {
            targets.push(("callback".into(), cb, s.network.callback.clone()));
        }
        for u in app.routing().upstreams() {
            targets.push((format!("upstream {}", u.id), format!("{}/models", u.base_url()), u.route.clone()));
        }
        if s.protocol1.enabled {
            targets.push(("telegram".into(), s.protocol1.telegram_api.clone(), s.network.telegram.clone()));
        }
        if s.tg_relay.enabled {
            targets.push(("telegram relay".into(), format!("{}/", crate::tgrelay::upstream(&s)), crate::tgrelay::route(&s)));
        }
    }
    let probes = targets.into_iter().map(|(name, url, route)| async move {
        let p = crate::netdiag::probe_route(&url, timeout, &route).await;
        json!({ "name": name, "probe": p })
    });
    let out = futures::future::join_all(probes).await;
    for v in &out {
        tracing::info!(target_name = %v["name"].as_str().unwrap_or(""), verdict = %v["probe"]["verdict"].as_str().unwrap_or(""), "network check");
    }
    out
}

// ------------------------------------------------------------ реле Telegram

/// Адрес реле для бота («Сервер Bot API»): адрес API; вместо `0.0.0.0` — IPv4 этой машины
/// в локальной сети (интерфейса без туннеля, если он найден).
pub fn tg_relay_url(app: &App) -> Option<String> {
    let a = app.bound.lock().client?;
    let ip = if a.ip().is_unspecified() {
        let mut list = crate::net::interfaces();
        list.sort_by_key(|i| !i.uplink);
        list.into_iter()
            .flat_map(|i| i.ipv4)
            .find(|ip| matches!(ip, std::net::IpAddr::V4(v) if !v.is_link_local()))
            .unwrap_or(std::net::IpAddr::from([127, 0, 0, 1]))
    } else {
        a.ip()
    };
    Some(format!("http://{}", std::net::SocketAddr::new(ip, a.port())))
}

/// Состояние реле для его раздела настроек: адрес для бота, маршрут, счётчики с запуска.
pub fn tg_relay_view(app: &App) -> Value {
    let s = app.settings();
    json!({
        "enabled": s.tg_relay.enabled,
        "url": tg_relay_url(app),
        "upstream": crate::tgrelay::upstream(&s),
        "route": crate::net::describe(&crate::tgrelay::route(&s)),
        "stats": app.tg_relay.snapshot(),
    })
}

/// Пробный токен для проверки без настоящего: Telegram отвечает на него 401 в формате Bot API.
const PROBE_TOKEN: &str = "0:lmq-relay-check";

/// `getMe` с токеном: HTTP-статус и тело или текст ошибки (без адреса — в нём токен).
async fn get_me(http: &reqwest::Client, base: &str, token: &str) -> (std::result::Result<(u16, Value), String>, i64) {
    let started = now_ms();
    let r = async {
        let resp = http.get(format!("{base}/bot{token}/getMe")).timeout(std::time::Duration::from_secs(15)).send().await?;
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        Ok::<_, reqwest::Error>((status, body))
    }
    .await
    .map_err(|e| crate::netdiag::error_chain(&e.without_url()));
    (r, now_ms() - started)
}

/// Ответ Bot API словами: `(успех, текст, @имя бота)`.
fn explain_get_me(r: &std::result::Result<(u16, Value), String>, ms: i64, real_token: bool) -> (bool, String, Option<String>) {
    match r {
        Err(e) => (false, format!("Запрос не прошёл: {e}"), None),
        Ok((_, v)) if v["ok"] == json!(true) => {
            let name = v["result"]["username"].as_str().map(|u| format!("@{u}"));
            (true, format!("Bot API отвечает за {ms} мс: бот {}", name.clone().unwrap_or_else(|| "?".into())), name)
        }
        Ok((status, v)) if v["ok"] == json!(false) => {
            let desc = v["description"].as_str().unwrap_or("");
            if desc.starts_with("LMQ relay:") {
                (false, format!("Реле LMQ отказало: HTTP {status} {desc}"), None)
            } else if real_token {
                (false, format!("Telegram отклонил токен: HTTP {status} {desc}"), None)
            } else {
                (true, format!("Bot API отвечает за {ms} мс (пробный токен отклонён — так и должно быть)"), None)
            }
        }
        Ok((status, _)) => (false, format!("Ответ не похож на Bot API: HTTP {status}. Проверьте адрес Bot API."), None),
    }
}

/// Проверка реле: маршрут до Telegram (DNS, TCP, HTTP, внешний IP), ответ Bot API по этому
/// маршруту и вся цепочка через адрес API LMQ. `token` — токен бота для `getMe` (не
/// сохраняется); без него — токен протокола 1 или пробный.
pub async fn tg_relay_check(app: &Arc<App>, token: Option<String>) -> Value {
    let s = app.settings();
    let route = crate::tgrelay::route(&s);
    let base = crate::tgrelay::upstream(&s);
    let token =
        token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).or_else(|| Some(s.protocol1.telegram_bot_token.clone()).filter(|t| !t.is_empty()));
    let real = token.is_some();
    let token = token.unwrap_or_else(|| PROBE_TOKEN.into());
    let mut steps = Vec::new();

    let probe = crate::netdiag::probe_route(&format!("{base}/"), std::time::Duration::from_secs(8), &route).await;
    steps.push(json!({ "name": "route", "title": "Маршрут до Telegram", "ok": probe.http_status.is_some(), "text": probe.verdict }));

    let mut bot = None;
    match app.net.client(&route, crate::net::Kind::NoRedirect) {
        Ok(http) => {
            let (r, ms) = get_me(&http, &base, &token).await;
            let (ok, text, name) = explain_get_me(&r, ms, real);
            bot = name;
            steps.push(json!({ "name": "api", "title": "Bot API по этому маршруту", "ok": ok, "text": text }));
        }
        Err(e) => steps.push(json!({ "name": "api", "title": "Bot API по этому маршруту", "ok": false, "text": e })),
    }

    let url = tg_relay_url(app);
    let bound = app.bound.lock().client;
    let local = match (s.tg_relay.enabled, bound) {
        (false, _) => json!({ "ok": false, "text": "Реле выключено: включите его и сохраните настройки." }),
        (true, None) => json!({ "ok": false, "text": "Адрес API LMQ не слушает — боту некуда обращаться." }),
        (true, Some(a)) => {
            let host = if a.ip().is_unspecified() { std::net::SocketAddr::from(([127, 0, 0, 1], a.port())) } else { a };
            // Без настоящего токена — пробный с id разрешённого бота: реле его пропустит,
            // а Telegram отклонит, и это проверяет всю цепочку.
            let token = match s.tg_relay.allowed_bots.first() {
                Some(id) if !real => format!("{}:lmq-relay-check", id.trim()),
                _ => token.clone(),
            };
            match reqwest::Client::builder().no_proxy().build() {
                Ok(http) => {
                    let (r, ms) = get_me(&http, &format!("http://{host}"), &token).await;
                    let (ok, text, name) = explain_get_me(&r, ms, real);
                    bot = bot.or(name);
                    if ok {
                        json!({ "ok": true, "text": format!("Цепочка бот → LMQ → Telegram работает. {text}") })
                    } else {
                        json!({ "ok": false, "text": text })
                    }
                }
                Err(e) => json!({ "ok": false, "text": e.to_string() }),
            }
        }
    };
    steps.push(json!({ "name": "lmq", "title": "Через адрес API LMQ", "ok": local["ok"], "text": local["text"] }));

    let ok = steps.iter().all(|x| x["ok"] == json!(true));
    for x in &steps {
        tracing::info!(step = %x["name"].as_str().unwrap_or(""), ok = x["ok"].as_bool().unwrap_or(false), text = %x["text"].as_str().unwrap_or(""), "telegram relay check");
    }
    json!({
        "ok": ok,
        "enabled": s.tg_relay.enabled,
        "url": url,
        "upstream": base,
        "route": crate::net::describe(&route),
        "exit_ip": probe.exit_ip,
        "bot": bot,
        "with_token": real,
        "steps": steps,
    })
}

// ---------------------------------------------------------- SOCKS5-прокси

/// Адрес прокси для этой машины: вместо `0.0.0.0` — `127.0.0.1`.
fn socks_local(a: std::net::SocketAddr) -> std::net::SocketAddr {
    if a.ip().is_unspecified() { std::net::SocketAddr::from(([127, 0, 0, 1], a.port())) } else { a }
}

fn socks_overview(app: &App) -> SocksOverview {
    let s = app.settings();
    SocksOverview {
        url: app.socks.bound().map(|a| crate::socks::link(&s.socks, &socks_local(a).ip().to_string(), a.port(), false)),
        route: crate::net::describe(&crate::socks::route_of(&s.socks.interface)),
        auth: !s.socks.username.is_empty(),
        stats: app.socks.snapshot(),
    }
}

/// Прокси для его раздела: состояние, счётчики и готовые ссылки — снаружи (внешний IP
/// интерфейса без туннеля или `public_host`), в локальной сети и на этой машине. Ссылки
/// содержат пароль: их видит только администратор. `refresh` — заново узнать внешний IP.
pub async fn socks_view(app: &Arc<App>, refresh: bool) -> Value {
    let s = app.settings();
    let c = &s.socks;
    let bound = app.socks.bound();
    let port = bound.map(|a| a.port()).or_else(|| c.listen.parse::<std::net::SocketAddr>().ok().map(|a| a.port())).unwrap_or(1080);
    let listen_ip = bound.map(|a| a.ip()).or_else(|| c.listen.parse::<std::net::SocketAddr>().ok().map(|a| a.ip()));
    let local_only = listen_ip.is_some_and(|ip| ip.is_loopback());
    // Интерфейс без туннеля: заданный или найденный сам (физический, с основным шлюзом).
    // Системный маршрут не годится: VPN в режиме TUN уводит его в туннель.
    let public_iface = Some(c.public_interface.trim().to_string()).filter(|s| !s.is_empty()).or_else(crate::net::primary_uplink).unwrap_or_default();
    let public_route = match (c.public_interface.trim().is_empty(), public_iface.is_empty()) {
        (_, true) => crate::net::describe(&crate::socks::route_of("")),
        (true, false) => format!("авто: интерфейс {public_iface}"),
        (false, false) => format!("интерфейс {public_iface}"),
    };
    let (public, public_error) = if !c.public_host.trim().is_empty() {
        (Some(c.public_host.trim().to_string()), None)
    } else {
        match app.socks.public_ip(&app.net, &public_iface, refresh).await {
            Ok(ip) => (Some(ip), None),
            Err(e) => (None, Some(e)),
        }
    };
    // Адрес в локальной сети: IPv4 интерфейса без туннеля.
    let lan = crate::net::interfaces()
        .into_iter()
        .filter(|i| if public_iface.is_empty() { i.uplink } else { i.name.eq_ignore_ascii_case(&public_iface) })
        .flat_map(|i| i.ipv4)
        .find(|ip| matches!(ip, std::net::IpAddr::V4(v) if v.is_private()))
        .map(|ip| ip.to_string());
    let mut links = Vec::new();
    let mut push = |kind: &str, title: &str, host: String| {
        links.push(json!({
            "kind": kind,
            "title": title,
            "host": host,
            "port": port,
            "url": crate::socks::link(c, &host, port, true),
            "telegram": crate::socks::telegram_link(c, &host, port),
        }));
    };
    if let Some(h) = public.clone() {
        push("public", "Снаружи", h);
    }
    if let Some(h) = lan {
        push("lan", "В локальной сети", h);
    }
    push("local", "На этой машине", listen_ip.filter(|ip| !ip.is_unspecified()).map(|ip| ip.to_string()).unwrap_or_else(|| "127.0.0.1".into()));
    json!({
        "enabled": c.enabled,
        "listen": c.listen,
        "local_only": local_only,
        "auth": !c.username.is_empty(),
        "route": crate::net::describe(&crate::socks::route_of(&c.interface)),
        "public_route": public_route,
        "public_ip": public,
        "public_error": public_error,
        "links": links,
        "stats": app.socks.snapshot(),
    })
}

/// Внешний IP через HTTP-клиент: `(IP или ошибка, мс)`.
async fn exit_ip(http: Result<reqwest::Client, String>) -> (std::result::Result<String, String>, i64) {
    let started = now_ms();
    let r = match http {
        Ok(h) => async {
            let r = h.get(crate::netdiag::IP_ECHO).timeout(std::time::Duration::from_secs(15)).send().await?.error_for_status()?;
            r.text().await
        }
        .await
        .map(|t| t.trim().chars().take(64).collect())
        .map_err(|e| crate::netdiag::error_chain(&e)),
        Err(e) => Err(e),
    };
    (r, now_ms() - started)
}

/// Проверка прокси: слушает ли он, работает ли соединение через него и с какого внешнего IP
/// выходит трафик — в сравнении с выбранным интерфейсом напрямую и с системным маршрутом.
pub async fn socks_check(app: &Arc<App>) -> Value {
    let s = app.settings();
    let c = &s.socks;
    let mut steps = Vec::new();
    let bound = app.socks.bound();
    let snap = app.socks.snapshot();
    let listen_text = match (c.enabled, bound, &snap.listen_error) {
        (false, _, _) => "Прокси выключен: включите его и сохраните настройки.".to_string(),
        (true, Some(a), _) => format!("Слушает {a}{}", if c.username.is_empty() { ", вход без пароля" } else { ", вход по логину и паролю" }),
        (true, None, Some(e)) => format!("Не слушает: {e}"),
        (true, None, None) => "Ещё не запустился.".to_string(),
    };
    steps.push(json!({ "name": "listen", "title": "Прокси", "ok": bound.is_some(), "text": listen_text }));

    let via_proxy = async {
        let a = bound.ok_or_else(|| "прокси не слушает".to_string())?;
        let mut u = url::Url::parse(&format!("socks5h://{}", socks_local(a))).map_err(|e| e.to_string())?;
        if !c.username.is_empty() {
            let _ = u.set_username(&c.username);
            let _ = u.set_password(Some(&c.password));
        }
        let proxy = reqwest::Proxy::all(u.as_str()).map_err(|e| e.to_string())?;
        reqwest::Client::builder().proxy(proxy).build().map_err(|e| e.to_string())
    };
    let direct = app.net.client(&crate::socks::route_of(&c.interface), crate::net::Kind::General);
    let system = app.net.client(&crate::socks::route_of(""), crate::net::Kind::General);
    let (proxy_r, direct_r, system_r) = futures::join!(async { exit_ip(via_proxy.await).await }, exit_ip(direct), exit_ip(system));

    let route = crate::net::describe(&crate::socks::route_of(&c.interface));
    let ip_of = |r: &(std::result::Result<String, String>, i64)| r.0.as_ref().ok().cloned();
    match &proxy_r.0 {
        Ok(ip) => steps.push(
            json!({ "name": "proxy", "title": "Соединение через прокси", "ok": true, "text": format!("Работает за {} мс, выход в интернет: {ip}", proxy_r.1) }),
        ),
        Err(e) => steps.push(json!({ "name": "proxy", "title": "Соединение через прокси", "ok": false, "text": format!("Не прошло: {e}") })),
    }
    let direct_text = match (&direct_r.0, ip_of(&proxy_r)) {
        (Ok(ip), Some(p)) if *ip == p => format!("{route}: выход {ip} — совпадает, трафик прокси идёт через него"),
        (Ok(ip), Some(p)) => format!("{route}: выход {ip}, а у прокси {p} — проверьте интерфейс"),
        (Ok(ip), None) => format!("{route}: выход {ip}"),
        (Err(e), _) => format!("{route}: {e}"),
    };
    let direct_ok = direct_r.0.is_ok() && ip_of(&direct_r) == ip_of(&proxy_r);
    steps.push(json!({ "name": "interface", "title": "Выбранный интерфейс напрямую", "ok": direct_ok, "text": direct_text }));
    let system_text = match (&system_r.0, ip_of(&proxy_r)) {
        (Ok(ip), Some(p)) if *ip == p => format!("Выход {ip} — тот же, что у прокси"),
        (Ok(ip), Some(_)) => format!("Выход {ip} — прокси выходит в интернет другим путём"),
        (Ok(ip), None) => format!("Выход {ip}"),
        (Err(e), _) => e.clone(),
    };
    steps.push(json!({ "name": "system", "title": "Системный маршрут", "ok": system_r.0.is_ok(), "text": system_text }));

    let ok = steps.iter().take(3).all(|x| x["ok"] == json!(true));
    for x in &steps {
        tracing::info!(step = %x["name"].as_str().unwrap_or(""), ok = x["ok"].as_bool().unwrap_or(false), text = %x["text"].as_str().unwrap_or(""), "socks proxy check");
    }
    json!({ "ok": ok, "route": route, "exit_ip": ip_of(&proxy_r), "steps": steps })
}

/// Сколько картинок в сообщениях (части `image_url` / `input_image`).
pub fn image_count(messages: Option<&Value>) -> usize {
    messages
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|m| m.get("content").and_then(Value::as_array))
                .flatten()
                .filter(|p| matches!(p.get("type").and_then(Value::as_str), Some("image_url" | "input_image")))
                .count()
        })
        .unwrap_or(0)
}

/// `data:image/jpeg;base64` → `data:image/jpeg`; ссылка → её хост; пусто — `none`.
pub fn image_source(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("data:") {
        return format!("data:{}", rest.split([';', ',']).next().unwrap_or(""));
    }
    match reqwest::Url::parse(url) {
        Ok(u) => format!("{}://{}", u.scheme(), u.host_str().unwrap_or("")),
        Err(_) if url.is_empty() => "none".into(),
        Err(_) => "invalid".into(),
    }
}

pub async fn redeliver(app: &App, job_id: &str) -> Result<bool> {
    let ok = app.store.redeliver(job_id).await?;
    if ok {
        app.delivery_wake.notify_one();
        tracing::info!(job_id, "redelivery requested");
    }
    Ok(ok)
}

pub async fn redeliver_all(app: &App) -> Result<u64> {
    let n = app.store.redeliver_all_dead().await?;
    app.delivery_wake.notify_one();
    tracing::info!(count = n, "redelivery of dead jobs requested");
    Ok(n)
}

/// Проверка модели из панели: короткий синхронный запрос.
pub async fn test_model(app: &Arc<App>, model: &str, prompt: Option<&str>) -> Value {
    let body: Map<String, Value> = serde_json::from_value(json!({
        "model": model,
        "messages": [{ "role": "user", "content": prompt.unwrap_or("Reply with: OK") }],
        "max_tokens": 16,
        "chat_id": 0,
    }))
    .unwrap_or_default();
    let started = now_ms();
    match submit::submit(app, "admin", body, SubmitOpts::default()).await {
        Ok(Submitted::Sync(b)) => json!({
            "ok": true,
            "latency_ms": now_ms() - started,
            "text": crate::upstream::extract_text(&b),
            "usage": b.get("usage"),
        }),
        Ok(Submitted::Queued { .. }) => json!({ "ok": false, "error": "unexpected queued answer" }),
        Err(e) => {
            tracing::warn!(model, status = e.status, error = %e.message, "model test failed");
            json!({ "ok": false, "status": e.status, "error": e.message, "latency_ms": now_ms() - started })
        }
    }
}

/// Задание с колбэком во встроенный приёмник (экран «Отладка»).
pub async fn debug_job(app: &Arc<App>, model: &str, text: &str, chat_id: i64) -> Result<Value> {
    let admin = app.bound.lock().admin.ok_or_else(|| anyhow!("admin listener is not running"))?;
    let host = if admin.ip().is_unspecified() { "127.0.0.1".to_string() } else { admin.ip().to_string() };
    let url = format!("http://{host}:{}/admin/debug/callback", admin.port());
    let body: Map<String, Value> = serde_json::from_value(json!({
        "model": model,
        "messages": [{ "role": "user", "content": text }],
        "protocol": 2,
        "delivery": "bot",
        "chat_id": if chat_id == 0 { -1 } else { chat_id },
        "reply_to": 1,
        "thread_id": null,
        "job_id": format!("debug-{}", crate::util::random_hex(6)),
    }))?;
    match submit::submit(app, "admin", body, SubmitOpts { internal_callback: Some((url, "debug-token".into())), deltas: None }).await {
        Ok(Submitted::Queued { body, .. }) => Ok(body),
        Ok(Submitted::Sync(b)) => Ok(b),
        Err(e) => bail!("{} {}", e.status, e.message),
    }
}

// -------------------------------------------------------------------- профили

#[derive(Serialize)]
pub struct ProfileInfo {
    pub name: String,
    pub active: bool,
    pub path: String,
}

pub fn profiles(app: &App) -> Vec<ProfileInfo> {
    let active = app.effective().profile.clone();
    let mut names = vec!["default".to_string()];
    if let Ok(rd) = std::fs::read_dir(app.paths.profiles_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "yaml")
                && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
            {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    names.into_iter().map(|n| ProfileInfo { active: n == active, path: app.paths.settings_file(&n).display().to_string(), name: n }).collect()
}

/// Новый профиль — копия текущих настроек.
pub fn create_profile(app: &App, name: &str) -> Result<()> {
    let safe = crate::util::safe_file_name(name.trim());
    if safe.is_empty() || safe == "default" {
        bail!("недопустимое имя профиля");
    }
    let path = app.paths.settings_file(&safe);
    if path.exists() {
        bail!("профиль '{safe}' уже есть");
    }
    config::save_file(&path, &app.effective().file, &app.cipher)
}

pub fn delete_profile(app: &App, name: &str) -> Result<()> {
    if name == "default" || name == app.effective().profile {
        bail!("нельзя удалить профиль default или активный профиль");
    }
    std::fs::remove_file(app.paths.settings_file(name)).context("remove profile")?;
    Ok(())
}

// ----------------------------------------------------------- экспорт / импорт

#[derive(Serialize, Deserialize)]
struct ExportFile {
    lmq_export: u32,
    version: String,
    exported_at: String,
    profile: String,
    /// Как хранятся секреты: `none` (вырезаны) | `password` (`pw:…`).
    secrets: String,
    settings: Settings,
}

fn walk_secrets(s: &mut Settings, f: &mut dyn FnMut(&mut String) -> Result<()>) -> Result<()> {
    f(&mut s.callback.token)?;
    f(&mut s.protocol1.telegram_bot_token)?;
    f(&mut s.metrics.token)?;
    f(&mut s.socks.password)?;
    for m in s.models.values_mut() {
        for u in &mut m.upstreams {
            f(&mut u.api_key)?;
        }
    }
    Ok(())
}

/// Экспорт настроек в YAML: без секретов или с секретами под паролем.
pub fn export_settings(app: &App, password: Option<&str>) -> Result<String> {
    let mut s = app.effective().file.clone();
    let mode = match password.filter(|p| !p.is_empty()) {
        Some(pw) => {
            walk_secrets(&mut s, &mut |v| {
                if !v.is_empty() {
                    *v = crypto::password_encrypt(pw, v)?;
                }
                Ok(())
            })?;
            "password"
        }
        None => {
            walk_secrets(&mut s, &mut |v| {
                v.clear();
                Ok(())
            })?;
            "none"
        }
    };
    let f = ExportFile {
        lmq_export: 1,
        version: VERSION.into(),
        exported_at: chrono::Local::now().to_rfc3339(),
        profile: app.effective().profile.clone(),
        secrets: mode.into(),
        settings: s,
    };
    Ok(format!("# LMQ: экспорт настроек\n{}", serde_yaml::to_string(&f)?))
}

/// Импорт: в текущий профиль или в новый (`into_profile`). Секреты без пароля остаются прежними.
pub fn import_settings(app: &App, text: &str, password: Option<&str>, into_profile: Option<&str>) -> Result<()> {
    let f: ExportFile = serde_yaml::from_str(text).context("это не файл экспорта LMQ")?;
    if f.lmq_export != 1 {
        bail!("неизвестная версия файла экспорта");
    }
    let mut s = f.settings;
    if f.secrets == "password" {
        let pw = password.filter(|p| !p.is_empty()).ok_or_else(|| anyhow!("файл защищён паролем"))?;
        walk_secrets(&mut s, &mut |v| {
            if v.starts_with(crypto::PW_PREFIX) {
                *v = crypto::password_decrypt(pw, v)?;
            }
            Ok(())
        })?;
    }
    let errors = validate_settings(app, &s);
    if !errors.is_empty() {
        bail!("настройки не прошли проверку:\n{}", errors.join("\n"));
    }
    match into_profile.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) if p != app.effective().profile => {
            let path = app.paths.settings_file(&crate::util::safe_file_name(p));
            config::save_file(&path, &s, &app.cipher)
        }
        _ => {
            if f.secrets == "none" {
                let old = app.effective().file.clone();
                // Пустые секреты из файла без секретов — оставить текущие.
                if s.callback.token.is_empty() {
                    s.callback.token = old.callback.token.clone();
                }
                if s.protocol1.telegram_bot_token.is_empty() {
                    s.protocol1.telegram_bot_token = old.protocol1.telegram_bot_token.clone();
                }
                if s.metrics.token.is_empty() {
                    s.metrics.token = old.metrics.token.clone();
                }
                if s.socks.password.is_empty() {
                    s.socks.password = old.socks.password.clone();
                }
                for (name, m) in s.models.iter_mut() {
                    for (i, u) in m.upstreams.iter_mut().enumerate() {
                        if u.api_key.is_empty()
                            && let Some(ou) = old.models.get(name).and_then(|om| om.upstreams.get(i))
                        {
                            u.api_key = ou.api_key.clone();
                        }
                    }
                }
            }
            app.apply_file_settings(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_merge() {
        let mut a = json!({"queue": {"max_queue": 1, "max_wait": 2}, "x": 1});
        merge_json(&mut a, json!({"queue": {"max_queue": 5}, "y": [1]}));
        assert_eq!(a, json!({"queue": {"max_queue": 5, "max_wait": 2}, "x": 1, "y": [1]}));
    }
}
