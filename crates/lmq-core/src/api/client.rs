//! Клиентский API (§2, §4): то, что вызывает бот.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::{Map, Value, json};
use tower::ServiceExt;
use tower_http::timeout::RequestBodyTimeoutLayer;

use super::{bearer, client_auth};
use crate::app::App;
use crate::submit::{self, ApiError, SubmitOpts, Submitted};
use crate::util::ct_eq;

pub fn router(app: Arc<App>) -> Router {
    let cfg = app.settings();
    let limit = (cfg.server.max_body_mb.max(1) as usize) * 1024 * 1024;
    let inner = Router::new()
        .route("/", get(index))
        .route("/v1/chat/completions", post(chat))
        .route("/v1/models", get(models))
        .route("/v1/jobs/{job_id}", get(job_get).delete(job_delete))
        .route("/v1/queue", get(queue_summary))
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .fallback(not_found)
        .method_not_allowed_fallback(wrong_method)
        .layer(DefaultBodyLimit::max(limit))
        .layer(RequestBodyTimeoutLayer::new(Duration::from_secs(cfg.server.body_read_timeout.max(1))))
        .with_state(app.clone());
    // Путь приводится к каноническому до маршрутизации: клиенты и прокси присылают
    // `/chat/completions/`, `/v1/chat/completions` с префиксом прокси и т. п.
    // Запросы к Bot API (`/bot<TOKEN>/…`) идут в реле со своими лимитами — и мимо журнала
    // неизвестных путей: в пути токен.
    Router::new().fallback(move |mut req: Request| {
        let inner = inner.clone();
        let app = app.clone();
        async move {
            if crate::tgrelay::is_relay_path(req.uri().path()) {
                return crate::tgrelay::handle(app, req).await;
            }
            if let Some(uri) = canonical_uri(req.uri()) {
                tracing::debug!(from = %req.uri().path(), to = %uri.path(), "api path normalized");
                *req.uri_mut() = uri;
            }
            inner.oneshot(req).await.unwrap_or_else(|e| match e {})
        }
    })
}

/// Канонический путь клиентского API: без завершающего `/`, без префикса прокси, с `/v1`.
/// `None` — путь уже канонический или не относится к API.
pub fn canonical_path(path: &str) -> Option<&'static str> {
    let p = path.trim_end_matches('/');
    const SUFFIXES: [(&str, &str); 5] =
        [("/chat/completions", "/v1/chat/completions"), ("/models", "/v1/models"), ("/queue", "/v1/queue"), ("/healthz", "/healthz"), ("/metrics", "/metrics")];
    SUFFIXES.iter().find(|(s, _)| p.ends_with(s)).map(|(_, t)| *t).filter(|t| *t != path)
}

fn canonical_uri(uri: &Uri) -> Option<Uri> {
    let path = uri.path();
    let target = match canonical_path(path) {
        Some(t) => t.to_string(),
        // `/jobs/{id}` с любым префиксом или завершающим `/`.
        None => {
            let p = path.trim_end_matches('/');
            let i = p.rfind("/jobs/")?;
            let id = &p[i + "/jobs/".len()..];
            if id.is_empty() || id.contains('/') {
                return None;
            }
            let t = format!("/v1/jobs/{id}");
            if t == path {
                return None;
            }
            t
        }
    };
    let pq = match uri.query() {
        Some(q) => format!("{target}?{q}"),
        None => target,
    };
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(pq.parse().ok()?);
    Uri::from_parts(parts).ok()
}

/// `GET /` — подсказка вместо пустого 404, если адрес открыли в браузере.
async fn index() -> Json<Value> {
    Json(json!({
        "service": "LMQ queue server",
        "version": crate::app::VERSION,
        "endpoints": {
            "chat": "POST /v1/chat/completions",
            "models": "GET /v1/models",
            "job": "GET|DELETE /v1/jobs/{job_id}",
            "queue": "GET /v1/queue",
            "health": "GET /healthz",
        },
        "auth": "Authorization: Bearer <client key>",
    }))
}

