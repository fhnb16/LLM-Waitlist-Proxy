//! Обновления через GitHub Releases: проверка раз в 6 часов, фоновая загрузка установщика,
//! состояние для панели и окна. Загрузка годится к установке только после проверки подписи
//! minisign (`<файл>.minisig` в том же релизе) публичным ключом из настроек; без ключа панель
//! показывает только ссылку на релиз. На Windows установщик ещё проверяется по Authenticode
//! (`authenticode.rs`). Запускает установщик окно (`lmq-desktop`), сервер только скачивает.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::Value;

use crate::app::{App, VERSION};
use crate::authenticode::{self, Signature, Verdict};
use crate::events::Event;
use crate::util::now_ms;

/// Как часто проверять релизы.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// Пауза после неудачной загрузки перед новой попыткой.
const RETRY_DOWNLOAD: Duration = Duration::from_secs(3600);

/// Скачанный и проверенный установщик.
#[derive(Clone, Debug, Serialize)]
pub struct Ready {
    pub version: String,
    pub path: PathBuf,
    pub authenticode: Signature,
    pub authenticode_text: String,
    /// `allow` — ставить можно без вопросов, `confirm` — только с согласия человека.
    pub verdict: Verdict,
}

/// Что известно об обновлениях сейчас — для панели, окна и автоустановки.
#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdateState {
    pub checking: bool,
    pub downloading: bool,
    /// Время последней проверки, мс.
    pub checked_at: Option<i64>,
    pub info: Option<UpdateInfo>,
    pub ready: Option<Ready>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Updates {
    state: parking_lot::Mutex<UpdateState>,
    download: tokio::sync::Mutex<()>,
}

impl Updates {
    pub fn state(&self) -> UpdateState {
        self.state.lock().clone()
    }

    /// Скачанный установщик, если он есть и файл на месте.
    pub fn ready(&self) -> Option<Ready> {
        self.state.lock().ready.clone().filter(|r| r.path.is_file())
    }
}

/// Проверка с запоминанием итога в состоянии.
pub async fn refresh(app: &App) -> UpdateInfo {
    app.updates.state.lock().checking = true;
    let info = check(app).await;
    let mut st = app.updates.state.lock();
    st.checking = false;
    st.checked_at = Some(now_ms());
    st.error = info.error.clone();
    // Вышла версия новее скачанной — старый установщик больше не нужен.
    if st.ready.as_ref().is_some_and(|r| Some(&r.version) != info.latest.as_ref()) {
        st.ready = None;
    }
    st.info = Some(info.clone());
    info
}

/// Скачивает и проверяет установщик из `info`. Уже скачанный той же версии повторно не качает.
pub async fn prepare(app: &App, info: &UpdateInfo) -> Result<Ready> {
    let _one = app.updates.download.lock().await;
    let version = info.latest.clone().unwrap_or_default();
    if let Some(r) = app.updates.ready().filter(|r| r.version == version) {
        return Ok(r);
    }
    app.updates.state.lock().downloading = true;
    let res = download_verified(app, info).await;
    let mut st = app.updates.state.lock();
    st.downloading = false;
    match res {
        Ok((path, sig)) => {
            let ready = Ready { version, path, authenticode_text: sig.describe(), verdict: authenticode::verdict(&sig), authenticode: sig };
            st.ready = Some(ready.clone());
            st.error = None;
            drop(st);
            app.events.emit(Event::Update { state: "ready".into(), version: ready.version.clone() });
            Ok(ready)
        }
        Err(e) => {
            st.error = Some(format!("{e:#}"));
            Err(e)
        }
    }
}

/// Качать ли установщик в фоне в этом процессе: окно его поставит, сервер на Linux положит
/// `.deb` для `apt install`. Серверу на Windows установщик окна ни к чему.
fn downloads_here(app: &App) -> bool {
    app.desktop_token_hash.is_some() || cfg!(target_os = "linux")
}

/// Фон: проверка через 30 с после запуска и дальше раз в `CHECK_EVERY`, загрузка новой версии
/// (`updates.auto_download`). Настройки перечитываются раз в 10 минут.
pub async fn run(app: Arc<App>) {
    // Установщики прошлых запусков: либо уже стоят, либо будут скачаны заново.
    if let Ok(dir) = std::fs::read_dir(app.paths.data_dir.join("updates")) {
        for e in dir.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let mut first = Duration::from_secs(30);
    let mut checked: Option<Instant> = None;
    let mut tried: Option<Instant> = None;
    let mut announced: Option<String> = None;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(first) => {}
            _ = app.shutdown.cancelled() => break,
        }
        first = Duration::from_secs(600);
        let cfg = app.settings().updates.clone();
        if !cfg.auto_check || cfg.repo.trim().is_empty() {
            continue;
        }
        if checked.is_none_or(|t| t.elapsed() >= CHECK_EVERY) {
            checked = Some(Instant::now());
            let info = refresh(&app).await;
            let latest = info.latest.clone().unwrap_or_default();
            if info.newer && announced.as_deref() != Some(latest.as_str()) {
                tracing::info!(current = VERSION, latest = %latest, installer = ?info.installer, "update available");
                app.events.emit(Event::Update { state: "available".into(), version: latest.clone() });
                announced = Some(latest);
                tried = None;
            }
        }
        let Some(info) = app.updates.state().info.filter(|i| i.newer && i.can_install) else { continue };
        if cfg.auto_download && downloads_here(&app) && app.updates.ready().is_none() && tried.is_none_or(|t| t.elapsed() >= RETRY_DOWNLOAD) {
            tried = Some(Instant::now());
            match prepare(&app, &info).await {
                Ok(r) => tracing::info!(version = %r.version, file = %r.path.display(), verdict = ?r.verdict, "update downloaded"),
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "update download failed"),
            }
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub signed: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub newer: bool,
    pub prerelease: bool,
    pub notes: String,
    pub url: Option<String>,
    pub published_at: Option<String>,
    pub assets: Vec<Asset>,
    /// Подходящий для этой платформы установщик с подписью.
    pub installer: Option<String>,
    pub can_install: bool,
    pub error: Option<String>,
}

