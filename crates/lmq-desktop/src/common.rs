//! Общее для обоих видов окна: ядро в процессе, автозапуск, системный акцент и тема,
//! уведомления Windows, перезапуск, открытие папок и ссылок.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use lmq_core::config::{Settings, UiMode};
use lmq_core::events::Event;
use lmq_core::{App, Core};
use parking_lot::Mutex;

pub struct Desktop {
    pub rt: tokio::runtime::Handle,
    pub app: Arc<App>,
    /// Токен окна (действует как токен администратора, живёт до выхода).
    pub token: String,
    /// `http://127.0.0.1:<порт панели>`.
    pub admin_base: String,
    pub minimized: bool,
    core: Mutex<Option<Core>>,
}

impl Desktop {
    pub fn new(rt: tokio::runtime::Handle, core: Core, token: String, admin_base: String, minimized: bool) -> Arc<Self> {
        let app = core.app.clone();
        Arc::new(Self { rt, app, token, admin_base, minimized, core: Mutex::new(Some(core)) })
    }

    pub fn settings(&self) -> Arc<Settings> {
        self.app.settings()
    }

    pub fn api_url(&self) -> String {
        match self.app.bound.lock().client {
            Some(a) => format!("http://{}/v1/chat/completions", a.to_string().replace("0.0.0.0", "127.0.0.1")),
            None => "—".into(),
        }
    }

    /// Останавливает ядро (движки, статистика, БД). Повторный вызов ничего не делает.
    pub fn shutdown(&self) {
        let core = self.core.lock().take();
        if let Some(core) = core {
            let _ = std::thread::scope(|s| {
                s.spawn(|| self.rt.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(20), core.shutdown()).await })).join()
            });
        }
    }

    pub fn toggle_pause(&self) -> bool {
        let p = !self.app.paused.load(Ordering::Relaxed);
        self.app.paused.store(p, Ordering::Relaxed);
        self.app.queue.wake.notify_one();
        tracing::info!(paused = p, "dispatch pause toggled from desktop");
        p
    }

    /// Автозапуск при входе в Windows (`HKCU\…\Run`), с флагом `--minimized`.
    pub fn apply_autostart(&self) {
        let want = self.settings().app.autostart;
        let Ok(exe) = std::env::current_exe() else { return };
        let launcher = auto_launch::AutoLaunchBuilder::new().set_app_name("LMQ").set_app_path(&exe.to_string_lossy()).set_args(&["--minimized"]).build();
        match launcher {
            Ok(l) => {
                let on = l.is_enabled().unwrap_or(false);
                let r = if want && !on {
                    l.enable()
                } else if !want && on {
                    l.disable()
                } else {
                    Ok(())
                };
                if let Err(e) = r {
                    tracing::warn!(error = %e, "autostart change failed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "autostart unavailable"),
        }
    }

    /// Перезапуск приложения в заданном виде окна. Новый процесс ждёт, пока этот
    /// освободит мьютекс единственного экземпляра.
    pub fn restart(&self, mode: Option<UiMode>) -> ! {
        let exe = std::env::current_exe().expect("current exe");
        let mut cmd = std::process::Command::new(exe);
        // Прочие аргументы (например, `--home`) переходят в новый процесс как есть.
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--after" | "--mode" => {
                    let _ = it.next();
                }
                "--minimized" => {}
                _ => {
                    cmd.arg(a);
                }
            }
        }
        cmd.arg("--after").arg(std::process::id().to_string());
        if let Some(m) = mode {
            cmd.arg("--mode").arg(match m {
                UiMode::Native => "native",
                UiMode::Web => "web",
            });
        }
        self.shutdown();
        let _ = cmd.spawn();
        std::process::exit(0);
    }

    pub fn open_path(&self, which: &str) {
        let p = match which {
            "config" => self.app.paths.config_dir.clone(),
            "data" => self.app.paths.data_dir.clone(),
            "logs" => self.app.paths.log_dir.clone(),
            _ => return,
        };
        let _ = open::that_detached(p);
    }

    pub fn open_url(&self, url: &str) {
        if url.starts_with("https://") || url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost") {
            let _ = open::that_detached(url);
        }
    }

    /// Полная веб-панель в браузере по одноразовой ссылке входа.
    pub fn open_panel_in_browser(&self) {
        self.open_panel_at("");
    }

    /// Панель в браузере сразу на нужном экране (`#/queue/<id>`).
    pub fn open_panel_at(&self, route: &str) {
        let once = self.app.create_login_link();
        let to: String = route
            .chars()
            .map(|c| {
                if c == '#' {
                    "%23".to_string()
                } else if c == '&' {
                    "%26".to_string()
                } else if c == '?' {
                    "%3F".to_string()
                } else if c == '=' {
                    "%3D".to_string()
                } else {
                    c.to_string()
                }
            })
            .collect();
        let suffix = if route.is_empty() { String::new() } else { format!("&to={to}") };
        self.open_url(&format!("{}/admin/login?once={once}{suffix}", self.admin_base));
    }

    pub fn run_installer(&self, path: &str) {
        let p = std::path::Path::new(path);
        if !p.starts_with(self.app.paths.data_dir.join("updates")) || !p.is_file() {
            return;
        }
        if std::process::Command::new(p).spawn().is_ok() {
            self.shutdown();
            std::process::exit(0);
        }
    }

    /// Подсказка значка в трее.
    pub fn tooltip(&self) -> String {
        let q = self.app.queue.total_queued();
        let r = self.app.queue.total_running();
        let paused = self.app.paused.load(Ordering::Relaxed);
        format!("LMQ — {}\nв очереди: {q}, выполняется: {r}", if paused { "выдача на паузе" } else { "работает" })
    }
}

