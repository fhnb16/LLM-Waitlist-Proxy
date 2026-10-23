//! Состояние приложения и его жизненный цикл.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use parking_lot::Mutex;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::{self, AppPaths, CliOverrides, Effective, PathOverrides, Secrets, Settings};
use crate::crypto::Cipher;
use crate::debug::DebugSink;
use crate::engines::EngineManager;
use crate::events::{Event, EventBus};
use crate::gpu::GpuMonitor;
use crate::metrics::Metrics;
use crate::queue::{QItem, Queue, TIER_GROUP, TIER_PRIVATE};
use crate::routing::{Routing, UpstreamRt};
use crate::schedule::{self, ScheduleNow};
use crate::stats::{Sample, Stats};
use crate::store::Store;
use crate::upstream::UpErr;
use crate::util::{hash_admin, now_ms, random_token};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Параметры запуска ядра.
#[derive(Clone, Default)]
pub struct StartOptions {
    pub paths: PathOverrides,
    /// Готовые пути (тесты); перекрывает `paths`.
    pub explicit_paths: Option<AppPaths>,
    pub profile: Option<String>,
    pub cli: CliOverrides,
    /// Окружение вместо системного (тесты).
    pub env: Option<HashMap<String, String>>,
    pub init_logging: bool,
    pub console_log: bool,
    /// Поднимать HTTP-слушатели.
    pub listeners: bool,
    /// Токен окна приложения (Tauri): действует как токен администратора.
    pub desktop_token: Option<String>,
    /// Запускать движки с `autostart`.
    pub autostart_engines: bool,
}

pub struct App {
    pub paths: AppPaths,
    pub cipher: Cipher,
    pub admin_hash: Mutex<Option<String>>,
    effective: ArcSwap<Effective>,
    settings: ArcSwap<Settings>,
    routing: ArcSwap<Routing>,
    schedule: ArcSwap<ScheduleNow>,
    pub store: Store,
    pub stats: Stats,
    pub metrics: Metrics,
    pub events: EventBus,
    pub queue: Queue,
    pub delivery_wake: Notify,
    /// Запросы к апстримам и Telegram.
    pub http: reqwest::Client,
    /// Клиенты исходящих запросов по маршрутам (интерфейс, прокси).
    pub net: crate::net::NetPool,
    /// Реле Telegram Bot API: счётчики и кэш файлов.
    pub tg_relay: crate::tgrelay::Relay,
    pub engines: Arc<EngineManager>,
    pub gpu: GpuMonitor,
    pub debug: DebugSink,
    pub started_at: i64,
    pub shutdown: CancellationToken,
    /// Сессии панели: токен → срок действия.
    pub sessions: Mutex<HashMap<String, i64>>,
    login_links: Mutex<HashMap<String, i64>>,
    /// Ручная пауза выдачи заданий (панель, трей).
    pub paused: AtomicBool,
    pub cli: CliOverrides,
    pub desktop_token_hash: Option<String>,
    /// Увеличивается, когда слушателям нужно перезапуститься (сменился адрес).
    pub listeners_epoch: watch::Sender<u64>,
    pub bound: Mutex<BoundAddrs>,
    autostart_engines: AtomicBool,
    env: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct BoundAddrs {
    pub client: Option<SocketAddr>,
    pub admin: Option<SocketAddr>,
    pub client_error: Option<String>,
    pub admin_error: Option<String>,
}

impl App {
    pub fn settings(&self) -> Arc<Settings> {
        self.settings.load_full()
    }

    pub fn effective(&self) -> Arc<Effective> {
        self.effective.load_full()
    }

    pub fn routing(&self) -> Arc<Routing> {
        self.routing.load_full()
    }

    pub fn schedule(&self) -> Arc<ScheduleNow> {
        self.schedule.load_full()
    }

    pub fn env_var(&self, name: &str) -> Option<String> {
        match &self.env {
            Some(m) => m.get(name).cloned(),
            None => std::env::var(name).ok(),
        }
    }

    pub fn emit_job(&self, job_id: &str, state: &str, model: &str, chat_id: i64, error_code: Option<&str>) {
        self.events.emit(Event::Job {
            job_id: job_id.to_string(),
            state: state.to_string(),
            model: model.to_string(),
            chat_id,
            error_code: error_code.map(String::from),
        });
    }

