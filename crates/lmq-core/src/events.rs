//! Шина событий для живого UI (SSE в веб-панели, подписка в нативном окне).

use serde::Serialize;
use tokio::sync::broadcast;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Задание сменило состояние.
    Job { job_id: String, state: String, model: String, chat_id: i64, error_code: Option<String> },
    /// Запись журнала сервера.
    Log(crate::logging::LogEntry),
    /// Движок сменил состояние.
    Engine { id: String, status: String },
    /// Строка вывода движка.
    EngineLog { id: String, line: crate::engines::EngineLine },
    /// Апстрим сменил здоровье.
    Upstream { id: String, model: String, state: String, error: Option<String> },
    /// Приёмник колбэков получил запрос.
    DebugCallback(crate::debug::Received),
    /// Конфигурация изменилась (панель перечитывает настройки).
    Config { profile: String },
    /// Обновление: `available` — вышла новая версия, `ready` — установщик скачан и проверен.
    Update { state: String, version: String },
    /// Сводка раз в пару секунд (для KPI-плиток и трея).
    Tick(Box<crate::admin::Overview>),
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl Default for EventBus {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(4096);
        Self { tx }
    }
}

impl EventBus {
    pub fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn has_listeners(&self) -> bool {
        self.tx.receiver_count() > 0
    }
}
