//! Реле Telegram Bot API — замена воркера Cloudflare: адрес API LMQ принимает
//! `/bot<TOKEN>/<метод>` и `/file/bot<TOKEN>/<путь>` и пересылает их в Telegram по
//! выбранному маршруту (интерфейс, прокси). Нужен, когда сервер бота не достаёт до
//! api.telegram.org напрямую. Токен бота в журнал не пишется: только id бота и метод.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Request};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::app::App;
use crate::config::{RouteCfg, Settings};
use crate::net::Kind;
use crate::util::{now_ms, sha256_hex};

/// Заголовки, которые не пересылаются в Telegram: адрес, служебные заголовки соединения
/// и обратных прокси. `accept-encoding` — чтобы ответ пришёл несжатым и длина сошлась.
const DROP_REQUEST: &[&str] = &[
    "host",
    "connection",
    "keep-alive",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "accept-encoding",
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-real-ip",
    "cf-connecting-ip",
    "cf-ipcountry",
    "cf-ray",
    "cf-visitor",
];

/// Заголовки ответа, которые нельзя отдавать как есть: длину ставит сервер по телу.
const DROP_RESPONSE: &[&str] = &["connection", "keep-alive", "transfer-encoding", "content-length", "content-encoding", "upgrade", "trailer"];

/// Файлы Telegram отдаёт как `application/octet-stream` — настоящий тип по расширению.
const FILE_TYPES: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("oga", "audio/ogg"),
    ("ogg", "audio/ogg"),
    ("mp3", "audio/mpeg"),
    ("tgs", "application/x-tgsticker"),
];

/// Срок жизни файла в кэше: по одному пути Telegram всегда отдаёт один и тот же файл.
const CACHE_TTL_MS: i64 = 3_600_000;
/// Файлы крупнее не кэшируются.
const CACHE_MAX_ITEM: usize = 5 * 1024 * 1024;
/// Сколько разных методов считать по отдельности (имя метода присылает клиент).
const MAX_METHODS: usize = 64;

/// Что разобрано из пути запроса.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    pub file: bool,
    /// Число до `:` в токене — открытая часть.
    pub bot_id: String,
    /// Метод API; у файлов — `file`.
    pub method: String,
}

/// Путь похож на запрос к Bot API (даже с неверным токеном): такие пути не попадают в
/// обычную обработку клиентского API и в журнал неизвестных путей.
pub fn is_relay_path(path: &str) -> bool {
    path.starts_with("/bot") || path.starts_with("/file/bot")
}

/// `/bot<id>:<secret>/<метод>` или `/file/bot<id>:<secret>/<путь>`.
pub fn parse(path: &str) -> Option<Target> {
    let (file, rest) = match path.strip_prefix("/file/bot") {
        Some(r) => (true, r),
        None => (false, path.strip_prefix("/bot")?),
    };
    let (token, tail) = rest.split_once('/')?;
    let (id, secret) = token.split_once(':')?;
    let token_ok = !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
        && !secret.is_empty()
        && secret.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    // `.` и `..` (в том числе `%2e%2e`) вывели бы запрос за пределы `/bot<TOKEN>/`.
    let dots = |s: &str| matches!(s.to_ascii_lowercase().replace("%2e", ".").as_str(), "." | "..");
    if !token_ok || tail.is_empty() || tail.split('/').any(|s| s.is_empty() || dots(s)) {
        return None;
    }
    if !file && tail.contains('/') {
        return None;
    }
    Some(Target { file, bot_id: id.to_string(), method: if file { "file".into() } else { tail.to_string() } })
}

/// Маршрут реле: свой интерфейс и прокси, иначе — `network.telegram`.
pub fn route(s: &Settings) -> RouteCfg {
    RouteCfg { interface: s.tg_relay.interface.clone(), proxy: s.tg_relay.proxy.clone() }.inherit(&s.network.telegram)
}

/// Куда пересылать: `https://api.telegram.org` без `/` в конце.
pub fn upstream(s: &Settings) -> String {
    let u = s.tg_relay.upstream.trim().trim_end_matches('/');
    if u.is_empty() { "https://api.telegram.org".into() } else { u.to_string() }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    pub requests: u64,
    /// Telegram ответил 2xx.
    pub ok: u64,
    /// Telegram ответил ошибкой (400, 401, 429 …) — это ответ Telegram, реле работает.
    pub tg_errors: u64,
    /// Telegram не ответил или маршрут не работает (502, 504).
    pub relay_errors: u64,
    /// Отказы самого реле: чужой бот, метод, размер.
    pub rejected: u64,
    pub files: u64,
    pub cache_hits: u64,
    /// Байт к Telegram и от Telegram к боту.
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub avg_ms: Option<f64>,
    pub last_at: Option<i64>,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
    pub methods: BTreeMap<String, u64>,
}

