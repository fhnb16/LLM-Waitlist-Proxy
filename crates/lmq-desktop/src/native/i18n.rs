//! Перевод строк, которые собирает Rust. Словарь общий с веб-панелью
//! (`web/i18n/en.json`: русская строка → английская); строки из .slint переводятся
//! вшитым каталогом `translations/en`.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static EN: AtomicBool = AtomicBool::new(false);

fn dict() -> &'static HashMap<String, String> {
    static D: OnceLock<HashMap<String, String>> = OnceLock::new();
    D.get_or_init(|| serde_json::from_str(include_str!("../../../../web/i18n/en.json")).unwrap_or_default())
}

/// `auto` → язык системы (`HKCU\Control Panel\International\LocaleName`).
pub fn resolve(pref: &str) -> &'static str {
    match pref {
        "ru" => "ru",
        "en" => "en",
        _ => {
            if system_locale().to_lowercase().starts_with("ru") {
                "ru"
            } else {
                "en"
            }
        }
    }
}

#[cfg(windows)]
fn system_locale() -> String {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey("Control Panel\\International")
        .and_then(|k| k.get_value::<String, _>("LocaleName"))
        .unwrap_or_else(|_| "ru-RU".into())
}

#[cfg(not(windows))]
fn system_locale() -> String {
    std::env::var("LANG").unwrap_or_else(|_| "ru".into())
}

/// Включает язык для строк Rust и для вшитых переводов Slint.
pub fn set(lang: &str) {
    EN.store(lang == "en", Ordering::Relaxed);
    let _ = slint::select_bundled_translation(if lang == "en" { "en" } else { "" });
}

pub fn is_en() -> bool {
    EN.load(Ordering::Relaxed)
}

pub fn t(s: &str) -> String {
    if is_en() { dict().get(s).cloned().unwrap_or_else(|| s.to_string()) } else { s.to_string() }
}

/// Перевод с подстановкой `{name}`.
pub fn tf(s: &str, vars: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut r = t(s);
    for (k, v) in vars {
        r = r.replace(&format!("{{{k}}}"), &v.to_string());
    }
    r
}

/// Число со словом: `tn(5, "задание", "задания", "заданий")`.
pub fn tn(n: i64, one: &str, few: &str, many: &str) -> String {
    if is_en() {
        return format!("{n} {}", t(if n.abs() == 1 { one } else { many }));
    }
    let a = n.abs() % 100;
    let b = a % 10;
    let w = if a > 10 && a < 20 {
        many
    } else if (2..5).contains(&b) {
        few
    } else if b == 1 {
        one
    } else {
        many
    };
    format!("{n} {w}")
}
