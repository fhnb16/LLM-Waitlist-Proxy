//! Подпись Authenticode у установщика обновления (Windows): цела ли она и доверена ли цепочка.
//! Действительная — ставится, нет подписи — с согласия, повреждённая — никогда. На других
//! системах подписи нет — `Unsigned`.

use std::path::Path;

use serde::Serialize;

/// Итог проверки подписи файла.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Signature {
    /// Подпись цела, цепочка доходит до доверенного корня.
    Valid {
        publisher: String,
    },
    /// Подпись цела, но цепочке Windows не доверяет: самоподписанный или истёкший без метки времени.
    Untrusted {
        publisher: String,
        code: u32,
    },
    Unsigned,
    /// Файл изменён после подписи, сертификат отозван или запрещён.
    Broken {
        code: u32,
    },
}

impl Signature {
    pub fn describe(&self) -> String {
        match self {
            Signature::Valid { publisher } => format!("подписан: {publisher}"),
            Signature::Untrusted { publisher, code } => format!("подписан недоверенным сертификатом: {publisher} (0x{code:08X})"),
            Signature::Unsigned => "без подписи".into(),
            Signature::Broken { code } => format!("подпись повреждена (0x{code:08X})"),
        }
    }
}

const TRUST_E_PROVIDER_UNKNOWN: u32 = 0x800B_0001;
const TRUST_E_SUBJECT_FORM_UNKNOWN: u32 = 0x800B_0003;
const TRUST_E_NOSIGNATURE: u32 = 0x800B_0100;
const CERT_E_EXPIRED: u32 = 0x800B_0101;
const CERT_E_UNTRUSTEDROOT: u32 = 0x800B_0109;
const CERT_E_CHAINING: u32 = 0x800B_010A;
const CERT_E_UNTRUSTEDTESTROOT: u32 = 0x800B_010D;
const CRYPT_E_SECURITY_SETTINGS: u32 = 0x8009_2026;

/// Код `WinVerifyTrust` → итог. Неизвестные ошибки считаются повреждением: лучше не ставить.
pub fn classify(code: u32, publisher: Option<String>) -> Signature {
    let publisher = publisher.unwrap_or_default();
    match code {
        0 => Signature::Valid { publisher },
        TRUST_E_NOSIGNATURE | TRUST_E_SUBJECT_FORM_UNKNOWN | TRUST_E_PROVIDER_UNKNOWN => Signature::Unsigned,
        CERT_E_UNTRUSTEDROOT | CERT_E_CHAINING | CERT_E_EXPIRED | CERT_E_UNTRUSTEDTESTROOT | CRYPT_E_SECURITY_SETTINGS => {
            Signature::Untrusted { publisher, code }
        }
        _ => Signature::Broken { code },
    }
}

/// Что делать с установщиком обновления.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Действительная подпись (издатель может смениться вместе с сертификатом) — ставить.
    Allow,
    /// Подписи нет или Windows ей не доверяет — ставить только с согласия человека: сертификат
    /// может пропасть или закончиться, обновления не должны из-за этого застревать.
    Confirm,
    /// Подпись повреждена: файл изменён после подписи, сертификат отозван — не ставить никогда.
    Deny,
}

pub fn verdict(new: &Signature) -> Verdict {
    match new {
        Signature::Valid { .. } => Verdict::Allow,
        Signature::Unsigned | Signature::Untrusted { .. } => Verdict::Confirm,
        Signature::Broken { .. } => Verdict::Deny,
    }
}

/// Можно ли запускать установщик: `confirmed` — человек согласился поставить его без подписи.
pub fn allow_run(sig: &Signature, confirmed: bool) -> Result<(), String> {
    match verdict(sig) {
        Verdict::Allow => Ok(()),
        Verdict::Confirm if confirmed => Ok(()),
        Verdict::Confirm => Err(format!("установщик {} — нужно согласие на установку", sig.describe())),
        Verdict::Deny => Err(format!("установщик не прошёл проверку Authenticode: {}", sig.describe())),
    }
}

/// Подпись файла.
#[cfg(windows)]
pub fn check(path: &Path) -> Signature {
    win::check(path)
}

