//! Темы веб-панели: паспорт `/*!theme … */` и проверка обязательных токенов
//! (порт `tools/build-index.py` из каталога тем).

use serde::Serialize;
use serde_json::{Map, Value, json};

/// 46 обязательных токенов основного блока темы.
pub const REQUIRED_TOKENS: &[&str] = &[
    "--font",
    "--radius",
    "--radius-lg",
    "--card-radius",
    "--bar-blur",
    "--bar-h",
    "--tab-h",
    "--hero-size",
    "--hero-weight",
    "--hero-tracking",
    "--nav-grid",
    "--nav-align",
    "--nav-weight",
    "--sw-w",
    "--sw-h",
    "--sw-r",
    "--sw-thumb",
    "--sw-thumb-r",
    "--sw-pad",
    "--sw-off",
    "--sw-on",
    "--sw-shadow",
    "--seg-r",
    "--seg-line",
    "--seg-text",
    "--seg-on",
    "--seg-on-text",
    "--chip-r",
    "--bg",
    "--bg-elev",
    "--surface",
    "--surface-press",
    "--sep",
    "--sep-strong",
    "--text",
    "--text-2",
    "--text-3",
    "--bar",
    "--bar-line",
    "--danger",
    "--on-danger",
    "--ok",
    "--thumb",
    "--toast-bg",
    "--toast-text",
    "--shadow-soft",
];

#[derive(Clone, Debug, Serialize)]
pub struct Passport {
    pub id: String,
    pub name: String,
    pub mode: String,
    pub accent: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
    pub file: String,
    /// Токены для миниатюры: `{light: {...}, dark: {...}}`.
    pub preview: Value,
}

const PREVIEW_TOKENS: &[&str] = &["--bg", "--bar", "--bar-line", "--surface", "--sep", "--text", "--text-3", "--radius", "--card-radius", "--font"];

fn declarations(block: &str) -> Map<String, Value> {
    let mut out = Map::new();
    for decl in block.split(';') {
        // Комментарии внутри блока отбрасываем.
        let mut d = decl.to_string();
        while let (Some(a), Some(b)) = (d.find("/*"), d.find("*/")) {
            if b < a {
                break;
            }
            d.replace_range(a..b + 2, "");
        }
        if let Some((k, v)) = d.split_once(':') {
            let k = k.trim();
            if PREVIEW_TOKENS.contains(&k) {
                let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
                out.insert(k.to_string(), json!(v));
            }
        }
    }
    out
}

fn block_after<'a>(css: &'a str, sel: &str) -> Option<&'a str> {
    let start = css.find(sel)? + sel.len();
    let end = start + css[start..].find('}')?;
    Some(&css[start..end])
}

fn preview(css: &str, id: &str, mode: &str) -> Value {
    let main = main_block(css, id).map(declarations).unwrap_or_default();
    let other_sel =
        if mode == "dark" { format!(":root[data-skin=\"{id}\"]:not(.theme--dark) {{") } else { format!(":root[data-skin=\"{id}\"].theme--dark {{") };
    let mut other = main.clone();
    if let Some(b) = block_after(css, &other_sel) {
        for (k, v) in declarations(b) {
            other.insert(k, v);
        }
    }
    if mode == "dark" { json!({ "dark": main, "light": other }) } else { json!({ "light": main, "dark": other }) }
}

fn bars(v: &str) -> Value {
    let parts: Vec<&str> = v.split(',').map(str::trim).collect();
    if parts.len() == 3 { json!([parts[0], parts[1].parse::<f64>().unwrap_or(1.0), parts[2]]) } else { json!(v) }
}