    pub fn emit_upstream(&self, up: &UpstreamRt, state: &str, error: Option<String>) {
        self.events.emit(Event::Upstream { id: up.id.clone(), model: up.model.clone(), state: state.to_string(), error });
    }

    pub fn note_upstream_failure(&self, up: &UpstreamRt, e: &UpErr) {
        let cfg = self.settings();
        let code = match e {
            UpErr::Timeout(_) => "upstream_timeout",
            _ => "upstream_error",
        };
        self.metrics.upstream_error(&up.model, code);
        if up.health.on_failure(&e.message(), &cfg.breaker) {
            tracing::warn!(upstream = %up.id, model = %up.model, error = %e.message(), "upstream breaker opened");
            self.emit_upstream(up, "open", Some(e.message()));
        }
    }

    pub fn metrics_upstream_error(&self, model: &str, code: &str) {
        self.metrics.upstream_error(model, code);
    }

    pub fn metrics_callback_failure(&self) {
        self.metrics.callback_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Итог задания — в статистику и метрики.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        model: &str,
        upstream: Option<&str>,
        client: &str,
        chat_kind: &str,
        error_code: Option<&str>,
        wait_ms: Option<i64>,
        latency_ms: Option<i64>,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) {
        self.stats.record(&Sample { model, upstream, client, chat_kind, error_code, wait_ms, latency_ms, prompt_tokens, completion_tokens });
        if let Some(w) = wait_ms {
            self.metrics.observe_wait(w);
        }
        if let (Some(l), None) = (latency_ms, error_code) {
            self.metrics.observe_latency(model, l);
        }
        self.metrics.finished(error_code.unwrap_or("done"));
    }

    /// Применяет новый файл настроек: проверка, запись, горячая замена.
    pub fn apply_file_settings(&self, new_file: Settings) -> Result<()> {
        let cur = self.effective();
        let env_fn = |k: &str| self.env_var(k);
        let eff = config::compose(new_file, &self.paths, &cur.profile, cur.settings_path.clone(), &self.cipher, &self.cli, &env_fn)?;
        config::save_file(&eff.settings_path, &eff.file, &self.cipher)?;
        self.install(eff, false);
        Ok(())
    }

    /// Ставит конфигурацию в работу без записи на диск.
    pub fn install(&self, eff: Effective, first: bool) {
        let old = self.settings();
        let new = eff.settings.clone();
        if old.tg_relay.upstream != new.tg_relay.upstream || old.tg_relay.file_cache_mb != new.tg_relay.file_cache_mb {
            self.tg_relay.clear_cache();
        }
        let routing = Routing::build(&new, Some(&self.routing()));
        self.routing.store(Arc::new(routing));
        self.settings.store(Arc::new(new.clone()));
        let profile = eff.profile.clone();
        self.effective.store(Arc::new(eff));
        crate::logging::set_level(&new.logging.level);
        self.engines.sync(&new.engines, first && self.autostart_engines.load(Ordering::Relaxed));
        self.schedule.store(Arc::new(schedule::evaluate(&new.schedule)));
        if !first
            && (old.server.listen != new.server.listen
                || old.server.admin_listen != new.server.admin_listen
                || old.server.max_body_mb != new.server.max_body_mb
                || old.server.body_read_timeout != new.server.body_read_timeout
                || old.server.admin_cors_origins != new.server.admin_cors_origins)
        {
            self.listeners_epoch.send_modify(|e| *e += 1);
        }
        if !first {
            tracing::info!(profile = %profile, "configuration applied");
            self.events.emit(Event::Config { profile });
        }
        self.queue.wake.notify_one();
    }

    /// Переключает профиль настроек.
    pub fn switch_profile(&self, profile: &str) -> Result<()> {
        let env_fn = |k: &str| self.env_var(k);
        let eff = config::build_effective(&self.paths, profile, &self.cipher, &self.cli, &env_fn)?;
        if !eff.settings_path.exists() {
            config::save_file(&eff.settings_path, &eff.file, &self.cipher)?;
        }
        std::fs::write(self.paths.active_profile_file(), profile)?;
        self.install(eff, false);
        Ok(())
    }

    /// Проверка токена администратора (панель, admin API).
    pub fn check_admin_token(&self, token: &str) -> bool {
        let h = hash_admin(token);
        if let Some(env) = self.env_var("ADMIN_TOKEN").filter(|t| !t.is_empty())
            && crate::util::ct_eq(token.as_bytes(), env.as_bytes())
        {
            return true;
        }
        if let Some(d) = &self.desktop_token_hash
            && crate::util::ct_eq(h.as_bytes(), d.as_bytes())
        {
            return true;
        }
        match self.admin_hash.lock().as_deref() {
            Some(stored) => crate::util::ct_eq(h.as_bytes(), stored.as_bytes()),
            None => false,
        }
    }

