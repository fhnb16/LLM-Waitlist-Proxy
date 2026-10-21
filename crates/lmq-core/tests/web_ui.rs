//! Проверки веб-панели без браузера: словарь en.json покрывает все строки t('…'),
//! импорты модулей указывают на существующие файлы.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn web() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web")
}

fn js_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![web().join("js")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "js") {
                out.push(p);
            }
        }
    }
    out
}

/// Строки из вызовов t('…') — с учётом экранирования.
fn t_strings(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;
    while let Some(pos) = src[i..].find("t('") {
        let start = i + pos;
        let prev = if start == 0 { b' ' } else { b[start - 1] };
        i = start + 3;
        if prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.' {
            continue;
        }
        let mut s = String::new();
        let mut chars = src[i..].chars();
        let mut consumed = 0;
        while let Some(c) = chars.next() {
            consumed += c.len_utf8();
            match c {
                '\\' => {
                    if let Some(n) = chars.next() {
                        consumed += n.len_utf8();
                        s.push(n);
                    }
                }
                '\'' => break,
                _ => s.push(c),
            }
        }
        i += consumed;
        out.push(s);
    }
    out
}

#[test]
fn english_dictionary_covers_ui_strings() {
    let dict: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(web().join("i18n/en.json")).unwrap()).expect("en.json is valid JSON");
    let mut missing = BTreeSet::new();
    for f in js_files() {
        let src = std::fs::read_to_string(&f).unwrap();
        for s in t_strings(&src) {
            let cyrillic = s.chars().any(|c| ('а'..='я').contains(&c.to_lowercase().next().unwrap_or(c)) || c == 'ё');
            if cyrillic && !dict.contains_key(&s) {
                missing.insert(format!("{}: {s}", f.file_name().unwrap().to_string_lossy()));
            }
        }
    }
    assert!(missing.is_empty(), "нет перевода в web/i18n/en.json:\n{}", missing.into_iter().collect::<Vec<_>>().join("\n"));
}

#[test]
fn module_imports_resolve() {
    let mut broken = Vec::new();
    for f in js_files() {
        let src = std::fs::read_to_string(&f).unwrap();
        for part in src.split("from '").skip(1).chain(src.split("import('").skip(1)) {
            let Some(path) = part.split('\'').next() else { continue };
            if !path.starts_with('.') {
                continue;
            }
            let target = f.parent().unwrap().join(path);
            if !target.exists() {
                broken.push(format!("{} → {path}", f.display()));
            }
        }
    }
    assert!(broken.is_empty(), "битые импорты:\n{}", broken.join("\n"));
}

#[test]
fn index_html_references_exist() {
    let html = std::fs::read_to_string(web().join("index.html")).unwrap();
    for attr in ["href=\"", "src=\""] {
        for part in html.split(attr).skip(1) {
            let p = part.split('"').next().unwrap();
            if p.starts_with("http") || p.starts_with('#') {
                continue;
            }
            assert!(web().join(p).exists(), "index.html ссылается на отсутствующий {p}");
        }
    }
}