/// Акцентный цвет Windows `#rrggbb` (реестр DWM).
#[cfg(windows)]
pub fn system_accent() -> Option<String> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey("Software\\Microsoft\\Windows\\DWM").ok()?;
    let v: u32 = key.get_value("AccentColor").ok()?;
    Some(format!("#{:02x}{:02x}{:02x}", v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff))
}

#[cfg(not(windows))]
pub fn system_accent() -> Option<String> {
    None
}

/// Тёмная тема приложений Windows.
#[cfg(windows)]
pub fn system_dark() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
        .map(|v| v == 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn system_dark() -> bool {
    false
}

/// Сборка Windows: 22000+ — Windows 11 (Mica).
#[cfg(windows)]
pub fn windows_build() -> u32 {
    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
        .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber"))
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[cfg(not(windows))]
pub fn windows_build() -> u32 {
    0
}

pub fn notify(title: &str, body: &str) {
    let (t, b) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new().appname("LMQ").summary(&t).body(&b).show();
    });
}

/// Уведомления Windows о важных событиях: апстрим исключён или вернулся, задание не
/// доставлено, движок остановлен после сбоев. «Не доставлено» собирается за 30 с.
pub fn watch_notifications(d: &Arc<Desktop>) {
    let app = d.app.clone();
    d.rt.spawn(async move {
        let mut rx = app.events.subscribe();
        let mut dead: Vec<String> = Vec::new();
        let mut flush_at: Option<tokio::time::Instant> = None;
        loop {
            let ev = tokio::select! {
                r = rx.recv() => r,
                _ = async { match flush_at { Some(t) => tokio::time::sleep_until(t).await, None => std::future::pending().await } } => {
                    if !dead.is_empty() && app.settings().app.notifications {
                        let body = if dead.len() == 1 { format!("Задание {} не доставлено боту", dead[0]) } else { format!("Не доставлено заданий: {}", dead.len()) };
                        notify("LMQ: доставка", &body);
                    }
                    dead.clear();
                    flush_at = None;
                    continue;
                }
                _ = app.shutdown.cancelled() => break,
            };
            let ev = match ev {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            if !app.settings().app.notifications {
                continue;
            }
            match ev {
                Event::Upstream { id, model, state, error } if state == "open" => {
                    notify("LMQ: апстрим исключён", &format!("{id} ({model}): {}", error.unwrap_or_default()));
                }
                Event::Upstream { id, state, .. } if state == "closed" => notify("LMQ: апстрим снова в работе", &id),
                Event::Job { job_id, state, .. } if state == "dead" => {
                    dead.push(job_id);
                    flush_at.get_or_insert(tokio::time::Instant::now() + std::time::Duration::from_secs(30));
                }
                Event::Engine { id, status } if status == "failed" => {
                    notify("LMQ: движок остановлен", &format!("{id} падал слишком часто и больше не перезапускается"));
                }
                _ => {}
            }
        }
    });
}

/// Проверка обновлений при запуске (если включена) — уведомлением.
pub fn check_updates_on_start(d: &Arc<Desktop>) {
    let app = d.app.clone();
    d.rt.spawn(async move {
        let s = app.settings();
        if !s.updates.auto_check || s.updates.repo.trim().is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        let info = lmq_core::updates::check(&app).await;
        if info.newer {
            notify("LMQ: доступно обновление", &format!("Версия {} — Настройки → Обновления", info.latest.unwrap_or_default()));
        }
    });
}