    /// Новый токен администратора (старый перестаёт действовать). Возвращает открытый токен.
    pub fn reset_admin_token(&self) -> Result<String> {
        let token = random_token(24);
        let h = hash_admin(&token);
        Secrets::save_admin_hash(&self.paths.secrets_file, Some(h.clone()))?;
        *self.admin_hash.lock() = Some(h);
        self.sessions.lock().clear();
        Ok(token)
    }

    pub fn create_session(&self) -> String {
        let t = random_token(32);
        let mut s = self.sessions.lock();
        let now = now_ms();
        s.retain(|_, exp| *exp > now);
        s.insert(t.clone(), now + 30 * 24 * 3_600_000);
        t
    }

    pub fn check_session(&self, t: &str) -> bool {
        let now = now_ms();
        self.sessions.lock().get(t).is_some_and(|exp| *exp > now)
    }

    pub fn drop_session(&self, t: &str) {
        self.sessions.lock().remove(t);
    }

    /// Одноразовая ссылка входа в панель (окно приложения → браузер): действует 60 с.
    pub fn create_login_link(&self) -> String {
        let t = random_token(24);
        let mut l = self.login_links.lock();
        let now = now_ms();
        l.retain(|_, exp| *exp > now);
        l.insert(t.clone(), now + 60_000);
        t
    }

    /// Погашает одноразовую ссылку: `true`, если она была и не истекла.
    pub fn redeem_login_link(&self, t: &str) -> bool {
        self.login_links.lock().remove(t).is_some_and(|exp| exp > now_ms())
    }

    pub fn db_path(&self) -> PathBuf {
        db_path(&self.paths, &self.settings())
    }

    /// Клиент для апстрима по его маршруту.
    pub fn upstream_client(&self, up: &crate::routing::UpstreamRt) -> Result<reqwest::Client, String> {
        self.net.client(&up.route, crate::net::Kind::General)
    }

    /// Клиент колбэков (без редиректов) по маршруту `network.callback`.
    pub fn callback_client(&self) -> Result<reqwest::Client, String> {
        self.net.client(&self.settings().network.callback, crate::net::Kind::NoRedirect)
    }

