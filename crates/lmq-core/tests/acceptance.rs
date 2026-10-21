//! Приёмочные тесты §11 ТЗ (`queue-server.md`), по одному на пункт.

mod common;

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use common::*;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn model(url: &str, slots: u32) -> String {
    format!("  m:\n    upstreams:\n      - url: {url}\n        slots: {slots}\n")
}

/// 1. Постановка: 202 + `status: "Added to queue"` быстрее 1 с; повтор с тем же job_id —
/// тот же ответ, задание одно.
#[tokio::test]
async fn t01_submit_fast_and_idempotent() {
    let up = upstream("OK", 300).await;
    let env = start(&model(&url(&up), 1), &[], bot_ok().await).await;
    let t = Instant::now();
    let (st, _, body) = post_job(&env, job("job-1", -100, "Привет")).await;
    assert!(t.elapsed() < Duration::from_secs(1), "enqueue took {:?}", t.elapsed());
    assert_eq!(st, StatusCode::ACCEPTED);
    assert_eq!(body["status"].as_str().unwrap().to_lowercase(), "added to queue");
    assert_eq!(body["job_id"], "job-1");
    assert!(body.get("position").is_some() && body.get("eta_seconds").is_some());

    let (st2, _, body2) = post_job(&env, job("job-1", -100, "Привет")).await;
    assert_eq!(st2, StatusCode::OK);
    assert_eq!(body2["status"], "Added to queue");
    assert_eq!(body2["job_id"], "job-1");
    let (rows, total) = env.core.app.store.list_jobs(&Default::default()).await.unwrap();
    assert_eq!(total, 1, "{rows:?}");
    wait_callback(&env.bot, "job-1", 5).await;
    // Колбэк пришёл один раз.
    let n = callbacks(&env.bot).await.iter().filter(|(_, b)| b["job_id"] == "job-1").count();
    assert_eq!(n, 1);
}

/// 2. Неверный ключ → 401; неизвестная модель → 400; переполнение → 429 с Retry-After.
#[tokio::test]
async fn t02_auth_unknown_model_overflow() {
    let up = upstream("OK", 1500).await;
    let env = start(&model(&url(&up), 1), &[("MAX_QUEUE", "1")], bot_ok().await).await;
    let (st, _, body) = call(&env.router, "POST", "/v1/chat/completions", Some("wrong"), Some(job("x", -1, "a"))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["message"], "invalid api key");
    let (st, _, _) = call(&env.router, "POST", "/v1/chat/completions", None, Some(job("x", -1, "a"))).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    let mut j = job("y", -1, "a");
    j["model"] = json!("nope");
    let (st, _, body) = post_job(&env, j).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("unknown model"));

    // Первое выполняется, второе в очереди (MAX_QUEUE = 1), третье — 429.
    assert_eq!(post_job(&env, job("a1", -1, "a")).await.0, StatusCode::ACCEPTED);
    let e = &env;
    assert!(wait_for(Duration::from_secs(3), move || async move { e.core.app.queue.total_running() == 1 }).await);
    assert_eq!(post_job(&env, job("a2", -2, "a")).await.0, StatusCode::ACCEPTED);
    let (st, headers, body) = post_job(&env, job("a3", -3, "a")).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["message"], "queue is full");
    let ra: u64 = headers.get("retry-after").unwrap().to_str().unwrap().parse().unwrap();
    assert!(ra >= 1);
}

/// 2а. Лимит на чат: MAX_QUEUE_PER_CHAT.
#[tokio::test]
async fn t02b_per_chat_limit() {
    let up = upstream("OK", 2000).await;
    let env = start(&model(&url(&up), 1), &[("MAX_QUEUE_PER_CHAT", "1")], bot_ok().await).await;
    assert_eq!(post_job(&env, job("c1", -7, "a")).await.0, StatusCode::ACCEPTED);
    let e = &env;
    assert!(wait_for(Duration::from_secs(3), move || async move { e.core.app.queue.total_running() == 1 }).await);
    assert_eq!(post_job(&env, job("c2", -7, "a")).await.0, StatusCode::ACCEPTED);
    let (st, h, _) = post_job(&env, job("c3", -7, "a")).await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.get("retry-after").is_some());
}

