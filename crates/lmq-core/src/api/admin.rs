//! Admin API веб-панели (`/admin/api/*`) и сама панель (`/ui/`).
//!
//! Вход: `Authorization: Bearer <токен администратора>` (окно приложения, скрипты) или
//! cookie-сессия после `POST /admin/api/login` (браузер). С cookie изменяющие запросы
//! требуют заголовок `X-LMQ: 1` (защита от CSRF). SSE-потоки принимают `?token=`.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use futures::Stream;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tower_http::cors::{AllowOrigin, CorsLayer};

use super::bearer;
use crate::admin as svc;
use crate::app::App;
use crate::config::{DebugReply, Settings};
use crate::debug::Received;
use crate::events::Event;
use crate::stats;
use crate::store::JobFilter;
use crate::submit::{self, ApiError, SubmitOpts, Submitted};
use crate::util::{mask_secret, now_ms};

type AppState = Arc<App>;
type R<T> = Result<T, ApiError>;

const COOKIE: &str = "lmq_session";

pub fn router(app: Arc<App>) -> Router {
    let cfg = app.settings();
    let mut origins: Vec<HeaderValue> =
        ["tauri://localhost", "http://tauri.localhost", "https://tauri.localhost"].iter().filter_map(|o| HeaderValue::from_str(o).ok()).collect();
    origins.extend(cfg.server.admin_cors_origins.iter().filter_map(|o| HeaderValue::from_str(o).ok()));
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::HeaderName::from_static("x-lmq")]);

    let protected = Router::new()
        .route("/admin/api/overview", get(overview))
        .route("/admin/api/stats", get(stats_q))
        .route("/admin/api/queue", get(queue))
        .route("/admin/api/jobs", get(jobs))
        .route("/admin/api/jobs/redeliver-dead", post(redeliver_dead))
        .route("/admin/api/jobs/purge", post(purge))
        .route("/admin/api/jobs/{id}", get(job))
        .route("/admin/api/jobs/{id}/cancel", post(job_cancel))
        .route("/admin/api/jobs/{id}/redeliver", post(job_redeliver))
        .route("/admin/api/pause", post(pause))
        .route("/admin/api/settings", get(settings_get).put(settings_put).patch(settings_patch))
        .route("/admin/api/settings/validate", post(settings_validate))
        .route("/admin/api/clients", get(clients).post(client_create))
        .route("/admin/api/clients/{name}", delete(client_delete).patch(client_patch))
        .route("/admin/api/clients/{name}/rotate", post(client_rotate))
        .route("/admin/api/models/{name}/test", post(model_test))
        .route("/admin/api/upstreams/{id}/reset", post(upstream_reset))
        .route("/admin/api/models/{name}/upstreams/{index}/models", get(upstream_models))
        .route("/admin/api/upstreams/probe", post(upstream_probe))
        .route("/admin/api/network", get(network_get))
        .route("/admin/api/network/check", post(network_check))
        .route("/admin/api/tg-relay", get(tg_relay_get))
        .route("/admin/api/tg-relay/check", post(tg_relay_check))
        .route("/admin/api/socks", get(socks_get))
        .route("/admin/api/socks/check", post(socks_check))
        .route("/admin/api/engines", get(engines))
        .route("/admin/api/engines/detect", post(engine_detect))
        .route("/admin/api/engines/{id}/start", post(engine_start))
        .route("/admin/api/engines/{id}/stop", post(engine_stop))
        .route("/admin/api/engines/{id}/restart", post(engine_restart))
        .route("/admin/api/engines/{id}/logs", get(engine_logs).delete(engine_logs_clear))
        .route("/admin/api/logs", get(logs))
        .route("/admin/api/events", get(events))
        .route("/admin/api/sandbox", post(sandbox))
        .route("/admin/api/debug/callbacks", get(debug_list).delete(debug_clear))
        .route("/admin/api/debug/job", post(debug_job))
        .route("/admin/api/profiles", get(profiles).post(profile_create))
        .route("/admin/api/profiles/{name}", delete(profile_delete))
        .route("/admin/api/profiles/{name}/activate", post(profile_activate))
        .route("/admin/api/export", post(export))
        .route("/admin/api/import", post(import))
        .route("/admin/api/updates", get(updates_check))
        .route("/admin/api/updates/download", post(updates_download))
        .route("/admin/api/admin-token/reset", post(admin_token_reset))
        .route("/admin/api/about", get(about))
        .route_layer(middleware::from_fn_with_state(app.clone(), require_admin));

    Router::new()
        .route("/", get(crate::assets::root))
        .route("/ui", get(crate::assets::root))
        .route("/ui/", get(crate::assets::index))
        .route("/ui/{*path}", get(crate::assets::file))
        .route("/admin/api/login", post(login))
        .route("/admin/login", get(login_link))
        .route("/admin/api/logout", post(logout))
        .route("/admin/api/session", get(session))
        .route("/admin/debug/callback", post(debug_sink))
        .route("/healthz", get(|| async { Json(json!({ "ok": true })) }))
        .merge(protected)
        .layer(middleware::from_fn(log_failures))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(cors)
        .with_state(app)
}

