//! Нативное окно: Slint (стиль Fluent) + Mica/Acrylic. Работает с ядром напрямую
//! через `lmq_core::admin`, без HTTP. Данные обновляются таймером, журналы — из шины
//! событий через буфер, который разбирается в потоке интерфейса.

mod fields;
mod i18n;
mod view;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use lmq_core::admin::{self, Overview};
use lmq_core::config::{Backdrop, ColorScheme, UiMode};
use lmq_core::engines::EngineLine;
use lmq_core::events::Event;
use lmq_core::logging::{LogEntry, level_rank};
use lmq_core::stats::{Query, Range, Series};
use lmq_core::store::JobFilter;
use parking_lot::Mutex;
use serde_json::{Value, json};
use slint::{CloseRequestResponse, ComponentHandle, Model, Timer, TimerMode, VecModel, Weak};

use crate::common::{self, Desktop};
use crate::tray::{Tray, TrayAction, copy_to_clipboard};
use i18n::{t, tf};
use view::model;

slint::include_modules!();

const PAGE_QUEUE: i32 = 1;
const PAGE_ENGINES: i32 = 3;
const PAGE_CLIENTS: i32 = 4;
const PAGE_LOGS: i32 = 5;
const PAGE_SETTINGS: i32 = 6;
const LOG_LIMIT: usize = 1000;
const RANGES: [Range; 5] = [Range::M5, Range::H1, Range::H24, Range::D7, Range::D30];

/// Что накопилось из шины событий между тиками интерфейса.
#[derive(Default)]
struct Inbox {
    logs: Vec<LogEntry>,
    engine: Vec<(String, EngineLine)>,
    jobs_changed: bool,
}

/// Состояние окна; живёт только в потоке интерфейса.
struct State {
    d: Arc<Desktop>,
    ui: Weak<AppWindow>,
    draft: Value,
    locked: Vec<String>,
    tests: view::Tests,
    confirm: Option<Box<dyn FnOnce()>>,
    toast_timer: Timer,
    overview: Option<Overview>,
    series: Option<Series>,
    logs: Rc<VecModel<LogLine>>,
    engine_log: Rc<VecModel<LogLine>>,
    engine_ids: Vec<String>,
    dark: bool,
    tray: Option<Tray>,
    ticks: u64,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Выполнить в потоке интерфейса с доступом к состоянию и окну.
fn on_ui(f: impl FnOnce(&mut State, &AppWindow) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        with(|s| {
            if let Some(ui) = s.ui.upgrade() {
                f(s, &ui);
            }
        });
    });
}

fn toast(s: &mut State, ui: &AppWindow, text: String, kind: &str) {
    let g = ui.global::<UiData>();
    g.set_toast(text.into());
    g.set_toast_kind(kind.into());
    let w = s.ui.clone();
    s.toast_timer.start(TimerMode::SingleShot, Duration::from_millis(3200), move || {
        if let Some(ui) = w.upgrade() {
            ui.global::<UiData>().set_toast("".into());
        }
    });
}

fn toast_result(r: anyhow::Result<()>, ok: String) {
    on_ui(move |s, ui| match r {
        Ok(()) => toast(s, ui, ok, "ok"),
        Err(e) => toast(s, ui, format!("{e:#}"), "bad"),
    });
}

fn confirm(s: &mut State, ui: &AppWindow, title: String, text: String, ok: String, action: impl FnOnce() + 'static) {
    let g = ui.global::<UiData>();
    g.set_confirm_title(title.into());
    g.set_confirm_text(text.into());
    g.set_confirm_ok(ok.into());
    s.confirm = Some(Box::new(action));
}

fn show_secret(ui: &AppWindow, title: String, value: String) {
    let g = ui.global::<UiData>();
    g.set_secret_text(t("Скопируйте ключ сейчас: LMQ хранит только его хэш и больше его не покажет.").into());
    g.set_secret_value(value.into());
    g.set_secret_title(title.into());
}

// ------------------------------------------------------------------ внешний вид

fn dark_for(scheme: ColorScheme) -> bool {
    match scheme {
        ColorScheme::Dark => true,
        ColorScheme::Light => false,
        ColorScheme::System => common::system_dark(),
    }
}