fn parse_ver(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches(['v', 'V'])).ok()
}

/// Установщик для текущей платформы по имени файла.
pub fn platform_asset(names: &[String]) -> Option<String> {
    let pick = |suffixes: &[&str]| names.iter().find(|n| suffixes.iter().any(|s| n.to_lowercase().ends_with(s))).cloned();
    if cfg!(windows) {
        pick(&["-setup.exe", "_x64-setup.exe", ".msi"])
    } else if cfg!(target_os = "linux") {
        pick(&["_amd64.deb", ".deb"])
    } else {
        None
    }
}

pub async fn check(app: &App) -> UpdateInfo {
    let cfg = app.settings().updates.clone();
    let mut info = UpdateInfo { current: VERSION.into(), ..Default::default() };
    if cfg.repo.trim().is_empty() {
        info.error = Some("репозиторий обновлений не задан (updates.repo)".into());
        return info;
    }
    match fetch(app, &cfg.repo, cfg.channel == "beta").await {
        Ok(Some(rel)) => {
            let tag = rel.get("tag_name").and_then(Value::as_str).unwrap_or("").to_string();
            let cur = parse_ver(VERSION);
            let latest = parse_ver(&tag);
            info.newer = matches!((&cur, &latest), (Some(c), Some(l)) if l > c);
            info.latest = Some(tag.trim_start_matches(['v', 'V']).to_string());
            info.prerelease = rel.get("prerelease").and_then(Value::as_bool).unwrap_or(false);
            info.notes = rel.get("body").and_then(Value::as_str).unwrap_or("").chars().take(8000).collect();
            info.url = rel.get("html_url").and_then(Value::as_str).map(String::from);
            info.published_at = rel.get("published_at").and_then(Value::as_str).map(String::from);
            let raw: Vec<(String, String, u64)> = rel
                .get("assets")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| {
                            Some((
                                x.get("name")?.as_str()?.to_string(),
                                x.get("browser_download_url")?.as_str()?.to_string(),
                                x.get("size").and_then(Value::as_u64).unwrap_or(0),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let names: Vec<String> = raw.iter().map(|r| r.0.clone()).collect();
            info.assets = raw
                .iter()
                .filter(|(n, _, _)| !n.ends_with(".minisig"))
                .map(|(n, u, s)| Asset { name: n.clone(), url: u.clone(), size: *s, signed: names.contains(&format!("{n}.minisig")) })
                .collect();
            info.installer = platform_asset(&names).filter(|n| names.contains(&format!("{n}.minisig")));
            info.can_install = info.newer && info.installer.is_some() && !cfg.pubkey.trim().is_empty();
        }
        Ok(None) => info.error = Some("релизов нет".into()),
        Err(e) => info.error = Some(e.to_string()),
    }
    info
}

/// `owner/name` из записи репозитория: принимается и ссылка `https://github.com/owner/name(.git)`.
pub fn repo_slug(repo: &str) -> String {
    let r = repo.trim();
    let r = r.strip_prefix("https://").or_else(|| r.strip_prefix("http://")).unwrap_or(r);
    let r = r.strip_prefix("www.").unwrap_or(r);
    let r = r.strip_prefix("github.com/").unwrap_or(r);
    r.trim_end_matches('/').trim_end_matches(".git").to_string()
}

async fn fetch(app: &App, repo: &str, beta: bool) -> Result<Option<Value>> {
    let url = format!("https://api.github.com/repos/{}/releases?per_page=15", repo_slug(repo));
    let resp = app
        .http
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| anyhow!("GitHub недоступен: {}", e.without_url()))?;
    if !resp.status().is_success() {
        bail!("GitHub ответил HTTP {}", resp.status().as_u16());
    }
    let list: Vec<Value> = resp.json().await.context("ответ GitHub не разобран")?;
    Ok(list
        .into_iter()
        .find(|r| !r.get("draft").and_then(Value::as_bool).unwrap_or(false) && (beta || !r.get("prerelease").and_then(Value::as_bool).unwrap_or(false))))
}

/// Проверяет подпись minisign (`<файл>.minisig`) файла `data` открытым ключом: строкой `RW…`
/// или содержимым файла `.pub`.
pub fn verify(pubkey: &str, data: &[u8], signature: &str) -> Result<()> {
    let pk = minisign_verify::PublicKey::from_base64(pubkey.trim())
        .or_else(|_| minisign_verify::PublicKey::decode(pubkey.trim()))
        .map_err(|e| anyhow!("updates.pubkey: {e}"))?;
    let sig = minisign_verify::Signature::decode(signature).map_err(|e| anyhow!("подпись: {e}"))?;
    pk.verify(data, &sig, false).map_err(|_| anyhow!("подпись не совпадает — файл не будет установлен"))
}

/// Скачивает установщик и его подпись, проверяет minisign, а на Windows ещё и Authenticode:
/// повреждённая подпись — ошибка. Возвращает путь к файлу и его подпись Authenticode; без
/// действительной подписи панель спрашивает согласие (`authenticode::verdict`).
pub async fn download_verified(app: &App, info: &UpdateInfo) -> Result<(PathBuf, Signature)> {
    let cfg = app.settings().updates.clone();
    let name = info.installer.clone().ok_or_else(|| anyhow!("нет подписанного установщика для этой платформы"))?;
    let asset = info.assets.iter().find(|a| a.name == name).ok_or_else(|| anyhow!("файл {name} не найден"))?;
    let get = |u: String| async move {
        let r = app.http.get(&u).timeout(std::time::Duration::from_secs(600)).send().await?;
        if !r.status().is_success() {
            bail!("HTTP {}", r.status().as_u16());
        }
        Ok::<_, anyhow::Error>(r.bytes().await?)
    };
    let data = get(asset.url.clone()).await.context("загрузка установщика")?;
    let sig_text = get(format!("{}.minisig", asset.url)).await.context("загрузка подписи")?;
    verify(&cfg.pubkey, &data, &String::from_utf8_lossy(&sig_text))?;
    let dir = app.paths.data_dir.join("updates");
    std::fs::create_dir_all(&dir)?;
    if name.contains(['/', '\\']) || name.contains("..") {
        bail!("недопустимое имя файла {name}");
    }
    let path = dir.join(&name);
    // В папке — только последний установщик.
    for e in std::fs::read_dir(&dir)?.flatten() {
        if e.path() != path {
            let _ = std::fs::remove_file(e.path());
        }
    }
    std::fs::write(&path, &data)?;
    let file = path.clone();
    let sig = tokio::task::spawn_blocking(move || authenticode::check(&file)).await?;
    tracing::info!(file = %path.display(), authenticode = %sig.describe(), "update downloaded and verified");
    if authenticode::verdict(&sig) == authenticode::Verdict::Deny {
        let _ = std::fs::remove_file(&path);
        bail!("установщик не прошёл проверку Authenticode: {}", sig.describe());
    }
    Ok((path, sig))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_UPDATES_PUBKEY;

    /// Подпись настоящим ключом релизов (`minisign -S`), как её кладёт CI рядом со сборкой.
    const SAMPLE: &[u8] = b"LMQ update signature check\n";
    const SAMPLE_SIG: &str = "untrusted comment: signature from minisign secret key
RUTnz+w5RwAHuUlcasR6tOfR4izrimhxrbtDEKJy9wKiJrtAis7xSOxQUKUBWYu1NeHlknp2OhQ58TsZzVRZZ+OekZjyxNDePA4=
trusted comment: LMQ test sample
Os+ZKHH3yYuER3zQKOOJ+5cxMoP4+f51VF7l5hTXxpriGF4+4TfOpOI3hsQg9HnidC2sKvzQYdvOf0YSQC7GAQ==
";

    #[test]
    fn release_signature_is_verified_with_default_key() {
        verify(DEFAULT_UPDATES_PUBKEY, SAMPLE, SAMPLE_SIG).expect("подпись ключом релизов должна проходить");
        assert!(verify(DEFAULT_UPDATES_PUBKEY, b"tampered", SAMPLE_SIG).is_err());
    }

    #[test]
    fn release_assets_match_platform() {
        // Архивы установщика идут раньше .exe: автообновление всё равно должно взять .exe.
        let names: Vec<String> = [
            "LMQ-2.3.0-windows-x64-setup.rar",
            "LMQ-2.3.0-windows-x64-setup.zip",
            "LMQ_2.3.0_x64-setup.exe",
            "LMQ-2.3.0-windows-x64-portable.zip",
            "lmq_2.3.0-1_amd64.deb",
            "SHA256SUMS.txt",
        ]
        .map(String::from)
        .into();
        let expect = if cfg!(windows) {
            Some("LMQ_2.3.0_x64-setup.exe")
        } else if cfg!(target_os = "linux") {
            Some("lmq_2.3.0-1_amd64.deb")
        } else {
            None
        };
        assert_eq!(platform_asset(&names).as_deref(), expect);
    }
}