/// Ошибки API панели — в журнал с путём и сообщением: иначе неудачная кнопка в панели
/// видна только в браузере. 401 не пишется (вход без токена — обычное дело).
async fn log_failures(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let resp = next.run(req).await;
    let status = resp.status();
    if !path.starts_with("/admin/api") || !(status.is_client_error() || status.is_server_error()) || status == StatusCode::UNAUTHORIZED {
        return resp;
    }
    let (parts, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, 256 * 1024).await.unwrap_or_default();
    let msg = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| String::from_utf8_lossy(&bytes).chars().take(300).collect());
    tracing::warn!(method = %method, path = %path, status = status.as_u16(), error = %msg, "admin request failed");
    Response::from_parts(parts, axum::body::Body::from(bytes))
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

fn query_token(uri: &axum::http::Uri) -> Option<String> {
    uri.query()?.split('&').find_map(|kv| kv.strip_prefix("token=").map(|t| t.to_string()))
}

async fn require_admin(State(app): State<AppState>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    if let Some(t) = bearer(headers) {
        if app.check_admin_token(t) {
            return next.run(req).await;
        }
        return ApiError::new(401, "invalid admin token").into_response();
    }
    if req.method() == Method::GET
        && let Some(t) = query_token(req.uri())
        && (app.check_admin_token(&t) || app.check_session(&t))
    {
        return next.run(req).await;
    }
    if let Some(s) = cookie_value(headers, COOKIE)
        && app.check_session(s)
    {
        let safe = req.method() == Method::GET || req.method() == Method::HEAD;
        if !safe && headers.get("x-lmq").and_then(|v| v.to_str().ok()) != Some("1") {
            return ApiError::new(403, "missing X-LMQ header").into_response();
        }
        return next.run(req).await;
    }
    ApiError::new(401, "authentication required").into_response()
}

// ------------------------------------------------------------------- вход

static LOGIN_FAILS: std::sync::OnceLock<Mutex<HashMap<String, (u32, i64)>>> = std::sync::OnceLock::new();

#[derive(Deserialize)]
struct LoginReq {
    token: String,
}

async fn login(State(app): State<AppState>, peer: Option<Extension<ConnectInfo<SocketAddr>>>, Json(req): Json<LoginReq>) -> R<Response> {
    let ip = peer.map(|Extension(ConnectInfo(a))| a.ip().to_string()).unwrap_or_default();
    let fails = LOGIN_FAILS.get_or_init(Default::default);
    {
        let mut f = fails.lock();
        let now = now_ms();
        let e = f.entry(ip.clone()).or_insert((0, now));
        if now - e.1 > 60_000 {
            *e = (0, now);
        }
        if e.0 >= 10 {
            return Err(ApiError::new(429, "too many attempts, try again in a minute").retry(60));
        }
    }
    if !app.check_admin_token(req.token.trim()) {
        fails.lock().entry(ip.clone()).or_insert((0, now_ms())).0 += 1;
        tokio::time::sleep(Duration::from_millis(400)).await;
        tracing::warn!(ip = %ip, "admin login failed");
        return Err(ApiError::new(401, "invalid admin token"));
    }
    let s = app.create_session();
    tracing::info!(ip = %ip, "admin login");
    let cookie = format!("{COOKIE}={s}; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000");
    let mut resp = Json(json!({ "ok": true, "session": s })).into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    Ok(resp)
}