/// Тема и подложка окна. Mica — Windows 11, Acrylic — Windows 10 1809+, иначе сплошной фон.
fn apply_look(s: &mut State, ui: &AppWindow) {
    let cfg = s.d.settings().ui.clone();
    let dark = dark_for(cfg.scheme);
    s.dark = dark;
    ui.set_dark(dark);
    let g = ui.global::<UiData>();
    g.set_scheme(match cfg.scheme {
        ColorScheme::System => 0,
        ColorScheme::Light => 1,
        ColorScheme::Dark => 2,
    });
    g.set_backdrop(match cfg.backdrop {
        Backdrop::Mica => 0,
        Backdrop::Acrylic => 1,
        Backdrop::None => 2,
    });
    let handle = ui.window().window_handle();
    let _ = window_vibrancy::clear_mica(&handle);
    let _ = window_vibrancy::clear_acrylic(&handle);
    let r = match cfg.backdrop {
        Backdrop::None => Ok(()),
        Backdrop::Mica if common::windows_build() >= 22000 => window_vibrancy::apply_mica(&handle, Some(dark)),
        _ => window_vibrancy::apply_acrylic(&handle, Some(if dark { (32, 32, 32, 200) } else { (243, 243, 243, 200) })),
    };
    if let Err(e) = &r {
        tracing::warn!(error = %e, backdrop = ?cfg.backdrop, "window backdrop unavailable");
    }
    let ok = cfg.backdrop != Backdrop::None && r.is_ok();
    set_dark_titlebar(&handle, dark);
    ui.set_solid(!ok);
}

/// Тёмный заголовок окна (DWMWA_USE_IMMERSIVE_DARK_MODE), если подложка его не задала.
#[cfg(windows)]
fn set_dark_titlebar(handle: &slint::WindowHandle, dark: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(h) = handle.window_handle() else { return };
    let RawWindowHandle::Win32(w) = h.as_raw() else { return };
    let value: i32 = dark as i32;
    // SAFETY: hwnd окна Slint, атрибут 20 — DWMWA_USE_IMMERSIVE_DARK_MODE.
    unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(w.hwnd.get() as _, 20, &value as *const i32 as *const _, std::mem::size_of::<i32>() as u32);
    }
}

#[cfg(not(windows))]
fn set_dark_titlebar(_handle: &slint::WindowHandle, _dark: bool) {}

