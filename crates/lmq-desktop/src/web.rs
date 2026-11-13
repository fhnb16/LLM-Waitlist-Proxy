//! Веб-окно: Tauri v2 + WebView2, панель из web/ с 26 темами, размытие Mica/Acrylic.

use std::cell::RefCell;
use std::sync::Arc;

use lmq_core::config::{Backdrop, ColorScheme};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::common::{self, Desktop};
use crate::tray::{Tray, TrayAction, copy_to_clipboard};

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

type D<'a> = State<'a, Arc<Desktop>>;

#[tauri::command]
fn system_accent() -> Option<String> {
    common::system_accent()
}

#[tauri::command]
fn apply_appearance(app: AppHandle, d: D<'_>) {
    if let Some(w) = app.get_webview_window("main") {
        apply_backdrop(&w, &d);
    }
}

#[tauri::command]
fn apply_app_settings(d: D<'_>) {
    d.apply_autostart();
}

#[tauri::command]
fn restart_app(d: D<'_>) {
    d.restart(Some(d.settings().ui.mode));
}

#[tauri::command]
fn open_path(which: String, d: D<'_>) {
    d.open_path(&which);
}

#[tauri::command]
fn open_url(url: String, d: D<'_>) {
    d.open_url(&url);
}

#[tauri::command]
fn run_installer(path: String, confirmed: Option<bool>, d: D<'_>) {
    d.run_installer(&path, confirmed.unwrap_or(false));
}

#[tauri::command]
fn quit(app: AppHandle, d: D<'_>) {
    d.shutdown();
    app.exit(0);
}

fn dark_for(scheme: ColorScheme) -> bool {
    match scheme {
        ColorScheme::Dark => true,
        ColorScheme::Light => false,
        ColorScheme::System => common::system_dark(),
    }
}

/// Mica (Windows 11) → Acrylic (Windows 10) → без размытия.
fn apply_backdrop(w: &WebviewWindow, d: &Desktop) {
    let ui = d.settings().ui.clone();
    let dark = dark_for(ui.scheme);
    let _ = w.set_theme(Some(if dark { tauri::Theme::Dark } else { tauri::Theme::Light }));
    let _ = window_vibrancy::clear_mica(w);
    let _ = window_vibrancy::clear_acrylic(w);
    let want = ui.backdrop;
    let ok = match want {
        Backdrop::None => false,
        Backdrop::Mica if common::windows_build() >= 22000 => window_vibrancy::apply_mica(w, Some(dark)).is_ok(),
        _ => window_vibrancy::apply_acrylic(w, Some(if dark { (32, 32, 32, 160) } else { (243, 243, 243, 160) })).is_ok(),
    };
    tracing::debug!(?want, ok, "backdrop applied");
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

pub fn run(d: Arc<Desktop>) -> anyhow::Result<()> {
    tauri::async_runtime::set(d.rt.clone());
    let init = format!(
        "window.__LMQ__ = {{ base: {}, token: {}, desktop: true, platform: 'windows' }};",
        serde_json::to_string(&d.admin_base)?,
        serde_json::to_string(&d.token)?
    );
    let d_setup = d.clone();
    let d_exit = d.clone();
    let app = tauri::Builder::default()
        .manage(d.clone())
        .invoke_handler(tauri::generate_handler![system_accent, apply_appearance, apply_app_settings, restart_app, open_path, open_url, run_installer, quit])
        .setup(move |app| {
            let d = d_setup.clone();
            let ui = d.settings().ui.clone();
            let ws = &ui.window;
            let mut b = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("LMQ")
                .inner_size(if ws.width >= 480 { ws.width as f64 } else { 1240.0 }, if ws.height >= 400 { ws.height as f64 } else { 820.0 })
                .min_inner_size(380.0, 480.0)
                .transparent(ui.backdrop != Backdrop::None)
                .visible(!d.minimized)
                .initialization_script(&init);
            if let (Some(x), Some(y)) = (ws.x, ws.y) {
                b = b.position(x as f64, y as f64);
            }
            let w = b.build()?;
            if ws.maximized {
                let _ = w.maximize();
            }
            apply_backdrop(&w, &d);

            let handle = app.handle().clone();
            let dd = d.clone();
            let tray = Tray::new(move |a| {
                let h = handle.clone();
                let d = dd.clone();
                let _ = handle.run_on_main_thread(move || match a {
                    TrayAction::Show => show_main(&h),
                    TrayAction::TogglePause => {
                        let p = d.toggle_pause();
                        TRAY.with(|t| {
                            if let Some(t) = t.borrow().as_ref() {
                                t.refresh(&d.tooltip(), p)
                            }
                        });
                    }
                    TrayAction::CopyApi => copy_to_clipboard(&d.api_url()),
                    TrayAction::OpenBrowser => d.open_panel_in_browser(),
                    TrayAction::Quit => {
                        d.shutdown();
                        h.exit(0);
                    }
                });
            })?;
            TRAY.with(|t| *t.borrow_mut() = Some(tray));

            // Подсказка трея раз в 3 с.
            let handle = app.handle().clone();
            let dd = d.clone();
            d.rt.spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let d = dd.clone();
                    let _ = handle.run_on_main_thread(move || {
                        let paused = d.app.paused.load(std::sync::atomic::Ordering::Relaxed);
                        TRAY.with(|t| {
                            if let Some(t) = t.borrow().as_ref() {
                                t.refresh(&d.tooltip(), paused)
                            }
                        });
                    });
                }
            });

            let handle = app.handle().clone();
            crate::single::listen(&d.rt, move || {
                let h = handle.clone();
                let _ = handle.run_on_main_thread(move || show_main(&h));
            });
            common::watch_notifications(&d);
            common::watch_updates(&d);
            Ok(())
        })
        .on_window_event(move |w, e| {
            let Some(d) = w.app_handle().try_state::<Arc<Desktop>>().map(|s| s.inner().clone()) else { return };
            match e {
                WindowEvent::CloseRequested { api, .. } => {
                    save_window_state(w, &d);
                    if d.settings().app.close_to_tray {
                        api.prevent_close();
                        let _ = w.hide();
                    } else {
                        d.shutdown();
                        w.app_handle().exit(0);
                    }
                }
                WindowEvent::ThemeChanged(_) => {
                    if let Some(ww) = w.app_handle().get_webview_window("main") {
                        apply_backdrop(&ww, &d);
                    }
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())?;
    app.run(move |_h, e| {
        if let tauri::RunEvent::Exit = e {
            d_exit.shutdown();
        }
    });
    Ok(())
}

/// Размер и положение окна — в настройки (ui.window).
fn save_window_state(w: &tauri::Window, d: &Desktop) {
    let (Ok(size), Ok(pos), Ok(max)) = (w.inner_size(), w.outer_position(), w.is_maximized()) else { return };
    let scale = w.scale_factor().unwrap_or(1.0);
    let mut file = d.app.effective().file.clone();
    let ws = &mut file.ui.window;
    if !max {
        ws.width = (size.width as f64 / scale) as u32;
        ws.height = (size.height as f64 / scale) as u32;
        ws.x = Some((pos.x as f64 / scale) as i32);
        ws.y = Some((pos.y as f64 / scale) as i32);
    }
    ws.maximized = max;
    if file.ui.window != d.app.effective().file.ui.window {
        let _ = d.app.apply_file_settings(file);
    }
}
