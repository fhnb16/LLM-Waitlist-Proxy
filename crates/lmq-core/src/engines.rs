//! Локальные движки инференса: llama.cpp (`llama-server`), vLLM, Ollama, KoboldCpp или любой
//! бинарник. Запуск, остановка, перезапуск при падении, ожидание готовности, журнал
//! вывода в реальном времени.
//!
//! На Windows каждый движок живёт в своём Job Object с `KILL_ON_JOB_CLOSE`: при выходе
//! LMQ (даже аварийном) процесс и его потомки завершаются. Окно консоли не создаётся.
//! На Unix движок запускается в своей группе процессов; остановка — SIGTERM, затем SIGKILL.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::Notify;

use crate::config::{AppPaths, EngineCfg, EngineKind};
use crate::events::{Event, EventBus};
use crate::routing::engine_endpoint;
use crate::util::now_ms;

const LOG_CAP: usize = 5000;
const LOG_FILE_MAX: u64 = 10 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct EngineLine {
    pub ts: i64,
    /// `out` | `err` | `sys`
    pub stream: &'static str,
    /// Уровень, угаданный по тексту: `error` | `warn` | `info` | `debug`.
    pub level: &'static str,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineStatus {
    Stopped,
    Starting,
    Ready,
    Stopping,
    Crashed,
    Failed,
}

impl EngineStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Stopping => "stopping",
            Self::Crashed => "crashed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct EngineInfo {
    pub id: String,
    pub name: String,
    pub kind: EngineKind,
    pub status: EngineStatus,
    pub pid: Option<u32>,
    pub started_at: Option<i64>,
    pub ready_at: Option<i64>,
    pub restarts: u32,
    pub last_exit: Option<String>,
    pub command: Vec<String>,
    pub exe_resolved: Option<String>,
    pub endpoint: String,
    pub health_url: String,
}

struct RunState {
    status: EngineStatus,
    pid: Option<u32>,
    started_at: Option<i64>,
    ready_at: Option<i64>,
    last_exit: Option<String>,
    restarts: u32,
    restart_times: VecDeque<i64>,
    running: bool,
}

struct EngineRt {
    cfg: Mutex<EngineCfg>,
    st: Mutex<RunState>,
    logs: Mutex<VecDeque<EngineLine>>,
    stop: AtomicBool,
    stop_notify: Notify,
    log_path: PathBuf,
}

pub struct EngineManager {
    engines: Mutex<BTreeMap<String, Arc<EngineRt>>>,
    events: EventBus,
    paths: AppPaths,
    http: reqwest::Client,
}

/// Команда запуска (исполняемый файл и аргументы) по настройкам движка.
pub fn build_command(e: &EngineCfg) -> (String, Vec<String>, BTreeMap<String, String>) {
    let mut args: Vec<String> = Vec::new();
    let mut env = e.env.clone();
    let l = &e.llama;
    let (endpoint, _) = engine_endpoint(e);
    let port = url::Url::parse(&endpoint).ok().and_then(|u| u.port()).unwrap_or(8080).to_string();
    let host = if l.host.is_empty() { "127.0.0.1".to_string() } else { l.host.clone() };
    fn kv(args: &mut Vec<String>, a: &str, v: String) {
        args.push(a.to_string());
        args.push(v);
    }
    match e.kind {
        EngineKind::LlamaCpp => {
            if !l.model.is_empty() {
                kv(&mut args, "-m", l.model.clone());
            }
            kv(&mut args, "--host", host);
            kv(&mut args, "--port", port);
            if l.ctx_size > 0 {
                kv(&mut args, "--ctx-size", l.ctx_size.to_string());
            }
            if l.parallel > 0 {
                kv(&mut args, "--parallel", l.parallel.to_string());
            }
            if l.n_gpu_layers >= 0 {
                kv(&mut args, "-ngl", l.n_gpu_layers.to_string());
            }
            if l.threads > 0 {
                kv(&mut args, "--threads", l.threads.to_string());
            }
            if !l.chat_template.is_empty() {
                kv(&mut args, "--chat-template", l.chat_template.clone());
            }
            if l.flash_attn {
                kv(&mut args, "--flash-attn", "on".into());
            }
            if !l.alias.is_empty() {
                kv(&mut args, "--alias", l.alias.clone());
            }
            if l.jinja {
                args.push("--jinja".into());
            }
        }
        EngineKind::Vllm => {
            args.push("serve".into());
            if !l.model.is_empty() {
                args.push(l.model.clone());
            }
            kv(&mut args, "--host", host);
            kv(&mut args, "--port", port);
            if l.ctx_size > 0 {
                kv(&mut args, "--max-model-len", l.ctx_size.to_string());
            }
            if !l.alias.is_empty() {
                kv(&mut args, "--served-model-name", l.alias.clone());
            }
        }
        EngineKind::Ollama => {
            args.push("serve".into());
            env.entry("OLLAMA_HOST".into()).or_insert(format!("{host}:{port}"));
            if l.parallel > 1 {
                env.entry("OLLAMA_NUM_PARALLEL".into()).or_insert(l.parallel.to_string());
            }
        }
        EngineKind::Koboldcpp => {
            if !l.model.is_empty() {
                kv(&mut args, "--model", l.model.clone());
            }
            kv(&mut args, "--host", host);
            kv(&mut args, "--port", port);
            if l.ctx_size > 0 {
                kv(&mut args, "--contextsize", l.ctx_size.to_string());
            }
            if l.n_gpu_layers >= 0 {
                kv(&mut args, "--gpulayers", l.n_gpu_layers.to_string());
            }
        }
        EngineKind::Custom => {}
    }
    args.extend(e.args.iter().cloned());
    (e.exe.clone(), args, env)
}

