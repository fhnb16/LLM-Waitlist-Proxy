//! Схема `settings.yaml`. Значения по умолчанию — из §6 ТЗ.
//!
//! Все секреты в этой структуре хранятся **расшифрованными**; шифрование при записи и
//! расшифровка при чтении — в `config::file`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub server: ServerCfg,
    pub queue: QueueCfg,
    pub callback: CallbackCfg,
    pub storage: StorageCfg,
    pub logging: LoggingCfg,
    pub metrics: MetricsCfg,
    pub protocol1: Protocol1Cfg,
    pub breaker: BreakerCfg,
    /// Через какой интерфейс или прокси ходит каждое направление.
    pub network: NetworkCfg,
    /// Реле Telegram Bot API на адресе API.
    pub tg_relay: TgRelayCfg,
    pub clients: Vec<ClientCfg>,
    pub models: BTreeMap<String, ModelCfg>,
    /// Алиасы моделей: `fast: qwen3-27b`.
    pub aliases: BTreeMap<String, String>,
    pub engines: Vec<EngineCfg>,
    pub schedule: Vec<ScheduleRule>,
    pub ui: UiCfg,
    pub app: AppCfg,
    pub updates: UpdatesCfg,
    pub debug: DebugCfg,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            server: Default::default(),
            queue: Default::default(),
            callback: Default::default(),
            storage: Default::default(),
            logging: Default::default(),
            metrics: Default::default(),
            protocol1: Default::default(),
            breaker: Default::default(),
            network: Default::default(),
            tg_relay: Default::default(),
            clients: Vec::new(),
            models: BTreeMap::new(),
            aliases: BTreeMap::new(),
            engines: Vec::new(),
            schedule: Vec::new(),
            ui: Default::default(),
            app: Default::default(),
            updates: Default::default(),
            debug: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ServerCfg {
    /// `LISTEN`: адрес клиентского API.
    pub listen: String,
    /// `ADMIN_LISTEN`: адрес панели и admin API. Пусто — панель выключена.
    pub admin_listen: String,
    /// `MAX_BODY_MB`.
    pub max_body_mb: u32,
    /// Не больше сообщений в одном задании.
    pub max_messages: u32,
    /// Не длиннее текстовой части сообщения, КБ (картинки base64 не считаются).
    pub max_string_kb: u32,
    /// Таймаут чтения тела запроса, с (защита от медленных клиентов).
    pub body_read_timeout: u64,
    /// Разрешить CORS для панели с этих источников (кроме своего и Tauri).
    pub admin_cors_origins: Vec<String>,
}