#[derive(Default)]
struct Counters {
    s: Snapshot,
    timed: u64,
    sum_ms: u64,
}

struct CachedFile {
    expires: i64,
    headers: HeaderMap,
    body: Bytes,
}

/// Счётчики реле (с запуска) и кэш файлов.
#[derive(Default)]
pub struct Relay {
    counters: Mutex<Counters>,
    cache: Mutex<HashMap<String, CachedFile>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Ok,
    TgError,
    RelayError,
}

impl Relay {
    pub fn snapshot(&self) -> Snapshot {
        let c = self.counters.lock();
        let mut s = c.s.clone();
        s.avg_ms = (c.timed > 0).then(|| c.sum_ms as f64 / c.timed as f64);
        s
    }

    fn reject(&self, why: &str) {
        let mut c = self.counters.lock();
        c.s.rejected += 1;
        c.s.last_error = Some(why.to_string());
        c.s.last_error_at = Some(now_ms());
    }

    /// `ms` — время ответа Telegram; у ответов из кэша и сбоев его нет.
    fn done(&self, t: &Target, outcome: Outcome, ms: Option<u64>, up: usize, down: usize, error: Option<String>) {
        let mut c = self.counters.lock();
        let now = now_ms();
        c.s.requests += 1;
        c.s.last_at = Some(now);
        c.s.bytes_up += up as u64;
        c.s.bytes_down += down as u64;
        match outcome {
            Outcome::Ok => c.s.ok += 1,
            Outcome::TgError => c.s.tg_errors += 1,
            Outcome::RelayError => c.s.relay_errors += 1,
        }
        if t.file {
            c.s.files += 1;
        }
        if let Some(ms) = ms {
            c.timed += 1;
            c.sum_ms += ms;
        }
        if let Some(e) = error {
            c.s.last_error = Some(e);
            c.s.last_error_at = Some(now);
        }
        let name = if c.s.methods.len() < MAX_METHODS || c.s.methods.contains_key(&t.method) { t.method.clone() } else { "other".into() };
        *c.s.methods.entry(name).or_default() += 1;
    }

    fn cache_get(&self, key: &str) -> Option<(HeaderMap, Bytes)> {
        let c = self.cache.lock();
        let f = c.get(key).filter(|f| f.expires > now_ms())?;
        Some((f.headers.clone(), f.body.clone()))
    }

    fn cache_put(&self, key: String, headers: &HeaderMap, body: &Bytes, limit: usize) {
        if limit == 0 || body.len() > CACHE_MAX_ITEM.min(limit) {
            return;
        }
        let mut c = self.cache.lock();
        let now = now_ms();
        c.retain(|_, f| f.expires > now);
        // Не влезает — вытесняются файлы, которые истекут раньше всех.
        let mut size: usize = c.values().map(|f| f.body.len()).sum();
        while size + body.len() > limit {
            let Some(oldest) = c.iter().min_by_key(|(_, f)| f.expires).map(|(k, _)| k.clone()) else { break };
            if let Some(f) = c.remove(&oldest) {
                size -= f.body.len();
            }
        }
        c.insert(key, CachedFile { expires: now + CACHE_TTL_MS, headers: headers.clone(), body: body.clone() });
    }

    pub fn clear_cache(&self) {
        self.cache.lock().clear();
    }
}

/// Ошибка в формате Bot API: библиотеки ботов читают `ok`, `error_code`, `description`.
fn tg_error(status: StatusCode, description: &str) -> Response {
    let body = json!({ "ok": false, "error_code": status.as_u16(), "description": format!("LMQ relay: {description}") });
    (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

/// Сколько секунд Telegram может держать `getUpdates` (long polling): `timeout` из
/// строки запроса, JSON или формы.
fn long_poll_secs(t: &Target, query: Option<&str>, body: &[u8]) -> u64 {
    if !t.method.eq_ignore_ascii_case("getUpdates") {
        return 0;
    }
    let from_form = |s: &[u8]| url::form_urlencoded::parse(s).find(|(k, _)| k == "timeout").and_then(|(_, v)| v.trim().parse::<u64>().ok());
    let from_json = || serde_json::from_slice::<Value>(body).ok().and_then(|v| v.get("timeout").and_then(Value::as_u64));
    query.and_then(|q| from_form(q.as_bytes())).or_else(from_json).or_else(|| from_form(body)).unwrap_or(0).min(600)
}

fn file_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    FILE_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, t)| *t)
}