/// Адрес проверки готовности.
pub fn health_url(e: &EngineCfg) -> String {
    if !e.health_url.trim().is_empty() {
        return e.health_url.trim().to_string();
    }
    let (endpoint, _) = engine_endpoint(e);
    let base = endpoint.trim_end_matches("/v1/chat/completions");
    match e.kind {
        EngineKind::LlamaCpp | EngineKind::Vllm => format!("{base}/health"),
        EngineKind::Ollama => format!("{base}/api/tags"),
        EngineKind::Koboldcpp => format!("{base}/api/v1/model"),
        EngineKind::Custom => format!("{base}/v1/models"),
    }
}

/// Ищет исполняемый файл: путь как есть или в `PATH`.
pub fn resolve_exe(exe: &str) -> Option<PathBuf> {
    if exe.trim().is_empty() {
        return None;
    }
    let p = Path::new(exe);
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    if p.components().count() > 1 {
        return None;
    }
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or(".EXE;.CMD;.BAT".into()).split(';').map(|s| s.to_ascii_lowercase()).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        for ext in &exts {
            let cand = dir.join(format!("{exe}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
        let cand = dir.join(exe);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn guess_level(text: &str) -> &'static str {
    let l = text.to_ascii_lowercase();
    if l.contains("error") || l.contains("fatal") || l.contains("panic") || l.contains("failed") {
        "error"
    } else if l.contains("warn") {
        "warn"
    } else if l.contains("debug") || l.starts_with("slot ") || l.contains("verbose") {
        "debug"
    } else {
        "info"
    }
}

fn quote(a: &str) -> String {
    if a.contains(' ') || a.is_empty() { format!("\"{a}\"") } else { a.to_string() }
}

impl EngineRt {
    fn push_line(&self, events: &EventBus, id: &str, stream: &'static str, text: String) {
        let line = EngineLine { ts: now_ms(), stream, level: if stream == "sys" { "info" } else { guess_level(&text) }, text };
        {
            let mut l = self.logs.lock();
            if l.len() >= LOG_CAP {
                l.pop_front();
            }
            l.push_back(line.clone());
        }
        append_file(&self.log_path, &line);
        events.emit(Event::EngineLog { id: id.to_string(), line });
    }

    fn set_status(&self, events: &EventBus, id: &str, s: EngineStatus) {
        self.st.lock().status = s;
        events.emit(Event::Engine { id: id.to_string(), status: s.as_str().into() });
    }
}

fn append_file(path: &Path, line: &EngineLine) {
    use std::io::Write;
    if std::fs::metadata(path).map(|m| m.len() > LOG_FILE_MAX).unwrap_or(false) {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let ts = chrono::DateTime::from_timestamp_millis(line.ts)
            .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S%.3f").to_string())
            .unwrap_or_default();
        let _ = writeln!(f, "{ts} [{}] {}", line.stream, line.text);
    }
}

impl EngineManager {
    pub fn new(paths: AppPaths, events: EventBus) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(3)).build().expect("http client");
        Self { engines: Mutex::new(BTreeMap::new()), events, paths, http }
    }

    /// Синхронизирует список движков с настройками. При `start_autostart` (старт
    /// приложения) движки с `autostart` запускаются.
    pub fn sync(self: &Arc<Self>, cfgs: &[EngineCfg], start_autostart: bool) {
        let mut to_start = Vec::new();
        {
            let mut map = self.engines.lock();
            map.retain(|id, rt| {
                let keep = cfgs.iter().any(|c| &c.id == id);
                if !keep {
                    rt.stop.store(true, Ordering::SeqCst);
                    rt.stop_notify.notify_waiters();
                }
                keep
            });
            for c in cfgs {
                match map.get(&c.id) {
                    Some(rt) => *rt.cfg.lock() = c.clone(),
                    None => {
                        let rt = Arc::new(EngineRt {
                            cfg: Mutex::new(c.clone()),
                            st: Mutex::new(RunState {
                                status: EngineStatus::Stopped,
                                pid: None,
                                started_at: None,
                                ready_at: None,
                                last_exit: None,
                                restarts: 0,
                                restart_times: VecDeque::new(),
                                running: false,
                            }),
                            logs: Mutex::new(VecDeque::new()),
                            stop: AtomicBool::new(false),
                            stop_notify: Notify::new(),
                            log_path: self.paths.engine_log(&c.id),
                        });
                        map.insert(c.id.clone(), rt);
                        if c.autostart && start_autostart {
                            to_start.push(c.id.clone());
                        }
                    }
                }
            }
        }
        for id in to_start {
            let _ = self.start(&id);
        }
    }

    fn get(&self, id: &str) -> Option<Arc<EngineRt>> {
        self.engines.lock().get(id).cloned()
    }

    pub fn list(&self) -> Vec<EngineInfo> {
        self.engines.lock().iter().map(|(id, rt)| info(id, rt)).collect()
    }

    pub fn info(&self, id: &str) -> Option<EngineInfo> {
        self.get(id).map(|rt| info(id, &rt))
    }

    pub fn is_ready(&self, id: &str) -> bool {
        self.get(id).is_some_and(|rt| rt.st.lock().status == EngineStatus::Ready)
    }

    pub fn logs(&self, id: &str, tail: usize) -> Vec<EngineLine> {
        self.get(id)
            .map(|rt| {
                let l = rt.logs.lock();
                let skip = l.len().saturating_sub(tail);
                l.iter().skip(skip).cloned().collect()
            })
            .unwrap_or_default()
    }

    pub fn clear_logs(&self, id: &str) {
        if let Some(rt) = self.get(id) {
            rt.logs.lock().clear();
        }
    }

    pub fn start(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let rt = self.get(id).ok_or_else(|| anyhow::anyhow!("engine '{id}' not found"))?;
        {
            let mut st = rt.st.lock();
            if st.running {
                return Ok(());
            }
            st.running = true;
            st.restart_times.clear();
        }
        rt.stop.store(false, Ordering::SeqCst);
        let me = self.clone();
        let id = id.to_string();
        tokio::spawn(async move { me.supervise(id, rt).await });
        Ok(())
    }

    pub fn stop(&self, id: &str) -> anyhow::Result<()> {
        let rt = self.get(id).ok_or_else(|| anyhow::anyhow!("engine '{id}' not found"))?;
        rt.stop.store(true, Ordering::SeqCst);
        rt.stop_notify.notify_waiters();
        Ok(())
    }

    pub async fn restart(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        self.stop(id)?;
        for _ in 0..150 {
            if self.get(id).is_some_and(|rt| !rt.st.lock().running) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.start(id)
    }

    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.engines.lock().keys().cloned().collect();
        for id in &ids {
            let _ = self.stop(id);
        }
        for _ in 0..120 {
            if self.engines.lock().values().all(|rt| !rt.st.lock().running) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn supervise(self: Arc<Self>, id: String, rt: Arc<EngineRt>) {
        let mut backoff = 2u64;
        loop {
            let cfg = rt.cfg.lock().clone();
            let outcome = self.run_once(&id, &rt, &cfg).await;
            let stopped = rt.stop.load(Ordering::SeqCst);
            {
                let mut st = rt.st.lock();
                st.pid = None;
                st.ready_at = None;
                st.last_exit = Some(outcome.clone());
            }
            if stopped {
                rt.push_line(&self.events, &id, "sys", format!("остановлен ({outcome})"));
                rt.set_status(&self.events, &id, EngineStatus::Stopped);
                break;
            }
            let now = now_ms();
            let allow = {
                let mut st = rt.st.lock();
                while st.restart_times.front().is_some_and(|t| now - t > 3_600_000) {
                    st.restart_times.pop_front();
                }
                let ok = cfg.restart_on_crash && (st.restart_times.len() as u32) < cfg.max_restarts_per_hour;
                if ok {
                    st.restart_times.push_back(now);
                    st.restarts += 1;
                }
                ok
            };
            if !allow {
                rt.push_line(&self.events, &id, "sys", format!("завершился: {outcome}; перезапуск не выполняется"));
                rt.set_status(&self.events, &id, EngineStatus::Failed);
                tracing::warn!(engine = %id, exit = %outcome, "engine failed");
                break;
            }
            rt.set_status(&self.events, &id, EngineStatus::Crashed);
            rt.push_line(&self.events, &id, "sys", format!("завершился: {outcome}; перезапуск через {backoff} с"));
            tracing::warn!(engine = %id, exit = %outcome, backoff, "engine crashed, restarting");
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                _ = rt.stop_notify.notified() => {}
            }
            if rt.stop.load(Ordering::SeqCst) {
                rt.set_status(&self.events, &id, EngineStatus::Stopped);
                break;
            }
            backoff = (backoff * 2).min(60);
        }
        rt.st.lock().running = false;
    }

    /// Один запуск процесса; возвращает описание завершения.
    async fn run_once(&self, id: &str, rt: &Arc<EngineRt>, cfg: &EngineCfg) -> String {
        let (exe, args, env) = build_command(cfg);
        let Some(exe_path) = resolve_exe(&exe) else {
            rt.push_line(&self.events, id, "sys", format!("исполняемый файл не найден: {exe}"));
            return "exe not found".into();
        };
        rt.set_status(&self.events, id, EngineStatus::Starting);
        rt.push_line(
            &self.events,
            id,
            "sys",
            format!("запуск: {} {}", quote(&exe_path.display().to_string()), args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")),
        );
        let mut cmd = tokio::process::Command::new(&exe_path);
        cmd.args(&args).envs(&env).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        if !cfg.cwd.trim().is_empty() {
            cmd.current_dir(&cfg.cwd);
        } else if let Some(dir) = exe_path.parent() {
            cmd.current_dir(dir);
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        }
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                rt.push_line(&self.events, id, "sys", format!("не удалось запустить: {e}"));
                return format!("spawn error: {e}");
            }
        };
        let pid = child.id();
        #[cfg(windows)]
        let job = pid.and_then(winjob::JobGuard::assign);
        {
            let mut st = rt.st.lock();
            st.pid = pid;
            st.started_at = Some(now_ms());
        }
        if let Some(out) = child.stdout.take() {
            self.pipe(id, rt, out, "out");
        }
        if let Some(err) = child.stderr.take() {
            self.pipe(id, rt, err, "err");
        }
        // Готовность.
        let ready_rt = rt.clone();
        let ready_id = id.to_string();
        let ready_url = health_url(cfg);
        let ready_timeout = cfg.ready_timeout.max(5);
        let http = self.http.clone();
        let events = self.events.clone();
        let ready_task = tokio::spawn(async move {
            let deadline = now_ms() + (ready_timeout as i64) * 1000;
            loop {
                tokio::time::sleep(Duration::from_millis(800)).await;
                if ready_rt.stop.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(r) = http.get(&ready_url).send().await
                    && r.status().is_success()
                {
                    ready_rt.st.lock().ready_at = Some(now_ms());
                    ready_rt.set_status(&events, &ready_id, EngineStatus::Ready);
                    ready_rt.push_line(&events, &ready_id, "sys", format!("готов ({ready_url})"));
                    tracing::info!(engine = %ready_id, "engine ready");
                    return;
                }
                if now_ms() > deadline {
                    ready_rt.push_line(&events, &ready_id, "sys", format!("не ответил на {ready_url} за {ready_timeout} с"));
                    return;
                }
            }
        });
        let status = tokio::select! {
            s = child.wait() => s,
            _ = rt.stop_notify.notified() => {
                rt.set_status(&self.events, id, EngineStatus::Stopping);
                rt.push_line(&self.events, id, "sys", "остановка…".into());
                graceful_stop(&mut child).await;
                #[cfg(windows)]
                if let Some(j) = &job { j.terminate(); }
                child.wait().await
            }
        };
        ready_task.abort();
        #[cfg(windows)]
        drop(job);
        match status {
            Ok(s) => match s.code() {
                Some(c) => format!("код {c}"),
                None => "сигнал".into(),
            },
            Err(e) => format!("ошибка ожидания: {e}"),
        }
    }

    fn pipe<R: AsyncRead + Unpin + Send + 'static>(&self, id: &str, rt: &Arc<EngineRt>, r: R, stream: &'static str) {
        let rt = rt.clone();
        let id = id.to_string();
        let events = self.events.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim_end().to_string();
                if !line.is_empty() {
                    rt.push_line(&events, &id, stream, line);
                }
            }
        });
    }

    /// Проверка бинарника: `--version` (до 10 с), первые строки вывода.
    pub async fn detect(exe: &str) -> anyhow::Result<(String, String)> {
        let path = resolve_exe(exe).ok_or_else(|| anyhow::anyhow!("файл не найден: {exe}"))?;
        let mut cmd = tokio::process::Command::new(&path);
        cmd.arg("--version").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        let out = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await.map_err(|_| anyhow::anyhow!("--version не ответил за 10 с"))??;
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        let head: String = text.lines().filter(|l| !l.trim().is_empty()).take(6).collect::<Vec<_>>().join("\n");
        Ok((path.display().to_string(), head))
    }
}

