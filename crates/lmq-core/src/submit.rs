//! Постановка задания (§2): проверки, лимиты, вытеснение, резерв, синхронный режим.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::app::App;
use crate::callback::validate_url;
use crate::engines::EngineStatus;
use crate::queue::{QItem, SyncCtx, SyncResult, TIER_GROUP, TIER_PRIVATE, TIER_SYNC};
use crate::routing::ModelRt;
use crate::store::{NewJob, SyncRecord};
use crate::upstream::QUEUE_FIELDS;
use crate::util::{now_ms, random_hex, valid_job_id};

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
    pub retry_after: Option<u64>,
}

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), retry_after: None }
    }
    pub fn retry(mut self, secs: u64) -> Self {
        self.retry_after = Some(secs.max(1));
        self
    }
}

pub enum Submitted {
    /// Принято в очередь (202) или повтор (200).
    Queued { status: u16, body: Value },
    /// Синхронный ответ в форме OpenAI.
    Sync(Value),
}

/// Особые режимы для заданий из панели.
#[derive(Default)]
pub struct SubmitOpts {
    /// Колбэк в обход белого списка (встроенный приёмник отладки): (url, token).
    pub internal_callback: Option<(String, String)>,
    /// Песочница: куски ответа по мере генерации.
    pub deltas: Option<mpsc::UnboundedSender<String>>,
}