/// Оттенок `#rrggbb` в тёплой части круга (красный … жёлтый, пурпурный).
fn is_warm(hex: &str) -> bool {
    let h = hex.trim_start_matches('#');
    let Ok(v) = u32::from_str_radix(h, 16) else { return false };
    let (r, g, b) = (((v >> 16) & 0xff) as f32 / 255.0, ((v >> 8) & 0xff) as f32 / 255.0, (v & 0xff) as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min < 0.08 {
        return false;
    }
    let d = max - min;
    let hue = if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    !(75.0..=300.0).contains(&hue)
}

fn apply_language(s: &mut State, ui: &AppWindow) {
    let pref = s.d.settings().ui.language.clone();
    i18n::set(i18n::resolve(&pref));
    ui.global::<UiData>().set_language(match pref.as_str() {
        "ru" => 1,
        "en" => 2,
        _ => 0,
    });
}

// ------------------------------------------------------------------- данные

fn refresh_overview(d: &Arc<Desktop>, with_stats: bool, range: i32) {
    let app = d.app.clone();
    d.rt.spawn(async move {
        let ov = admin::overview(&app).await;
        let series = if with_stats {
            let q = Query { range: Some(RANGES[range.clamp(0, 4) as usize]), ..Default::default() };
            app.stats.query(&q).await.ok()
        } else {
            None
        };
        let engines = app.engines.list();
        on_ui(move |s, ui| {
            if let Some(se) = series {
                s.series = Some(se);
            }
            s.overview = Some(ov);
            s.engine_ids = engines.iter().map(|e| e.id.clone()).collect();
            paint_overview(s, ui, &engines);
        });
    });
}

fn paint_overview(s: &mut State, ui: &AppWindow, engines: &[lmq_core::engines::EngineInfo]) {
    let Some(o) = &s.overview else { return };
    let g = ui.global::<UiData>();
    let settings = s.d.settings();
    g.set_paused(o.paused);
    g.set_status_line(view::status_line(o).into());
    g.set_banners(model(view::banners(o)));
    g.set_kpis(model(view::kpis(o, s.series.as_ref())));
    if let Some(se) = &s.series {
        g.set_charts(model(view::charts(se, g.get_range() as usize)));
        g.set_wait_hist(view::wait_hist(se));
    }
    g.set_models(model(view::models(o, &settings, &s.tests)));
    g.set_running(model(view::running(o)));
    g.set_engines(model(view::engines(engines, o.now)));
    g.set_gpus(model(view::gpus(o)));
    let dead = o.counts.get("dead").copied().unwrap_or(0);
    g.set_dead_count(dead as i32);
    ui.set_queued(o.queued as i32);
    ui.set_dead(dead as i32);
    g.set_api_url(s.d.api_url().into());
    if let Some(tray) = &s.tray {
        tray.refresh(&s.d.tooltip(), o.paused);
    }
}

fn reload_jobs(d: &Arc<Desktop>, ui: &AppWindow) {
    let g = ui.global::<UiData>();
    let filter = JobFilter {
        state: Some(view::JOB_FILTERS.get(g.get_job_filter() as usize).copied().unwrap_or("").to_string()).filter(|s| !s.is_empty()),
        query: Some(g.get_job_query().trim().to_string()).filter(|s| !s.is_empty()),
        model: None,
        limit: 200,
        offset: 0,
    };
    let app = d.app.clone();
    d.rt.spawn(async move {
        let r = app.store.list_jobs(&filter).await;
        on_ui(move |_, ui| {
            let g = ui.global::<UiData>();
            match r {
                Ok((rows, total)) => {
                    g.set_jobs(model(view::jobs(&rows, lmq_core::util::now_ms())));
                    g.set_jobs_total(
                        if total as usize > rows.len() {
                            tf("показано {n} из {total}", &[("n", &rows.len()), ("total", &total)])
                        } else {
                            i18n::tn(total, "задание", "задания", "заданий")
                        }
                        .into(),
                    );
                }
                Err(e) => g.set_jobs_total(format!("{e:#}").into()),
            }
        });
    });
}

fn log_matches(e: &LogEntry, min: u8, q: &str) -> bool {
    if level_rank(&e.level) < min {
        return false;
    }
    if q.is_empty() {
        return true;
    }
    let q = q.to_lowercase();
    e.message.to_lowercase().contains(&q) || e.fields.values().any(|v| v.to_string().to_lowercase().contains(&q))
}

fn reload_logs(s: &mut State, ui: &AppWindow) {
    let g = ui.global::<UiData>();
    let min = g.get_log_level().clamp(0, 4) as u8;
    let q = g.get_log_query().to_string();
    let lines: Vec<LogLine> = lmq_core::logging::hub().tail(2000, None, None).iter().filter(|e| log_matches(e, min, &q)).map(view::log_line).collect();
    let skip = lines.len().saturating_sub(LOG_LIMIT);
    s.logs.set_vec(lines.into_iter().skip(skip).collect::<Vec<_>>());
}

fn selected_engine(s: &State, ui: &AppWindow) -> Option<String> {
    s.engine_ids.get(ui.global::<UiData>().get_engine_selected().max(0) as usize).cloned()
}

fn reload_engine_log(s: &mut State, ui: &AppWindow) {
    let lines = match selected_engine(s, ui) {
        Some(id) => s.d.app.engines.logs(&id, LOG_LIMIT).iter().map(view::engine_line).collect(),
        None => Vec::new(),
    };
    s.engine_log.set_vec(lines);
}

fn push_capped(m: &VecModel<LogLine>, items: impl IntoIterator<Item = LogLine>) {
    for l in items {
        m.push(l);
    }
    let over = m.row_count().saturating_sub(LOG_LIMIT);
    if over > 0 {
        let rest: Vec<LogLine> = m.iter().skip(over).collect();
        m.set_vec(rest);
    }
}

fn drain_inbox(s: &mut State, ui: &AppWindow, inbox: &Mutex<Inbox>) {
    let batch = std::mem::take(&mut *inbox.lock());
    let g = ui.global::<UiData>();
    if !batch.logs.is_empty() {
        let min = g.get_log_level().clamp(0, 4) as u8;
        let q = g.get_log_query().to_string();
        push_capped(&s.logs, batch.logs.iter().filter(|e| log_matches(e, min, &q)).map(view::log_line));
    }
    if !batch.engine.is_empty()
        && let Some(id) = selected_engine(s, ui)
    {
        push_capped(&s.engine_log, batch.engine.iter().filter(|(e, _)| *e == id).map(|(_, l)| view::engine_line(l)));
    }
    if batch.jobs_changed && g.get_page() == PAGE_QUEUE {
        reload_jobs(&s.d, ui);
    }
}

fn load_settings(s: &mut State, ui: &AppWindow) {
    let v = admin::settings_view(&s.d.app);
    s.draft = v["file"].clone();
    s.locked = v["locked"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let g = ui.global::<UiData>();
    g.set_fields(model(fields::rows(&s.draft, &s.locked)));
    g.set_settings_dirty(false);
    g.set_settings_error("".into());
    g.set_settings_file(v["paths"]["settings_file"].as_str().unwrap_or("").into());
    g.set_config_dir(v["paths"]["config_dir"].as_str().unwrap_or("").into());
    g.set_data_dir(v["paths"]["data_dir"].as_str().unwrap_or("").into());
    g.set_log_dir(v["paths"]["log_dir"].as_str().unwrap_or("").into());
    let st = s.d.settings();
    g.set_autostart(st.app.autostart);
    g.set_start_minimized(st.app.start_minimized);
    g.set_close_to_tray(st.app.close_to_tray);
    g.set_notifications(st.app.notifications);
    g.set_server_autostart(st.app.server_autostart);
    g.set_update_auto(st.updates.auto_check);
}

fn patch(d: &Desktop, p: Value) -> anyhow::Result<()> {
    admin::patch_settings(&d.app, p)
}

fn save_window_state(d: &Desktop, ui: &AppWindow) {
    let w = ui.window();
    let scale = w.scale_factor().max(0.5);
    let size = w.size();
    let pos = w.position();
    let max = w.is_maximized();
    let mut file = d.app.effective().file.clone();
    let ws = &mut file.ui.window;
    if !max && !w.is_minimized() {
        ws.width = (size.width as f32 / scale) as u32;
        ws.height = (size.height as f32 / scale) as u32;
        ws.x = Some((pos.x as f32 / scale) as i32);
        ws.y = Some((pos.y as f32 / scale) as i32);
    }
    ws.maximized = max;
    if file.ui.window != d.app.effective().file.ui.window {
        let _ = d.app.apply_file_settings(file);
    }
}

fn show_window(_s: &mut State, ui: &AppWindow) {
    let _ = ui.show();
    ui.window().set_minimized(false);
    apply_look_soon();
    bring_to_front(ui);
}

/// Окно winit появляется только в цикле событий, поэтому подложка ставится чуть позже
/// `show()`: до этого у Slint нет дескриптора окна.
fn apply_look_soon() {
    Timer::single_shot(Duration::from_millis(30), || on_ui_now(apply_look));
}

#[cfg(windows)]
fn bring_to_front(ui: &AppWindow) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = ui.window().window_handle();
    let Ok(h) = handle.window_handle() else { return };
    if let RawWindowHandle::Win32(w) = h.as_raw() {
        // SAFETY: hwnd окна Slint.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(w.hwnd.get() as _);
        }
    }
}