/// 3. Синхронный режим (chat_id: 0): ответ OpenAI-формы, колбэка нет.
#[tokio::test]
async fn t03_sync_mode() {
    let up = upstream("OK", 50).await;
    let env = start(&model(&url(&up), 1), &[], bot_ok().await).await;
    let (st, _, body) = post_job(&env, json!({"model": "m", "messages": [{"role": "user", "content": "ping"}], "chat_id": 0})).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["model"], "m");
    assert_eq!(body["choices"][0]["message"]["content"], "OK");
    assert_eq!(body["usage"]["total_tokens"], 15);
    // Без chat_id — тоже синхронно.
    let (st, _, _) = post_job(&env, json!({"model": "m", "messages": [{"role": "user", "content": "ping"}]})).await;
    assert_eq!(st, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(callbacks(&env.bot).await.is_empty(), "sync mode must not call back");
}

/// 3а. Синхронный режим: таймаут → 504.
#[tokio::test]
async fn t03b_sync_timeout() {
    let up = upstream("late", 3000).await;
    let env = start(&model(&url(&up), 1), &[("SYNC_TIMEOUT", "1")], bot_ok().await).await;
    let (st, _, body) = post_job(&env, json!({"model": "m", "messages": [{"role": "user", "content": "ping"}], "chat_id": 0})).await;
    assert_eq!(st, StatusCode::GATEWAY_TIMEOUT);
    assert!(body["error"]["message"].is_string());
}

/// 4. Результат приходит на callback.url с X-Callback-Token; в теле job_id, chat_id,
/// reply_to, thread_id, text, model.
#[tokio::test]
async fn t04_callback_content() {
    let up = upstream("<think>x</think>Ответ модели", 20).await;
    let bot = bot_ok().await;
    let bot_url = format!("{}/webhook/llm", bot.uri());
    let env = start(&model(&url(&up), 1), &[], bot).await;
    let mut j = job("cb-1", -1001234567890, "Привет");
    j["thread_id"] = json!(55);
    j["callback"] = json!({"url": bot_url, "token": "job-token"});
    assert_eq!(post_job(&env, j).await.0, StatusCode::ACCEPTED);
    let b = wait_callback(&env.bot, "cb-1", 5).await;
    let (token, _) = callbacks(&env.bot).await.into_iter().find(|(_, b)| b["job_id"] == "cb-1").unwrap();
    assert_eq!(token.as_deref(), Some("job-token"));
    assert_eq!(b["chat_id"], -1001234567890i64);
    assert_eq!(b["reply_to"], 4711);
    assert_eq!(b["thread_id"], 55);
    assert_eq!(b["text"], "<think>x</think>Ответ модели", "text must be raw");
    assert_eq!(b["model"], "m");
    assert_eq!(b["usage"]["completion_tokens"], 3);
    assert!(b["queued_ms"].is_number() && b["latency_ms"].is_number());
    assert!(b.get("error").is_none());
    // Без callback в задании — BOT_CALLBACK_URL/TOKEN из конфигурации.
    assert_eq!(post_job(&env, job("cb-2", -5, "x")).await.0, StatusCode::ACCEPTED);
    wait_callback(&env.bot, "cb-2", 5).await;
    let (token, _) = callbacks(&env.bot).await.into_iter().find(|(_, b)| b["job_id"] == "cb-2").unwrap();
    assert_eq!(token.as_deref(), Some(CB_TOKEN));
}

/// 5. Один чат — одно задание: два задания одного чата по очереди, разных — параллельно.
#[tokio::test]
async fn t05_one_job_per_chat() {
    let up = upstream("OK", 400).await;
    let env = start(&model(&url(&up), 2), &[], bot_ok().await).await;
    for (id, chat) in [("s1", -1), ("s2", -1), ("p1", -2), ("p2", -3)] {
        assert_eq!(post_job(&env, job(id, chat, "x")).await.0, StatusCode::ACCEPTED);
    }
    for id in ["s1", "s2", "p1", "p2"] {
        wait_callback(&env.bot, id, 10).await;
    }
    let s1 = job_row(&env, "s1").await;
    let s2 = job_row(&env, "s2").await;
    assert!(s2.started_at.unwrap() >= s1.finished_at.unwrap(), "same chat ran in parallel");
    // s1 и p1 стартовали вместе (два слота).
    let p1 = job_row(&env, "p1").await;
    assert!(p1.started_at.unwrap() < s1.finished_at.unwrap(), "different chats did not run in parallel");
}

