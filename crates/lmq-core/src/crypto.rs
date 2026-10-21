//! Шифрование секретов на диске.
//!
//! - `enc:` — ChaCha20-Poly1305 мастер-ключом (`SECRET_KEY` или ключ из `secrets.bin`):
//!   токены колбэков в БД, ключи апстримов и прочие секреты в `settings.yaml`.
//! - `pw:` — то же, но ключ выводится из пароля Argon2id: экспорт настроек с секретами.

use anyhow::{Context, Result, anyhow, bail};
use argon2::Argon2;
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

use crate::util::random_bytes;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD_NO_PAD;
pub const ENC_PREFIX: &str = "enc:";
pub const PW_PREFIX: &str = "pw:";

#[derive(Clone)]
pub struct Cipher {
    inner: ChaCha20Poly1305,
}

impl std::fmt::Debug for Cipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cipher(…)")
    }
}

impl Cipher {
    pub fn new(key: &[u8; 32]) -> Self {
        Self { inner: ChaCha20Poly1305::new(&Key::from(*key)) }
    }

    fn seal(&self, plain: &[u8]) -> Vec<u8> {
        let nonce_bytes = random_bytes::<12>();
        let nonce = Nonce::from(nonce_bytes);
        let ct = self.inner.encrypt(&nonce, plain).expect("chacha20poly1305 encrypt");
        let mut out = Vec::with_capacity(12 + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        out
    }

    fn open(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < 12 + 16 {
            bail!("ciphertext too short");
        }
        let (nonce, ct) = data.split_at(12);
        self.inner.decrypt(&Nonce::from(<[u8; 12]>::try_from(nonce).expect("12-byte nonce")), ct).map_err(|_| anyhow!("decrypt failed (wrong SECRET_KEY?)"))
    }

    /// Шифрует строку в вид `enc:<base64>`. Пустая строка остаётся пустой.
    pub fn encrypt_str(&self, plain: &str) -> String {
        if plain.is_empty() || plain.starts_with(ENC_PREFIX) {
            return plain.to_string();
        }
        format!("{ENC_PREFIX}{}", B64.encode(self.seal(plain.as_bytes())))
    }

    /// Расшифровывает `enc:…`; строку без префикса возвращает как есть (значение,
    /// вписанное в файл руками, — оно будет зашифровано при следующем сохранении).
    pub fn decrypt_str(&self, value: &str) -> Result<String> {
        match value.strip_prefix(ENC_PREFIX) {
            None => Ok(value.to_string()),
            Some(b64) => {
                let raw = B64.decode(b64.trim()).context("bad base64 in enc: value")?;
                String::from_utf8(self.open(&raw)?).context("secret is not utf-8")
            }
        }
    }
}

fn password_key(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let mut key = [0u8; 32];
    Argon2::default().hash_password_into(password.as_bytes(), salt, &mut key).map_err(|e| anyhow!("argon2: {e}"))?;
    Ok(key)
}

/// Шифрование паролем: `pw:<base64(salt16 | nonce12 | ct)>`.
pub fn password_encrypt(password: &str, plain: &str) -> Result<String> {
    let salt = random_bytes::<16>();
    let cipher = Cipher::new(&password_key(password, &salt)?);
    let mut out = salt.to_vec();
    out.extend(cipher.seal(plain.as_bytes()));
    Ok(format!("{PW_PREFIX}{}", B64.encode(out)))
}

pub fn password_decrypt(password: &str, value: &str) -> Result<String> {
    let b64 = value.strip_prefix(PW_PREFIX).ok_or_else(|| anyhow!("not a pw: value"))?;
    let raw = B64.decode(b64.trim()).context("bad base64")?;
    if raw.len() < 16 {
        bail!("ciphertext too short");
    }
    let (salt, rest) = raw.split_at(16);
    let cipher = Cipher::new(&password_key(password, salt)?);
    let plain = cipher.open(rest).map_err(|_| anyhow!("wrong password"))?;
    String::from_utf8(plain).context("secret is not utf-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let c = Cipher::new(&[7u8; 32]);
        let e = c.encrypt_str("секрет");
        assert!(e.starts_with("enc:"));
        assert_eq!(c.decrypt_str(&e).unwrap(), "секрет");
        assert_eq!(c.decrypt_str("plain").unwrap(), "plain");
        assert_eq!(c.encrypt_str(""), "");
        let other = Cipher::new(&[8u8; 32]);
        assert!(other.decrypt_str(&e).is_err());
    }

    #[test]
    fn password_roundtrip() {
        let e = password_encrypt("pass", "value").unwrap();
        assert_eq!(password_decrypt("pass", &e).unwrap(), "value");
        assert!(password_decrypt("nope", &e).is_err());
    }
}