#[cfg(not(windows))]
fn bring_to_front(_ui: &AppWindow) {}

fn quit(d: &Desktop, ui: &AppWindow) {
    save_window_state(d, ui);
    let _ = ui.hide();
    let _ = slint::quit_event_loop();
}

// --------------------------------------------------------------------- запуск

pub fn run(d: Arc<Desktop>) -> anyhow::Result<()> {
    let ui = AppWindow::new()?;
    let g = ui.global::<UiData>();
    let th = ui.global::<Theme>();
    th.set_icon_font(if common::windows_build() >= 22000 { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" }.into());
    g.set_version(format!("v{}", lmq_core::VERSION).into());
    g.set_mica_available(common::windows_build() >= 22000);
    th.set_warm_accent(common::system_accent().as_deref().is_some_and(is_warm));

    let logs = Rc::new(VecModel::<LogLine>::default());
    let engine_log = Rc::new(VecModel::<LogLine>::default());
    g.set_logs(logs.clone().into());
    g.set_engine_log(engine_log.clone().into());

    // Размер и положение окна.
    let ws = d.settings().ui.window.clone();
    if ws.width >= 400 && ws.height >= 400 {
        ui.window().set_size(slint::LogicalSize::new(ws.width as f32, ws.height as f32));
    }
    if let (Some(x), Some(y)) = (ws.x, ws.y) {
        ui.window().set_position(slint::LogicalPosition::new(x as f32, y as f32));
    }

    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            d: d.clone(),
            ui: ui.as_weak(),
            draft: Value::Null,
            locked: Vec::new(),
            tests: Default::default(),
            confirm: None,
            toast_timer: Timer::default(),
            overview: None,
            series: None,
            logs,
            engine_log,
            engine_ids: Vec::new(),
            dark: false,
            tray: None,
            ticks: 0,
        })
    });
    with(|s| {
        apply_language(s, &ui);
        load_settings(s, &ui);
        reload_logs(s, &ui);
    });

    wire_actions(&ui, &d);
    // Отладочная сборка: стартовый экран для снимков (`LMQ_NATIVE_PAGE=3`).
    if cfg!(debug_assertions)
        && let Some(p) = std::env::var("LMQ_NATIVE_PAGE").ok().and_then(|v| v.parse().ok())
    {
        ui.global::<UiActions>().invoke_navigate(p);
    }

    // Значок в трее: события приходят из его потока — передаём в поток интерфейса.
    let tray = Tray::new(|a| {
        on_ui(move |s, ui| match a {
            TrayAction::Show => show_window(s, ui),
            TrayAction::TogglePause => {
                let p = s.d.toggle_pause();
                ui.global::<UiData>().set_paused(p);
                if let Some(t) = &s.tray {
                    t.refresh(&s.d.tooltip(), p);
                }
            }
            TrayAction::CopyApi => copy_to_clipboard(&s.d.api_url()),
            TrayAction::OpenBrowser => s.d.open_panel_in_browser(),
            TrayAction::Quit => {
                let d = s.d.clone();
                quit(&d, ui);
            }
        });
    });
    match tray {
        Ok(t) => {
            with(|s| s.tray = Some(t));
        }
        Err(e) => tracing::warn!(error = %e, "tray unavailable"),
    }

    // Закрытие окна: в трей или выход.
    {
        let d = d.clone();
        let w = ui.as_weak();
        ui.window().on_close_requested(move || {
            let Some(ui) = w.upgrade() else { return CloseRequestResponse::HideWindow };
            save_window_state(&d, &ui);
            let has_tray = with(|s| s.tray.is_some()).unwrap_or(false);
            if d.settings().app.close_to_tray && has_tray {
                CloseRequestResponse::HideWindow
            } else {
                let _ = slint::quit_event_loop();
                CloseRequestResponse::HideWindow
            }
        });
    }

    // Поток событий ядра → буфер.
    let inbox = Arc::new(Mutex::new(Inbox::default()));
    {
        let inbox = inbox.clone();
        let app = d.app.clone();
        d.rt.spawn(async move {
            let mut rx = app.events.subscribe();
            let mut logs = lmq_core::logging::hub().subscribe();
            loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Ok(Event::EngineLog { id, line }) => inbox.lock().engine.push((id, line)),
                        Ok(Event::Job { .. }) => inbox.lock().jobs_changed = true,
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => break,
                    },
                    l = logs.recv() => match l {
                        Ok(e) => {
                            let mut b = inbox.lock();
                            if b.logs.len() < 5000 {
                                b.logs.push(e);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => break,
                    },
                    _ = app.shutdown.cancelled() => break,
                }
            }
        });
    }

    // Тик интерфейса: журналы — 4 раза в секунду, обзор — раз в 2 с, графики — реже.
    let fast = Timer::default();
    {
        let inbox = inbox.clone();
        fast.start(TimerMode::Repeated, Duration::from_millis(250), move || {
            on_ui_now(|s, ui| drain_inbox(s, ui, &inbox));
        });
    }
    let slow = Timer::default();
    slow.start(TimerMode::Repeated, Duration::from_secs(2), move || {
        on_ui_now(|s, ui| {
            s.ticks += 1;
            let range = ui.global::<UiData>().get_range();
            let every = if range == 0 { 5 } else { 15 };
            refresh_overview(&s.d, s.series.is_none() || s.ticks % every == 0, range);
            // Тема Windows могла смениться.
            if s.d.settings().ui.scheme == ColorScheme::System && common::system_dark() != s.dark {
                apply_look(s, ui);
            }
        });
    });
    refresh_overview(&d, true, g.get_range());

    // Второй запуск приложения показывает это окно.
    {
        crate::single::listen(&d.rt, move || on_ui(show_window));
    }
    common::watch_notifications(&d);
    common::check_updates_on_start(&d);

    if !d.minimized {
        ui.show()?;
        if ws.maximized {
            ui.window().set_maximized(true);
        }
        apply_look_soon();
    }
    slint::run_event_loop_until_quit()?;
    drop((fast, slow));
    with(|s| s.tray = None);
    Ok(())
}