impl Default for ServerCfg {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8800".into(),
            admin_listen: "127.0.0.1:8801".into(),
            max_body_mb: 8,
            max_messages: 500,
            max_string_kb: 512,
            body_read_timeout: 30,
            admin_cors_origins: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct QueueCfg {
    pub max_queue: u32,
    pub max_queue_per_chat: u32,
    /// `MAX_WAIT`, с.
    pub max_wait: u64,
    /// `UPSTREAM_TIMEOUT`, с.
    pub upstream_timeout: u64,
    /// `SYNC_TIMEOUT`, с.
    pub sync_timeout: u64,
    pub supersede_same_chat: bool,
    pub private_first: bool,
    pub upstream_retries: u32,
}

impl Default for QueueCfg {
    fn default() -> Self {
        Self {
            max_queue: 200,
            max_queue_per_chat: 5,
            max_wait: 300,
            upstream_timeout: 120,
            sync_timeout: 60,
            supersede_same_chat: false,
            private_first: true,
            upstream_retries: 1,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CallbackCfg {
    /// `BOT_CALLBACK_URL`.
    pub url: String,
    /// `BOT_CALLBACK_TOKEN` (секрет).
    pub token: String,
    /// `CALLBACK_ALLOWED_HOSTS`. Пусто — только хост из `url`. Допускается `*.example.com`.
    pub allowed_hosts: Vec<String>,
    /// `CALLBACK_ALLOW_HTTP_PRIVATE`: разрешить `http://` для localhost и частных сетей.
    pub allow_http_private: bool,
    /// `CALLBACK_RETRIES`.
    pub retries: u32,
    /// Таймаут одного обратного вызова, с.
    pub timeout: u64,
}

impl Default for CallbackCfg {
    fn default() -> Self {
        Self { url: String::new(), token: String::new(), allowed_hosts: Vec::new(), allow_http_private: false, retries: 6, timeout: 15 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StorageCfg {
    /// `DB_PATH`. Пусто — `<папка данных>/queue.sqlite`.
    pub db_path: String,
    /// `MODELS_CONFIG`: внешний файл с таблицей моделей (формат §5.4). Пусто — модели из этого файла.
    pub models_config: String,
    /// `RETENTION_HOURS`.
    pub retention_hours: u64,
    /// `STATS_RETENTION_DAYS`.
    pub stats_retention_days: u64,
}

impl Default for StorageCfg {
    fn default() -> Self {
        Self { db_path: String::new(), models_config: String::new(), retention_hours: 24, stats_retention_days: 30 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LoggingCfg {
    /// `LOG_LEVEL`: error | warn | info | debug | trace.
    pub level: String,
    /// `LOG_CONTENT`: писать ли тексты сообщений и ответов.
    pub content: bool,
    /// Сколько дней хранить файлы журнала.
    pub keep_days: u32,
    /// Подробный журнал запросов: строение входящего запроса, что ушло апстриму и что
    /// ответил бот. Тексты — только вместе с `content`.
    pub requests: bool,
}

impl Default for LoggingCfg {
    fn default() -> Self {
        Self { level: "info".into(), content: false, keep_days: 14, requests: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MetricsCfg {
    pub enabled: bool,
    /// `METRICS_TOKEN`. Пусто — `/metrics` отвечает только на запросы с loopback.
    pub token: String,
}

impl Default for MetricsCfg {
    fn default() -> Self {
        Self { enabled: true, token: String::new() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Protocol1Cfg {
    /// `PROTOCOL1_ENABLED`.
    pub enabled: bool,
    /// `TELEGRAM_BOT_TOKEN` (секрет).
    pub telegram_bot_token: String,
    pub telegram_api: String,
}

impl Default for Protocol1Cfg {
    fn default() -> Self {
        Self { enabled: false, telegram_bot_token: String::new(), telegram_api: "https://api.telegram.org".into() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BreakerCfg {
    pub enabled: bool,
    /// Подряд неудач до размыкания.
    pub failure_threshold: u32,
    /// Сколько секунд апстрим исключён после размыкания.
    pub open_seconds: u64,
    /// Период фоновой проверки апстримов, с. 0 — не проверять.
    pub health_interval: u64,
}

impl Default for BreakerCfg {
    fn default() -> Self {
        Self { enabled: true, failure_threshold: 3, open_seconds: 30, health_interval: 15 }
    }
}

/// Маршрут исходящих запросов: интерфейс и прокси.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(default)]
pub struct RouteCfg {
    /// Имя интерфейса (`Ethernet`, `happ-tun`, `eth0`) или его IP. Пусто или `system` — системный маршрут.
    pub interface: String,
    /// `socks5://host:port`, `socks5h://…`, `http://host:port`. Пусто — как в системе
    /// (переменные `HTTP(S)_PROXY`), `none` — напрямую.
    pub proxy: String,
}

impl RouteCfg {
    /// Поле за полем: своё значение, иначе — из `base`.
    pub fn inherit(&self, base: &RouteCfg) -> RouteCfg {
        let pick = |own: &String, base: &String| if own.trim().is_empty() { base.clone() } else { own.clone() };
        RouteCfg { interface: pick(&self.interface, &base.interface), proxy: pick(&self.proxy, &base.proxy) }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct NetworkCfg {
    /// Запросы к моделям (у апстрима можно задать свой маршрут).
    pub upstream: RouteCfg,
    /// Колбэки боту.
    pub callback: RouteCfg,
    /// Telegram Bot API (протокол 1 и реле).
    pub telegram: RouteCfg,
}

/// Реле Telegram Bot API: адрес API принимает `/bot<TOKEN>/<метод>` и
/// `/file/bot<TOKEN>/<путь>` и пересылает их в Telegram — как воркер Cloudflare.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TgRelayCfg {
    pub enabled: bool,
    /// Куда пересылать. Пусто — `https://api.telegram.org`.
    pub upstream: String,
    /// id ботов (число до `:` в токене), которым можно пользоваться реле. Пусто — любой бот.
    pub allowed_bots: Vec<String>,
    /// Интерфейс или `system`. Пусто — как в `network.telegram`.
    pub interface: String,
    /// Прокси или `none`. Пусто — как в `network.telegram`.
    pub proxy: String,
    /// Ждать ответа Telegram, с. У `getUpdates` к нему прибавляется его `timeout`.
    pub timeout: u64,
    /// Предел тела запроса от бота, МБ (загрузка файлов — до 50 МБ).
    pub max_body_mb: u32,
    /// Кэш скачанных файлов в памяти, МБ. 0 — без кэша.
    pub file_cache_mb: u32,
}

impl Default for TgRelayCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            upstream: "https://api.telegram.org".into(),
            allowed_bots: Vec::new(),
            interface: String::new(),
            proxy: String::new(),
            timeout: 30,
            max_body_mb: 50,
            file_cache_mb: 32,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClientCfg {
    pub name: String,
    /// `sha256:<hex>` от ключа. Сам ключ показывается один раз при создании.
    pub key_hash: String,
    pub enabled: bool,
    pub created_at: i64,
    pub note: String,
}

impl Default for ClientCfg {
    fn default() -> Self {
        Self { name: String::new(), key_hash: String::new(), enabled: true, created_at: 0, note: String::new() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelCfg {
    pub upstreams: Vec<UpstreamCfg>,
    /// Подставляются, если бот их не прислал.
    pub defaults: Map<String, Value>,
    /// Перекрывают присланное.
    pub force: Map<String, Value>,
    /// Резервные модели: при ошибке, переполнении или разомкнутом выключателе.
    pub fallback: Vec<String>,
    /// Предел очереди этой модели (сверх — резерв или 429). Пусто — без предела.
    pub max_queue: Option<u32>,
    pub enabled: bool,
    pub description: String,
    /// Модель понимает картинки: `/v1/models` сообщает об этом клиентам (бот по этому
    /// признаку решает, отправлять ли фото).
    pub vision: bool,
}

impl Default for ModelCfg {
    fn default() -> Self {
        Self {
            upstreams: Vec::new(),
            defaults: Map::new(),
            force: Map::new(),
            fallback: Vec::new(),
            max_queue: None,
            enabled: true,
            description: String::new(),
            vision: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpstreamCfg {
    /// Подпись в UI и журнале. Пусто — `<модель>#<номер>`.
    pub name: String,
    /// Полный адрес `…/v1/chat/completions`. Может быть пустым, если указан `engine`.
    pub url: String,
    /// Имя, которое ждёт апстрим. Пусто — ключ модели.
    pub model: String,
    /// Ключ апстрима (секрет).
    pub api_key: String,
    /// Одновременных запросов (`--parallel` у llama.cpp). При `engine` берётся из движка.
    pub slots: u32,
    /// Ссылка на локальный движок из раздела `engines`.
    pub engine: String,
    /// Говорить с апстримом стримом и собирать ответ внутри.
    pub stream: bool,
    /// Свой таймаут, с. 0 — `UPSTREAM_TIMEOUT`.
    pub timeout: u64,
    /// Интерфейс (`Ethernet`, `happ-tun`, IP) или `system`. Пусто — как в `network.upstream`.
    pub interface: String,
    /// Прокси (`socks5://127.0.0.1:10808`, `http://…`) или `none`. Пусто — как в `network.upstream`.
    pub proxy: String,
    /// Параметры, которые апстрим не принимает (`mirostat`, `chat_template_kwargs`): убираются
    /// из запроса. Отклонённые апстримом параметры LMQ находит и убирает и сам.
    pub drop_params: Vec<String>,
}

impl Default for UpstreamCfg {
    fn default() -> Self {
        Self {
            name: String::new(),
            url: String::new(),
            model: String::new(),
            api_key: String::new(),
            slots: 1,
            engine: String::new(),
            stream: false,
            timeout: 0,
            interface: String::new(),
            proxy: String::new(),
            drop_params: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
    #[default]
    LlamaCpp,
    Vllm,
    Ollama,
    Koboldcpp,
    Custom,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EngineCfg {
    pub id: String,
    pub name: String,
    pub kind: EngineKind,
    /// Путь к исполняемому файлу (или имя из PATH).
    pub exe: String,
    /// Параметры формы для llama.cpp. Для других движков используются только host/port.
    pub llama: LlamaArgs,
    /// Дополнительные аргументы (для `custom` — все аргументы).
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: String,
    pub autostart: bool,
    pub restart_on_crash: bool,
    pub max_restarts_per_hour: u32,
    /// Адрес проверки готовности. Пусто — по типу движка (`/health`, `/v1/models`, `/api/tags`).
    pub health_url: String,
    /// Сколько ждать готовности после запуска, с.
    pub ready_timeout: u64,
}

impl Default for EngineCfg {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            kind: EngineKind::LlamaCpp,
            exe: String::new(),
            llama: LlamaArgs::default(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: String::new(),
            autostart: false,
            restart_on_crash: true,
            max_restarts_per_hour: 5,
            health_url: String::new(),
            ready_timeout: 600,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LlamaArgs {
    /// `-m`
    pub model: String,
    pub host: String,
    pub port: u16,
    /// `--ctx-size`
    pub ctx_size: u32,
    /// `--parallel` (= слоты апстрима)
    pub parallel: u32,
    /// `-ngl`; -1 — не передавать.
    pub n_gpu_layers: i32,
    /// `--threads`; 0 — не передавать.
    pub threads: u32,
    /// `--chat-template`
    pub chat_template: String,
    /// `--flash-attn`
    pub flash_attn: bool,
    /// `--jinja`
    pub jinja: bool,
    /// `--alias` (имя модели, которое отдаёт сервер)
    pub alias: String,
}

impl Default for LlamaArgs {
    fn default() -> Self {
        Self {
            model: String::new(),
            host: "127.0.0.1".into(),
            port: 8080,
            ctx_size: 8192,
            parallel: 1,
            n_gpu_layers: 99,
            threads: 0,
            chat_template: String::new(),
            flash_attn: false,
            jinja: true,
            alias: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleAction {
    /// Не принимать новые задания: 503 + Retry-After до конца окна.
    PauseIntake,
    /// Принимать, но не выдавать модели (задания ждут, MAX_WAIT действует).
    #[default]
    PauseDispatch,
    /// Ограничить одновременные задания модели числом `slots`.
    LimitSlots,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ScheduleRule {
    pub name: String,
    pub enabled: bool,
    /// Дни недели: mon … sun. Пусто — каждый день.
    pub days: Vec<String>,
    /// Начало окна `HH:MM` (местное время).
    pub from: String,
    /// Конец окна `HH:MM`; если меньше начала — окно через полночь.
    pub to: String,
    pub action: ScheduleAction,
    /// Модели, к которым относится правило. Пусто — ко всем.
    pub models: Vec<String>,
    pub slots: u32,
}

impl Default for ScheduleRule {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            days: Vec::new(),
            from: "00:00".into(),
            to: "06:00".into(),
            action: ScheduleAction::PauseDispatch,
            models: Vec::new(),
            slots: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiMode {
    #[default]
    Native,
    Web,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Backdrop {
    #[default]
    Mica,
    Acrylic,
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiCfg {
    pub mode: UiMode,
    /// Тема веб-панели (`data-skin`).
    pub skin: String,
    pub scheme: ColorScheme,
    /// Акцент `#rrggbb`. Пусто — акцент темы; `system` — акцент Windows.
    pub accent: String,
    pub solid_bars: bool,
    pub backdrop: Backdrop,
    /// Непрозрачность фона темы поверх размытия, 0.0–1.0.
    pub backdrop_opacity: f32,
    /// `auto` | `ru` | `en`.
    pub language: String,
    pub window: WindowState,
}

impl Default for UiCfg {
    fn default() -> Self {
        Self {
            mode: UiMode::Native,
            skin: "fluent".into(),
            scheme: ColorScheme::System,
            accent: "system".into(),
            solid_bars: false,
            backdrop: Backdrop::Mica,
            backdrop_opacity: 0.55,
            language: "auto".into(),
            window: WindowState::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct WindowState {
    pub width: u32,
    pub height: u32,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub maximized: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppCfg {
    pub autostart: bool,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub notifications: bool,
    /// Сразу выдавать задания моделям; `false` — приложение стартует с выдачей на паузе.
    pub server_autostart: bool,
}

impl Default for AppCfg {
    fn default() -> Self {
        Self { autostart: false, start_minimized: false, close_to_tray: true, notifications: true, server_autostart: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdatesCfg {
    pub auto_check: bool,
    /// `stable` | `beta`.
    pub channel: String,
    /// Репозиторий GitHub `owner/name`. Пусто — проверка выключена.
    pub repo: String,
    /// Публичный ключ minisign для проверки подписи загрузок. Пусто — без установки, только ссылка.
    pub pubkey: String,
}

impl Default for UpdatesCfg {
    fn default() -> Self {
        Self { auto_check: true, channel: "stable".into(), repo: String::new(), pubkey: String::new() }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DebugReply {
    #[default]
    Ok,
    Forbidden,
    ServerError,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DebugCfg {
    /// Как отвечает встроенный приёмник колбэков.
    pub reply: DebugReply,
    /// Задержка ответа приёмника, мс.
    pub delay_ms: u64,
}

impl Default for DebugCfg {
    fn default() -> Self {
        Self { reply: DebugReply::Ok, delay_ms: 0 }
    }
}