#[cfg(not(windows))]
pub fn check(_path: &Path) -> Signature {
    Signature::Unsigned
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Security::Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
    };

    use super::{Signature, classify};

    pub fn check(path: &Path) -> Signature {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut file = WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: wide.as_ptr(),
            hFile: null_mut(),
            pgKnownSubject: null_mut(),
        };
        // SAFETY: структура из простых полей и указателей, нули — допустимые значения.
        let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
        data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        // Отзыв не проверяется: без сети проверка иначе падает, а файл уже сверен по minisign.
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous.pFile = &mut file;
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        // SAFETY: `data` и `file` живут до закрывающего вызова, путь оканчивается нулём.
        let code = unsafe { WinVerifyTrust(null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast::<c_void>()) } as u32;
        let publisher = signer(data.hWVTStateData);
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        // SAFETY: закрывает состояние, открытое первым вызовом.
        unsafe { WinVerifyTrust(null_mut(), &mut action, (&mut data as *mut WINTRUST_DATA).cast::<c_void>()) };
        classify(code, publisher)
    }

    /// Имя издателя (CN сертификата подписи) из состояния проверки.
    fn signer(state: *mut c_void) -> Option<String> {
        if state.is_null() {
            return None;
        }
        // SAFETY: указатели берутся из состояния WinVerifyTrust и проверяются на null.
        unsafe {
            let prov = WTHelperProvDataFromStateData(state);
            if prov.is_null() {
                return None;
            }
            let sgnr = WTHelperGetProvSignerFromChain(prov, 0, 0, 0);
            if sgnr.is_null() || (*sgnr).csCertChain == 0 || (*sgnr).pasCertChain.is_null() {
                return None;
            }
            let cert = (*(*sgnr).pasCertChain).pCert;
            if cert.is_null() {
                return None;
            }
            let mut buf = [0u16; 256];
            let n = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, null(), buf.as_mut_ptr(), buf.len() as u32);
            (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(p: &str) -> Signature {
        Signature::Valid { publisher: p.into() }
    }

    #[test]
    fn codes_are_classified() {
        assert_eq!(classify(0, Some("LMQ".into())), valid("LMQ"));
        assert_eq!(classify(TRUST_E_NOSIGNATURE, None), Signature::Unsigned);
        assert_eq!(classify(CERT_E_UNTRUSTEDROOT, Some("Test".into())), Signature::Untrusted { publisher: "Test".into(), code: CERT_E_UNTRUSTEDROOT });
        // TRUST_E_BAD_DIGEST: файл изменён после подписи.
        assert_eq!(classify(0x8009_6010, Some("LMQ".into())), Signature::Broken { code: 0x8009_6010 });
    }

    #[test]
    fn update_policy() {
        let broken = Signature::Broken { code: 0x8009_6010 };
        let untrusted = Signature::Untrusted { publisher: "LMQ".into(), code: CERT_E_UNTRUSTEDROOT };
        // Действительная подпись любого издателя — сразу: сертификат могут сменить.
        assert_eq!(verdict(&valid("LMQ")), Verdict::Allow);
        assert_eq!(verdict(&valid("New Owner")), Verdict::Allow);
        assert!(allow_run(&valid("New Owner"), false).is_ok());
        // Без подписи или с недоверенной — только с согласия.
        for s in [Signature::Unsigned, untrusted] {
            assert_eq!(verdict(&s), Verdict::Confirm);
            assert!(allow_run(&s, false).is_err());
            assert!(allow_run(&s, true).is_ok());
        }
        // Повреждённая — никогда, даже с согласием.
        assert_eq!(verdict(&broken), Verdict::Deny);
        assert!(allow_run(&broken, true).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn real_files_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.exe");
        std::fs::write(&plain, b"MZ not really a program").unwrap();
        assert_eq!(check(&plain), Signature::Unsigned);
        // Подписанный Microsoft файл, если он есть на машине.
        let edge = std::path::PathBuf::from(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe");
        if edge.exists() {
            match check(&edge) {
                Signature::Valid { publisher } => assert!(publisher.contains("Microsoft"), "{publisher}"),
                other => panic!("msedge.exe: {other:?}"),
            }
        }
    }
}