/// 6. Round-robin: 10 заданий чата A и 1 задание чата B — B не ждёт все 10.
#[tokio::test]
async fn t06_round_robin() {
    let up = upstream("OK", 150).await;
    let env = start(&model(&url(&up), 1), &[("MAX_QUEUE_PER_CHAT", "20")], bot_ok().await).await;
    for i in 0..10 {
        assert_eq!(post_job(&env, job(&format!("a{i}"), -10, "x")).await.0, StatusCode::ACCEPTED);
    }
    assert_eq!(post_job(&env, job("b0", -20, "x")).await.0, StatusCode::ACCEPTED);
    wait_callback(&env.bot, "b0", 10).await;
    let b = job_row(&env, "b0").await;
    let mut started_before_b = 0;
    for i in 0..10 {
        let a = job_row(&env, &format!("a{i}")).await;
        if a.started_at.is_some_and(|t| t < b.started_at.unwrap()) {
            started_before_b += 1;
        }
    }
    assert!(started_before_b <= 2, "B waited behind {started_before_b} jobs of A");
}

/// 7. MAX_WAIT → expired; DELETE → cancelled; вытеснение → superseded.
#[tokio::test]
async fn t07_expired_cancelled_superseded() {
    let up = upstream("OK", 2500).await;
    let env = start(&model(&url(&up), 1), &[("MAX_WAIT", "1")], bot_ok().await).await;
    assert_eq!(post_job(&env, job("busy", -1, "x")).await.0, StatusCode::ACCEPTED);
    let e = &env;
    assert!(wait_for(Duration::from_secs(3), move || async move { e.core.app.queue.total_running() == 1 }).await);
    assert_eq!(post_job(&env, job("late", -2, "x")).await.0, StatusCode::ACCEPTED);
    let b = wait_callback(&env.bot, "late", 6).await;
    assert_eq!(b["error_code"], "expired");
    assert!(b["text"].is_null());

    // Отмена выполняющегося задания.
    let (st, _, body) = call(&env.router, "DELETE", "/v1/jobs/busy", Some(KEY), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["state"], "cancelled");
    let b = wait_callback(&env.bot, "busy", 5).await;
    assert_eq!(b["error_code"], "cancelled");
}

#[tokio::test]
async fn t07b_cancel_queued_and_supersede() {
    let up = upstream("OK", 1500).await;
    let env = start(&model(&url(&up), 1), &[("SUPERSEDE_SAME_CHAT", "true")], bot_ok().await).await;
    assert_eq!(post_job(&env, job("run", -1, "x")).await.0, StatusCode::ACCEPTED);
    let e = &env;
    assert!(wait_for(Duration::from_secs(3), move || async move { e.core.app.queue.total_running() == 1 }).await);
    // Отмена ещё не начатого.
    assert_eq!(post_job(&env, job("q1", -2, "x")).await.0, StatusCode::ACCEPTED);
    let (_, _, body) = call(&env.router, "DELETE", "/v1/jobs/q1", Some(KEY), None).await;
    assert_eq!(body["state"], "cancelled");
    assert_eq!(wait_callback(&env.bot, "q1", 5).await["error_code"], "cancelled");
    // Вытеснение: старое не начатое задание чата -3 заменяется новым.
    assert_eq!(post_job(&env, job("old", -3, "x")).await.0, StatusCode::ACCEPTED);
    assert_eq!(post_job(&env, job("new", -3, "x")).await.0, StatusCode::ACCEPTED);
    assert_eq!(wait_callback(&env.bot, "old", 5).await["error_code"], "superseded");
    let b = wait_callback(&env.bot, "new", 10).await;
    assert_eq!(b["text"], "OK");
    // GET /v1/jobs/{id}: статус без текста.
    let (st, _, v) = call(&env.router, "GET", "/v1/jobs/old", Some(KEY), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["state"], "superseded");
    assert!(v.get("text").is_none() && v.get("result_text").is_none());
}

/// 8. Ошибка апстрима → второй апстрим; все упали → колбэк upstream_error.
#[tokio::test]
async fn t08_failover_and_upstream_error() {
    let bad = failing_upstream(500).await;
    let good = upstream("from-second", 20).await;
    let models = format!(
        "  m:\n    upstreams:\n      - url: {}\n        slots: 1\n      - url: {}\n        slots: 1\n  dead:\n    upstreams:\n      - url: {}\n        slots: 1\n",
        url(&bad),
        url(&good),
        url(&bad)
    );
    let env = start(&models, &[("UPSTREAM_RETRIES", "1")], bot_ok().await).await;
    assert_eq!(post_job(&env, job("fo", -1, "x")).await.0, StatusCode::ACCEPTED);
    assert_eq!(wait_callback(&env.bot, "fo", 5).await["text"], "from-second");
    let mut j = job("all-bad", -2, "x");
    j["model"] = json!("dead");
    assert_eq!(post_job(&env, j).await.0, StatusCode::ACCEPTED);
    let b = wait_callback(&env.bot, "all-bad", 10).await;
    assert_eq!(b["error_code"], "upstream_error");
    assert!(b["error"].as_str().unwrap().contains("500"));
    // UPSTREAM_RETRIES = 1: два круга по одному апстриму.
    let posts = bad.received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "POST").count();
    assert!(posts >= 3, "expected retries, got {posts} upstream calls");
}

