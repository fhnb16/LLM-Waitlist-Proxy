//! Веб-панель, встроенная в бинарник (папка `web/` репозитория), по адресу `/ui/`.

use axum::extract::Path;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web"]
#[exclude = "*.map"]
#[exclude = ".*"]
pub struct WebAssets;

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
                   font-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

fn serve(path: &str) -> Response {
    let path = if path.is_empty() || path.ends_with('/') { format!("{path}index.html") } else { path.to_string() };
    match WebAssets::get(&path) {
        Some(file) => {
            let mime = file.metadata.mimetype();
            let mut resp = (StatusCode::OK, file.data.into_owned()).into_response();
            let h = resp.headers_mut();
            if let Ok(v) = HeaderValue::from_str(mime) {
                h.insert(header::CONTENT_TYPE, v);
            }
            let cache = if path.ends_with(".html") { "no-cache" } else { "public, max-age=300" };
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
            h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
            h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
            if path.ends_with(".html") {
                h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
                h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
            }
            resp
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

pub async fn index() -> Response {
    serve("")
}

pub async fn file(Path(path): Path<String>) -> Response {
    serve(&path)
}

pub async fn root() -> Redirect {
    Redirect::temporary("/ui/")
}

/// Текст файла из встроенных ассетов (для тестов тем и документации).
pub fn text(path: &str) -> Option<String> {
    WebAssets::get(path).map(|f| String::from_utf8_lossy(&f.data).into_owned())
}

pub fn list(prefix: &str) -> Vec<String> {
    WebAssets::iter().filter(|p| p.starts_with(prefix)).map(|p| p.to_string()).collect()
}