async fn graceful_stop(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // SAFETY: отправка сигнала группе процессов движка.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        if tokio::time::timeout(Duration::from_secs(10), child.wait()).await.is_ok() {
            return;
        }
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

fn info(id: &str, rt: &EngineRt) -> EngineInfo {
    let cfg = rt.cfg.lock().clone();
    let st = rt.st.lock();
    let (exe, args, _) = build_command(&cfg);
    let mut command = vec![exe.clone()];
    command.extend(args);
    let (endpoint, _) = engine_endpoint(&cfg);
    EngineInfo {
        id: id.to_string(),
        name: if cfg.name.is_empty() { id.to_string() } else { cfg.name.clone() },
        kind: cfg.kind,
        status: st.status,
        pid: st.pid,
        started_at: st.started_at,
        ready_at: st.ready_at,
        restarts: st.restarts,
        last_exit: st.last_exit.clone(),
        command,
        exe_resolved: resolve_exe(&exe).map(|p| p.display().to_string()),
        endpoint,
        health_url: health_url(&cfg),
    }
}

#[cfg(windows)]
mod winjob {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    /// Job Object движка: закрытие дескриптора (выход LMQ) завершает все процессы в нём.
    pub struct JobGuard(HANDLE);

    // SAFETY: дескриптор Job Object можно использовать из любого потока.
    unsafe impl Send for JobGuard {}
    unsafe impl Sync for JobGuard {}

