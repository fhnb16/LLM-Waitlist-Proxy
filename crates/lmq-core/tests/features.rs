//! Тесты дополнительных функций: выключатель, алиасы и резерв, расписание, «мёртвые»
//! задания, статистика, панель (admin API), профили, экспорт/импорт, протокол 1, движки.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use common::*;
use serde_json::json;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ADMIN: &str = "admin-test";

#[tokio::test]
async fn breaker_opens_and_503() {
    let bad = failing_upstream(502).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n", url(&bad));
    let env = start(&models, &[("UPSTREAM_RETRIES", "0")], bot_ok().await).await;
    for i in 0..3 {
        post_job(&env, job(&format!("b{i}"), -(i + 1), "x")).await;
    }
    for i in 0..3 {
        assert_eq!(wait_callback(&env.bot, &format!("b{i}"), 5).await["error_code"], "upstream_error");
    }
    // Порог 3 — выключатель разомкнут, новые задания получают 503 + Retry-After.
    let (st, h, _) = post_job(&env, job("b9", -9, "x")).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    assert!(h.get("retry-after").is_some());
    // Сброс из панели.
    let id = env.core.app.routing().upstreams().next().unwrap().id.clone();
    let (st, _, _) = call(&env.admin, "POST", &format!("/admin/api/upstreams/{}/reset", id.replace('#', "%23")), Some(ADMIN), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(post_job(&env, job("b10", -10, "x")).await.0, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn alias_and_fallback_report_actual_model() {
    let bad = failing_upstream(500).await;
    let good = upstream("from-backup", 0).await;
    let models =
        format!("  main:\n    fallback: [backup]\n    upstreams:\n      - url: {}\n  backup:\n    upstreams:\n      - url: {}\n", url(&bad), url(&good));
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.yaml"), format!("{}aliases:\n  fast: main\n", settings_yaml(&models))).unwrap();
    let env = start_in(dir, "", &[("UPSTREAM_RETRIES", "0")], bot_ok().await).await;
    // Алиас виден в /v1/models.
    let (_, _, v) = call(&env.router, "GET", "/v1/models", Some(KEY), None).await;
    assert!(v["data"].as_array().unwrap().iter().any(|m| m["id"] == "fast" && m["alias_of"] == "main"));
    let mut j = job("fb", -1, "x");
    j["model"] = json!("fast");
    assert_eq!(post_job(&env, j).await.0, StatusCode::ACCEPTED);
    let b = wait_callback(&env.bot, "fb", 10).await;
    assert_eq!(b["text"], "from-backup");
    assert_eq!(b["model"], "backup", "callback must report the actual model");
    let row = job_row(&env, "fb").await;
    assert_eq!(row.model, "fast");
    assert_eq!(row.hops, 1);
}

#[tokio::test]
async fn schedule_pause_intake() {
    let up = upstream("OK", 0).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n", url(&up));
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("settings.yaml"),
        format!("{}schedule:\n  - name: always\n    from: '00:00'\n    to: '00:00'\n    action: pause_intake\n", settings_yaml(&models)),
    )
    .unwrap();
    let env = start_in(dir, "", &[], bot_ok().await).await;
    let (st, h, v) = post_job(&env, job("s", -1, "x")).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    let ra: u64 = h.get("retry-after").unwrap().to_str().unwrap().parse().unwrap();
    assert!(ra > 0 && ra <= 86_400);
}

#[tokio::test]
async fn schedule_pause_dispatch_holds_jobs() {
    let up = upstream("OK", 0).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n", url(&up));
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.yaml"), format!("{}schedule:\n  - from: '00:00'\n    to: '00:00'\n    action: pause_dispatch\n", settings_yaml(&models)))
        .unwrap();
    let env = start_in(dir, "", &[], bot_ok().await).await;
    assert_eq!(post_job(&env, job("held", -1, "x")).await.0, StatusCode::ACCEPTED);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(job_row(&env, "held").await.state, "queued");
    // Правило убрано — задание уходит.
    let (st, _, v) = call(&env.admin, "PATCH", "/admin/api/settings", Some(ADMIN), Some(json!({"schedule": []}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(wait_callback(&env.bot, "held", 10).await["text"], "OK");
}

#[tokio::test]
async fn dead_job_redelivery() {
    let up = upstream("OK", 0).await;
    let bot = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(403)).up_to_n_times(1).mount(&bot).await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)).mount(&bot).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n", url(&up));
    let env = start(&models, &[], bot).await;
    post_job(&env, job("dead1", -1, "x")).await;
    assert_eq!(wait_delivery(&env, "dead1", 5).await.delivery, "dead");
    let (st, _, v) = call(&env.admin, "GET", "/admin/api/jobs?state=dead", Some(ADMIN), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["total"], 1);
    let (st, _, _) = call(&env.admin, "POST", "/admin/api/jobs/dead1/redeliver", Some(ADMIN), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(wait_delivery(&env, "dead1", 5).await.delivery, "delivered");
    assert_eq!(env.bot.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn stats_and_overview() {
    let up = upstream("OK", 30).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n        slots: 2\n", url(&up));
    let env = start(&models, &[], bot_ok().await).await;
    for i in 0..4 {
        post_job(&env, job(&format!("st{i}"), -(i + 1), "x")).await;
    }
    post_job(&env, json!({"model": "m", "messages": [{"role": "user", "content": "x"}], "chat_id": 0})).await;
    for i in 0..4 {
        wait_callback(&env.bot, &format!("st{i}"), 5).await;
    }
    let (st, _, v) = call(&env.admin, "GET", "/admin/api/stats?range=1h&group=chat_kind", Some(ADMIN), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["totals"]["jobs"], 5);
    assert_eq!(v["totals"]["ok"], 5);
    assert_eq!(v["points"].as_array().unwrap().len(), 60);
    let kinds: Vec<&str> = v["groups"].as_array().unwrap().iter().map(|g| g["key"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"group") && kinds.contains(&"sync"));
    let (_, _, o) = call(&env.admin, "GET", "/admin/api/overview", Some(ADMIN), None).await;
    assert_eq!(o["models"][0]["name"], "m");
    assert_eq!(o["models"][0]["slots"], 2);
    assert_eq!(o["last_1h"]["jobs"], 5);
    // Метрики с loopback без токена (в тестах нет ConnectInfo — считается локальным).
    let (st, _, m) = call(&env.router, "GET", "/metrics", None, None).await;
    assert_eq!(st, StatusCode::OK);
    let text = m.as_str().unwrap();
    assert!(text.contains("queue_jobs{state=\"done\"}"));
    assert!(text.contains("upstream_latency_seconds_bucket{model=\"m\""));
    assert!(text.contains("callback_failures_total 0"));
}

#[tokio::test]
async fn admin_auth_settings_and_clients() {
    let up = upstream("OK", 0).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n        api_key: upstream-secret\n", url(&up));
    let env = start(&models, &[], bot_ok().await).await;
    let (st, _, _) = call(&env.admin, "GET", "/admin/api/overview", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, _) = call(&env.admin, "GET", "/admin/api/overview", Some("wrong"), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Настройки: секреты замаскированы, перекрытые окружением поля помечены.
    let (_, _, s) = call(&env.admin, "GET", "/admin/api/settings", Some(ADMIN), None).await;
    assert_eq!(s["file"]["models"]["m"]["upstreams"][0]["api_key"], s["mask"]);
    assert!(s["locked"].as_array().unwrap().iter().any(|l| l == "callback.url"));
    let raw = serde_json::to_string(&s).unwrap();
    assert!(!raw.contains("upstream-secret"));

    // Сохранение с маской не теряет секрет.
    let mut file = s["file"].clone();
    file["queue"]["max_queue"] = json!(77);
    let (st, _, v) = call(&env.admin, "PUT", "/admin/api/settings", Some(ADMIN), Some(file)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(env.core.app.settings().queue.max_queue, 77);
    assert_eq!(env.core.app.settings().models["m"].upstreams[0].api_key, "upstream-secret");
    let on_disk = std::fs::read_to_string(&env.core.app.effective().settings_path).unwrap();
    assert!(!on_disk.contains("upstream-secret") && on_disk.contains("enc:"));

    // Неверные настройки отклоняются с понятной ошибкой.
    let (st, _, v) = call(&env.admin, "PATCH", "/admin/api/settings", Some(ADMIN), Some(json!({"server": {"listen": "nope"}}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(v["error"]["message"].as_str().unwrap().contains("server.listen"));

    // Клиенты: ключ показывается один раз и сразу работает.
    let (st, _, c) = call(&env.admin, "POST", "/admin/api/clients", Some(ADMIN), Some(json!({"name": "second", "note": "тест"}))).await;
    assert_eq!(st, StatusCode::OK);
    let key = c["key"].as_str().unwrap().to_string();
    let (st, _, _) = call(&env.router, "GET", "/v1/models", Some(&key), None).await;
    assert_eq!(st, StatusCode::OK);
    let (_, _, list) = call(&env.admin, "GET", "/admin/api/clients", Some(ADMIN), None).await;
    assert!(!serde_json::to_string(&list).unwrap().contains(&key));
    let (st, _, _) = call(&env.admin, "PATCH", "/admin/api/clients/second", Some(ADMIN), Some(json!({"enabled": false}))).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = call(&env.router, "GET", "/v1/models", Some(&key), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, _) = call(&env.admin, "DELETE", "/admin/api/clients/second", Some(ADMIN), None).await;
    assert_eq!(st, StatusCode::OK);

    // Вход по токену: cookie-сессия, изменения только с X-LMQ.
    let (st, h, _) = call(&env.admin, "POST", "/admin/api/login", None, Some(json!({"token": ADMIN}))).await;
    assert_eq!(st, StatusCode::OK);
    let cookie = h.get("set-cookie").unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/admin/api/pause")
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"paused":true}"#))
        .unwrap();
    assert_eq!(env.admin.clone().oneshot(req).await.unwrap().status(), StatusCode::FORBIDDEN);
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/admin/api/pause")
        .header("cookie", &cookie)
        .header("x-lmq", "1")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"paused":true}"#))
        .unwrap();
    assert_eq!(env.admin.clone().oneshot(req).await.unwrap().status(), StatusCode::OK);
    assert!(env.core.app.paused.load(std::sync::atomic::Ordering::Relaxed));

    // Одноразовая ссылка из окна: переход на экран панели, чужой адрес не принимается, повтор — 403.
    let once = env.core.app.create_login_link();
    let (st, h, _) = call(&env.admin, "GET", &format!("/admin/login?once={once}&to=%23%2Fqueue%2Fjob-1"), None, None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(h.get("location").unwrap(), "/ui/#/queue/job-1");
    let (st, _, _) = call(&env.admin, "GET", &format!("/admin/login?once={once}"), None, None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let once = env.core.app.create_login_link();
    let (_, h, _) = call(&env.admin, "GET", &format!("/admin/login?once={once}&to=https%3A%2F%2Fevil.example"), None, None).await;
    assert_eq!(h.get("location").unwrap(), "/ui/");
}

#[tokio::test]
async fn export_import_with_password_and_profiles() {
    let up = upstream("OK", 0).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n        api_key: k-123\n", url(&up));
    let env = start(&models, &[], bot_ok().await).await;
    let app = &env.core.app;
    let plain = lmq_core::admin::export_settings(app, None).unwrap();
    assert!(!plain.contains("k-123"));
    let locked = lmq_core::admin::export_settings(app, Some("pw")).unwrap();
    assert!(!locked.contains("k-123") && locked.contains("pw:"));
    assert!(lmq_core::admin::import_settings(app, &locked, Some("wrong"), None).is_err());
    lmq_core::admin::import_settings(app, &locked, Some("pw"), Some("restored")).unwrap();
    let names: Vec<String> = lmq_core::admin::profiles(app).into_iter().map(|p| p.name).collect();
    assert!(names.contains(&"restored".to_string()));
    app.switch_profile("restored").unwrap();
    assert_eq!(app.effective().profile, "restored");
    assert_eq!(app.settings().models["m"].upstreams[0].api_key, "k-123");
    assert!(lmq_core::admin::delete_profile(app, "restored").is_err(), "active profile can't be deleted");
}

#[tokio::test]
async fn protocol1_sends_to_telegram() {
    let up = upstream("hello tg", 0).await;
    let tg = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/bot.+/sendMessage$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": {"message_id": 99, "chat": {"id": -1}}})))
        .mount(&tg)
        .await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n", url(&up));
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.yaml"), format!("{}protocol1:\n  telegram_api: {}\n", settings_yaml(&models), tg.uri())).unwrap();

    // Выключен по умолчанию.
    let env_off = start(&models, &[], bot_ok().await).await;
    let mut j = job("p1", -1, "x");
    j["protocol"] = json!(1);
    assert_eq!(post_job(&env_off, j.clone()).await.0, StatusCode::BAD_REQUEST);

    let env = start_in(dir, "", &[("PROTOCOL1_ENABLED", "true"), ("TELEGRAM_BOT_TOKEN", "123:abc")], bot_ok().await).await;
    assert_eq!(post_job(&env, j).await.0, StatusCode::ACCEPTED);
    assert_eq!(wait_delivery(&env, "p1", 5).await.delivery, "delivered");
    // Боту — {"result": Message} (§9).
    let cbs = callbacks(&env.bot).await;
    assert_eq!(cbs.len(), 1);
    assert_eq!(cbs[0].1["result"]["message_id"], 99);
    assert_eq!(cbs[0].0.as_deref(), Some(CB_TOKEN));
    // В Telegram — текст и reply_parameters.
    let sent = tg.received_requests().await.unwrap();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].url.path().contains("/bot123:abc/sendMessage"));
    let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body["text"], "hello tg");
    assert_eq!(body["chat_id"], -1);
    assert_eq!(body["reply_parameters"]["message_id"], 4711);
}

#[tokio::test]
async fn debug_sink_receives_callbacks() {
    let up = upstream("debug answer", 0).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n", url(&up));
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.yaml"), settings_yaml(&models).replace("admin_listen: ''", "admin_listen: 127.0.0.1:0")).unwrap();
    let paths = lmq_core::config::AppPaths::from_dirs(cfg, dir.path().join("data"), false);
    let bot = bot_ok().await;
    let env_map = std::collections::HashMap::from([
        ("API_KEYS".to_string(), format!("bot:{KEY}")),
        ("ADMIN_TOKEN".to_string(), ADMIN.to_string()),
        ("BOT_CALLBACK_URL".to_string(), format!("{}/webhook/llm", bot.uri())),
    ]);
    let core =
        lmq_core::Core::start(lmq_core::StartOptions { explicit_paths: Some(paths), env: Some(env_map), listeners: true, ..Default::default() }).await.unwrap();
    let bound = core.wait_bound(Duration::from_secs(5)).await;
    let admin_addr = bound.admin.expect("admin bound");
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .post(format!("http://{admin_addr}/admin/api/debug/job"))
        .bearer_auth(ADMIN)
        .json(&json!({"model": "m", "text": "hi"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["status"], "Added to queue", "{r}");
    let mut got = None;
    for _ in 0..100 {
        let list: serde_json::Value =
            client.get(format!("http://{admin_addr}/admin/api/debug/callbacks")).bearer_auth(ADMIN).send().await.unwrap().json().await.unwrap();
        if let Some(first) = list["items"].as_array().and_then(|a| a.first()).cloned() {
            got = Some(first);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let got = got.expect("debug sink got a callback");
    assert_eq!(got["body"]["text"], "debug answer");
    assert_ne!(got["token"], "debug-token", "token must be masked");
    // Веб-панель отдаётся с адреса панели.
    let page = client.get(format!("http://{admin_addr}/ui/")).send().await.unwrap();
    assert_eq!(page.status(), 200);
    core.shutdown().await;
}

#[tokio::test]
async fn engine_runs_captures_output_and_restarts() {
    let up = upstream("OK", 0).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n", url(&up));
    let (exe, args) = if cfg!(windows) {
        ("cmd", vec!["/C", "echo engine-line-1 && echo error: boom 1>&2"])
    } else {
        ("sh", vec!["-c", "echo engine-line-1; echo 'error: boom' 1>&2"])
    };
    let engines = format!(
        "engines:\n  - id: fake\n    kind: custom\n    exe: {exe}\n    args: [{}]\n    restart_on_crash: true\n    max_restarts_per_hour: 1\n    health_url: http://127.0.0.1:9/health\n    ready_timeout: 5\n",
        args.iter().map(|a| format!("'{a}'")).collect::<Vec<_>>().join(", ")
    );
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("settings.yaml"), format!("{}{engines}", settings_yaml(&models))).unwrap();
    let env = start_in(dir, "", &[], bot_ok().await).await;
    let app = env.core.app.clone();
    app.engines.start("fake").unwrap();
    let mut status = String::new();
    for _ in 0..200 {
        let info = app.engines.info("fake").unwrap();
        status = format!("{:?}", info.status);
        if info.status == lmq_core::engines::EngineStatus::Failed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(status, "Failed", "after one restart the engine must stop restarting");
    let logs = app.engines.logs("fake", 100);
    assert!(logs.iter().any(|l| l.text.contains("engine-line-1") && l.stream == "out"));
    assert!(logs.iter().any(|l| l.stream == "err" && l.level == "error"));
    assert_eq!(app.engines.info("fake").unwrap().restarts, 1);
    // Лог движка пишется и в файл.
    let file = std::fs::read_to_string(app.paths.engine_log("fake")).unwrap();
    assert!(file.contains("engine-line-1"));
}

/// Имя модели со слешем (`qwen/qwen3-27b`, как у облачных провайдеров) работает в путях панели.
#[tokio::test]
async fn model_name_with_slash_in_admin_paths() {
    let up = upstream("OK", 0).await;
    let models = format!("  qwen/q-27b:\n    upstreams:\n      - url: {}\n", url(&up));
    let env = start(&models, &[], bot_ok().await).await;
    let (st, _, v) = call(&env.admin, "POST", "/admin/api/models/qwen%2Fq-27b/test", Some(ADMIN), Some(json!({}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], true, "{v}");
}

/// Апстрим отклоняет параметр (как Groq с `mirostat`): LMQ убирает его, повторяет запрос,
/// запоминает и дальше не отправляет. Параметры из `drop_params` не уходят никогда.
#[tokio::test]
async fn rejected_params_are_dropped_and_remembered() {
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("mirostat"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": {"message": "property 'mirostat' is unsupported"}})))
        .with_priority(1)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("top_k"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": {"message": "property 'top_k' is unsupported"}})))
        .with_priority(2)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"choices": [{"message": {"role": "assistant", "content": "OK"}}]})))
        .with_priority(5)
        .mount(&s)
        .await;
    let models = format!("  m:\n    upstreams:\n      - url: {}/v1/chat/completions\n        drop_params: [top_k]\n", s.uri());
    let env = start(&models, &[], bot_ok().await).await;
    let req = json!({"model": "m", "messages": [{"role": "user", "content": "ping"}], "mirostat": 2, "top_k": 40});
    let (st, _, body) = call(&env.router, "POST", "/v1/chat/completions", Some(KEY), Some(req.clone())).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let up = env.core.app.routing().models["m"].upstreams[0].clone();
    assert!(up.learned_drops.lock().contains("mirostat"));
    let sent_before = s.received_requests().await.unwrap().len();
    // Второй запрос: mirostat уже не уходит — одна попытка.
    let (st, _, _) = call(&env.router, "POST", "/v1/chat/completions", Some(KEY), Some(req)).await;
    assert_eq!(st, StatusCode::OK);
    let reqs = s.received_requests().await.unwrap();
    assert_eq!(reqs.len(), sent_before + 1);
    let last = String::from_utf8_lossy(&reqs.last().unwrap().body).to_string();
    assert!(!last.contains("mirostat") && !last.contains("top_k"), "{last}");
}
