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

/// Запрос без разбора тела: для файлов реле.
async fn raw(router: &axum::Router, method_: &str, uri: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    use tower::ServiceExt;
    let req = axum::http::Request::builder().method(method_).uri(uri).body(axum::body::Body::empty()).unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    (status, headers, axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap().to_vec())
}

#[tokio::test]
async fn telegram_relay_forwards_methods_and_files() {
    let tg = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/bot123:abc/sendMessage$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": {"message_id": 7}})))
        .mount(&tg)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/bot123:abc/getChat$"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"ok": false, "error_code": 400, "description": "Bad Request: chat not found"})))
        .mount(&tg)
        .await;
    let photo = vec![0xFFu8, 0xD8, 0xFF, 0xE0, 1, 2, 3];
    Mock::given(method("GET"))
        .and(path_regex(r"^/file/bot123:abc/photos/a\.jpg$"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(photo.clone(), "application/octet-stream"))
        .mount(&tg)
        .await;

    // Выключено по умолчанию: путь не уходит в Telegram и не считается неизвестным.
    let off = start("  {}\n", &[], bot_ok().await).await;
    let (st, _, v) = call(&off.router, "GET", "/bot123:abc/getMe", None, None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(v["ok"], false);

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let relay = format!("tg_relay:\n  enabled: true\n  upstream: {}\n  allowed_bots: ['123']\n", tg.uri());
    std::fs::write(cfg.join("settings.yaml"), format!("{}{relay}", settings_yaml("  {}\n"))).unwrap();
    let env = start_in(dir, "", &[], bot_ok().await).await;

    // Метод: тело и ответ — как есть.
    let (st, _, v) = call(&env.router, "POST", "/bot123:abc/sendMessage", None, Some(json!({"chat_id": 1, "text": "hi"}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["result"]["message_id"], 7);
    let sent = tg.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
    assert_eq!(body["text"], "hi");
    // Ошибка Telegram доходит до бота без изменений.
    let (st, _, v) = call(&env.router, "GET", "/bot123:abc/getChat?chat_id=5", None, None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["description"], "Bad Request: chat not found");
    // Чужой бот и запись в файлы — отказ без обращения к Telegram.
    assert_eq!(call(&env.router, "GET", "/bot999:abc/getMe", None, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(raw(&env.router, "POST", "/file/bot123:abc/photos/a.jpg").await.0, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(tg.received_requests().await.unwrap().len(), 2);

    // Файл: настоящий тип по расширению, второй раз — из кэша.
    for _ in 0..2 {
        let (st, h, bytes) = raw(&env.router, "GET", "/file/bot123:abc/photos/a.jpg").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(h["content-type"], "image/jpeg");
        assert_eq!(bytes, photo);
    }
    assert_eq!(tg.received_requests().await.unwrap().len(), 3);

    // Telegram недоступен — 502 в формате Bot API.
    lmq_core::admin::patch_settings(&env.core.app, json!({"tg_relay": {"upstream": "http://127.0.0.1:1"}})).unwrap();
    let (st, _, v) = call(&env.router, "GET", "/bot123:abc/getMe", None, None).await;
    assert_eq!(st, StatusCode::BAD_GATEWAY);
    assert!(v["description"].as_str().unwrap().starts_with("LMQ relay:"));
    assert!(!v["description"].as_str().unwrap().contains("abc"), "token must not leak into errors");

    // Счётчики — в панели и в /metrics.
    let (st, _, v) = call(&env.admin, "GET", "/admin/api/tg-relay", Some(ADMIN), None).await;
    assert_eq!(st, StatusCode::OK);
    let s = &v["stats"];
    assert_eq!((s["ok"].as_u64(), s["tg_errors"].as_u64(), s["relay_errors"].as_u64(), s["rejected"].as_u64()), (Some(3), Some(1), Some(1), Some(2)));
    assert_eq!(s["cache_hits"], 1);
    assert_eq!(s["methods"]["file"], 2);
    let text = lmq_core::admin::prometheus(&env.core.app).await;
    assert!(text.contains("tg_relay_requests_total{outcome=\"ok\"} 3"));
    assert!(lmq_core::admin::overview(&env.core.app).await.tg_relay.is_some());
}

#[tokio::test]
async fn socks_proxy_connects_and_gives_links() {
    let target = MockServer::start().await;
    Mock::given(method("GET")).and(path_regex(r"^/hello$")).respond_with(ResponseTemplate::new(200).set_body_string("hi")).mount(&target).await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let socks = "socks:\n  enabled: true\n  listen: 127.0.0.1:0\n  username: u\n  password: 'p@ss:1'\n  public_host: proxy.example.com\n";
    std::fs::write(cfg.join("settings.yaml"), format!("{}{socks}", settings_yaml("  {}\n"))).unwrap();
    let env = start_in(dir, "", &[], bot_ok().await).await;
    let app = env.core.app.clone();
    tokio::spawn(lmq_core::socks::run(app.clone()));
    assert!(wait_for(Duration::from_secs(5), || async { app.socks.bound().is_some() }).await, "socks proxy did not start");
    let port = app.socks.bound().unwrap().port();

    // Имя разрешает прокси (socks5h), вход по логину и паролю.
    let via = |user: &str, pass: &str| {
        let mut u = url::Url::parse(&format!("socks5h://127.0.0.1:{port}")).unwrap();
        u.set_username(user).unwrap();
        u.set_password(Some(pass)).unwrap();
        reqwest::Client::builder().proxy(reqwest::Proxy::all(u.as_str()).unwrap()).build().unwrap()
    };
    let target_url = format!("http://localhost:{}/hello", target.address().port());
    let r = via("u", "p@ss:1").get(&target_url).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "hi");
    assert!(via("u", "wrong").get(&target_url).send().await.is_err());

    // Счётчики меняет задача соединения — ждём её, а не полагаемся на порядок задач.
    assert!(wait_for(Duration::from_secs(5), || async { app.socks.snapshot().rejected == 1 }).await, "wrong password must be counted");
    assert_eq!(app.socks.snapshot().connections, 1);
    // Байты считаются по ходу передачи: соединение ещё может быть открыто (keep-alive).
    let s = app.socks.snapshot();
    assert!(s.bytes_up > 0 && s.bytes_down > 0, "traffic must be counted while the connection is open: {s:?}");

    // Готовые ссылки: socks5h с логином и паролем и t.me/socks для Telegram.
    let v = lmq_core::admin::socks_view(&app, false).await;
    let public = &v["links"][0];
    assert_eq!(public["kind"], "public");
    assert_eq!(public["url"], format!("socks5h://u:p%40ss%3A1@proxy.example.com:{port}"));
    assert_eq!(public["telegram"], format!("https://t.me/socks?server=proxy.example.com&port={port}&user=u&pass=p%40ss%3A1"));
    assert_eq!(v["local_only"], true);
    let o = lmq_core::admin::overview(&app).await;
    assert!(!o.socks.unwrap().url.unwrap().contains("p%40ss"), "overview must not show the password");

    // Выключение останавливает слушатель.
    lmq_core::admin::patch_settings(&app, json!({"socks": {"enabled": false}})).unwrap();
    assert!(wait_for(Duration::from_secs(5), || async { app.socks.bound().is_none() }).await);
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
    // Аргументы — JSON-строками: это валидный YAML при любых кавычках внутри (`echo 'error: boom'`).
    let engines = format!(
        "engines:\n  - id: fake\n    kind: custom\n    exe: {exe}\n    args: [{}]\n    restart_on_crash: true\n    max_restarts_per_hour: 1\n    health_url: http://127.0.0.1:9/health\n    ready_timeout: 5\n",
        args.iter().map(|a| serde_json::to_string(a).unwrap()).collect::<Vec<_>>().join(", ")
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

/// Окно «Новая модель»: адрес `…/v1` дополняется до `…/v1/chat/completions`, список моделей
/// приходит до сохранения, ключ проверяется сразу; маска берёт сохранённый ключ апстрима.
/// Короткий адрес в настройках тоже работает.
#[tokio::test]
async fn upstream_probe_lists_models_before_save() {
    use wiremock::matchers::{header, method, path};
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer sk-good"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "llama-3.3-70b"}, {"id": "gemma-2-9b"}]})))
        .with_priority(1)
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": {"message": "Invalid API Key"}})))
        .with_priority(5)
        .mount(&s)
        .await;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(ResponseTemplate::new(200).set_body_json(ok_answer("OK"))).mount(&s).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}/v1\n        api_key: sk-good\n", s.uri());
    let env = start(&models, &[], bot_ok().await).await;
    let probe = |body: serde_json::Value| call(&env.admin, "POST", "/admin/api/upstreams/probe", Some(ADMIN), Some(body));

    let (st, _, v) = probe(json!({"url": format!("{}/v1/", s.uri()), "api_key": "sk-good"})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["url"], format!("{}/v1/chat/completions", s.uri()));
    assert_eq!(v["models"], json!(["gemma-2-9b", "llama-3.3-70b"]));

    let (_, _, v) = probe(json!({"url": format!("{}/v1", s.uri())})).await;
    assert_eq!(v["ok"], false);
    assert_eq!(v["need_key"], true);
    assert_eq!(v["error"], "Сервер просит ключ");

    let (_, _, v) = probe(json!({"url": format!("{}/v1", s.uri()), "api_key": "sk-bad"})).await;
    assert_eq!(v["error"], "Ключ не подошёл", "{v}");

    let (_, _, v) = probe(json!({"url": format!("{}/v1", s.uri()), "api_key": lmq_core::admin::MASK, "model": "m", "index": 0})).await;
    assert_eq!(v["ok"], true, "{v}");

    let (_, _, v) = probe(json!({"url": "ftp://x"})).await;
    assert_eq!(v["ok"], false);

    // Апстрим с адресом `…/v1` в настройках выполняет задания.
    assert_eq!(env.core.app.routing().models["m"].upstreams[0].url, format!("{}/v1/chat/completions", s.uri()));
    let (st, _, v) = call(&env.admin, "POST", "/admin/api/models/m/test", Some(ADMIN), Some(json!({}))).await;
    assert_eq!(st, StatusCode::OK);
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

/// Апстрим со `stream: true`, который отвечает телом `body` типа `mime`.
async fn stream_upstream(body: &str, mime: &str) -> MockServer {
    use wiremock::matchers::path;
    let s = MockServer::start().await;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(ResponseTemplate::new(200).set_body_raw(body.to_string(), mime)).mount(&s).await;
    s
}

fn stream_models(s: &MockServer) -> String {
    format!("  m:\n    upstreams:\n      - url: {}\n        stream: true\n", url(s))
}

/// Апстрим не соблюдает `stream: true` и отвечает обычным JSON — с заголовком JSON или без него.
#[tokio::test]
async fn stream_upstream_answering_plain_json() {
    let body = ok_answer("plain json").to_string();
    for (i, mime) in ["application/json", "text/plain"].into_iter().enumerate() {
        let up = stream_upstream(&body, mime).await;
        let env = start(&stream_models(&up), &[], bot_ok().await).await;
        let id = format!("pj{i}");
        post_job(&env, job(&id, 42, "hi")).await;
        let b = wait_callback(&env.bot, &id, 5).await;
        assert_eq!(b["text"], "plain json", "{mime}: {b}");
    }
}

/// SSE без перевода строки после последнего события и с целым `message` вместо `delta`.
#[tokio::test]
async fn stream_tail_without_newline_and_message_chunks() {
    let sse = concat!(
        "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"думаю\"}}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"Ответ\"},\"finish_reason\":\"stop\"}]}"
    );
    let up = stream_upstream(sse, "text/event-stream").await;
    let env = start(&stream_models(&up), &[], bot_ok().await).await;
    post_job(&env, job("tail", 42, "hi")).await;
    assert_eq!(wait_callback(&env.bot, "tail", 5).await["text"], "Ответ");
}

/// Модель с размышлениями израсходовала `max_tokens` до ответа: `empty_answer` с объяснением.
#[tokio::test]
async fn stream_reasoning_only_is_explained() {
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"долго думаю\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let up = stream_upstream(sse, "text/event-stream").await;
    let env = start(&stream_models(&up), &[], bot_ok().await).await;
    post_job(&env, job("think", 42, "hi")).await;
    let b = wait_callback(&env.bot, "think", 5).await;
    assert_eq!(b["error_code"], "empty_answer");
    assert!(b["error"].as_str().unwrap().contains("max_tokens"), "{b}");
}

/// Пустое тело на стрим-запрос — ошибка апстрима (переход к следующему), а не пустой ответ.
#[tokio::test]
async fn stream_empty_body_fails_over() {
    let empty = stream_upstream("", "text/event-stream").await;
    let good = upstream("from-second", 0).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n        stream: true\n      - url: {}\n", url(&empty), url(&good));
    let env = start(&models, &[], bot_ok().await).await;
    post_job(&env, job("e1", 42, "hi")).await;
    assert_eq!(wait_callback(&env.bot, "e1", 5).await["text"], "from-second");
}

/// Синхронный клиент со `stream: true` получает SSE, а не JSON, который он не разберёт.
#[tokio::test]
async fn sync_stream_client_gets_sse() {
    let up = upstream("sync streamed", 0).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n", url(&up));
    let env = start(&models, &[], bot_ok().await).await;
    let req = json!({"model": "m", "messages": [{"role": "user", "content": "ping"}], "stream": true});
    let (st, headers, body) = call(&env.router, "POST", "/v1/chat/completions", Some(KEY), Some(req)).await;
    assert_eq!(st, StatusCode::OK);
    assert!(headers["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let text = body.as_str().expect("SSE text");
    let events = sse_events(text);
    assert_eq!(events[0]["object"], "chat.completion.chunk");
    assert_eq!(events[0]["choices"][0]["delta"]["content"], "sync streamed");
    assert_eq!(events[0]["model"], "m");
    assert_eq!(events[1]["choices"][0]["finish_reason"], "stop");
    assert_eq!(events.last().unwrap()["usage"]["completion_tokens"], 3);
    assert!(text.trim_end().ends_with("data: [DONE]"));
}

/// События `data:` ответа SSE без `[DONE]`.
fn sse_events(text: &str) -> Vec<serde_json::Value> {
    text.lines().filter_map(|l| l.strip_prefix("data: ")).filter(|d| *d != "[DONE]").map(|d| serde_json::from_str(d).unwrap()).collect()
}

fn stream_req() -> serde_json::Value {
    json!({"model": "m", "messages": [{"role": "user", "content": "ping"}], "stream": true})
}

/// Клиент со стримом получает куски апстрима как есть: размышления, текст, вызовы инструментов.
/// К апстриму запрос уходит стримом, даже если у него `stream: false`.
#[tokio::test]
async fn sync_stream_relays_upstream_chunks() {
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"хм\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"При\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"вет\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"t1\",\"function\":{\"name\":\"f\",\"arguments\":\"{}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n"
    );
    let up = stream_upstream(sse, "text/event-stream").await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n", url(&up));
    let env = start(&models, &[], bot_ok().await).await;
    let (st, headers, body) = call(&env.router, "POST", "/v1/chat/completions", Some(KEY), Some(stream_req())).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(headers["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let ev = sse_events(body.as_str().unwrap());
    let delta = |i: usize| ev[i]["choices"][0]["delta"].clone();
    assert_eq!((delta(0)["role"].as_str(), delta(0)["reasoning_content"].as_str()), (Some("assistant"), Some("хм")));
    assert_eq!((delta(1)["content"].as_str(), delta(2)["content"].as_str()), (Some("При"), Some("вет")));
    assert_eq!(delta(3)["tool_calls"][0]["function"]["name"], "f");
    assert_eq!(ev[4]["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(ev[5]["usage"]["completion_tokens"], 2);
    assert!(ev.iter().all(|e| e["model"] == "m" && e["id"] == ev[0]["id"]));
    let sent: serde_json::Value = serde_json::from_slice(&up.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(sent["stream"], true);
}

/// Стрим оборвался после первых кусков: клиент получает событие `error`, а не склейку с ответом
/// другого апстрима, — второй апстрим не вызывается.
#[tokio::test]
async fn sync_stream_broken_midway_does_not_fail_over() {
    let sse = concat!("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"нача\"}}]}\n\n", "data: {\"error\":{\"message\":\"backend crashed\"}}\n\n");
    let broken = stream_upstream(sse, "text/event-stream").await;
    let good = upstream("from-second", 0).await;
    let models = format!("  m:\n    upstreams:\n      - url: {}\n      - url: {}\n", url(&broken), url(&good));
    let env = start(&models, &[], bot_ok().await).await;
    let (st, _, body) = call(&env.router, "POST", "/v1/chat/completions", Some(KEY), Some(stream_req())).await;
    assert_eq!(st, StatusCode::OK);
    let text = body.as_str().unwrap();
    let ev = sse_events(text);
    assert_eq!(ev[0]["choices"][0]["delta"]["content"], "нача");
    assert!(ev.last().unwrap()["error"]["message"].as_str().unwrap().contains("backend crashed"), "{text}");
    assert!(!text.contains("from-second") && text.trim_end().ends_with("data: [DONE]"));
    assert!(good.received_requests().await.unwrap().is_empty());
}

/// Ошибка до первого куска: ответ ещё не начат — обычный HTTP-статус, и работает переход к другому апстриму.
#[tokio::test]
async fn sync_stream_error_before_first_chunk() {
    let bad = failing_upstream(400).await;
    let env = start(&format!("  m:\n    upstreams:\n      - url: {}\n", url(&bad)), &[], bot_ok().await).await;
    let (st, _, body) = call(&env.router, "POST", "/v1/chat/completions", Some(KEY), Some(stream_req())).await;
    assert_eq!(st, StatusCode::BAD_GATEWAY, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("boom"), "{body}");

    let down = failing_upstream(500).await;
    let good = upstream("from-second", 0).await;
    let env = start(&format!("  m:\n    upstreams:\n      - url: {}\n      - url: {}\n", url(&down), url(&good)), &[], bot_ok().await).await;
    let (st, _, body) = call(&env.router, "POST", "/v1/chat/completions", Some(KEY), Some(stream_req())).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sse_events(body.as_str().unwrap())[0]["choices"][0]["delta"]["content"], "from-second");
}

/// Куски уходят клиенту сразу: первый приходит задолго до конца генерации, а не одним пакетом в конце.
#[tokio::test]
async fn sync_stream_is_not_buffered() {
    use axum::body::Body;
    use futures::StreamExt;
    // Апстрим: кусок, пауза 600 мс, кусок, пауза 600 мс, конец.
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(|| async {
            let s = async_stream::stream! {
                for (i, piece) in ["раз", "два"].into_iter().enumerate() {
                    if i > 0 {
                        tokio::time::sleep(Duration::from_millis(600)).await;
                    }
                    yield Ok::<String, std::convert::Infallible>(format!("data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{piece}\"}}}}]}}\n\n"));
                }
                tokio::time::sleep(Duration::from_millis(600)).await;
                yield Ok("data: [DONE]\n\n".to_string());
            };
            ([("content-type", "text/event-stream")], Body::from_stream(s))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let models = format!("  m:\n    upstreams:\n      - url: http://{addr}/v1/chat/completions\n");
    let env = start(&models, &[], bot_ok().await).await;

    let started = std::time::Instant::now();
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("authorization", format!("Bearer {KEY}"))
        .header("content-type", "application/json")
        .body(Body::from(stream_req().to_string()))
        .unwrap();
    let resp = tower::ServiceExt::oneshot(env.router.clone(), req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut body = resp.into_body().into_data_stream();
    let mut seen = String::new();
    let mut first_at = None;
    while let Some(frame) = body.next().await {
        seen.push_str(&String::from_utf8_lossy(&frame.unwrap()));
        if first_at.is_none() && seen.contains("раз") {
            first_at = Some(started.elapsed());
        }
    }
    let (first, total) = (first_at.expect("первый кусок не пришёл"), started.elapsed());
    assert!(first < Duration::from_millis(500), "первый кусок через {first:?}");
    assert!(total >= Duration::from_millis(1100), "весь ответ за {total:?}");
    assert!(seen.contains("два") && seen.trim_end().ends_with("data: [DONE]"), "{seen}");
}

/// Из стрима собираются вызовы инструментов: аргументы, пришедшие кусками, склеиваются.
#[tokio::test]
async fn stream_tool_calls_are_assembled() {
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Riga\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let up = stream_upstream(sse, "text/event-stream").await;
    let env = start(&stream_models(&up), &[], bot_ok().await).await;
    let req = json!({"model": "m", "messages": [{"role": "user", "content": "погода"}]});
    let (st, _, body) = call(&env.router, "POST", "/v1/chat/completions", Some(KEY), Some(req)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let call0 = &body["choices"][0]["message"]["tool_calls"][0];
    assert_eq!((call0["id"].as_str(), call0["function"]["name"].as_str()), (Some("c1"), Some("get_weather")));
    assert_eq!(call0["function"]["arguments"], "{\"city\":\"Riga\"}");
    assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
}

/// Состояние обновлений для панели: до проверки — пусто, настройки автообновления — по умолчанию.
#[tokio::test]
async fn updates_state_and_defaults() {
    let up = upstream("OK", 0).await;
    let env = start(&format!("  m:\n    upstreams:\n      - url: {}\n", url(&up)), &[], bot_ok().await).await;
    let (st, _, v) = call(&env.admin, "GET", "/admin/api/updates/state", Some(ADMIN), None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!((v["checking"].as_bool(), v["downloading"].as_bool()), (Some(false), Some(false)));
    assert!(v["ready"].is_null() && v["info"].is_null(), "{v}");
    let u = &env.core.app.settings().updates;
    assert!(u.auto_check && u.auto_download && !u.auto_install);
}