    pub fn telegram_client(&self) -> Result<reqwest::Client, String> {
        self.net.client(&self.settings().network.telegram, crate::net::Kind::General)
    }
}

fn db_path(paths: &AppPaths, s: &Settings) -> PathBuf {
    if s.storage.db_path.trim().is_empty() { paths.default_db() } else { PathBuf::from(&s.storage.db_path) }
}

pub struct Core {
    pub app: Arc<App>,
    tasks: Vec<JoinHandle<()>>,
    /// Токен администратора, созданный при первом запуске (показать один раз).
    pub first_admin_token: Option<String>,
}

impl Core {
    pub async fn start(opts: StartOptions) -> Result<Core> {
        let paths = match &opts.explicit_paths {
            Some(p) => p.clone(),
            None => AppPaths::resolve(&opts.paths)?,
        };
        paths.ensure()?;
        let env = opts.env.clone();
        let env_fn = |k: &str| match &env {
            Some(m) => m.get(k).cloned(),
            None => std::env::var(k).ok(),
        };
        let secrets = Secrets::load_or_create(&paths.secrets_file)?;
        let cipher = Cipher::new(&secrets.master_key);
        let profile = opts
            .profile
            .clone()
            .or_else(|| std::fs::read_to_string(paths.active_profile_file()).ok().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "default".into());
        let eff = config::build_effective(&paths, &profile, &cipher, &opts.cli, &env_fn)?;
        if !eff.settings_path.exists() {
            config::save_file(&eff.settings_path, &eff.file, &cipher).context("write default settings")?;
        }
        if opts.init_logging {
            crate::logging::init(Some(&paths.log_dir), &eff.settings.logging.level, eff.settings.logging.keep_days, opts.console_log);
        }

        let db = db_path(&paths, &eff.settings);
        let store = Store::open(&db).await?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent(format!("LMQ/{VERSION}"))
            .build()?;

        let events = EventBus::default();
        let engines = Arc::new(EngineManager::new(paths.clone(), events.clone()));
        let (epoch_tx, _) = watch::channel(0u64);

        let mut first_admin_token = None;
        let mut admin_hash = secrets.admin_token_hash.clone();
        if admin_hash.is_none() && env_fn("ADMIN_TOKEN").is_none_or(|t| t.is_empty()) {
            let token = random_token(24);
            let h = hash_admin(&token);
            Secrets::save_admin_hash(&paths.secrets_file, Some(h.clone()))?;
            admin_hash = Some(h);
            first_admin_token = Some(token);
        }

        let settings = eff.settings.clone();
        let app = Arc::new(App {
            paths: paths.clone(),
            cipher,
            admin_hash: Mutex::new(admin_hash),
            routing: ArcSwap::from_pointee(Routing::default()),
            settings: ArcSwap::from_pointee(settings.clone()),
            effective: ArcSwap::from_pointee(eff.clone()),
            schedule: ArcSwap::from_pointee(ScheduleNow::default()),
            stats: Stats::new(store.clone()),
            store,
            metrics: Metrics::default(),
            events,
            queue: Queue::default(),
            delivery_wake: Notify::new(),
            http,
            net: crate::net::NetPool::default(),
            tg_relay: crate::tgrelay::Relay::default(),
            engines,
            gpu: GpuMonitor::default(),
            debug: DebugSink::default(),
            started_at: now_ms(),
            shutdown: CancellationToken::new(),
            sessions: Mutex::new(HashMap::new()),
            login_links: Mutex::new(HashMap::new()),
            paused: AtomicBool::new(false),
            cli: opts.cli.clone(),
            desktop_token_hash: opts.desktop_token.as_deref().map(hash_admin),
            listeners_epoch: epoch_tx,
            bound: Mutex::new(BoundAddrs::default()),
            autostart_engines: AtomicBool::new(false),
            env: opts.env.clone(),
        });
        app.autostart_engines.store(opts.autostart_engines, Ordering::Relaxed);
        app.install(eff, true);

        // Восстановление очереди после перезапуска (§5.1).
        let max_attempts = 1 + app.settings().queue.upstream_retries as i64;
        let (requeued, failed) = app.store.recover(max_attempts).await?;
        let queued = app.store.load_queued().await?;
        let private_first = app.settings().queue.private_first;
        for q in &queued {
            let tier = if private_first && q.priority == 1 { TIER_PRIVATE } else { TIER_GROUP };
            app.queue.push(
                &q.model_used,
                tier,
                QItem { job_id: q.job_id.clone(), chat_id: q.chat_id, created_at: q.created_at, client: q.client.clone(), sync: None },
            );
        }
        tracing::info!(
            version = VERSION,
            config = %app.effective().settings_path.display(),
            db = %db.display(),
            queued = queued.len(),
            requeued,
            failed,
            "LMQ core started"
        );
        if app.settings().clients.is_empty() {
            tracing::warn!("no API clients configured: requests will get 401 until a client key is created");
        }

        let mut tasks = vec![
            tokio::spawn(crate::dispatcher::run(app.clone())),
            tokio::spawn(crate::callback::run(app.clone())),
            tokio::spawn(housekeeping(app.clone())),
            tokio::spawn(health_checks(app.clone())),
            tokio::spawn(ticker(app.clone())),
            tokio::spawn(forward_logs(app.clone())),
        ];
        app.gpu.spawn(Duration::from_secs(2), app.shutdown.clone());
        if opts.listeners {
            tasks.push(tokio::spawn(crate::api::serve_loop(app.clone(), crate::api::Listener::Client)));
            tasks.push(tokio::spawn(crate::api::serve_loop(app.clone(), crate::api::Listener::Admin)));
        }
        app.delivery_wake.notify_one();
        Ok(Core { app, tasks, first_admin_token })
    }

