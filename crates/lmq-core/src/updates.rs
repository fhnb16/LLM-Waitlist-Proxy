//! Проверка обновлений через GitHub Releases. Загрузка устанавливается только после
//! проверки подписи minisign (`<файл>.minisig` в том же релизе) публичным ключом из
//! настроек; без ключа панель показывает только ссылку на релиз.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::Value;

use crate::app::{App, VERSION};

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

async fn fetch(app: &App, repo: &str, beta: bool) -> Result<Option<Value>> {
    let url = format!("https://api.github.com/repos/{}/releases?per_page=15", repo.trim());
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

/// Скачивает установщик и его подпись, проверяет подпись. Возвращает путь к файлу.
pub async fn download_verified(app: &App, info: &UpdateInfo) -> Result<PathBuf> {
    let cfg = app.settings().updates.clone();
    let name = info.installer.clone().ok_or_else(|| anyhow!("нет подписанного установщика для этой платформы"))?;
    let pk = minisign_verify::PublicKey::from_base64(cfg.pubkey.trim())
        .or_else(|_| minisign_verify::PublicKey::decode(cfg.pubkey.trim()))
        .map_err(|e| anyhow!("updates.pubkey: {e}"))?;
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
    let sig = minisign_verify::Signature::decode(&String::from_utf8_lossy(&sig_text)).map_err(|e| anyhow!("подпись: {e}"))?;
    pk.verify(&data, &sig, false).map_err(|_| anyhow!("подпись не совпадает — файл не будет установлен"))?;
    let dir = app.paths.data_dir.join("updates");
    std::fs::create_dir_all(&dir)?;
    if name.contains(['/', '\\']) || name.contains("..") {
        bail!("недопустимое имя файла {name}");
    }
    let path = dir.join(&name);
    std::fs::write(&path, &data)?;
    tracing::info!(file = %path.display(), "update downloaded and verified");
    Ok(path)
}
