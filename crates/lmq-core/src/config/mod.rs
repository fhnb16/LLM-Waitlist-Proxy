//! Конфигурация: значения по умолчанию < `settings.yaml` < окружение (§6 ТЗ) < CLI.

pub mod paths;
pub mod secrets;
pub mod settings;

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub use paths::{AppPaths, PathOverrides};
pub use secrets::Secrets;
pub use settings::*;

use crate::crypto::Cipher;
use crate::util::sha256_hex;

/// Действующая конфигурация.
#[derive(Clone, Debug)]
pub struct Effective {
    /// Файл + окружение + CLI. Секреты расшифрованы.
    pub settings: Settings,
    /// Только то, что записано в файле (для редактирования в UI). Секреты расшифрованы.
    pub file: Settings,
    /// Пути полей, перекрытых окружением или CLI (`server.listen`, `models` …).
    pub locked: BTreeSet<String>,
    pub profile: String,
    pub settings_path: PathBuf,
}

/// Перекрытия из командной строки.
#[derive(Clone, Debug, Default)]
pub struct CliOverrides {
    pub listen: Option<String>,
    pub admin_listen: Option<String>,
    pub log_level: Option<String>,
    /// Адрес панели, если в настройках он пуст (окну приложения панель нужна всегда).
    pub admin_listen_fallback: Option<String>,
}

/// Внешний файл моделей (`MODELS_CONFIG`) — формат §5.4.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelsFile {
    pub models: std::collections::BTreeMap<String, ModelCfg>,
    pub aliases: std::collections::BTreeMap<String, String>,
}

/// Читает файл настроек; если его нет — значения по умолчанию.
pub fn load_file(path: &Path, cipher: &Cipher) -> Result<Settings> {
    if !path.exists() {
        return Ok(Settings::default());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut s: Settings =
        if text.trim().is_empty() { Settings::default() } else { serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))? };
    migrate(&mut s);
    decrypt_secrets(&mut s, cipher)?;
    Ok(s)
}

/// Пишет файл атомарно, секреты — в виде `enc:…`.
pub fn save_file(path: &Path, s: &Settings, cipher: &Cipher) -> Result<()> {
    let mut out = s.clone();
    out.schema_version = SCHEMA_VERSION;
    encrypt_secrets(&mut out, cipher);
    let yaml = serde_yaml::to_string(&out)?;
    let body = format!(
        "# LMQ — настройки. Файл редактирует панель; ручные правки допустимы.\n\
         # Секреты хранятся зашифрованными (enc:…); открытое значение зашифруется при следующем сохранении.\n{yaml}"
    );
    paths::write_atomic(path, body.as_bytes(), true)
}

fn migrate(s: &mut Settings) {
    // schema_version 0 → 1: поля появились с значениями по умолчанию (serde(default)).
    // 1 → 2: у репозитория обновлений появилось значение по умолчанию — репозиторий проекта;
    // в старых файлах там записана пустая строка.
    if s.schema_version < 2 && s.updates.repo.trim().is_empty() {
        s.updates.repo = DEFAULT_UPDATES_REPO.into();
    }
    if s.schema_version < SCHEMA_VERSION {
        s.schema_version = SCHEMA_VERSION;
    }
}

fn for_each_secret(s: &mut Settings, mut f: impl FnMut(&mut String)) {
    f(&mut s.callback.token);
    f(&mut s.protocol1.telegram_bot_token);
    f(&mut s.metrics.token);
    f(&mut s.socks.password);
    for m in s.models.values_mut() {
        for u in &mut m.upstreams {
            f(&mut u.api_key);
        }
    }
}

pub fn encrypt_secrets(s: &mut Settings, cipher: &Cipher) {
    for_each_secret(s, |v| *v = cipher.encrypt_str(v));
}