    /// Адреса слушателей, когда они поднялись (до `timeout`).
    pub async fn wait_bound(&self, timeout: Duration) -> BoundAddrs {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let b = self.app.bound.lock().clone();
            let admin_needed = !self.app.settings().server.admin_listen.is_empty();
            if (b.client.is_some() || b.client_error.is_some()) && (!admin_needed || b.admin.is_some() || b.admin_error.is_some()) {
                return b;
            }
            if tokio::time::Instant::now() > deadline {
                return b;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub async fn shutdown(self) {
        tracing::info!("LMQ core stopping");
        self.app.shutdown.cancel();
        self.app.engines.stop_all().await;
        let _ = self.app.stats.flush(true).await;
        for t in self.tasks {
            let _ = tokio::time::timeout(Duration::from_secs(5), t).await;
        }
        self.app.store.pool.close().await;
    }
}

/// Истечение `MAX_WAIT`, сброс статистики, чистка, расписание.
async fn housekeeping(app: Arc<App>) {
    let mut tick: u64 = 0;
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = app.shutdown.cancelled() => break,
        }
        tick += 1;
        let cfg = app.settings();
        let now = now_ms();
        let expired = app.queue.take_expired(now, (cfg.queue.max_wait as i64) * 1000);
        for (id, model, item) in &expired {
            let msg = format!("job waited longer than MAX_WAIT ({}s)", cfg.queue.max_wait);
            if let Ok(true) = app.store.finish_err(id, "expired", "expired", &msg, None, now).await {
                let kind = if item.chat_id > 0 { "private" } else { "group" };
                app.record(model, None, &item.client, kind, Some("expired"), Some(now - item.created_at), None, 0, 0);
                app.emit_job(id, "expired", model, item.chat_id, Some("expired"));
                tracing::info!(job_id = %id, model = %model, chat_id = item.chat_id, state = "expired", wait_ms = now - item.created_at, "job expired");
            }
        }
        if !expired.is_empty() {
            app.delivery_wake.notify_one();
        }
        let load = app.queue.load();
        let slots: u32 = app.routing().models.values().map(|m| m.slots()).sum();
        app.stats.sample_queue(app.queue.total_queued() as i64, app.queue.total_running() as i64, slots as i64);
        if tick.is_multiple_of(5) {
            let _ = load;
            if let Err(e) = app.stats.flush(false).await {
                tracing::warn!(error = %e, "stats flush failed");
            }
            let new = schedule::evaluate(&cfg.schedule);
            let changed = serde_json::to_string(&new).ok() != serde_json::to_string(&*app.schedule()).ok();
            app.schedule.store(Arc::new(new));
            if changed {
                app.queue.wake.notify_one();
            }
        }
        if tick.is_multiple_of(600) || tick == 30 {
            match app.store.cleanup(cfg.storage.retention_hours).await {
                Ok(n) if n > 0 => tracing::info!(removed = n, "old jobs removed"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "cleanup failed"),
            }
            let _ = app.stats.cleanup(cfg.storage.stats_retention_days).await;
        }
    }
}

/// Фоновая проверка апстримов (`GET {base}/models`).
async fn health_checks(app: Arc<App>) {
    loop {
        let cfg = app.settings();
        let period = cfg.breaker.health_interval;
        if period > 0 {
            let routing = app.routing();
            let ups: Vec<Arc<UpstreamRt>> = routing.upstreams().cloned().collect();
            let checks = ups.into_iter().map(|u| {
                let app = app.clone();
                async move {
                    if let Some(e) = &u.engine
                        && !app.engines.is_ready(e)
                    {
                        u.health.on_check(false, 0, Some(format!("engine '{e}' is not ready")));
                        return;
                    }
                    let before = u.health.breaker();
                    let (ok, ms, err) = match app.upstream_client(&u) {
                        Ok(http) => crate::upstream::check(&http, &u).await,
                        Err(e) => (false, 0, Some(format!("route: {e}"))),
                    };
                    u.health.on_check(ok, ms, err.clone());
                    if before != u.health.breaker() {
                        app.emit_upstream(&u, if ok { "half_open" } else { "open" }, err);
                    }
                }
            });
            futures::future::join_all(checks).await;
        }
        let wait = Duration::from_secs(if period == 0 { 30 } else { period });
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = app.shutdown.cancelled() => break,
        }
    }
}

/// Сводка для панели и трея раз в 2 секунды (только если кто-то слушает).
async fn ticker(app: Arc<App>) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = app.shutdown.cancelled() => break,
        }
        if app.events.has_listeners() {
            let o = crate::admin::overview(&app).await;
            app.events.emit(Event::Tick(Box::new(o)));
        }
    }
}

/// Записи журнала — в шину событий (SSE панели).
async fn forward_logs(app: Arc<App>) {
    let mut rx = crate::logging::hub().subscribe();
    loop {
        tokio::select! {
            r = rx.recv() => match r {
                Ok(e) => app.events.emit(Event::Log(e)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
            _ = app.shutdown.cancelled() => break,
        }
    }
}