/// То же, что `on_ui`, но уже в потоке интерфейса.
fn on_ui_now(f: impl FnOnce(&mut State, &AppWindow)) {
    with(|s| {
        if let Some(ui) = s.ui.upgrade() {
            f(s, &ui);
        }
    });
}

fn wire_actions(ui: &AppWindow, d: &Arc<Desktop>) {
    let a = ui.global::<UiActions>();

    a.on_navigate(|page| {
        on_ui_now(|s, ui| {
            let g = ui.global::<UiData>();
            g.set_page(page);
            match page {
                PAGE_QUEUE => reload_jobs(&s.d, ui),
                PAGE_ENGINES => reload_engine_log(s, ui),
                PAGE_CLIENTS => ui.global::<UiData>().set_clients(model(view::clients(&admin::clients(&s.d.app)))),
                PAGE_LOGS => reload_logs(s, ui),
                PAGE_SETTINGS => load_settings(s, ui),
                _ => {}
            }
        });
    });
    a.on_set_range(|i| {
        on_ui_now(|s, ui| {
            ui.global::<UiData>().set_range(i);
            refresh_overview(&s.d, true, i);
        });
    });
    a.on_toggle_pause(|| {
        on_ui_now(|s, ui| {
            let p = s.d.toggle_pause();
            ui.global::<UiData>().set_paused(p);
            toast(s, ui, if p { t("Выдача заданий на паузе") } else { t("Выдача заданий возобновлена") }, if p { "warn" } else { "ok" });
            refresh_overview(&s.d, false, ui.global::<UiData>().get_range());
        });
    });
    a.on_open_panel(|route| {
        on_ui_now(|s, _| s.d.open_panel_at(&route));
    });
    a.on_copy(|text| {
        copy_to_clipboard(&text);
        on_ui_now(|s, ui| toast(s, ui, t("Скопировано"), "ok"));
    });

    // Очередь.
    a.on_jobs_reload(|| on_ui_now(|s, ui| reload_jobs(&s.d, ui)));
    a.on_cancel_job(|id| {
        on_ui_now(|s, ui| {
            let d = s.d.clone();
            let id = id.to_string();
            confirm(s, ui, t("Отменить задание?"), id.clone(), t("Отменить"), move || {
                let app = d.app.clone();
                d.rt.spawn(async move {
                    let r = lmq_core::submit::cancel(&app, &id).await.map(|_| ());
                    toast_result(r, t("Задание отменено"));
                    on_ui(|s, ui| reload_jobs(&s.d, ui));
                });
            });
        });
    });
    a.on_redeliver_job(|id| {
        on_ui_now(|s, _| {
            let app = s.d.app.clone();
            let id = id.to_string();
            s.d.rt.spawn(async move {
                let r = admin::redeliver(&app, &id).await.map(|_| ());
                toast_result(r, t("Доставка повторится"));
                on_ui(|s, ui| reload_jobs(&s.d, ui));
            });
        });
    });
    a.on_redeliver_all(|| {
        on_ui_now(|s, ui| {
            let d = s.d.clone();
            let n = ui.global::<UiData>().get_dead_count();
            confirm(
                s,
                ui,
                t("Повторить доставку всех?"),
                i18n::tn(n as i64, "задание", "задания", "заданий"),
                t("Повторить"),
                move || {
                    let app = d.app.clone();
                    d.rt.spawn(async move {
                        let r = admin::redeliver_all(&app).await;
                        let ok = r.as_ref().map(|n| *n).unwrap_or(0);
                        toast_result(r.map(|_| ()), tf("Доставка повторится: {n}", &[("n", &ok)]));
                        on_ui(|s, ui| reload_jobs(&s.d, ui));
                    });
                },
            );
        });
    });

    // Модели.
    a.on_model_test(|name| {
        on_ui_now(|s, ui| {
            let name = name.to_string();
            s.tests.insert(name.clone(), (true, String::new()));
            let engines = s.d.app.engines.list();
            paint_overview(s, ui, &engines);
            let app = s.d.app.clone();
            s.d.rt.spawn(async move {
                let v = admin::test_model(&app, &name, None).await;
                let text = if v["ok"].as_bool().unwrap_or(false) {
                    let answer: String = v["text"].as_str().unwrap_or("").chars().take(200).collect();
                    format!("✓ {} · {}", view::fmt::ms(v["latency_ms"].as_f64()), answer.trim())
                } else {
                    format!("✗ {}", v["error"]["message"].as_str().or(v["error"].as_str()).unwrap_or("error"))
                };
                on_ui(move |s, ui| {
                    s.tests.insert(name, (false, text));
                    let engines = s.d.app.engines.list();
                    paint_overview(s, ui, &engines);
                });
            });
        });
    });
    a.on_model_toggle(|name, on| {
        on_ui_now(|s, ui| {
            let r = patch(&s.d, json!({ "models": { name.as_str(): { "enabled": on } } }));
            match r {
                Ok(()) => toast(s, ui, if on { tf("Модель {m} включена", &[("m", &name)]) } else { tf("Модель {m} выключена", &[("m", &name)]) }, "ok"),
                Err(e) => toast(s, ui, format!("{e:#}"), "bad"),
            }
            refresh_overview(&s.d, false, ui.global::<UiData>().get_range());
        });
    });

    // Движки.
    a.on_engine_select(|i| {
        on_ui_now(|s, ui| {
            ui.global::<UiData>().set_engine_selected(i);
            reload_engine_log(s, ui);
        });
    });
    a.on_engine_start(|id| {
        on_ui_now(|s, ui| {
            let _rt = s.d.rt.enter();
            if let Err(e) = s.d.app.engines.start(&id) {
                toast(s, ui, format!("{e:#}"), "bad");
            }
        });
    });
    a.on_engine_stop(|id| {
        on_ui_now(|s, ui| {
            let d = s.d.clone();
            let id = id.to_string();
            confirm(
                s,
                ui,
                tf("Остановить {e}?", &[("e", &id)]),
                t("Задания этой модели будут ждать в очереди."),
                t("Остановить"),
                move || {
                    let _ = d.app.engines.stop(&id);
                },
            );
        });
    });
    a.on_engine_restart(|id| {
        on_ui_now(|s, _| {
            let app = s.d.app.clone();
            let id = id.to_string();
            s.d.rt.spawn(async move {
                let r = app.engines.restart(&id).await;
                toast_result(r, t("Движок перезапускается"));
            });
        });
    });
    a.on_engine_clear(|id| {
        on_ui_now(|s, _| {
            s.d.app.engines.clear_logs(&id);
            s.engine_log.set_vec(Vec::new());
        });
    });

    // Клиенты.
    fn refresh_clients(s: &State, ui: &AppWindow) {
        ui.global::<UiData>().set_clients(model(view::clients(&admin::clients(&s.d.app))));
    }
    a.on_client_create(|name, note| {
        on_ui_now(|s, ui| match admin::create_client(&s.d.app, &name, &note) {
            Ok(key) => {
                refresh_clients(s, ui);
                show_secret(ui, tf("Ключ для {c}", &[("c", &name.trim())]), key);
            }
            Err(e) => toast(s, ui, format!("{e:#}"), "bad"),
        });
    });
    a.on_client_toggle(|name, on| {
        on_ui_now(|s, ui| {
            if let Err(e) = admin::set_client(&s.d.app, &name, Some(on), None) {
                toast(s, ui, format!("{e:#}"), "bad");
            }
            refresh_clients(s, ui);
        });
    });
    a.on_client_rotate(|name| {
        on_ui_now(|s, ui| {
            let d = s.d.clone();
            let w = s.ui.clone();
            let n = name.to_string();
            confirm(
                s,
                ui,
                tf("Новый ключ для {c}?", &[("c", &n)]),
                t("Старый ключ перестанет работать сразу — обновите его в боте."),
                t("Заменить ключ"),
                move || {
                    let Some(ui) = w.upgrade() else { return };
                    match admin::rotate_client(&d.app, &n) {
                        Ok(key) => show_secret(&ui, tf("Ключ для {c}", &[("c", &n)]), key),
                        Err(e) => on_ui_now(|s, ui| toast(s, ui, format!("{e:#}"), "bad")),
                    }
                },
            );
        });
    });
    a.on_client_delete(|name| {
        on_ui_now(|s, ui| {
            let d = s.d.clone();
            let w = s.ui.clone();
            let n = name.to_string();
            confirm(
                s,
                ui,
                tf("Удалить клиента {c}?", &[("c", &n)]),
                t("Запросы с его ключом начнут получать 401."),
                t("Удалить"),
                move || {
                    let r = admin::delete_client(&d.app, &n);
                    on_ui_now(|s, ui| {
                        if let Err(e) = r {
                            toast(s, ui, format!("{e:#}"), "bad");
                        }
                        refresh_clients(s, ui);
                    });
                    drop(w);
                },
            );
        });
    });

    // Журнал.
    a.on_logs_reload(|| on_ui_now(reload_logs));

    // Настройки сервера: черновик.
    a.on_field_text(|i, text| {
        on_ui_now(|s, ui| {
            let g = ui.global::<UiData>();
            match fields::edit_text(&mut s.draft, i as usize, &text) {
                Ok(()) => {
                    g.set_settings_dirty(true);
                    g.set_settings_error("".into());
                }
                Err(e) => g.set_settings_error(format!("{}: {e}", t(fields::DEFS[i as usize].label)).into()),
            }
        });
    });
    a.on_field_bool(|i, on| {
        on_ui_now(|s, ui| {
            fields::edit_bool(&mut s.draft, i as usize, on);
            ui.global::<UiData>().set_settings_dirty(true);
        });
    });
    a.on_field_select(|i, idx| {
        on_ui_now(|s, ui| {
            fields::edit_select(&mut s.draft, i as usize, idx as usize);
            ui.global::<UiData>().set_settings_dirty(true);
        });
    });
    a.on_settings_save(|| {
        on_ui_now(|s, ui| {
            let g = ui.global::<UiData>();
            let parsed: Result<lmq_core::config::Settings, _> = serde_json::from_value(s.draft.clone());
            let r = match parsed {
                Err(e) => Err(e.to_string()),
                Ok(new) => {
                    let errors = admin::validate_settings(&s.d.app, &new);
                    if errors.is_empty() { admin::update_settings(&s.d.app, new).map_err(|e| format!("{e:#}")) } else { Err(errors.join("\n")) }
                }
            };
            match r {
                Ok(()) => {
                    load_settings(s, ui);
                    toast(s, ui, t("Настройки сохранены"), "ok");
                }
                Err(e) => g.set_settings_error(e.into()),
            }
        });
    });
    a.on_settings_revert(|| on_ui_now(load_settings));

    // Оформление и приложение.
    a.on_set_scheme(|i| {
        on_ui_now(|s, ui| {
            let v = ["system", "light", "dark"][i.clamp(0, 2) as usize];
            if let Err(e) = patch(&s.d, json!({ "ui": { "scheme": v } })) {
                toast(s, ui, format!("{e:#}"), "bad");
            }
            apply_look(s, ui);
        });
    });
    a.on_set_backdrop(|i| {
        on_ui_now(|s, ui| {
            let v = ["mica", "acrylic", "none"][i.clamp(0, 2) as usize];
            if let Err(e) = patch(&s.d, json!({ "ui": { "backdrop": v } })) {
                toast(s, ui, format!("{e:#}"), "bad");
            }
            apply_look(s, ui);
        });
    });
    a.on_set_language(|i| {
        on_ui_now(|s, ui| {
            let v = ["auto", "ru", "en"][i.clamp(0, 2) as usize];
            if let Err(e) = patch(&s.d, json!({ "ui": { "language": v } })) {
                toast(s, ui, format!("{e:#}"), "bad");
            }
            apply_language(s, ui);
            load_settings(s, ui);
            let engines = s.d.app.engines.list();
            paint_overview(s, ui, &engines);
            if ui.global::<UiData>().get_page() == PAGE_QUEUE {
                reload_jobs(&s.d, ui);
            }
        });
    });
    a.on_switch_to_web(|| {
        on_ui_now(|s, ui| {
            let d = s.d.clone();
            let w = s.ui.clone();
            confirm(
                s,
                ui,
                t("Перейти в веб-режим?"),
                t("Окно откроется заново с веб-панелью и 26 темами. Сервер перезапустится, задания в очереди сохранятся."),
                t("Перейти"),
                move || {
                    if let Some(ui) = w.upgrade() {
                        save_window_state(&d, &ui);
                    }
                    let _ = patch(&d, json!({ "ui": { "mode": "web" } }));
                    d.restart(Some(UiMode::Web));
                },
            );
        });
    });
    a.on_set_app_flag(|key, on| {
        on_ui_now(|s, ui| {
            let p = match key.as_str() {
                "updates_auto" => json!({ "updates": { "auto_check": on } }),
                k => json!({ "app": { k: on } }),
            };
            match patch(&s.d, p) {
                Ok(()) => {
                    if key == "autostart" {
                        s.d.apply_autostart();
                    }
                }
                Err(e) => toast(s, ui, format!("{e:#}"), "bad"),
            }
        });
    });
    a.on_open_folder(|which| on_ui_now(|s, _| s.d.open_path(&which)));
    a.on_netcheck(|| {
        on_ui_now(|s, ui| {
            let g = ui.global::<UiData>();
            g.set_netcheck_running(true);
            g.set_netcheck(t("Проверяю маршруты…").into());
            let app = s.d.app.clone();
            s.d.rt.spawn(async move {
                let items = admin::network_check(&app, None).await;
                let text = if items.is_empty() {
                    t("Проверять нечего: нет ни адреса колбэка, ни апстримов.")
                } else {
                    items
                        .iter()
                        .map(|v| {
                            let p = &v["probe"];
                            let mark = if p["http_status"].is_null() { "✗" } else { "✓" };
                            format!(
                                "{mark} {} — {}\n    {} · {}",
                                v["name"].as_str().unwrap_or(""),
                                p["verdict"].as_str().unwrap_or(""),
                                p["url"].as_str().unwrap_or(""),
                                p["route"].as_str().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                on_ui(move |_, ui| {
                    let g = ui.global::<UiData>();
                    g.set_netcheck(text.into());
                    g.set_netcheck_running(false);
                });
            });
        });
    });
    a.on_check_updates(|| {
        on_ui_now(|s, ui| {
            ui.global::<UiData>().set_update_status(t("Проверяю…").into());
            let app = s.d.app.clone();
            s.d.rt.spawn(async move {
                let info = lmq_core::updates::check(&app).await;
                let text = if let Some(e) = info.error {
                    e
                } else if info.newer {
                    tf("Доступна версия {v}", &[("v", &info.latest.clone().unwrap_or_default())])
                } else {
                    t("Установлена последняя версия")
                };
                let url = info.url.clone().filter(|_| info.newer);
                on_ui(move |s, ui| {
                    ui.global::<UiData>().set_update_status(text.into());
                    if let Some(u) = url {
                        s.d.open_url(&u);
                    }
                });
            });
        });
    });

    // Модальные окна.
    a.on_confirm_accept(|| {
        let action = with(|s| s.confirm.take()).flatten();
        on_ui_now(|_, ui| ui.global::<UiData>().set_confirm_title("".into()));
        if let Some(f) = action {
            f();
        }
    });
    a.on_confirm_dismiss(|| {
        on_ui_now(|s, ui| {
            s.confirm = None;
            ui.global::<UiData>().set_confirm_title("".into());
        });
    });
    a.on_secret_dismiss(|| {
        on_ui_now(|_, ui| {
            let g = ui.global::<UiData>();
            g.set_secret_title("".into());
            g.set_secret_value("".into());
        });
    });
    {
        let d = d.clone();
        a.on_quit(move || {
            on_ui_now(|s, ui| {
                let d2 = d.clone();
                confirm(
                    s,
                    ui,
                    t("Выйти из LMQ?"),
                    t("Сервер очереди и движки остановятся; бот будет получать ошибки соединения."),
                    t("Выход"),
                    move || {
                        on_ui_now(|_, ui| quit(&d2, ui));
                    },
                );
            });
        });
    }
}