/// Обработка запроса реле. Путь уже проверен `is_relay_path`.
pub async fn handle(app: Arc<App>, req: Request) -> Response {
    let cfg = app.settings();
    let r = &cfg.tg_relay;
    let Some(target) = parse(req.uri().path()) else {
        return tg_error(StatusCode::NOT_FOUND, "expected /bot<token>/<method> or /file/bot<token>/<path>");
    };
    if !r.enabled {
        return tg_error(StatusCode::NOT_FOUND, "Telegram relay is disabled (LMQ settings → Telegram relay)");
    }
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip().to_string()).unwrap_or_default();
    if !r.allowed_bots.is_empty() && !r.allowed_bots.iter().any(|b| b.trim() == target.bot_id) {
        tracing::info!(bot = %target.bot_id, peer = %peer, "telegram relay: bot is not in allowed_bots");
        app.tg_relay.reject(&format!("бот {} не в списке разрешённых", target.bot_id));
        return tg_error(StatusCode::FORBIDDEN, "this bot is not allowed to use the relay");
    }
    let allowed: &[Method] = if target.file { &[Method::GET, Method::HEAD] } else { &[Method::GET, Method::POST] };
    if !allowed.contains(req.method()) {
        app.tg_relay.reject(&format!("метод {} для {}", req.method(), target.method));
        let mut resp = tg_error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
        let list = allowed.iter().map(Method::as_str).collect::<Vec<_>>().join(", ");
        if let Ok(v) = HeaderValue::from_str(&list) {
            resp.headers_mut().insert(header::ALLOW, v);
        }
        return resp;
    }

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(String::from);
    let cache_key = sha256_hex(&format!("{path}?{}", query.as_deref().unwrap_or("")));
    let cache_limit = (r.file_cache_mb as usize) * 1024 * 1024;
    if target.file
        && method == Method::GET
        && let Some((headers, body)) = app.tg_relay.cache_get(&cache_key)
    {
        app.tg_relay.counters.lock().s.cache_hits += 1;
        app.tg_relay.done(&target, Outcome::Ok, None, 0, body.len(), None);
        let mut resp = Response::new(Body::from(body));
        *resp.headers_mut() = headers;
        return resp;
    }

    let (parts, body) = req.into_parts();
    let limit = (r.max_body_mb.max(1) as usize) * 1024 * 1024;
    let read_timeout = Duration::from_secs(cfg.server.body_read_timeout.max(1));
    let body = match tokio::time::timeout(read_timeout, axum::body::to_bytes(body, limit)).await {
        Ok(Ok(b)) => b,
        Ok(Err(_)) => {
            app.tg_relay.reject(&format!("тело запроса больше {} МБ", r.max_body_mb));
            return tg_error(StatusCode::PAYLOAD_TOO_LARGE, &format!("request body is larger than {} MB", r.max_body_mb));
        }
        Err(_) => {
            app.tg_relay.reject("бот слишком медленно присылал тело запроса");
            return tg_error(StatusCode::REQUEST_TIMEOUT, "request body was not received in time");
        }
    };

    let route = route(&cfg);
    let http = match app.net.client(&route, Kind::NoRedirect) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "telegram relay: route is not usable");
            app.tg_relay.done(&target, Outcome::RelayError, None, 0, 0, Some(format!("маршрут: {e}")));
            return tg_error(StatusCode::BAD_GATEWAY, &format!("route: {e}"));
        }
    };
    let url = match &query {
        Some(q) => format!("{}{path}?{q}", upstream(&cfg)),
        None => format!("{}{path}", upstream(&cfg)),
    };
    let mut headers = parts.headers.clone();
    for name in DROP_REQUEST {
        headers.remove(*name);
    }
    let timeout = Duration::from_secs(r.timeout.max(1) + long_poll_secs(&target, query.as_deref(), &body));
    let up_len = body.len();
    let mut rb = http.request(method.clone(), &url).headers(headers).timeout(timeout);
    if method == Method::POST {
        rb = rb.body(body);
    }
    let started = Instant::now();
    let result = async {
        let resp = rb.send().await?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.bytes().await?;
        Ok::<_, reqwest::Error>((status, headers, bytes))
    }
    .await;
    let ms = started.elapsed().as_millis() as u64;

    match result {
        Ok((status, up_headers, bytes)) => {
            let mut headers = HeaderMap::new();
            for (k, v) in &up_headers {
                if !DROP_RESPONSE.contains(&k.as_str()) {
                    headers.append(k.clone(), v.clone());
                }
            }
            if target.file
                && status.is_success()
                && headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_none_or(|v| v.starts_with("application/octet-stream"))
                && let Some(t) = file_type(&path)
            {
                headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(t));
            }
            let (outcome, error) = if status.is_success() {
                (Outcome::Ok, None)
            } else {
                // Описание ошибки Telegram («Bad Request: chat not found») не содержит токена.
                let desc = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v["description"].as_str().map(String::from)).unwrap_or_default();
                tracing::info!(bot = %target.bot_id, method = %target.method, status = status.as_u16(), description = %desc, ms, "telegram relay: telegram answered with an error");
                (Outcome::TgError, Some(format!("{}: HTTP {} {desc}", target.method, status.as_u16())))
            };
            tracing::debug!(bot = %target.bot_id, method = %target.method, status = status.as_u16(), ms, bytes = bytes.len(), "telegram relay");
            if target.file && method == Method::GET && status == StatusCode::OK {
                app.tg_relay.cache_put(cache_key, &headers, &bytes, cache_limit);
            }
            app.tg_relay.done(&target, outcome, Some(ms), up_len, bytes.len(), error);
            let mut resp = Response::new(if method == Method::HEAD { Body::empty() } else { Body::from(bytes) });
            *resp.status_mut() = status;
            *resp.headers_mut() = headers;
            resp
        }
        Err(e) => {
            // В тексте ошибки reqwest есть адрес с токеном — убираем его.
            let timed_out = e.is_timeout();
            let msg = crate::netdiag::error_chain(&e.without_url());
            let (status, text) = if timed_out {
                (StatusCode::GATEWAY_TIMEOUT, format!("Telegram did not answer in {} s", timeout.as_secs()))
            } else {
                (StatusCode::BAD_GATEWAY, msg.clone())
            };
            tracing::warn!(bot = %target.bot_id, method = %target.method, route = %crate::net::describe(&route), error = %msg, ms, "telegram relay: request failed");
            app.tg_relay.done(&target, Outcome::RelayError, None, up_len, 0, Some(format!("{}: {text}", target.method)));
            tg_error(status, &text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(parse("/bot123:AA-b_c/sendMessage"), Some(Target { file: false, bot_id: "123".into(), method: "sendMessage".into() }));
        assert_eq!(parse("/file/bot123:abc/photos/file_1.jpg").map(|t| (t.file, t.method)), Some((true, "file".into())));
        assert!(parse("/bot123:abc/").is_none());
        assert!(parse("/bot123:abc/a/b").is_none());
        assert!(parse("/botabc:abc/getMe").is_none());
        assert!(parse("/bot123:a%2e/getMe").is_none());
        assert!(parse("/file/bot123:abc/../../x").is_none());
        assert!(parse("/file/bot123:abc/photos//x").is_none());
        assert!(parse("/file/bot123:abc/%2E%2e/x").is_none());
        assert!(is_relay_path("/bot1:x/getMe") && is_relay_path("/file/bot1:x/a") && !is_relay_path("/v1/models"));
    }

    #[test]
    fn long_poll() {
        let t = Target { file: false, bot_id: "1".into(), method: "getUpdates".into() };
        assert_eq!(long_poll_secs(&t, Some("offset=5&timeout=50"), b""), 50);
        assert_eq!(long_poll_secs(&t, None, br#"{"timeout": 30}"#), 30);
        assert_eq!(long_poll_secs(&t, None, b"timeout=25&limit=10"), 25);
        assert_eq!(long_poll_secs(&t, None, b""), 0);
        let other = Target { method: "sendMessage".into(), ..t };
        assert_eq!(long_poll_secs(&other, Some("timeout=50"), b""), 0);
    }

    #[test]
    fn cache_evicts_to_limit() {
        let r = Relay::default();
        let h = HeaderMap::new();
        r.cache_put("a".into(), &h, &Bytes::from(vec![0u8; 600]), 1000);
        r.cache_put("b".into(), &h, &Bytes::from(vec![0u8; 600]), 1000);
        assert!(r.cache_get("a").is_none());
        assert_eq!(r.cache_get("b").map(|(_, b)| b.len()), Some(600));
        r.cache_put("c".into(), &h, &Bytes::from(vec![0u8; 10]), 0);
        assert!(r.cache_get("c").is_none());
        assert_eq!(file_type("/file/bot1:x/photos/a.JPG"), Some("image/jpeg"));
    }
}