    impl JobGuard {
        pub fn assign(pid: u32) -> Option<Self> {
            // SAFETY: вызовы WinAPI с проверкой результатов.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                let proc_ = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if !proc_.is_null() {
                    AssignProcessToJobObject(job, proc_);
                    CloseHandle(proc_);
                }
                Some(Self(job))
            }
        }

        pub fn terminate(&self) {
            // SAFETY: дескриптор действителен, пока жив guard.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for JobGuard {
        fn drop(&mut self) {
            // SAFETY: дескриптор принадлежит guard.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_command() {
        let mut e = EngineCfg { id: "x".into(), exe: "llama-server".into(), ..Default::default() };
        e.llama.model = "C:\\m\\q.gguf".into();
        e.llama.parallel = 2;
        e.llama.port = 8081;
        e.args = vec!["--verbose".into()];
        let (exe, args, _) = build_command(&e);
        assert_eq!(exe, "llama-server");
        let joined = args.join(" ");
        assert!(joined.contains("-m C:\\m\\q.gguf"));
        assert!(joined.contains("--port 8081"));
        assert!(joined.contains("--parallel 2"));
        assert!(joined.ends_with("--verbose"));
        assert_eq!(health_url(&e), "http://127.0.0.1:8081/health");
    }

    #[test]
    fn ollama_env() {
        let e = EngineCfg { kind: EngineKind::Ollama, exe: "ollama".into(), ..Default::default() };
        let (_, args, env) = build_command(&e);
        assert_eq!(args, vec!["serve"]);
        assert_eq!(env.get("OLLAMA_HOST").map(String::as_str), Some("127.0.0.1:8080"));
    }
}
