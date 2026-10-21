//! Общая обвязка интеграционных тестов: ядро во временной папке, апстрим и бот — wiremock.

#![allow(dead_code)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use lmq_core::config::AppPaths;
use lmq_core::{Core, StartOptions};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub const KEY: &str = "test-key";
pub const CB_TOKEN: &str = "cb-token-secret";

pub struct Env {
    pub core: Core,
    pub dir: tempfile::TempDir,
    pub bot: MockServer,
    pub router: Router,
    pub admin: Router,
    pub env: HashMap<String, String>,
}

pub fn ok_answer(text: &str) -> Value {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": "upstream-model",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15}
    })
}

/// Апстрим, который отвечает `text` с задержкой.
pub async fn upstream(text: &str, delay_ms: u64) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_answer(text)).set_delay(Duration::from_millis(delay_ms)))
        .mount(&s)
        .await;
    Mock::given(method("GET")).and(path("/v1/models")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []}))).mount(&s).await;
    s
}

pub async fn failing_upstream(status: u16) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({"error": {"message": "boom"}})))
        .mount(&s)
        .await;
    s
}

pub async fn bot_ok() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("POST")).and(path("/webhook/llm")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true}))).mount(&s).await;
    s
}

pub fn url(s: &MockServer) -> String {
    format!("{}/v1/chat/completions", s.uri())
}

/// Настройки с моделями. `models` — YAML-фрагмент раздела `models:`.
pub async fn start(models: &str, extra_env: &[(&str, &str)], bot: MockServer) -> Env {
    start_in(tempfile::tempdir().unwrap(), models, extra_env, bot).await
}

pub fn settings_yaml(models: &str) -> String {
    format!(
        "breaker:\n  enabled: true\n  failure_threshold: 3\n  open_seconds: 30\n  health_interval: 0\nserver:\n  listen: 127.0.0.1:0\n  admin_listen: ''\nmodels:\n{models}"
    )
}

pub async fn start_in(dir: tempfile::TempDir, models: &str, extra_env: &[(&str, &str)], bot: MockServer) -> Env {
    lmq_core::logging::init(None, "debug", 1, false);
    let paths = AppPaths::from_dirs(dir.path().join("cfg"), dir.path().join("data"), false);
    paths.ensure().unwrap();
    let settings_path = paths.settings_file("default");
    if !settings_path.exists() {
        std::fs::write(&settings_path, settings_yaml(models)).unwrap();
    }
    let mut env: HashMap<String, String> = HashMap::from([
        ("API_KEYS".into(), format!("bot:{KEY}")),
        ("BOT_CALLBACK_URL".into(), format!("{}/webhook/llm", bot.uri())),
        ("BOT_CALLBACK_TOKEN".into(), CB_TOKEN.into()),
        ("CALLBACK_ALLOW_HTTP_PRIVATE".into(), "true".into()),
        ("CALLBACK_ALLOWED_HOSTS".into(), "127.0.0.1".into()),
        ("ADMIN_TOKEN".into(), "admin-test".into()),
    ]);
    for (k, v) in extra_env {
        env.insert(k.to_string(), v.to_string());
    }
    let core =
        Core::start(StartOptions { explicit_paths: Some(paths), env: Some(env.clone()), listeners: false, ..Default::default() }).await.expect("core start");
    let router = lmq_core::api::client::router(core.app.clone());
    let admin = lmq_core::api::admin::router(core.app.clone());
    Env { core, dir, bot, router, admin, env }
}

pub fn job(job_id: &str, chat_id: i64, text: &str) -> Value {
    json!({
        "model": "m",
        "messages": [{"role": "user", "content": text}],
        "temperature": 0.7,
        "stream": false,
        "protocol": 2,
        "delivery": "bot",
        "job_id": job_id,
        "chat_id": chat_id,
        "reply_to": 4711,
        "thread_id": null,
    })
}

pub async fn call(router: &Router, method_: &str, uri: &str, key: Option<&str>, body: Option<Value>) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut b = Request::builder().method(method_).uri(uri);
    if let Some(k) = key {
        b = b.header("authorization", format!("Bearer {k}"));
    }
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, headers, v)
}

pub async fn post_job(env: &Env, body: Value) -> (StatusCode, axum::http::HeaderMap, Value) {
    call(&env.router, "POST", "/v1/chat/completions", Some(KEY), Some(body)).await
}

/// Ждёт, пока условие станет истинным.
pub async fn wait_for<F, Fut>(timeout: Duration, mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Тела колбэков, полученных ботом.
pub async fn callbacks(bot: &MockServer) -> Vec<(Option<String>, Value)> {
    bot.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| {
            let token = r.headers.get("x-callback-token").and_then(|v| v.to_str().ok()).map(String::from);
            (token, serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        })
        .collect()
}

pub async fn callback_for(bot: &MockServer, job_id: &str) -> Option<Value> {
    callbacks(bot).await.into_iter().map(|(_, b)| b).find(|b| b["job_id"] == job_id)
}

pub async fn wait_callback(bot: &MockServer, job_id: &str, secs: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(b) = callback_for(bot, job_id).await {
            return b;
        }
        assert!(Instant::now() < deadline, "callback for {job_id} not received in {secs}s");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub async fn job_row(env: &Env, job_id: &str) -> lmq_core::store::JobRow {
    env.core.app.store.get_job(job_id).await.unwrap().expect("job exists")
}

/// Ждёт, пока задание доставлено (delivery = delivered | dead).
pub async fn wait_delivery(env: &Env, job_id: &str, secs: u64) -> lmq_core::store::JobRow {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Ok(Some(r)) = env.core.app.store.get_job(job_id).await
            && (r.delivery == "delivered" || r.delivery == "dead")
        {
            return r;
        }
        assert!(Instant::now() < deadline, "job {job_id} was not delivered in {secs}s");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