/// 8а. 4xx апстрима — ошибка задания без перехода к другому апстриму.
#[tokio::test]
async fn t08b_4xx_is_job_error() {
    let bad = failing_upstream(400).await;
    let good = upstream("x", 0).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n      - url: {}\n", url(&bad), url(&good));
    let env = start(&models, &[], bot_ok().await).await;
    assert_eq!(post_job(&env, job("4xx", -1, "x")).await.0, StatusCode::ACCEPTED);
    assert_eq!(wait_callback(&env.bot, "4xx", 5).await["error_code"], "upstream_error");
    assert!(good.received_requests().await.unwrap().iter().all(|r| r.method.as_str() != "POST"));
}

/// 8б. Пустой ответ после удаления <think> → empty_answer.
#[tokio::test]
async fn t08c_empty_answer() {
    let up = upstream("<think>только размышления</think>  ", 0).await;
    let env = start(&model(&url(&up), 1), &[], bot_ok().await).await;
    post_job(&env, job("empty", -1, "x")).await;
    assert_eq!(wait_callback(&env.bot, "empty", 5).await["error_code"], "empty_answer");
}

/// 9. Колбэк: 5xx → повторы с паузами; 403 → без повторов, dead; после доставки
/// result_text стёрт.
#[tokio::test]
async fn t09_callback_retries_and_forbidden() {
    let up = upstream("OK", 0).await;
    let bot = MockServer::start().await;
    Mock::given(method("POST")).and(path("/webhook/llm")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&bot).await;
    Mock::given(method("POST")).and(path("/webhook/llm")).respond_with(ResponseTemplate::new(200)).mount(&bot).await;
    let forbid = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(403)).mount(&forbid).await;
    let forbid_url = format!("{}/webhook/llm", forbid.uri());
    let env = start(&model(&url(&up), 1), &[], bot).await;

    let t = Instant::now();
    post_job(&env, job("retry", -1, "x")).await;
    let row = wait_delivery(&env, "retry", 10).await;
    assert_eq!(row.delivery, "delivered");
    assert!(t.elapsed() >= Duration::from_millis(1800), "retry pause must be ~2s");
    assert_eq!(env.bot.received_requests().await.unwrap().len(), 2);
    assert!(row.result_text.is_none() && row.payload.is_none() && row.callback_token.is_none(), "texts must be wiped");

    let mut j = job("forbid", -2, "x");
    j["callback"] = json!({"url": forbid_url, "token": "bad"});
    post_job(&env, j).await;
    let row = wait_delivery(&env, "forbid", 5).await;
    assert_eq!(row.delivery, "dead");
    assert_eq!(row.delivery_error.as_deref(), Some("callback_forbidden"));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(forbid.received_requests().await.unwrap().len(), 1, "403 must not be retried");
    let (_, _, v) = call(&env.router, "GET", "/v1/jobs/forbid", Some(KEY), None).await;
    assert_eq!(v["state"], "dead");
}

/// 10. Перезапуск: задания queued и running выполняются после старта, ни одно не потеряно.
#[tokio::test]
async fn t10_restart_keeps_jobs() {
    let up = upstream("after-restart", 10).await;
    let models = model(&url(&up), 1);
    let env = start(&models, &[], bot_ok().await).await;
    env.core.app.paused.store(true, std::sync::atomic::Ordering::Relaxed);
    for (id, chat) in [("r1", -1), ("r2", -2), ("r3", -3)] {
        assert_eq!(post_job(&env, job(id, chat, "x")).await.0, StatusCode::ACCEPTED);
    }
    // Имитация падения посреди выполнения: r1 — running.
    env.core.app.store.mark_running("r1", lmq_core::util::now_ms()).await.unwrap();
    let Env { core, dir, bot, .. } = env;
    core.shutdown().await;

    let env2 = start_in(dir, &models, &[], bot).await;
    for id in ["r1", "r2", "r3"] {
        assert_eq!(wait_callback(&env2.bot, id, 10).await["text"], "after-restart");
    }
}