/// Разбор паспорта темы.
pub fn passport(css: &str, file: &str) -> Option<Passport> {
    let start = css.find("/*!theme")? + "/*!theme".len();
    let end = start + css[start..].find("*/")?;
    let mut fields = Map::new();
    for line in css[start..end].lines() {
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim(), v.trim());
            if k.is_empty() {
                continue;
            }
            let val = if k.starts_with("bars") { bars(v) } else { json!(v) };
            fields.insert(k.to_string(), val);
        }
    }
    let take = |m: &mut Map<String, Value>, k: &str| m.remove(k).and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let id = take(&mut fields, "id");
    if id.is_empty() {
        return None;
    }
    let mode = take(&mut fields, "mode");
    Some(Passport {
        name: take(&mut fields, "name"),
        accent: take(&mut fields, "accent"),
        preview: preview(css, &id, &mode),
        mode,
        id,
        extra: fields,
        file: file.to_string(),
    })
}

/// Основной блок токенов `:root[data-skin="<id>"] { … }`.
fn main_block<'a>(css: &'a str, id: &str) -> Option<&'a str> {
    let sel = format!(":root[data-skin=\"{id}\"] {{");
    let start = css.find(&sel)? + sel.len();
    let end = start + css[start..].find('}')?;
    Some(&css[start..end])
}

/// Проверка темы: пустой список — всё на месте.
pub fn check(css: &str, file: &str) -> Vec<String> {
    let mut errs = Vec::new();
    let Some(p) = passport(css, file) else {
        return vec![format!("{file}: нет паспорта /*!theme */")];
    };
    if !file.ends_with(&format!("{}.css", p.id)) {
        errs.push(format!("{file}: id '{}' не совпадает с именем файла", p.id));
    }
    if p.mode != "light" && p.mode != "dark" {
        errs.push(format!("{file}: mode должен быть light или dark"));
    }
    let Some(block) = main_block(css, &p.id) else {
        errs.push(format!("{file}: нет основного блока :root[data-skin=\"{}\"]", p.id));
        return errs;
    };
    for t in REQUIRED_TOKENS {
        let declared = block.contains(&format!("{t}:")) || block.contains(&format!("{t} :"));
        if !declared {
            errs.push(format!("{file}: нет токена {t}"));
        }
    }
    // color-scheme в каталоге обязателен, но у Fluent, Frost и Material 3 его нет:
    // значение по умолчанию (light / dark по .theme--dark) задаёт web/css/app.css.
    // Правила темы — только под :root[data-skin="<id>"] (или .bars--solid[data-skin]).
    for line in css.lines() {
        let l = line.trim_start();
        if l.starts_with(":root[data-skin=") && !l.starts_with(&format!(":root[data-skin=\"{}\"]", p.id)) {
            errs.push(format!("{file}: чужой селектор темы: {}", l.chars().take(60).collect::<String>()));
        }
    }
    errs
}

/// `index.json` из встроенных тем.
pub fn index() -> Vec<Passport> {
    let mut out: Vec<Passport> = crate::assets::list("css/themes/")
        .into_iter()
        .filter(|f| f.ends_with(".css"))
        .filter_map(|f| crate::assets::text(&f).and_then(|css| passport(&css, &f)))
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_themes_are_complete() {
        let files: Vec<String> = crate::assets::list("css/themes/").into_iter().filter(|f| f.ends_with(".css")).collect();
        assert_eq!(files.len(), 26, "ожидается 26 тем: {files:?}");
        let mut errs = Vec::new();
        for f in &files {
            errs.extend(check(&crate::assets::text(f).unwrap(), f));
        }
        assert!(errs.is_empty(), "проблемы тем:\n{}", errs.join("\n"));
    }

    #[test]
    fn index_json_is_in_sync() {
        let on_disk: Vec<Value> = serde_json::from_str(&crate::assets::text("css/themes/index.json").expect("web/css/themes/index.json")).unwrap();
        let built = serde_json::to_value(index()).unwrap();
        assert_eq!(serde_json::to_value(&on_disk).unwrap(), built, "пересоберите: cargo run -p lmq-core --example themes_index");
    }

    #[test]
    fn passport_parsing() {
        let css = "/*!theme\nid: x\nname: X\nmode: dark\naccent: #fff\nbarsLight: #fff, 0.9, #eee\n*/";
        let p = passport(css, "css/themes/x.css").unwrap();
        assert_eq!(p.id, "x");
        assert_eq!(p.extra["barsLight"], json!(["#fff", 0.9, "#eee"]));
    }
}
