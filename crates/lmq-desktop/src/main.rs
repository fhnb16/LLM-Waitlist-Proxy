//! LMQ для Windows: сервер очереди в процессе и окно управления — нативное (Slint,
//! стиль Fluent, Mica) или веб (Tauri + WebView2, 26 тем панели).
//!
//! Аргументы: `--minimized` — запуск в трей; `--mode native|web` — вид окна на этот
//! запуск; `--home <папка>` — все данные в одной папке; `--after <pid>` (служебный) —
//! дождаться завершения прежнего экземпляра.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod common;
mod native;
mod single;
mod tray;
mod web;

use std::path::PathBuf;
use std::time::Duration;

use lmq_core::config::{CliOverrides, PathOverrides, UiMode};
use lmq_core::{Core, StartOptions};

struct Args {
    minimized: bool,
    mode: Option<UiMode>,
    home: Option<PathBuf>,
    after: bool,
}

fn parse_args() -> Args {
    let mut a = Args { minimized: false, mode: None, home: None, after: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--minimized" => a.minimized = true,
            "--mode" => {
                a.mode = match it.next().as_deref() {
                    Some("web") => Some(UiMode::Web),
                    Some("native") => Some(UiMode::Native),
                    _ => None,
                }
            }
            "--home" => a.home = it.next().map(PathBuf::from),
            "--after" => {
                let _ = it.next();
                a.after = true;
            }
            _ => {}
        }
    }
    a
}

fn fatal(msg: &str) -> ! {
    #[cfg(windows)]
    {
        let text: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
        let title: Vec<u16> = "LMQ".encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: строки нуль-терминированы.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), 0x10);
        }
    }
    eprintln!("{msg}");
    std::process::exit(1);
}

/// WebView2 установлен? (Windows 11 — всегда; Windows 10 — не обязательно.)
#[cfg(windows)]
fn webview2_available() -> bool {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    const GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let paths = [
        (HKEY_LOCAL_MACHINE, format!("SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{GUID}")),
        (HKEY_LOCAL_MACHINE, format!("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{GUID}")),
        (HKEY_CURRENT_USER, format!("Software\\Microsoft\\EdgeUpdate\\Clients\\{GUID}")),
    ];
    paths
        .iter()
        .any(|(root, p)| RegKey::predef(*root).open_subkey(p).and_then(|k| k.get_value::<String, _>("pv")).is_ok_and(|v| !v.is_empty() && v != "0.0.0.0"))
}

#[cfg(not(windows))]
fn webview2_available() -> bool {
    true
}

fn main() {
    let args = parse_args();
    let wait = if args.after { Duration::from_secs(20) } else { Duration::ZERO };
    if !single::acquire(wait) {
        single::signal_show();
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("lmq").build().expect("tokio runtime");
    let token = lmq_core::util::random_token(32);
    let opts = StartOptions {
        paths: PathOverrides { home: args.home.clone(), ..Default::default() },
        // Если адрес панели выключен, окну она поднимается на случайном локальном порту.
        cli: CliOverrides { admin_listen_fallback: Some("127.0.0.1:0".into()), ..Default::default() },
        init_logging: true,
        console_log: cfg!(debug_assertions),
        listeners: true,
        desktop_token: Some(token.clone()),
        autostart_engines: true,
        ..Default::default()
    };
    let core = match rt.block_on(Core::start(opts)) {
        Ok(c) => c,
        Err(e) => fatal(&format!("LMQ не запустился:\n\n{e:#}")),
    };
    let bound = rt.block_on(core.wait_bound(Duration::from_secs(5)));
    let admin = match bound.admin {
        Some(a) => format!("http://{}", a.to_string().replace("0.0.0.0", "127.0.0.1")),
        None => fatal(&format!("Панель LMQ не смогла занять адрес: {}\n\nИзмените ADMIN_LISTEN или освободите порт.", bound.admin_error.unwrap_or_default())),
    };
    let settings = core.app.settings();
    if !settings.app.server_autostart {
        core.app.paused.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let minimized = args.minimized || settings.app.start_minimized;
    let mut mode = args.mode.unwrap_or(settings.ui.mode);
    if mode == UiMode::Web && !webview2_available() {
        mode = UiMode::Native;
        common::notify("LMQ", "WebView2 не установлен — открыто нативное окно. Веб-окно появится после установки WebView2.");
    }
    let desktop = common::Desktop::new(rt.handle().clone(), core, token, admin, minimized);
    desktop.apply_autostart();
    let result = match mode {
        UiMode::Web => web::run(desktop.clone()),
        UiMode::Native => native::run(desktop.clone()),
    };
    desktop.shutdown();
    if let Err(e) = result {
        fatal(&format!("Окно LMQ закрылось с ошибкой:\n\n{e:#}"));
    }
    rt.shutdown_timeout(Duration::from_secs(3));
}
