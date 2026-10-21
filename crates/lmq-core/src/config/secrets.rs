//! `secrets.bin`: мастер-ключ шифрования и хэш токена администратора.
//!
//! На Windows содержимое защищено DPAPI (ключ пользователя, расшифровать может только
//! он же). На Unix — JSON с правами 0600. `SECRET_KEY` из окружения заменяет мастер-ключ.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::util::{random_bytes, sha256};

#[derive(Clone, Serialize, Deserialize, Default)]
struct SecretsFile {
    master_key: String,
    admin_token_hash: Option<String>,
}

#[derive(Clone)]
pub struct Secrets {
    pub master_key: [u8; 32],
    /// `sha256:<hex>` от токена администратора.
    pub admin_token_hash: Option<String>,
    /// Ключ задан через `SECRET_KEY`.
    pub key_from_env: bool,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets").field("key_from_env", &self.key_from_env).finish_non_exhaustive()
    }
}

impl Secrets {
    pub fn load_or_create(path: &Path) -> Result<Self> {
        let file = if path.exists() {
            let raw = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
            let plain = unprotect(&raw).context("cannot decrypt secrets.bin (другой пользователь Windows?)")?;
            serde_json::from_slice::<SecretsFile>(&plain).context("secrets.bin is corrupted")?
        } else {
            let f = SecretsFile { master_key: hex::encode(random_bytes::<32>()), admin_token_hash: None };
            write_file(path, &f)?;
            f
        };
        let mut master_key = [0u8; 32];
        hex::decode_to_slice(&file.master_key, &mut master_key).context("bad master key in secrets.bin")?;
        let mut key_from_env = false;
        if let Ok(env_key) = std::env::var("SECRET_KEY")
            && !env_key.is_empty()
        {
            master_key = sha256(env_key.as_bytes());
            key_from_env = true;
        }
        Ok(Self { master_key, admin_token_hash: file.admin_token_hash, key_from_env })
    }

    /// Сохраняет хэш токена администратора (мастер-ключ из файла не меняется).
    pub fn save_admin_hash(path: &Path, hash: Option<String>) -> Result<()> {
        let raw = std::fs::read(path)?;
        let mut f: SecretsFile = serde_json::from_slice(&unprotect(&raw)?)?;
        f.admin_token_hash = hash;
        write_file(path, &f)
    }
}

fn write_file(path: &Path, f: &SecretsFile) -> Result<()> {
    let json = serde_json::to_vec(f)?;
    let data = protect(&json)?;
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, data).with_context(|| format!("write {}", path.display()))?;
    super::paths::restrict_permissions(path)?;
    Ok(())
}

#[cfg(windows)]
fn protect(data: &[u8]) -> Result<Vec<u8>> {
    dpapi::protect(data)
}

#[cfg(windows)]
fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    // Файл, созданный не на Windows (или вручную), — обычный JSON.
    if data.first() == Some(&b'{') {
        return Ok(data.to_vec());
    }
    dpapi::unprotect(data)
}

#[cfg(not(windows))]
fn protect(data: &[u8]) -> Result<Vec<u8>> {
    Ok(data.to_vec())
}

#[cfg(not(windows))]
fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    Ok(data.to_vec())
}

#[cfg(windows)]
mod dpapi {
    use anyhow::{Result, bail};
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData};

    fn take(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: блоб выделен DPAPI и действителен до LocalFree.
        unsafe {
            let out = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
            LocalFree(blob.pbData as HLOCAL);
            out
        }
    }

    pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: указатели действительны на время вызова.
        let ok =
            unsafe { CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output) };
        if ok == 0 {
            bail!("CryptProtectData failed: {}", std::io::Error::last_os_error());
        }
        Ok(take(output))
    }

    pub fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        // SAFETY: указатели действительны на время вызова.
        let ok = unsafe {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        };
        if ok == 0 {
            bail!("CryptUnprotectData failed: {}", std::io::Error::last_os_error());
        }
        Ok(take(output))
    }
}