fn as_i64(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Можно ли модели сейчас давать задания: хотя бы один апстрим не исключён выключателем,
/// а привязанный движок запущен (или запускается).
pub fn model_available(app: &App, m: &ModelRt) -> bool {
    let breaker = app.settings().breaker.clone();
    m.upstreams.iter().any(|u| {
        let engine_ok = match &u.engine {
            None => true,
            Some(e) => app.engines.info(e).is_some_and(|i| matches!(i.status, EngineStatus::Ready | EngineStatus::Starting | EngineStatus::Crashed)),
        };
        engine_ok && u.health.available(&breaker)
    })
}

fn check_messages(body: &Map<String, Value>, max_messages: u32, max_kb: u32) -> Result<(), ApiError> {
    let msgs = body.get("messages").and_then(Value::as_array).ok_or_else(|| ApiError::new(400, "messages must be a non-empty array"))?;
    if msgs.is_empty() {
        return Err(ApiError::new(400, "messages must be a non-empty array"));
    }
    if msgs.len() > max_messages as usize {
        return Err(ApiError::new(413, format!("too many messages (max {max_messages})")));
    }
    let max_len = max_kb as usize * 1024;
    for m in msgs {
        let Some(obj) = m.as_object() else { return Err(ApiError::new(400, "each message must be an object")) };
        if obj.get("role").and_then(Value::as_str).is_none() {
            return Err(ApiError::new(400, "each message needs a role"));
        }
        match obj.get("content") {
            Some(Value::String(s)) if s.len() > max_len => return Err(ApiError::new(413, format!("message text is longer than {max_kb} KB"))),
            Some(Value::Array(parts)) => {
                for p in parts {
                    if let Some(t) = p.get("text").and_then(Value::as_str)
                        && t.len() > max_len
                    {
                        return Err(ApiError::new(413, format!("message text is longer than {max_kb} KB")));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn eta_secs(app: &App, model: &ModelRt, position: usize) -> u64 {
    let slots = model.slots().max(1) as f64;
    let avg = app.stats.avg_latency(&model.name);
    (((position as f64 + 1.0) / slots).ceil() * avg / 1000.0).round().max(1.0) as u64
}

fn retry_after_full(app: &App, model: &ModelRt) -> u64 {
    let queued = app.queue.queued_in_model(&model.name);
    let slots = model.slots().max(1) as f64;
    ((queued as f64 / slots) * app.stats.avg_latency(&model.name) / 1000.0).ceil().clamp(5.0, 300.0) as u64
}

/// Ответ на постановку и на повтор того же `job_id` (§2.2).
pub fn queued_body(app: &App, job_id: &str, model: &str, state: &str) -> Value {
    let routing = app.routing();
    let position = app.queue.position(job_id);
    let eta = match (position, routing.models.get(model)) {
        (Some(p), Some(m)) => eta_secs(app, m, p),
        (None, Some(m)) if state == "running" => (app.stats.avg_latency(&m.name) / 1000.0).round() as u64,
        _ => 0,
    };
    json!({
        "status": "Added to queue",
        "job_id": job_id,
        "state": state,
        "position": position.map(|p| p + 1).unwrap_or(0),
        "eta_seconds": eta,
    })
}

pub async fn submit(app: &Arc<App>, client: &str, body: Map<String, Value>, opts: SubmitOpts) -> Result<Submitted, ApiError> {
    let cfg = app.settings();
    let routing = app.routing();
    let protocol = as_i64(body.get("protocol")).unwrap_or(2);
    let delivery = body.get("delivery").or_else(|| body.get("_delivery")).and_then(Value::as_str).unwrap_or("bot");
    let protocol1 = protocol == 1 || delivery.eq_ignore_ascii_case("telegram");
    let requested = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
    if requested.is_empty() {
        return Err(ApiError::new(400, "model is required"));
    }
    let mut model = routing.resolve(&requested).ok_or_else(|| ApiError::new(400, format!("unknown model '{requested}'")))?;
    check_messages(&body, cfg.server.max_messages, cfg.server.max_string_kb)?;
    let chat_id = as_i64(body.get("chat_id")).unwrap_or(0);

    let job_id = match body.get("job_id").and_then(Value::as_str) {
        Some(id) if valid_job_id(id) => id.to_string(),
        Some(_) => return Err(ApiError::new(400, "job_id must be 1-64 chars of [A-Za-z0-9_-]")),
        None => random_hex(12),
    };
    // Повтор того же job_id — текущий статус, второе задание не создаётся.
    if chat_id != 0
        && let Ok(Some(existing)) = app.store.get_job(&job_id).await
    {
        return Ok(Submitted::Queued { status: 200, body: queued_body(app, &job_id, &existing.model_used, existing.public_state()) });
    }

    if let Some(secs) = app.schedule().intake_paused(&model.name) {
        return Err(ApiError::new(503, "queue is paused by schedule").retry(secs));
    }

    // Резерв при недоступности модели: открытый выключатель или переполнение модели.
    let mut hops = 0usize;
    loop {
        let full = model.cfg.max_queue.is_some_and(|mq| app.queue.queued_in_model(&model.name) >= mq as usize);
        if model_available(app, &model) && !full {
            break;
        }
        let next = model.cfg.fallback.get(hops).and_then(|f| routing.resolve(f));
        match next {
            Some(n) if n.name != model.name && hops < 8 => {
                hops += 1;
                model = n;
            }
            _ if full => {
                return Err(ApiError::new(429, "queue is full").retry(retry_after_full(app, &model)));
            }
            _ => {
                return Err(ApiError::new(503, format!("model '{}' is unavailable: all upstreams are down", model.name)).retry(cfg.breaker.open_seconds.max(5)));
            }
        }
    }

    let payload: Map<String, Value> = body.iter().filter(|(k, _)| !QUEUE_FIELDS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    // Для журнала: сколько сообщений и картинок пришло, какие параметры (без текстов).
    let images = crate::admin::image_count(payload.get("messages"));
    let msg_count = payload.get("messages").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
    let body_keys: Vec<String> = payload.keys().filter(|k| *k != "messages").cloned().collect();
    if cfg.logging.requests {
        let shape = payload.get("messages").map(crate::admin::messages_summary).unwrap_or_default();
        tracing::info!(client, model = %model.name, chat_id, sync = chat_id == 0, params = ?body_keys, images, messages = %shape, "incoming request");
    }

    // Синхронный режим (§2.3).
    if chat_id == 0 {
        return submit_sync(app, client, &requested, &model, payload, opts.deltas).await;
    }

    if protocol1 && !(cfg.protocol1.enabled && !cfg.protocol1.telegram_bot_token.is_empty()) {
        return Err(ApiError::new(400, "protocol 1 (delivery=telegram) is disabled on this server"));
    }

    let (cb_url, cb_token) = match &opts.internal_callback {
        Some((u, t)) => (u.clone(), t.clone()),
        None => {
            let cb = body.get("callback");
            let url = cb.and_then(|c| c.get("url")).and_then(Value::as_str).map(String::from).filter(|s| !s.is_empty());
            let token = cb.and_then(|c| c.get("token")).and_then(Value::as_str).map(String::from);
            let (url, token) = match url {
                Some(u) => (u, token.unwrap_or_default()),
                None => (cfg.callback.url.clone(), token.filter(|t| !t.is_empty()).unwrap_or_else(|| cfg.callback.token.clone())),
            };
            if url.is_empty() {
                if !protocol1 {
                    return Err(ApiError::new(400, "no callback.url in job and BOT_CALLBACK_URL is not set"));
                }
            } else {
                validate_url(&url, &cfg.callback).map_err(|e| ApiError::new(400, e))?;
            }
            (url, token)
        }
    };

    // Вытеснение: ещё не начатые задания того же чата.
    if cfg.queue.supersede_same_chat {
        for old in app.queue.queued_of_chat(chat_id) {
            if let Some((m, item)) = app.queue.remove(&old) {
                let now = now_ms();
                if let Ok(true) = app.store.finish_err(&old, "superseded", "superseded", "superseded by a newer job of the same chat", None, now).await {
                    let kind = if chat_id > 0 { "private" } else { "group" };
                    app.record(&m, None, &item.client, kind, Some("superseded"), Some(now - item.created_at), None, 0, 0);
                    app.emit_job(&old, "superseded", &m, chat_id, Some("superseded"));
                    tracing::info!(job_id = %old, chat_id, superseded_by = %job_id, state = "superseded", "job superseded");
                }
            }
        }
        app.delivery_wake.notify_one();
    }

    if app.queue.total_queued() >= cfg.queue.max_queue as usize {
        return Err(ApiError::new(429, "queue is full").retry(retry_after_full(app, &model)));
    }
    if app.queue.queued_in_chat(chat_id) >= cfg.queue.max_queue_per_chat {
        return Err(ApiError::new(429, "too many queued jobs for this chat").retry(retry_after_full(app, &model)));
    }

    let priority = if cfg.queue.private_first && chat_id > 0 { 1 } else { 2 };
    let created_at = now_ms();
    let new = NewJob {
        job_id: job_id.clone(),
        client: client.to_string(),
        model: requested.clone(),
        model_used: model.name.clone(),
        chat_id,
        reply_to: as_i64(body.get("reply_to")),
        thread_id: as_i64(body.get("thread_id")),
        protocol: if protocol1 { 1 } else { 2 },
        priority,
        payload: Value::Object(payload).to_string(),
        callback_url: cb_url,
        callback_token: app.cipher.encrypt_str(&cb_token),
        created_at,
    };
    match app.store.insert_job(&new).await {
        Ok(true) => {}
        Ok(false) => {
            // Гонка двух одинаковых постановок: вторая получает статус первой.
            let state = app.store.get_job(&job_id).await.ok().flatten().map(|j| j.public_state().to_string()).unwrap_or("queued".into());
            return Ok(Submitted::Queued { status: 200, body: queued_body(app, &job_id, &model.name, &state) });
        }
        Err(e) => {
            tracing::error!(error = %e, "insert job failed");
            return Err(ApiError::new(503, "storage error").retry(5));
        }
    }
    let tier = if priority == 1 { TIER_PRIVATE } else { TIER_GROUP };
    app.queue.push(&model.name, tier, QItem { job_id: job_id.clone(), chat_id, created_at, client: client.to_string(), sync: None });
    app.emit_job(&job_id, "queued", &model.name, chat_id, None);
    let body = queued_body(app, &job_id, &model.name, "queued");
    tracing::info!(
        job_id = %job_id, client, model = %model.name, requested = %requested, chat_id, state = "queued",
        position = body["position"].as_u64().unwrap_or(0), protocol = if protocol1 { 1 } else { 2 }, messages = msg_count, images, "job queued"
    );
    Ok(Submitted::Queued { status: 202, body })
}

async fn submit_sync(
    app: &Arc<App>,
    client: &str,
    requested: &str,
    model: &Arc<ModelRt>,
    payload: Map<String, Value>,
    deltas: Option<mpsc::UnboundedSender<String>>,
) -> Result<Submitted, ApiError> {
    let cfg = app.settings();
    let job_id = format!("sync-{}", random_hex(8));
    let (tx, rx) = oneshot::channel::<SyncResult>();
    let created_at = now_ms();
    app.queue.push(
        &model.name,
        TIER_SYNC,
        QItem {
            job_id: job_id.clone(),
            chat_id: 0,
            created_at,
            client: client.to_string(),
            sync: Some(SyncCtx { payload, tx: Some(tx), deltas, client: client.to_string(), requested_model: requested.to_string() }),
        },
    );
    app.emit_job(&job_id, "queued", &model.name, 0, None);
    tracing::debug!(job_id = %job_id, client, model = %model.name, "sync request queued");
    match tokio::time::timeout(Duration::from_secs(cfg.queue.sync_timeout.max(1)), rx).await {
        Ok(Ok(Ok(body))) => Ok(Submitted::Sync(body)),
        Ok(Ok(Err((status, code, msg)))) => Err(ApiError::new(status, format!("{code}: {msg}"))),
        Ok(Err(_)) => Err(ApiError::new(502, "request dropped")),
        Err(_) => {
            // Запись — до отмены: отменённый запрос потом не перезапишет её «cancelled».
            let rec = SyncRecord {
                job_id: &job_id,
                client,
                model: requested,
                model_used: &model.name,
                state: "failed",
                error_code: Some("upstream_timeout"),
                error: Some("no answer within SYNC_TIMEOUT"),
                created_at,
                finished_at: now_ms(),
                ..Default::default()
            };
            if let Err(e) = app.store.insert_sync_record(&rec).await {
                tracing::error!(job_id = %job_id, error = %e, "store sync record failed");
            }
            tracing::warn!(
                job_id = %job_id, client, model = %model.name, chat_id = 0, state = "failed", error_code = "upstream_timeout",
                timeout_s = cfg.queue.sync_timeout, "sync request timed out"
            );
            if !app.queue.remove_sync(&job_id) {
                app.queue.cancel_running(&job_id);
            }
            app.emit_job(&job_id, "failed", &model.name, 0, Some("upstream_timeout"));
            Err(ApiError::new(504, format!("no answer within SYNC_TIMEOUT ({}s)", cfg.queue.sync_timeout)))
        }
    }
}

/// Отмена задания (§4.3). Возвращает новое публичное состояние.
pub async fn cancel(app: &Arc<App>, job_id: &str) -> anyhow::Result<Option<String>> {
    if let Some((model, item)) = app.queue.remove(job_id) {
        let now = now_ms();
        if app.store.finish_err(job_id, "cancelled", "cancelled", "cancelled via API", None, now).await? {
            let kind = if item.chat_id > 0 { "private" } else { "group" };
            app.record(&model, None, &item.client, kind, Some("cancelled"), Some(now - item.created_at), None, 0, 0);
            app.emit_job(job_id, "cancelled", &model, item.chat_id, Some("cancelled"));
            tracing::info!(job_id, model = %model, state = "cancelled", "job cancelled");
            app.delivery_wake.notify_one();
        }
        return Ok(Some("cancelled".into()));
    }
    if app.queue.cancel_running(job_id) {
        return Ok(Some("cancelled".into()));
    }
    Ok(app.store.get_job(job_id).await?.map(|j| j.public_state().to_string()))
}