/// 11. callback.url вне белого списка → 400, запрос наружу не делается.
#[tokio::test]
async fn t11_callback_allowlist() {
    let up = upstream("OK", 0).await;
    let outsider = MockServer::start().await;
    let env = start(&model(&url(&up), 1), &[("CALLBACK_ALLOWED_HOSTS", "bot.example.com")], bot_ok().await).await;
    let mut j = job("ssrf", -1, "x");
    j["callback"] = json!({"url": format!("{}/steal", outsider.uri()), "token": "t"});
    let (st, _, body) = post_job(&env, j).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(body["error"]["message"].as_str().unwrap().contains("not in CALLBACK_ALLOWED_HOSTS"));
    let mut j = job("ssrf2", -1, "x");
    j["callback"] = json!({"url": "http://bot.example.com/webhook/llm", "token": "t"});
    assert_eq!(post_job(&env, j).await.0, StatusCode::BAD_REQUEST, "plain http to a public host must be refused");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(outsider.received_requests().await.unwrap().is_empty());
    assert!(up.received_requests().await.unwrap().iter().all(|r| r.method.as_str() != "POST"));
}

/// 12. Журнал не содержит текстов сообщений и токенов при LOG_CONTENT=false.
#[tokio::test]
async fn t12_logs_have_no_content() {
    let up = upstream("SECRET-ANSWER-777", 0).await;
    let env = start(&model(&url(&up), 1), &[], bot_ok().await).await;
    let mut j = job("logjob-unique", -1, "SECRET-QUESTION-555");
    let bot_url = format!("{}/webhook/llm", env.bot.uri());
    j["callback"] = json!({"url": bot_url, "token": "SECRET-CB-TOKEN-999"});
    post_job(&env, j).await;
    wait_delivery(&env, "logjob-unique", 5).await;
    let all = lmq_core::logging::hub().tail(3000, None, None);
    let text = serde_json::to_string(&all).unwrap();
    assert!(text.contains("logjob-unique"), "job must be logged");
    for secret in ["SECRET-QUESTION-555", "SECRET-ANSWER-777", "SECRET-CB-TOKEN-999", KEY, CB_TOKEN] {
        assert!(!text.contains(secret), "log leaked {secret}");
    }
    // В базе токен колбэка зашифрован.
    let raw: Option<(Option<String>,)> =
        sqlx::query_as("SELECT callback_token FROM jobs WHERE job_id = 'logjob-unique'").fetch_optional(&env.core.app.store.pool).await.unwrap();
    assert!(raw.unwrap().0.is_none(), "token must be wiped after delivery");
}

/// 13. GET /v1/models — список из конфига, формат OpenAI.
#[tokio::test]
async fn t13_models_list() {
    let up = upstream("OK", 0).await;
    let models = format!("{}  gpt-oss-20b:\n    upstreams:\n      - url: {}\n", model(&url(&up), 1), url(&up));
    let env = start(&models, &[], bot_ok().await).await;
    let (st, _, v) = call(&env.router, "GET", "/v1/models", Some(KEY), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["object"], "list");
    let ids: Vec<&str> = v["data"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["gpt-oss-20b", "m"]);
    assert!(v["data"].as_array().unwrap().iter().all(|m| m["object"] == "model"));
    let (st, _, _) = call(&env.router, "GET", "/v1/models", Some("bad"), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

/// /healthz без авторизации; /v1/queue — сводка.
#[tokio::test]
async fn t14_healthz_and_queue() {
    let up = upstream("OK", 0).await;
    let env = start(&model(&url(&up), 2), &[], bot_ok().await).await;
    let (st, _, v) = call(&env.router, "GET", "/healthz", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v, json!({"ok": true}));
    let (st, _, v) = call(&env.router, "GET", "/v1/queue", Some(KEY), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["models"]["m"]["slots"], 2);
    // Модель без апстримов-живых: /healthz → 503.
    let dead = "  m:\n    upstreams:\n      - url: http://127.0.0.1:9/v1/chat/completions\n".to_string();
    let env2 = start(&dead, &[], bot_ok().await).await;
    let routing = env2.core.app.routing();
    let u = routing.upstreams().next().unwrap().clone();
    let cfg = env2.core.app.settings().breaker.clone();
    for _ in 0..3 {
        u.health.on_failure("down", &cfg);
    }
    let (st, _, _) = call(&env2.router, "GET", "/healthz", None, None).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
}

/// Слишком большое тело → 413.
#[tokio::test]
async fn t15_body_too_large() {
    let up = upstream("OK", 0).await;
    let env = start(&model(&url(&up), 1), &[("MAX_BODY_MB", "1")], bot_ok().await).await;
    let big = "x".repeat(1_200_000);
    let (st, _, v) = post_job(&env, job("big", -1, &big)).await;
    assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE, "{v}");
    assert!(v["error"]["message"].is_string());
}
