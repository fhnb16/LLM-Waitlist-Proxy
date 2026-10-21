//! Журнал: JSON-строки в файл с ротацией, вывод в консоль, кольцевой буфер для панели.
//!
//! Тексты сообщений и ответов в журнал не попадают, пока не включён `LOG_CONTENT`;
//! секреты не пишутся никогда (код просто не передаёт их в поля событий).

use std::collections::VecDeque;
use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event as TEvent, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, reload};

const RING_CAP: usize = 3000;

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    pub ts: i64,
    pub level: String,
    pub target: String,
    pub message: String,
    pub fields: Map<String, Value>,
}

pub struct LogHub {
    ring: Mutex<VecDeque<LogEntry>>,
    tx: broadcast::Sender<LogEntry>,
}

impl LogHub {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(2048);
        Self { ring: Mutex::new(VecDeque::with_capacity(RING_CAP)), tx }
    }

    fn push(&self, e: LogEntry) {
        {
            let mut r = self.ring.lock();
            if r.len() >= RING_CAP {
                r.pop_front();
            }
            r.push_back(e.clone());
        }
        let _ = self.tx.send(e);
    }

    /// Последние записи (новые в конце), с фильтром по уровню и подстроке.
    pub fn tail(&self, limit: usize, min_level: Option<&str>, query: Option<&str>) -> Vec<LogEntry> {
        let min = min_level.map(level_rank).unwrap_or(0);
        let q = query.map(|q| q.to_lowercase()).filter(|q| !q.is_empty());
        let r = self.ring.lock();
        let mut out: Vec<LogEntry> = r
            .iter()
            .rev()
            .filter(|e| level_rank(&e.level) >= min)
            .filter(|e| match &q {
                None => true,
                Some(q) => e.message.to_lowercase().contains(q) || Value::Object(e.fields.clone()).to_string().to_lowercase().contains(q),
            })
            .take(limit)
            .cloned()
            .collect();
        out.reverse();
        out
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.tx.subscribe()
    }
}

pub fn level_rank(l: &str) -> u8 {
    match l.to_ascii_lowercase().as_str() {
        "trace" => 0,
        "debug" => 1,
        "info" => 2,
        "warn" | "warning" => 3,
        "error" => 4,
        _ => 0,
    }
}

static HUB: OnceLock<LogHub> = OnceLock::new();
static RELOAD: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
static GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

pub fn hub() -> &'static LogHub {
    HUB.get_or_init(LogHub::new)
}

struct HubLayer;

#[derive(Default)]
struct FieldCollector {
    message: String,
    fields: Map<String, Value>,
}

impl Visit for FieldCollector {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.insert(field.name().into(), Value::String(value.into()));
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.insert(field.name().into(), Value::from(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.insert(field.name().into(), Value::from(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields.insert(field.name().into(), Value::from(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields.insert(field.name().into(), Value::from(value));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let s = format!("{value:?}");
        if field.name() == "message" {
            self.message = s;
        } else {
            self.fields.insert(field.name().into(), Value::String(s));
        }
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for HubLayer {
    fn on_event(&self, event: &TEvent<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        // Шум зависимостей в панель не пускаем.
        let target = meta.target();
        if !target.starts_with("lmq") && *meta.level() > tracing::Level::WARN {
            return;
        }
        let mut c = FieldCollector::default();
        event.record(&mut c);
        hub().push(LogEntry {
            ts: crate::util::now_ms(),
            level: meta.level().as_str().to_ascii_lowercase(),
            target: target.to_string(),
            message: c.message,
            fields: c.fields,
        });
    }
}

fn filter_for(level: &str) -> EnvFilter {
    let level = match level.to_ascii_lowercase().as_str() {
        l @ ("error" | "warn" | "info" | "debug" | "trace") => l.to_string(),
        _ => "info".to_string(),
    };
    // Собственные модули — на выбранном уровне, зависимости — не подробнее warn.
    EnvFilter::new(format!("warn,lmq_core={level},lmq_server={level},lmq_desktop={level},lmq={level}"))
}

/// Подключает журнал один раз на процесс. Повторный вызов только меняет уровень.
pub fn init(log_dir: Option<&Path>, level: &str, keep_days: u32, console: bool) {
    if RELOAD.get().is_some() {
        set_level(level);
        return;
    }
    let (filter, handle) = reload::Layer::new(filter_for(level));
    let file_layer = log_dir.and_then(|dir| {
        std::fs::create_dir_all(dir).ok()?;
        let appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("lmq")
            .filename_suffix("log")
            .max_log_files(keep_days.max(1) as usize)
            .build(dir)
            .ok()?;
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let _ = GUARD.set(guard);
        Some(tracing_subscriber::fmt::layer().json().flatten_event(true).with_current_span(false).with_span_list(false).with_writer(writer))
    });
    let console_layer = console.then(|| {
        let json = std::env::var("LOG_FORMAT").map(|v| v == "json").unwrap_or(false);
        if json {
            tracing_subscriber::fmt::layer().json().flatten_event(true).boxed()
        } else {
            tracing_subscriber::fmt::layer().with_target(false).compact().boxed()
        }
    });
    let ok = tracing_subscriber::registry().with(filter).with(HubLayer).with(file_layer).with(console_layer).try_init().is_ok();
    if ok {
        let _ = RELOAD.set(handle);
    }
}

pub fn set_level(level: &str) {
    if let Some(h) = RELOAD.get() {
        let _ = h.modify(|f| *f = filter_for(level));
    }
}