async fn not_found(method: Method, uri: Uri, peer: Option<Extension<ConnectInfo<SocketAddr>>>) -> ApiError {
    let ip = peer.map(|Extension(ConnectInfo(a))| a.ip().to_string()).unwrap_or_default();
    tracing::info!(method = %method, path = %uri.path(), peer = %ip, "unknown API path");
    ApiError::new(404, format!("unknown path {} {}: the bot API is POST /v1/chat/completions and GET /v1/models", method, uri.path()))
}

async fn wrong_method(method: Method, uri: Uri) -> ApiError {
    tracing::info!(method = %method, path = %uri.path(), "wrong method for API path");
    let hint = if uri.path() == "/v1/chat/completions" { "use POST with a JSON body" } else { "use GET" };
    ApiError::new(405, format!("method {} is not allowed for {}: {hint}", method, uri.path()))
}

/// Тело запроса как JSON-объект; превышение лимита — 413 в форме ошибки OpenAI.
pub fn parse_body(body: Result<Bytes, BytesRejection>) -> Result<Map<String, Value>, ApiError> {
    let bytes = body.map_err(|r| {
        if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(413, "request body is too large")
        } else {
            ApiError::new(400, format!("cannot read body: {}", r.body_text()))
        }
    })?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(ApiError::new(400, "body must be a JSON object")),
        Err(e) => Err(ApiError::new(400, format!("invalid JSON: {e}"))),
    }
}

async fn chat(State(app): State<Arc<App>>, headers: HeaderMap, body: Result<Bytes, BytesRejection>) -> Result<Response, ApiError> {
    let client = client_auth(&app, &headers)?;
    let body = parse_body(body)?;
    if body.get("stream").and_then(Value::as_bool) == Some(true) && body.get("chat_id").is_none_or(|c| c.as_i64() == Some(0) || c.is_null()) {
        // Синхронный стрим сервер не отдаёт: отвечаем обычным ответом.
        tracing::debug!(client = %client, "stream=true ignored in sync mode");
    }
    let model = body.get("model").and_then(Value::as_str).unwrap_or("").to_string();
    let res = submit::submit(&app, &client, body, SubmitOpts::default()).await.inspect_err(|e| {
        // Отказ до постановки (400, 429, 503 …) — иначе в журнале его не видно.
        tracing::info!(client = %client, model = %model, status = e.status, error = %e.message, "request rejected");
    })?;
    match res {
        Submitted::Queued { status, body } => Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::ACCEPTED), Json(body)).into_response()),
        Submitted::Sync(body) => Ok(Json(body).into_response()),
    }
}

async fn models(State(app): State<Arc<App>>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    client_auth(&app, &headers)?;
    let routing = app.routing();
    let mut data: Vec<Value> = routing
        .models
        .values()
        .map(|m| {
            let mut v = json!({ "id": m.name, "object": "model", "owned_by": "local" });
            if !m.cfg.description.is_empty() {
                v["description"] = json!(m.cfg.description);
            }
            add_capabilities(&mut v, m.cfg.vision);
            v
        })
        .collect();
    for (alias, target) in &routing.aliases {
        let mut v = json!({ "id": alias, "object": "model", "owned_by": "local", "alias_of": target });
        add_capabilities(&mut v, routing.models.get(target).is_some_and(|m| m.cfg.vision));
        data.push(v);
    }
    Ok(Json(json!({ "object": "list", "data": data })))
}

/// Возможности модели в распространённых формах (OpenAI-совместимые серверы, OpenRouter).
fn add_capabilities(v: &mut Value, vision: bool) {
    let input: &[&str] = if vision { &["text", "image"] } else { &["text"] };
    v["input_modalities"] = json!(input);
    v["architecture"] = json!({ "input_modalities": input, "output_modalities": ["text"] });
    if vision {
        v["capabilities"] = json!(["chat", "vision"]);
        v["vision"] = json!(true);
    }
}

