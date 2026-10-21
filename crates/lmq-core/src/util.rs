//! Мелкие общие помощники: время, случайные токены, хэши, сравнение за постоянное время.

use base64::Engine as _;
use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Текущее время в миллисекундах Unix.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Случайные байты из ОС.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

/// Случайный токен в base64url без выравнивания.
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Случайный идентификатор в hex (для заданий без job_id и синхронных запросов).
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(sha256(s.as_bytes()))
}

/// Хэш токена (ключа клиента или администратора) в виде `sha256:<hex>`.
pub fn hash_admin(token: &str) -> String {
    format!("sha256:{}", sha256_hex(token))
}

/// Сравнение за постоянное время (для ключей и токенов).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Маскирует секрет для показа: первые 4 символа и длина.
pub fn mask_secret(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let head: String = s.chars().take(4).collect();
    format!("{head}… ({} симв.)", s.chars().count())
}

/// Проверка job_id: ≤ 64 символа `[A-Za-z0-9_-]`.
pub fn valid_job_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Убирает `<think>…</think>` (и незакрытый `<think>` до конца). Нужен только для
/// проверки «пустого ответа»: боту текст уходит сырым.
pub fn strip_think(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        match rest.find("<think>") {
            None => {
                out.push_str(rest);
                break;
            }
            Some(start) => {
                out.push_str(&rest[..start]);
                let after = &rest[start + "<think>".len()..];
                match after.find("</think>") {
                    Some(end) => rest = &after[end + "</think>".len()..],
                    None => break,
                }
            }
        }
    }
    out
}

/// Идентификатор, безопасный для имени файла.
pub fn safe_file_name(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_is_stripped() {
        assert_eq!(strip_think("<think>a</think>OK").trim(), "OK");
        assert_eq!(strip_think("x<think>a").trim(), "x");
        assert_eq!(strip_think("<think>a</think>").trim(), "");
        assert_eq!(strip_think("plain"), "plain");
    }

    #[test]
    fn job_id_rules() {
        assert!(valid_job_id("3f9c1a2b4d5e6f708192a3b4"));
        assert!(valid_job_id("a_b-C"));
        assert!(!valid_job_id(""));
        assert!(!valid_job_id("a b"));
        assert!(!valid_job_id(&"a".repeat(65)));
    }
}
