//! Где лежат настройки, данные и журналы.
//!
//! | Платформа | Настройки | Данные (БД, журналы) |
//! |---|---|---|
//! | Windows | `%APPDATA%\LMQ` (Roaming) | `%LOCALAPPDATA%\LMQ` |
//! | Linux, пользователь | `~/.config/lmq` | `~/.local/share/lmq` |
//! | Linux, служба | `LMQ_HOME` или `--config`/`--data` (`/etc/lmq`, `/var/lib/lmq`) | |
//! | Портативный режим | `<папка exe>\data` (если рядом с exe лежит файл `portable`) | там же |

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Clone, Debug, Default)]
pub struct PathOverrides {
    /// Общая папка для всего (`LMQ_HOME`, `--home`).
    pub home: Option<PathBuf>,
    pub config_dir: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub secrets_file: PathBuf,
    pub portable: bool,
}

const APP_DIR_WIN: &str = "LMQ";
const APP_DIR_UNIX: &str = "lmq";

impl AppPaths {
    pub fn resolve(ov: &PathOverrides) -> Result<Self> {
        let home = ov.home.clone().or_else(|| std::env::var_os("LMQ_HOME").map(PathBuf::from));
        let portable_dir = portable_dir();
        let portable = home.is_none() && portable_dir.is_some();

        let (config_dir, data_dir) = if let Some(h) = home {
            (h.clone(), h)
        } else if let Some(p) = portable_dir {
            (p.clone(), p)
        } else {
            let base = directories::BaseDirs::new().context("cannot find user profile directories")?;
            let name = if cfg!(windows) { APP_DIR_WIN } else { APP_DIR_UNIX };
            (base.config_dir().join(name), base.data_local_dir().join(name))
        };
        let config_dir = ov.config_dir.clone().unwrap_or(config_dir);
        let data_dir = ov.data_dir.clone().unwrap_or(data_dir);
        Ok(Self::from_dirs(config_dir, data_dir, portable))
    }

    pub fn from_dirs(config_dir: PathBuf, data_dir: PathBuf, portable: bool) -> Self {
        Self { log_dir: data_dir.join("logs"), secrets_file: config_dir.join("secrets.bin"), config_dir, data_dir, portable }
    }

    pub fn ensure(&self) -> Result<()> {
        for d in [&self.config_dir, &self.data_dir, &self.log_dir, &self.profiles_dir()] {
            std::fs::create_dir_all(d).with_context(|| format!("create {}", d.display()))?;
        }
        Ok(())
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.config_dir.join("profiles")
    }

    /// Файл настроек профиля: `default` — `settings.yaml`, остальные — `profiles/<имя>.yaml`.
    pub fn settings_file(&self, profile: &str) -> PathBuf {
        if profile == "default" || profile.is_empty() {
            self.config_dir.join("settings.yaml")
        } else {
            self.profiles_dir().join(format!("{}.yaml", crate::util::safe_file_name(profile)))
        }
    }

    pub fn active_profile_file(&self) -> PathBuf {
        self.config_dir.join("active-profile")
    }

    pub fn default_db(&self) -> PathBuf {
        self.data_dir.join("queue.sqlite")
    }

    pub fn engine_log(&self, id: &str) -> PathBuf {
        self.log_dir.join(format!("engine-{}.log", crate::util::safe_file_name(id)))
    }
}

fn portable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    if dir.join("portable").exists() { Some(dir.join("data")) } else { None }
}

/// Атомарная запись: во временный файл рядом, затем переименование. Прежняя версия — в `.bak`.
pub fn write_atomic(path: &Path, data: &[u8], keep_backup: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).with_context(|| format!("write {}", tmp.display()))?;
    #[cfg(unix)]
    restrict_permissions(&tmp)?;
    if keep_backup && path.exists() {
        let _ = std::fs::copy(path, path.with_extension("yaml.bak"));
    }
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

/// Права 0600 на файлы с секретами (Unix).
#[cfg(unix)]
pub fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
pub fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
