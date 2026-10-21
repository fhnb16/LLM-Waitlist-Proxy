//! Значок в трее (общий для обоих видов окна). Создаётся в главном потоке; события
//! меню и щелчков приходят в отдельный поток и передаются обработчику.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    TogglePause,
    CopyApi,
    OpenBrowser,
    Quit,
}

pub struct Tray {
    pub icon: TrayIcon,
    pause: MenuItem,
}

fn load_icon() -> Option<Icon> {
    let bytes = include_bytes!("../icons/32x32.png");
    let decoder = png::Decoder::new(std::io::Cursor::new(&bytes[..]));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        _ => return None,
    };
    Icon::from_rgba(rgba, info.width, info.height).ok()
}

impl Tray {
    pub fn new(on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> anyhow::Result<Self> {
        let show = MenuItem::with_id("show", "Открыть LMQ", true, None);
        let pause = MenuItem::with_id("pause", "Пауза выдачи", true, None);
        let copy = MenuItem::with_id("copy", "Копировать адрес API", true, None);
        let browser = MenuItem::with_id("browser", "Панель в браузере", true, None);
        let quit = MenuItem::with_id("quit", "Выход", true, None);
        let menu = Menu::new();
        menu.append_items(&[&show, &PredefinedMenuItem::separator(), &pause, &copy, &browser, &PredefinedMenuItem::separator(), &quit])?;
        let mut b = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("LMQ").with_menu_on_left_click(false);
        if let Some(i) = load_icon() {
            b = b.with_icon(i);
        }
        let icon = b.build()?;
        let handler = std::sync::Arc::new(on_action);
        let h1 = handler.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let a = match e.id.0.as_str() {
                "show" => TrayAction::Show,
                "pause" => TrayAction::TogglePause,
                "copy" => TrayAction::CopyApi,
                "browser" => TrayAction::OpenBrowser,
                "quit" => TrayAction::Quit,
                _ => return,
            };
            h1(a);
        }));
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
            | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = e
            {
                handler(TrayAction::Show);
            }
        }));
        Ok(Self { icon, pause })
    }

    /// Обновляет подсказку и подпись пункта паузы.
    pub fn refresh(&self, tooltip: &str, paused: bool) {
        let _ = self.icon.set_tooltip(Some(tooltip));
        self.pause.set_text(if paused { "Возобновить выдачу" } else { "Пауза выдачи" });
    }
}

/// Копирует текст в буфер обмена через `clip.exe` (без лишних зависимостей).
pub fn copy_to_clipboard(text: &str) {
    #[cfg(windows)]
    {
        use std::io::Write;
        use std::os::windows::process::CommandExt;
        if let Ok(mut child) = std::process::Command::new("clip.exe").stdin(std::process::Stdio::piped()).creation_flags(0x0800_0000).spawn() {
            if let Some(stdin) = child.stdin.as_mut() {
                // clip.exe ждёт UTF-16 LE для не-ASCII; адрес — ASCII.
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
    #[cfg(not(windows))]
    let _ = text;
}