#[derive(Deserialize)]
struct LinkQ {
    once: String,
    /// Экран панели: `#/queue/<job_id>` и т. п.
    #[serde(default)]
    to: String,
}

/// Одноразовая ссылка из окна приложения: cookie-сессия и переход в панель.
async fn login_link(State(app): State<AppState>, Query(q): Query<LinkQ>) -> Response {
    if !app.redeem_login_link(&q.once) {
        return (StatusCode::FORBIDDEN, "link expired").into_response();
    }
    let s = app.create_session();
    // Только фрагмент маршрута панели: открытого редиректа на чужой адрес не бывает.
    let safe = q.to.starts_with("#/") && q.to.chars().all(|c| c.is_ascii_alphanumeric() || "#/?=&_-.%".contains(c));
    let location = if safe { format!("/ui/{}", q.to) } else { "/ui/".to_string() };
    let mut resp = (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response();
    if let Ok(v) = HeaderValue::from_str(&format!("{COOKIE}={s}; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000")) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

async fn logout(State(app): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(s) = cookie_value(&headers, COOKIE) {
        app.drop_session(s);
    }
    let mut resp = Json(json!({ "ok": true })).into_response();
    resp.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_static("lmq_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"));
    resp
}

async fn session(State(app): State<AppState>, headers: HeaderMap) -> Json<Value> {
    let ok = bearer(&headers).is_some_and(|t| app.check_admin_token(t)) || cookie_value(&headers, COOKIE).is_some_and(|s| app.check_session(s));
    Json(json!({ "authenticated": ok, "version": crate::app::VERSION }))
}

// ---------------------------------------------------------------- обзор

async fn overview(State(app): State<AppState>) -> Json<svc::Overview> {
    Json(svc::overview(&app).await)
}

#[derive(Deserialize, Default)]
struct StatsQ {
    range: Option<String>,
    group: Option<String>,
    model: Option<String>,
    upstream: Option<String>,
    client: Option<String>,
}

async fn stats_q(State(app): State<AppState>, Query(q): Query<StatsQ>) -> R<Json<stats::Series>> {
    let query = stats::Query {
        range: q.range.as_deref().map(stats::Range::parse),
        group: q.group.as_deref().map(stats::Group::parse),
        model: q.model.filter(|s| !s.is_empty()),
        upstream: q.upstream.filter(|s| !s.is_empty()),
        client: q.client.filter(|s| !s.is_empty()),
    };
    Ok(Json(app.stats.query(&query).await?))
}

async fn queue(State(app): State<AppState>) -> Json<Value> {
    Json(super::client::queue_json(&app).await)
}

// --------------------------------------------------------------- задания

#[derive(Deserialize, Default)]
struct JobsQ {
    state: Option<String>,
    q: Option<String>,
    model: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn jobs(State(app): State<AppState>, Query(q): Query<JobsQ>) -> R<Json<Value>> {
    let f = JobFilter { state: q.state, query: q.q, model: q.model, limit: q.limit.unwrap_or(100), offset: q.offset.unwrap_or(0) };
    let (rows, total) = app.store.list_jobs(&f).await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|j| {
            let mut v = serde_json::to_value(j).unwrap_or(Value::Null);
            v["public_state"] = json!(j.public_state());
            v["position"] = json!(app.queue.position(&j.job_id).map(|p| p + 1));
            v
        })
        .collect();
    let counts: HashMap<String, i64> = app.store.count_by_state().await?.into_iter().collect();
    Ok(Json(json!({ "items": items, "total": total, "counts": counts })))
}

async fn job(State(app): State<AppState>, Path(id): Path<String>) -> R<Json<Value>> {
    svc::job_detail(&app, &id).await?.map(Json).ok_or_else(|| ApiError::new(404, "job not found"))
}

async fn job_cancel(State(app): State<AppState>, Path(id): Path<String>) -> R<Json<Value>> {
    let state = submit::cancel(&app, &id).await?.ok_or_else(|| ApiError::new(404, "job not found"))?;
    Ok(Json(json!({ "job_id": id, "state": state })))
}

async fn job_redeliver(State(app): State<AppState>, Path(id): Path<String>) -> R<Json<Value>> {
    let ok = svc::redeliver(&app, &id).await?;
    if !ok {
        return Err(ApiError::new(409, "job is not dead"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn redeliver_dead(State(app): State<AppState>) -> R<Json<Value>> {
    Ok(Json(json!({ "count": svc::redeliver_all(&app).await? })))
}

#[derive(Deserialize)]
struct PurgeReq {
    states: Vec<String>,
}

async fn purge(State(app): State<AppState>, Json(req): Json<PurgeReq>) -> R<Json<Value>> {
    let n = app.store.purge(&req.states).await?;
    tracing::info!(removed = n, states = ?req.states, "jobs purged from panel");
    Ok(Json(json!({ "removed": n })))
}

#[derive(Deserialize)]
struct PauseReq {
    paused: bool,
}

async fn pause(State(app): State<AppState>, Json(req): Json<PauseReq>) -> Json<Value> {
    app.paused.store(req.paused, Ordering::Relaxed);
    app.queue.wake.notify_one();
    tracing::info!(paused = req.paused, "dispatch pause toggled");
    Json(json!({ "paused": req.paused }))
}

// ------------------------------------------------------------- настройки

async fn settings_get(State(app): State<AppState>) -> Json<Value> {
    Json(svc::settings_view(&app))
}

fn parse_json<T: serde::de::DeserializeOwned>(body: Result<Bytes, BytesRejection>) -> R<T> {
    let b = body.map_err(|e| ApiError::new(400, e.body_text()))?;
    serde_json::from_slice(&b).map_err(|e| ApiError::new(400, format!("invalid JSON: {e}")))
}

fn cfg_err(e: anyhow::Error) -> ApiError {
    ApiError::new(400, format!("{e:#}"))
}

async fn settings_put(State(app): State<AppState>, body: Result<Bytes, BytesRejection>) -> R<Json<Value>> {
    let s: Settings = parse_json(body)?;
    svc::update_settings(&app, s).map_err(cfg_err)?;
    Ok(Json(svc::settings_view(&app)))
}

async fn settings_patch(State(app): State<AppState>, body: Result<Bytes, BytesRejection>) -> R<Json<Value>> {
    let v: Value = parse_json(body)?;
    svc::patch_settings(&app, v).map_err(cfg_err)?;
    Ok(Json(svc::settings_view(&app)))
}

async fn settings_validate(State(app): State<AppState>, body: Result<Bytes, BytesRejection>) -> R<Json<Value>> {
    let s: Settings = parse_json(body)?;
    let errors = svc::validate_settings(&app, &s);
    Ok(Json(json!({ "ok": errors.is_empty(), "errors": errors })))
}

// --------------------------------------------------------------- клиенты

async fn clients(State(app): State<AppState>) -> Json<Value> {
    Json(json!({ "items": svc::clients(&app) }))
}

#[derive(Deserialize)]
struct ClientReq {
    name: String,
    #[serde(default)]
    note: String,
}

async fn client_create(State(app): State<AppState>, Json(req): Json<ClientReq>) -> R<Json<Value>> {
    let key = svc::create_client(&app, &req.name, &req.note).map_err(cfg_err)?;
    Ok(Json(json!({ "name": req.name, "key": key })))
}

#[derive(Deserialize)]
struct ClientPatch {
    enabled: Option<bool>,
    note: Option<String>,
}

async fn client_patch(State(app): State<AppState>, Path(name): Path<String>, Json(req): Json<ClientPatch>) -> R<Json<Value>> {
    svc::set_client(&app, &name, req.enabled, req.note).map_err(cfg_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn client_delete(State(app): State<AppState>, Path(name): Path<String>) -> R<Json<Value>> {
    svc::delete_client(&app, &name).map_err(cfg_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn client_rotate(State(app): State<AppState>, Path(name): Path<String>) -> R<Json<Value>> {
    let key = svc::rotate_client(&app, &name).map_err(cfg_err)?;
    Ok(Json(json!({ "name": name, "key": key })))
}

// ---------------------------------------------------------------- модели

#[derive(Deserialize, Default)]
struct TestReq {
    prompt: Option<String>,
}

async fn model_test(State(app): State<AppState>, Path(name): Path<String>, body: Option<Json<TestReq>>) -> Json<Value> {
    let prompt = body.and_then(|Json(b)| b.prompt);
    Json(svc::test_model(&app, &name, prompt.as_deref()).await)
}

async fn upstream_reset(State(app): State<AppState>, Path(id): Path<String>) -> R<Json<Value>> {
    let routing = app.routing();
    let up = routing.upstreams().find(|u| u.id == id).ok_or_else(|| ApiError::new(404, "upstream not found"))?;
    up.health.reset();
    app.emit_upstream(up, "closed", None);
    app.queue.wake.notify_one();
    Ok(Json(json!({ "ok": true })))
}

async fn upstream_models(State(app): State<AppState>, Path((name, index)): Path<(String, usize)>) -> R<Json<Value>> {
    let ids = svc::upstream_models(&app, &name, index).await.map_err(|e| ApiError::new(502, e.to_string()))?;
    Ok(Json(json!({ "items": ids })))
}

async fn upstream_probe(State(app): State<AppState>, body: Option<Json<svc::UpstreamProbe>>) -> Json<Value> {
    Json(svc::probe_upstream(&app, body.map(|Json(b)| b).unwrap_or_default()).await)
}

// ---------------------------------------------------------------- сеть

async fn network_get(State(app): State<AppState>) -> Json<Value> {
    Json(svc::network_view(&app).await)
}

#[derive(Deserialize, Default)]
struct NetCheckReq {
    /// Свой адрес; без него проверяются все маршруты из настроек.
    url: Option<String>,
    interface: Option<String>,
    proxy: Option<String>,
}

async fn network_check(State(app): State<AppState>, body: Option<Json<NetCheckReq>>) -> Json<Value> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    let custom = req
        .url
        .filter(|u| !u.trim().is_empty())
        .map(|u| (u, crate::config::RouteCfg { interface: req.interface.unwrap_or_default(), proxy: req.proxy.unwrap_or_default() }));
    Json(json!({ "items": svc::network_check(&app, custom).await }))
}

async fn tg_relay_get(State(app): State<AppState>) -> Json<Value> {
    Json(svc::tg_relay_view(&app))
}

#[derive(Deserialize, Default)]
struct RelayCheckReq {
    /// Токен бота для `getMe`; не сохраняется и не пишется в журнал.
    token: Option<String>,
}

async fn tg_relay_check(State(app): State<AppState>, body: Option<Json<RelayCheckReq>>) -> Json<Value> {
    let req = body.map(|Json(b)| b).unwrap_or_default();
    Json(svc::tg_relay_check(&app, req.token).await)
}

#[derive(Deserialize, Default)]
struct SocksQuery {
    /// `1` — заново узнать внешний IP.
    refresh: Option<u8>,
}

async fn socks_get(State(app): State<AppState>, Query(q): Query<SocksQuery>) -> Json<Value> {
    Json(svc::socks_view(&app, q.refresh == Some(1)).await)
}

async fn socks_check(State(app): State<AppState>) -> Json<Value> {
    Json(svc::socks_check(&app).await)
}

// ---------------------------------------------------------------- движки

async fn engines(State(app): State<AppState>) -> Json<Value> {
    Json(json!({ "items": app.engines.list() }))
}

#[derive(Deserialize)]
struct DetectReq {
    exe: String,
}

async fn engine_detect(Json(req): Json<DetectReq>) -> Json<Value> {
    match crate::engines::EngineManager::detect(&req.exe).await {
        Ok((path, version)) => Json(json!({ "ok": true, "path": path, "version": version })),
        Err(e) => Json(json!({ "ok": false, "error": e.to_string() })),
    }
}

async fn engine_start(State(app): State<AppState>, Path(id): Path<String>) -> R<Json<Value>> {
    app.engines.start(&id).map_err(|e| ApiError::new(404, e.to_string()))?;
    tracing::info!(engine = %id, "engine start requested");
    Ok(Json(json!({ "ok": true })))
}

async fn engine_stop(State(app): State<AppState>, Path(id): Path<String>) -> R<Json<Value>> {
    app.engines.stop(&id).map_err(|e| ApiError::new(404, e.to_string()))?;
    tracing::info!(engine = %id, "engine stop requested");
    Ok(Json(json!({ "ok": true })))
}

async fn engine_restart(State(app): State<AppState>, Path(id): Path<String>) -> R<Json<Value>> {
    app.engines.restart(&id).await.map_err(|e| ApiError::new(404, e.to_string()))?;
    tracing::info!(engine = %id, "engine restart requested");
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
struct TailQ {
    tail: Option<usize>,
    level: Option<String>,
    q: Option<String>,
}

async fn engine_logs(State(app): State<AppState>, Path(id): Path<String>, Query(q): Query<TailQ>) -> Json<Value> {
    Json(json!({ "items": app.engines.logs(&id, q.tail.unwrap_or(1000).min(5000)) }))
}

async fn engine_logs_clear(State(app): State<AppState>, Path(id): Path<String>) -> Json<Value> {
    app.engines.clear_logs(&id);
    Json(json!({ "ok": true }))
}

async fn logs(Query(q): Query<TailQ>) -> Json<Value> {
    Json(json!({ "items": crate::logging::hub().tail(q.tail.unwrap_or(500).min(3000), q.level.as_deref(), q.q.as_deref()) }))
}

// ------------------------------------------------------------------- SSE

fn event_name(e: &Event) -> &'static str {
    match e {
        Event::Job { .. } => "job",
        Event::Log(_) => "log",
        Event::Engine { .. } => "engine",
        Event::EngineLog { .. } => "engine_log",
        Event::Upstream { .. } => "upstream",
        Event::DebugCallback(_) => "debug_callback",
        Event::Config { .. } => "config",
        Event::Tick(_) => "tick",
    }
}

async fn events(State(app): State<AppState>) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut rx = app.events.subscribe();
    let mut epoch = app.listeners_epoch.subscribe();
    let shutdown = app.shutdown.clone();
    let first = svc::overview(&app).await;
    let stream = async_stream::stream! {
        if let Ok(data) = serde_json::to_string(&first) {
            yield Ok(SseEvent::default().event("tick").data(data));
        }
        loop {
            tokio::select! {
                r = rx.recv() => match r {
                    Ok(e) => {
                        let name = event_name(&e);
                        if let Ok(data) = serde_json::to_value(&e) {
                            // Без обёртки type: данные события как есть.
                            let payload = match data {
                                Value::Object(mut m) => { m.remove("type"); Value::Object(m) }
                                v => v,
                            };
                            yield Ok(SseEvent::default().event(name).data(payload.to_string()));
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        yield Ok(SseEvent::default().event("lagged").data(n.to_string()));
                    }
                    Err(_) => break,
                },
                _ = epoch.changed() => break,
                _ = shutdown.cancelled() => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[derive(Deserialize)]
struct SandboxReq {
    models: Vec<String>,
    messages: Vec<Value>,
    #[serde(default)]
    params: Map<String, Value>,
}

/// Песочница: один запрос нескольким моделям параллельно, ответ потоком.
async fn sandbox(State(app): State<AppState>, Json(req): Json<SandboxReq>) -> R<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    if req.models.is_empty() || req.models.len() > 4 {
        return Err(ApiError::new(400, "choose from 1 to 4 models"));
    }
    if req.messages.is_empty() {
        return Err(ApiError::new(400, "messages are empty"));
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<(String, Value)>();
    for model in req.models.clone() {
        let app = app.clone();
        let tx = tx.clone();
        let mut body = req.params.clone();
        body.insert("model".into(), json!(model));
        body.insert("messages".into(), Value::Array(req.messages.clone()));
        body.insert("chat_id".into(), json!(0));
        tokio::spawn(async move {
            let (dtx, mut drx) = mpsc::unbounded_channel::<crate::upstream::Delta>();
            let started = now_ms();
            let first_token = Arc::new(Mutex::new(None::<i64>));
            let fwd_tx = tx.clone();
            let fwd_model = model.clone();
            let ft = first_token.clone();
            let fwd = tokio::spawn(async move {
                while let Some(d) = drx.recv().await {
                    let crate::upstream::Delta::Text(piece) = d else { continue };
                    ft.lock().get_or_insert(now_ms() - started);
                    let _ = fwd_tx.send(("delta".into(), json!({ "model": fwd_model, "text": piece })));
                }
            });
            let res = submit::submit(&app, "admin", body, SubmitOpts { internal_callback: None, deltas: Some(dtx) }).await;
            let _ = fwd.await;
            let latency = now_ms() - started;
            let done = match res {
                Ok(Submitted::Sync(b)) => {
                    let (pt, ct) = crate::upstream::usage_tokens(b.get("usage"));
                    let text = crate::upstream::extract_text(&b);
                    let gen_s = (latency as f64 / 1000.0).max(0.001);
                    json!({
                        "model": model, "ok": true, "latency_ms": latency, "first_token_ms": *first_token.lock(),
                        "prompt_tokens": pt, "completion_tokens": ct, "tps": if ct > 0 { ct as f64 / gen_s } else { 0.0 },
                        "chars": text.chars().count(), "text": text,
                    })
                }
                Ok(Submitted::Queued { .. }) => json!({ "model": model, "ok": false, "error": "unexpected queued answer" }),
                Err(e) => json!({ "model": model, "ok": false, "status": e.status, "error": e.message, "latency_ms": latency }),
            };
            let _ = tx.send(("done".into(), done));
        });
    }
    drop(tx);
    let stream = async_stream::stream! {
        while let Some((name, data)) = rx.recv().await {
            yield Ok(SseEvent::default().event(name).data(data.to_string()));
        }
        yield Ok(SseEvent::default().event("end").data("{}"));
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(10))))
}

// ----------------------------------------------------------------- отладка

/// Приёмник колбэков. Без авторизации: принимает всё и показывает в панели.
async fn debug_sink(State(app): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let cfg = app.settings().debug.clone();
    let status = match cfg.reply {
        DebugReply::Ok => 200,
        DebugReply::Forbidden => 403,
        DebugReply::ServerError => 500,
    };
    let body_v: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!(String::from_utf8_lossy(&body).chars().take(4000).collect::<String>()));
    let rec = app.debug.record(Received {
        id: 0,
        ts: now_ms(),
        token: headers.get("x-callback-token").and_then(|v| v.to_str().ok()).map(mask_secret),
        content_type: headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(String::from),
        body: body_v,
        replied: status,
        delay_ms: cfg.delay_ms,
    });
    app.events.emit(Event::DebugCallback(rec));
    if cfg.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(cfg.delay_ms.min(120_000))).await;
    }
    (StatusCode::from_u16(status).unwrap_or(StatusCode::OK), Json(json!({ "ok": status == 200 }))).into_response()
}

async fn debug_list(State(app): State<AppState>) -> Json<Value> {
    Json(json!({ "items": app.debug.list(), "config": app.settings().debug }))
}

async fn debug_clear(State(app): State<AppState>) -> Json<Value> {
    app.debug.clear();
    Json(json!({ "ok": true }))
}

#[derive(Deserialize)]
struct DebugJobReq {
    model: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    chat_id: i64,
}

async fn debug_job(State(app): State<AppState>, Json(req): Json<DebugJobReq>) -> R<Json<Value>> {
    let text = if req.text.trim().is_empty() { "Привет! Ответь одним словом." } else { req.text.as_str() };
    let v = svc::debug_job(&app, &req.model, text, req.chat_id).await.map_err(cfg_err)?;
    Ok(Json(v))
}

// ---------------------------------------------------------------- профили

async fn profiles(State(app): State<AppState>) -> Json<Value> {
    Json(json!({ "items": svc::profiles(&app), "active": app.effective().profile }))
}

#[derive(Deserialize)]
struct ProfileReq {
    name: String,
}

async fn profile_create(State(app): State<AppState>, Json(req): Json<ProfileReq>) -> R<Json<Value>> {
    svc::create_profile(&app, &req.name).map_err(cfg_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn profile_delete(State(app): State<AppState>, Path(name): Path<String>) -> R<Json<Value>> {
    svc::delete_profile(&app, &name).map_err(cfg_err)?;
    Ok(Json(json!({ "ok": true })))
}

async fn profile_activate(State(app): State<AppState>, Path(name): Path<String>) -> R<Json<Value>> {
    app.switch_profile(&name).map_err(cfg_err)?;
    Ok(Json(json!({ "ok": true, "active": name })))
}

#[derive(Deserialize, Default)]
struct ExportReq {
    password: Option<String>,
}

async fn export(State(app): State<AppState>, body: Option<Json<ExportReq>>) -> R<Response> {
    let pw = body.and_then(|Json(b)| b.password);
    let text = svc::export_settings(&app, pw.as_deref()).map_err(cfg_err)?;
    let name = format!("lmq-settings-{}.yaml", chrono::Local::now().format("%Y%m%d-%H%M"));
    Ok((
        [(header::CONTENT_TYPE, "application/x-yaml; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))],
        text,
    )
        .into_response())
}

#[derive(Deserialize)]
struct ImportReq {
    text: String,
    password: Option<String>,
    profile: Option<String>,
}

async fn import(State(app): State<AppState>, Json(req): Json<ImportReq>) -> R<Json<Value>> {
    svc::import_settings(&app, &req.text, req.password.as_deref(), req.profile.as_deref()).map_err(cfg_err)?;
    Ok(Json(json!({ "ok": true })))
}

// --------------------------------------------------------------- обновления

async fn updates_check(State(app): State<AppState>) -> Json<crate::updates::UpdateInfo> {
    Json(crate::updates::check(&app).await)
}

async fn updates_download(State(app): State<AppState>) -> R<Json<Value>> {
    let info = crate::updates::check(&app).await;
    let (path, sig) = crate::updates::download_verified(&app, &info).await.map_err(cfg_err)?;
    Ok(Json(json!({
        "ok": true,
        "path": path,
        "authenticode": sig,
        "authenticode_text": sig.describe(),
        "verdict": crate::authenticode::verdict(&sig),
    })))
}

async fn admin_token_reset(State(app): State<AppState>) -> R<Json<Value>> {
    let token = app.reset_admin_token()?;
    tracing::warn!("admin token was reset from the panel");
    Ok(Json(json!({ "token": token })))
}

async fn about(State(app): State<AppState>) -> Json<Value> {
    Json(json!({
        "version": crate::app::VERSION,
        "started_at": app.started_at,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "gpu_feature": cfg!(feature = "gpu"),
        "paths": svc::settings_view(&app)["paths"],
        "bound": app.bound.lock().clone(),
    }))
}
