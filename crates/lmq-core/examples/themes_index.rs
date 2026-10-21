//! Пересборка `web/css/themes/index.json` из паспортов тем:
//! `cargo run -p lmq-core --example themes_index`.

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/css/themes");
    let mut items = Vec::new();
    let mut errors = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir).expect("web/css/themes").flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".css") {
            continue;
        }
        let css = std::fs::read_to_string(e.path()).unwrap();
        let rel = format!("css/themes/{name}");
        errors.extend(lmq_core::themes::check(&css, &rel));
        if let Some(p) = lmq_core::themes::passport(&css, &rel) {
            items.push(p);
        }
    }
    items.sort_by(|a, b| a.id.cmp(&b.id));
    let json = serde_json::to_string_pretty(&items).unwrap();
    std::fs::write(dir.join("index.json"), format!("{json}\n")).unwrap();
    println!("index.json: {} тем", items.len());
    if !errors.is_empty() {
        eprintln!("{}", errors.join("\n"));
        std::process::exit(1);
    }
}