pub fn decrypt_secrets(s: &mut Settings, cipher: &Cipher) -> Result<()> {
    let mut err = None;
    for_each_secret(s, |v| match cipher.decrypt_str(v) {
        Ok(p) => *v = p,
        Err(e) => err = Some(e),
    });
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Заменяет секреты маской — для показа в UI и экспорта без секретов.
pub fn mask_secrets(s: &mut Settings, mask: &str) {
    for_each_secret(s, |v| {
        if !v.is_empty() {
            *v = mask.to_string();
        }
    });
}

/// Возвращает секреты из `old` туда, где в `new` стоит маска (UI не прислал новое значение).
pub fn restore_masked(new: &mut Settings, old: &Settings, mask: &str) {
    let keep = |v: &mut String, o: &str| {
        if v == mask {
            *v = o.to_string();
        }
    };
    keep(&mut new.callback.token, &old.callback.token);
    keep(&mut new.protocol1.telegram_bot_token, &old.protocol1.telegram_bot_token);
    keep(&mut new.metrics.token, &old.metrics.token);
    keep(&mut new.socks.password, &old.socks.password);
    // Ключи апстримов сопоставляем по модели и номеру апстрима.
    for (name, m) in new.models.iter_mut() {
        for (i, u) in m.upstreams.iter_mut().enumerate() {
            if u.api_key == mask {
                u.api_key = old.models.get(name).and_then(|om| om.upstreams.get(i)).map(|ou| ou.api_key.clone()).unwrap_or_default();
            }
        }
    }
}

type EnvFn<'a> = &'a dyn Fn(&str) -> Option<String>;

fn env_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn split_list(v: &str) -> Vec<String> {
    v.split([',', ';', ' ']).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// Накладывает переменные окружения §6. Возвращает перекрытые поля.
pub fn apply_env(s: &mut Settings, env: EnvFn) -> Result<BTreeSet<String>> {
    let mut locked = BTreeSet::new();
    macro_rules! num {
        ($name:literal, $field:expr, $path:literal) => {
            if let Some(v) = env($name) {
                $field = v.trim().parse().with_context(|| format!("{} must be a number", $name))?;
                locked.insert($path.to_string());
            }
        };
    }
    macro_rules! boolean {
        ($name:literal, $field:expr, $path:literal) => {
            if let Some(v) = env($name) {
                $field = env_bool(&v).with_context(|| format!("{} must be true/false", $name))?;
                locked.insert($path.to_string());
            }
        };
    }
    macro_rules! string {
        ($name:literal, $field:expr, $path:literal) => {
            if let Some(v) = env($name) {
                $field = v;
                locked.insert($path.to_string());
            }
        };
    }
    string!("LISTEN", s.server.listen, "server.listen");
    string!("ADMIN_LISTEN", s.server.admin_listen, "server.admin_listen");
    num!("MAX_BODY_MB", s.server.max_body_mb, "server.max_body_mb");
    string!("BOT_CALLBACK_URL", s.callback.url, "callback.url");
    string!("BOT_CALLBACK_TOKEN", s.callback.token, "callback.token");
    if let Some(v) = env("CALLBACK_ALLOWED_HOSTS") {
        s.callback.allowed_hosts = split_list(&v);
        locked.insert("callback.allowed_hosts".into());
    }
    boolean!("CALLBACK_ALLOW_HTTP_PRIVATE", s.callback.allow_http_private, "callback.allow_http_private");
    num!("CALLBACK_RETRIES", s.callback.retries, "callback.retries");
    string!("MODELS_CONFIG", s.storage.models_config, "storage.models_config");
    string!("DB_PATH", s.storage.db_path, "storage.db_path");
    num!("RETENTION_HOURS", s.storage.retention_hours, "storage.retention_hours");
    num!("STATS_RETENTION_DAYS", s.storage.stats_retention_days, "storage.stats_retention_days");
    num!("MAX_QUEUE", s.queue.max_queue, "queue.max_queue");
    num!("MAX_QUEUE_PER_CHAT", s.queue.max_queue_per_chat, "queue.max_queue_per_chat");
    num!("MAX_WAIT", s.queue.max_wait, "queue.max_wait");
    num!("UPSTREAM_TIMEOUT", s.queue.upstream_timeout, "queue.upstream_timeout");
    num!("SYNC_TIMEOUT", s.queue.sync_timeout, "queue.sync_timeout");
    boolean!("SUPERSEDE_SAME_CHAT", s.queue.supersede_same_chat, "queue.supersede_same_chat");
    boolean!("PRIVATE_FIRST", s.queue.private_first, "queue.private_first");
    num!("UPSTREAM_RETRIES", s.queue.upstream_retries, "queue.upstream_retries");
    string!("LOG_LEVEL", s.logging.level, "logging.level");
    boolean!("LOG_CONTENT", s.logging.content, "logging.content");
    string!("METRICS_TOKEN", s.metrics.token, "metrics.token");
    boolean!("PROTOCOL1_ENABLED", s.protocol1.enabled, "protocol1.enabled");
    string!("TELEGRAM_BOT_TOKEN", s.protocol1.telegram_bot_token, "protocol1.telegram_bot_token");
    boolean!("TG_RELAY_ENABLED", s.tg_relay.enabled, "tg_relay.enabled");
    if let Some(v) = env("TG_RELAY_ALLOWED_BOTS") {
        s.tg_relay.allowed_bots = split_list(&v);
        locked.insert("tg_relay.allowed_bots".into());
    }
    boolean!("SOCKS_ENABLED", s.socks.enabled, "socks.enabled");
    string!("SOCKS_LISTEN", s.socks.listen, "socks.listen");

    // API_KEYS=name:key,name2:key2 — клиенты из окружения (ключи только в памяти, хэшами).
    if let Some(v) = env("API_KEYS") {
        for (i, item) in v.split(',').map(str::trim).filter(|x| !x.is_empty()).enumerate() {
            let (name, key) = match item.split_once(':') {
                Some((n, k)) => (n.trim().to_string(), k.trim().to_string()),
                None => (format!("env{}", i + 1), item.to_string()),
            };
            if key.is_empty() {
                bail!("API_KEYS: empty key for '{name}'");
            }
            s.clients.retain(|c| c.name != name);
            s.clients.push(ClientCfg { name, key_hash: format!("sha256:{}", sha256_hex(&key)), enabled: true, created_at: 0, note: "API_KEYS".into() });
        }
        locked.insert("clients.env".into());
    }
    Ok(locked)
}

fn apply_cli(s: &mut Settings, cli: &CliOverrides, locked: &mut BTreeSet<String>) {
    if let Some(v) = &cli.listen {
        s.server.listen = v.clone();
        locked.insert("server.listen".into());
    }
    if let Some(v) = &cli.admin_listen {
        s.server.admin_listen = v.clone();
        locked.insert("server.admin_listen".into());
    }
    if let Some(v) = &cli.log_level {
        s.logging.level = v.clone();
        locked.insert("logging.level".into());
    }
    if s.server.admin_listen.trim().is_empty()
        && let Some(v) = &cli.admin_listen_fallback
    {
        s.server.admin_listen = v.clone();
    }
}

fn apply_models_file(s: &mut Settings, base_dir: &Path, cipher: &Cipher, locked: &mut BTreeSet<String>) -> Result<()> {
    if s.storage.models_config.trim().is_empty() {
        return Ok(());
    }
    let mut p = PathBuf::from(&s.storage.models_config);
    if p.is_relative() && !p.exists() {
        p = base_dir.join(&p);
    }
    let text = std::fs::read_to_string(&p).with_context(|| format!("MODELS_CONFIG {}", p.display()))?;
    let mut mf: ModelsFile = serde_yaml::from_str(&text).with_context(|| format!("parse {}", p.display()))?;
    for m in mf.models.values_mut() {
        for u in &mut m.upstreams {
            u.api_key = cipher.decrypt_str(&u.api_key)?;
        }
    }
    s.models = mf.models;
    if !mf.aliases.is_empty() {
        s.aliases = mf.aliases;
    }
    locked.insert("models".into());
    Ok(())
}

/// Собирает действующую конфигурацию из файла профиля, окружения и CLI.
pub fn build_effective(paths: &AppPaths, profile: &str, cipher: &Cipher, cli: &CliOverrides, env: EnvFn) -> Result<Effective> {
    let settings_path = paths.settings_file(profile);
    let file = load_file(&settings_path, cipher)?;
    compose(file, paths, profile, settings_path, cipher, cli, env)
}

pub fn compose(file: Settings, paths: &AppPaths, profile: &str, settings_path: PathBuf, cipher: &Cipher, cli: &CliOverrides, env: EnvFn) -> Result<Effective> {
    let mut settings = file.clone();
    let mut locked = apply_env(&mut settings, env)?;
    apply_cli(&mut settings, cli, &mut locked);
    apply_models_file(&mut settings, &paths.config_dir, cipher, &mut locked)?;
    let errors = validate(&settings);
    if !errors.is_empty() {
        bail!("invalid configuration:\n  - {}", errors.join("\n  - "));
    }
    Ok(Effective { settings, file, locked, profile: profile.to_string(), settings_path })
}

pub fn system_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Проверка конфигурации. Пустой список — всё в порядке.
pub fn validate(s: &Settings) -> Vec<String> {
    let mut e = Vec::new();
    if s.server.listen.parse::<SocketAddr>().is_err() {
        e.push(format!("server.listen: '{}' не адрес вида host:port", s.server.listen));
    }
    if !s.server.admin_listen.is_empty() && s.server.admin_listen.parse::<SocketAddr>().is_err() {
        e.push(format!("server.admin_listen: '{}' не адрес вида host:port", s.server.admin_listen));
    }
    if s.server.max_body_mb == 0 {
        e.push("server.max_body_mb должен быть больше 0".into());
    }
    if !s.callback.url.is_empty() && url::Url::parse(&s.callback.url).is_err() {
        e.push(format!("callback.url: '{}' не URL", s.callback.url));
    }
    let mut names = BTreeSet::new();
    for c in &s.clients {
        if c.name.trim().is_empty() {
            e.push("clients: пустое имя клиента".into());
        }
        if !names.insert(c.name.clone()) {
            e.push(format!("clients: имя '{}' повторяется", c.name));
        }
        if !c.key_hash.starts_with("sha256:") {
            e.push(format!("clients.{}: key_hash должен начинаться с sha256:", c.name));
        }
    }
    let mut engine_ids = BTreeSet::new();
    for en in &s.engines {
        if en.id.trim().is_empty() {
            e.push("engines: пустой id".into());
        }
        if !engine_ids.insert(en.id.clone()) {
            e.push(format!("engines: id '{}' повторяется", en.id));
        }
    }
    for (name, m) in &s.models {
        if name.trim().is_empty() {
            e.push("models: пустое имя модели".into());
        }
        if m.upstreams.is_empty() {
            e.push(format!("models.{name}: нет апстримов"));
        }
        for (i, u) in m.upstreams.iter().enumerate() {
            if !u.engine.is_empty() {
                if !engine_ids.contains(&u.engine) {
                    e.push(format!("models.{name}.upstreams[{i}]: движок '{}' не найден", u.engine));
                }
            } else if url::Url::parse(&u.url).is_err() {
                e.push(format!("models.{name}.upstreams[{i}]: '{}' не URL", u.url));
            }
            if u.slots == 0 && u.engine.is_empty() {
                e.push(format!("models.{name}.upstreams[{i}]: slots должен быть ≥ 1"));
            }
        }
        for f in &m.fallback {
            if f == name {
                e.push(format!("models.{name}: модель не может быть резервом самой себя"));
            } else if !s.models.contains_key(f) && !s.aliases.contains_key(f) {
                e.push(format!("models.{name}.fallback: модель '{f}' не найдена"));
            }
        }
    }
    for (alias, target) in &s.aliases {
        if s.models.contains_key(alias) {
            e.push(format!("aliases.{alias}: совпадает с именем модели"));
        }
        if !s.models.contains_key(target) {
            e.push(format!("aliases.{alias}: модель '{target}' не найдена"));
        }
    }
    for (i, r) in s.schedule.iter().enumerate() {
        if parse_hhmm(&r.from).is_none() || parse_hhmm(&r.to).is_none() {
            e.push(format!("schedule[{i}]: время в формате HH:MM"));
        }
        for d in &r.days {
            if !DAYS.contains(&d.to_ascii_lowercase().as_str()) {
                e.push(format!("schedule[{i}]: неизвестный день '{d}' (mon…sun)"));
            }
        }
    }
    let relay = &s.tg_relay;
    if !relay.upstream.trim().is_empty() && !url::Url::parse(relay.upstream.trim()).is_ok_and(|u| matches!(u.scheme(), "http" | "https")) {
        e.push(format!("tg_relay.upstream: '{}' не адрес http(s)://", relay.upstream));
    }
    for b in &relay.allowed_bots {
        // Значение не повторяем: вместо id сюда могли вставить токен целиком.
        if b.contains(':') {
            e.push("tg_relay.allowed_bots: похоже на токен целиком — оставьте только число до двоеточия".into());
        } else if b.trim().is_empty() || !b.trim().bytes().all(|c| c.is_ascii_digit()) {
            e.push(format!("tg_relay.allowed_bots: '{b}' — нужен id бота (число до двоеточия в токене)"));
        }
    }
    if relay.max_body_mb == 0 || relay.timeout == 0 {
        e.push("tg_relay: max_body_mb и timeout должны быть больше 0".into());
    }
    let socks = &s.socks;
    if socks.enabled && socks.listen.parse::<SocketAddr>().is_err() {
        e.push(format!("socks.listen: '{}' не адрес вида host:port", socks.listen));
    }
    if socks.username.is_empty() && !socks.password.is_empty() {
        e.push("socks: пароль без логина".into());
    }
    if socks.username.len() > 255 || socks.password.len() > 255 {
        e.push("socks: логин и пароль — не длиннее 255 байт".into());
    }
    if !(0.0..=1.0).contains(&s.ui.backdrop_opacity) {
        e.push("ui.backdrop_opacity: от 0 до 1".into());
    }
    e
}

/// Хэш ключа клиента в формате `sha256:<hex>`.
pub fn hash_key(key: &str) -> String {
    format!("sha256:{}", sha256_hex(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(map: HashMap<&'static str, &'static str>) -> impl Fn(&str) -> Option<String> {
        move |k| map.get(k).map(|v| v.to_string())
    }

    #[test]
    fn env_overrides_file() {
        let mut s = Settings::default();
        s.queue.max_queue = 10;
        let env = env_of(HashMap::from([
            ("MAX_QUEUE", "77"),
            ("SUPERSEDE_SAME_CHAT", "true"),
            ("API_KEYS", "bot:secret1,other:secret2"),
            ("CALLBACK_ALLOWED_HOSTS", "a.example, b.example"),
        ]));
        let locked = apply_env(&mut s, &env).unwrap();
        assert_eq!(s.queue.max_queue, 77);
        assert!(s.queue.supersede_same_chat);
        assert_eq!(s.clients.len(), 2);
        assert_eq!(s.clients[0].key_hash, hash_key("secret1"));
        assert_eq!(s.callback.allowed_hosts, vec!["a.example", "b.example"]);
        assert!(locked.contains("queue.max_queue"));
    }

    #[test]
    fn bad_env_number_is_error() {
        let mut s = Settings::default();
        let env = env_of(HashMap::from([("MAX_WAIT", "abc")]));
        assert!(apply_env(&mut s, &env).is_err());
    }

    #[test]
    fn validation_finds_problems() {
        let mut s = Settings::default();
        s.models.insert("m".into(), ModelCfg { fallback: vec!["nope".into()], ..Default::default() });
        let errs = validate(&s);
        assert!(errs.iter().any(|e| e.contains("нет апстримов")));
        assert!(errs.iter().any(|e| e.contains("nope")));
    }

    #[test]
    fn secrets_roundtrip_in_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.yaml");
        let cipher = Cipher::new(&[1u8; 32]);
        let mut s = Settings::default();
        s.callback.token = "cb-secret".into();
        s.models.insert(
            "m".into(),
            ModelCfg {
                upstreams: vec![UpstreamCfg { url: "http://127.0.0.1:1/v1/chat/completions".into(), api_key: "k".into(), ..Default::default() }],
                ..Default::default()
            },
        );
        save_file(&path, &s, &cipher).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("cb-secret"));
        assert!(raw.contains("enc:"));
        let back = load_file(&path, &cipher).unwrap();
        assert_eq!(back.callback.token, "cb-secret");
        assert_eq!(back.models["m"].upstreams[0].api_key, "k");
    }

    #[test]
    fn old_files_get_the_updates_repo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.yaml");
        let cipher = Cipher::new(&[1u8; 32]);
        std::fs::write(&path, "schema_version: 1\nupdates:\n  repo: ''\n").unwrap();
        assert_eq!(load_file(&path, &cipher).unwrap().updates.repo, DEFAULT_UPDATES_REPO);
        // В новом файле пустой репозиторий — осознанный выбор: проверка выключена.
        std::fs::write(&path, "schema_version: 2\nupdates:\n  repo: ''\n").unwrap();
        assert_eq!(load_file(&path, &cipher).unwrap().updates.repo, "");
        assert_eq!(crate::updates::repo_slug("https://github.com/fhnb16/LLM-Waitlist-Proxy.git/"), DEFAULT_UPDATES_REPO);
        assert_eq!(crate::updates::repo_slug(" fhnb16/LLM-Waitlist-Proxy "), DEFAULT_UPDATES_REPO);
    }

    #[test]
    fn masked_secrets_are_restored() {
        let mut old = Settings::default();
        old.callback.token = "tok".into();
        old.models.insert("m".into(), ModelCfg { upstreams: vec![UpstreamCfg { api_key: "key".into(), ..Default::default() }], ..Default::default() });
        let mut new = old.clone();
        mask_secrets(&mut new, "***");
        new.queue.max_queue = 5;
        restore_masked(&mut new, &old, "***");
        assert_eq!(new.callback.token, "tok");
        assert_eq!(new.models["m"].upstreams[0].api_key, "key");
    }
}
