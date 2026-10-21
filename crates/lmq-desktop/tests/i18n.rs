//! Переводы нативного окна полны: строки `@tr("…")` из ui/*.slint есть в
//! translations/en, а строки Rust (`t`, `tf`, `tn`, подписи полей) — в web/i18n/en.json.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Читает строковый литерал, начиная сразу после открывающей кавычки.
fn literal(s: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                if let Some((_, n)) = chars.next() {
                    out.push(match n {
                        'n' => '\n',
                        other => other,
                    });
                }
            }
            '"' => return Some((out, i + 1)),
            c => out.push(c),
        }
    }
    None
}

/// Литералы, стоящие сразу после `prefix` (например, `t("`), если перед ним не буква.
fn calls(src: &str, prefix: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut from = 0;
    while let Some(pos) = src[from..].find(prefix) {
        let at = from + pos;
        let prev = src[..at].chars().next_back();
        let start = at + prefix.len();
        if !prev.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
            && let Some((s, _)) = literal(&src[start..])
        {
            v.push(s);
        }
        from = start;
    }
    v
}

/// Все литералы внутри вызова `name(…)` (для `tn(n, "одно", "два", "пять")` и полей).
fn call_args(src: &str, name: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(pos) = src[from..].find(name) {
        let at = from + pos;
        let prev = src[..at].chars().next_back();
        from = at + name.len();
        if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let mut args = Vec::new();
        let mut i = from;
        let bytes = src.as_bytes();
        let mut depth = 1;
        while i < src.len() && depth > 0 {
            match bytes[i] {
                b'"' => {
                    if let Some((s, n)) = literal(&src[i + 1..]) {
                        args.push(s);
                        i += n + 1;
                        continue;
                    }
                }
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        out.push(args);
    }
    out
}

fn read_dir(dir: &Path, ext: &str) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == ext))
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect()
}

fn po_ids(path: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(path).unwrap();
    text.lines().filter_map(|l| l.strip_prefix("msgid \"")).filter_map(literal).map(|(s, _)| s).filter(|s| !s.is_empty()).collect()
}

#[test]
fn slint_strings_are_translated() {
    let root = Path::new(ROOT);
    let ids = po_ids(&root.join("translations/en/LC_MESSAGES/lmq-desktop.po"));
    let mut missing = BTreeSet::new();
    for src in read_dir(&root.join("ui"), "slint") {
        for s in calls(&src, "@tr(\"") {
            if !ids.contains(&s) {
                missing.insert(s);
            }
        }
    }
    assert!(missing.is_empty(), "нет перевода в translations/en: {missing:#?}");
}

#[test]
fn rust_strings_are_translated() {
    let root = Path::new(ROOT);
    let dict: HashMap<String, String> = serde_json::from_str(&std::fs::read_to_string(root.join("../../web/i18n/en.json")).unwrap()).unwrap();
    let mut needed = BTreeSet::new();
    for src in read_dir(&root.join("src/native"), "rs") {
        // Модуль i18n сам строк не содержит; пропускаем его определения функций.
        for s in calls(&src, "t(\"").into_iter().chain(calls(&src, "tf(\"")) {
            needed.insert(s);
        }
        for args in call_args(&src, "tn(") {
            needed.extend(args);
        }
        for name in ["f(Kind::", "head(", "note("] {
            for args in call_args(&src, name) {
                // У поля первый литерал — путь в settings.yaml, его не переводим.
                let skip = usize::from(name.starts_with("f("));
                needed.extend(args.into_iter().skip(skip).filter(|s| !s.is_empty()));
            }
        }
    }
    let missing: Vec<&String> = needed.iter().filter(|s| !dict.contains_key(*s)).collect();
    assert!(missing.is_empty(), "нет перевода в web/i18n/en.json: {missing:#?}");
}