fn job_view(app: &App, j: &crate::store::JobRow) -> Value {
    json!({
        "job_id": j.job_id,
        "state": j.public_state(),
        "position": app.queue.position(&j.job_id).map(|p| p + 1),
        "model": j.model,
        "model_used": j.model_used,
        "chat_id": j.chat_id,
        "attempts": j.attempts,
        "created_at": j.created_at,
        "started_at": j.started_at,
        "finished_at": j.finished_at,
        "queued_ms": j.queued_ms,
        "latency_ms": j.latency_ms,
        "error": j.error,
        "error_code": j.error_code,
        "delivery": j.delivery,
        "delivery_error": j.delivery_error,
    })
}

async fn job_get(State(app): State<Arc<App>>, headers: HeaderMap, Path(job_id): Path<String>) -> Result<Json<Value>, ApiError> {
    let client = client_auth(&app, &headers)?;
    let job = app.store.get_job(&job_id).await?.filter(|j| j.client == client).ok_or_else(|| ApiError::new(404, "job not found"))?;
    Ok(Json(job_view(&app, &job)))
}

async fn job_delete(State(app): State<Arc<App>>, headers: HeaderMap, Path(job_id): Path<String>) -> Result<Json<Value>, ApiError> {
    let client = client_auth(&app, &headers)?;
    app.store.get_job(&job_id).await?.filter(|j| j.client == client).ok_or_else(|| ApiError::new(404, "job not found"))?;
    let state = submit::cancel(&app, &job_id).await?.unwrap_or_else(|| "unknown".into());
    Ok(Json(json!({ "job_id": job_id, "state": state })))
}

async fn queue_summary(State(app): State<Arc<App>>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    client_auth(&app, &headers)?;
    Ok(Json(queue_json(&app).await))
}

pub async fn queue_json(app: &App) -> Value {
    let load = app.queue.load();
    let routing = app.routing();
    let w5 = app.stats.window(5).await.unwrap_or_default();
    let w60 = app.stats.window(60).await.unwrap_or_default();
    let mut models = BTreeMap::new();
    for (name, m) in &routing.models {
        let l = load.get(name).cloned().unwrap_or_default();
        let p5 = w5.get(name);
        let p60 = w60.get(name);
        models.insert(
            name.clone(),
            json!({
                "queued": l.queued,
                "running": l.running,
                "slots": m.slots(),
                "wait_avg_ms": { "5m": p5.and_then(|p| p.wait_avg), "60m": p60.and_then(|p| p.wait_avg) },
                "latency_avg_ms": { "5m": p5.and_then(|p| p.lat_avg), "60m": p60.and_then(|p| p.lat_avg) },
                "jobs": { "5m": p5.map(|p| p.jobs).unwrap_or(0), "60m": p60.map(|p| p.jobs).unwrap_or(0) },
            }),
        );
    }
    json!({
        "queued": app.queue.total_queued(),
        "running": app.queue.total_running(),
        "paused": app.paused.load(std::sync::atomic::Ordering::Relaxed),
        "models": models,
    })
}

async fn healthz(State(app): State<Arc<App>>) -> Response {
    let db = app.store.ping().await;
    let routing = app.routing();
    let any_up = routing.upstreams().any(|u| crate::dispatcher::upstream_usable(&app, u) && u.health.snapshot().reachable != Some(false));
    if db && any_up { Json(json!({ "ok": true })).into_response() } else { (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "ok": false }))).into_response() }
}

async fn metrics(State(app): State<Arc<App>>, headers: HeaderMap, peer: Option<Extension<ConnectInfo<SocketAddr>>>) -> Result<Response, ApiError> {
    let cfg = app.settings();
    if !cfg.metrics.enabled {
        return Err(ApiError::new(404, "metrics are disabled"));
    }
    if !cfg.metrics.token.is_empty() {
        let ok = bearer(&headers).is_some_and(|t| ct_eq(t.as_bytes(), cfg.metrics.token.as_bytes()));
        if !ok {
            return Err(ApiError::new(401, "invalid metrics token"));
        }
    } else if let Some(Extension(ConnectInfo(addr))) = peer
        && !addr.ip().is_loopback()
    {
        return Err(ApiError::new(403, "metrics are available from localhost only (set METRICS_TOKEN)"));
    }
    let text = crate::admin::prometheus(&app).await;
    Ok(([(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")], text).into_response())
}
