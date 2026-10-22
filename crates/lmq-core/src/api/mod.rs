//! HTTP: клиентский API (бот) и admin API с веб-панелью — на разных адресах.

pub mod admin;
pub mod client;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::app::App;
use crate::config::hash_key;
use crate::submit::ApiError;
use crate::util::ct_eq;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut resp = (status, Json(json!({ "error": { "message": self.message } }))).into_response();
        if let Some(s) = self.retry_after
            && let Ok(v) = HeaderValue::from_str(&s.to_string())
        {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        resp
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!(error = %e, "request failed");
        ApiError::new(500, e.to_string())
    }
}

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim()).filter(|t| !t.is_empty())
}

/// Клиент по ключу `Authorization: Bearer` (сравнение хэшей за постоянное время).
pub fn client_auth(app: &App, headers: &HeaderMap) -> Result<String, ApiError> {
    let Some(key) = bearer(headers) else {
        tracing::info!("api request without Authorization: Bearer <key>");
        return Err(ApiError::new(401, "invalid api key"));
    };
    let h = hash_key(key);
    let cfg = app.settings();
    let mut found: Option<String> = None;
    for c in &cfg.clients {
        // Проходим все записи, чтобы время не зависело от позиции ключа.
        if ct_eq(h.as_bytes(), c.key_hash.as_bytes()) && c.enabled && found.is_none() {
            found = Some(c.name.clone());
        }
    }
    found.ok_or_else(|| {
        // Сам ключ не пишем: только начало и длину, чтобы отличить чужой или обрезанный.
        tracing::info!(key = %crate::util::mask_secret(key), "api key rejected (unknown or disabled client)");
        ApiError::new(401, "invalid api key")
    })
}

#[derive(Clone, Copy, Debug)]
pub enum Listener {
    Client,
    Admin,
}

/// Поднимает слушатель и перезапускает его при смене адреса в настройках.
pub async fn serve_loop(app: Arc<App>, kind: Listener) {
    loop {
        if app.shutdown.is_cancelled() {
            break;
        }
        let mut epoch = app.listeners_epoch.subscribe();
        epoch.borrow_and_update();
        let cfg = app.settings();
        let addr_s = match kind {
            Listener::Client => cfg.server.listen.clone(),
            Listener::Admin => cfg.server.admin_listen.clone(),
        };
        let set_bound = |addr: Option<SocketAddr>, err: Option<String>| {
            let mut b = app.bound.lock();
            match kind {
                Listener::Client => {
                    b.client = addr;
                    b.client_error = err;
                }
                Listener::Admin => {
                    b.admin = addr;
                    b.admin_error = err;
                }
            }
        };
        if addr_s.trim().is_empty() {
            set_bound(None, None);
            tokio::select! {
                _ = epoch.changed() => continue,
                _ = app.shutdown.cancelled() => break,
            }
        }
        let addr: SocketAddr = match addr_s.parse() {
            Ok(a) => a,
            Err(_) => {
                set_bound(None, Some(format!("bad address {addr_s}")));
                tokio::select! {
                    _ = epoch.changed() => continue,
                    _ = app.shutdown.cancelled() => break,
                }
            }
        };
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                let local = listener.local_addr().ok();
                set_bound(local, None);
                tracing::info!(listener = ?kind, addr = %local.map(|a| a.to_string()).unwrap_or_default(), "listening");
                let router = match kind {
                    Listener::Client => client::router(app.clone()),
                    Listener::Admin => admin::router(app.clone()),
                };
                let app2 = app.clone();
                let stop = async move {
                    tokio::select! {
                        _ = epoch.changed() => {}
                        _ = app2.shutdown.cancelled() => {}
                    }
                };
                let res = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(stop).await;
                if let Err(e) = res {
                    tracing::error!(listener = ?kind, error = %e, "server error");
                }
                set_bound(None, None);
            }
            Err(e) => {
                let msg = format!("cannot listen on {addr}: {e}");
                tracing::error!(listener = ?kind, error = %msg, "bind failed");
                set_bound(None, Some(msg));
                tokio::select! {
                    _ = epoch.changed() => {}
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    _ = app.shutdown.cancelled() => break,
                }
            }
        }
    }
}
