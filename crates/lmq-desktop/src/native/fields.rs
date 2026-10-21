//! Поля экрана «Настройки» — те же разделы и подписи, что в веб-панели
//! (`web/js/screens/settings.js`). Правки копятся в черновике и сохраняются кнопкой.

use serde_json::{Value, json};
use slint::SharedString;

use super::FieldRow;
use super::i18n::t;
use super::view::model;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Header,
    Note,
    Text,
    Number,
    Bool,
    Secret,
    List,
    Select(&'static [&'static str]),
}

pub struct Def {
    pub kind: Kind,
    pub path: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}

const fn f(kind: Kind, path: &'static str, label: &'static str, hint: &'static str) -> Def {
    Def { kind, path, label, hint }
}

const fn head(label: &'static str) -> Def {
    Def { kind: Kind::Header, path: "", label, hint: "" }
}

const fn note(label: &'static str) -> Def {
    Def { kind: Kind::Note, path: "", label, hint: "" }
}

const LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace"];

pub static DEFS: &[Def] = &[
    head("Сервер"),
    note("Смена адреса или лимитов перезапускает слушатель внутри процесса."),
    f(Kind::Text, "server.listen", "Адрес API", "Наружу — только через обратный прокси с HTTPS."),
    f(Kind::Text, "server.admin_listen", "Адрес панели", "Пусто — панель выключена."),
    f(Kind::Number, "server.max_body_mb", "Предел тела, МБ", ""),
    f(Kind::Number, "server.max_messages", "Сообщений в задании", ""),
    f(Kind::Number, "server.max_string_kb", "Длина текста, КБ", "Картинки base64 не считаются."),
    f(Kind::Number, "server.body_read_timeout", "Чтение тела, с", "Защита от медленных клиентов."),
    head("Очередь и лимиты"),
    f(Kind::Number, "queue.max_queue", "Всего в очереди", ""),
    f(Kind::Number, "queue.max_queue_per_chat", "На один чат", "Сверх — 429 с Retry-After."),
    f(Kind::Number, "queue.max_wait", "Ждать не дольше, с", "Не начатое за это время задание — expired, бот промолчит."),
    f(Kind::Number, "queue.upstream_timeout", "Ответ модели, с", ""),
    f(Kind::Number, "queue.sync_timeout", "Синхронный режим, с", "«Проверить» и «Песочница» в боте."),
    f(Kind::Number, "queue.upstream_retries", "Повторов при ошибке", ""),
    f(Kind::Bool, "queue.supersede_same_chat", "Вытеснять старое задание чата", "Не начатое задание чата завершается superseded, если пришло новое."),
    f(Kind::Bool, "queue.private_first", "Личные чаты раньше групп", ""),
    head("Доставка боту"),
    note("callback.url каждого задания проверяется по белому списку хостов (защита от SSRF)."),
    f(Kind::Text, "callback.url", "Адрес колбэка", "Если задание пришло без callback."),
    f(Kind::Secret, "callback.token", "Токен колбэка", "Заголовок X-Callback-Token."),
    f(Kind::List, "callback.allowed_hosts", "Разрешённые хосты", "Пусто — только хост адреса колбэка."),
    f(Kind::Bool, "callback.allow_http_private", "http:// для localhost и частных сетей", ""),
    f(Kind::Number, "callback.retries", "Повторов доставки", "Паузы 2, 5, 15, 30, 60 с."),
    f(Kind::Number, "callback.timeout", "Таймаут, с", ""),
    head("Хранение и журнал"),
    f(Kind::Text, "storage.db_path", "База данных", "Меняется после перезапуска."),
    f(Kind::Text, "storage.models_config", "Внешний файл моделей", "Формат §5.4. Пусто — модели из настроек."),
    f(Kind::Number, "storage.retention_hours", "Хранить задания, ч", ""),
    f(Kind::Number, "storage.stats_retention_days", "Хранить статистику, дн", ""),
    f(Kind::Select(LEVELS), "logging.level", "Уровень журнала", ""),
    f(Kind::Bool, "logging.content", "Писать тексты сообщений", "Персональные данные. Оставьте выключенным."),
    f(Kind::Number, "logging.keep_days", "Файлы журнала, дн", ""),
    f(Kind::Bool, "metrics.enabled", "Метрики Prometheus", "GET /metrics на адресе API."),
    f(Kind::Secret, "metrics.token", "Токен метрик", "Пусто — /metrics только с localhost."),
    head("Выключатель апстримов"),
    note("Упавший апстрим временно исключается; фоновая проверка возвращает его, когда он ожил."),
    f(Kind::Bool, "breaker.enabled", "Включён", ""),
    f(Kind::Number, "breaker.failure_threshold", "Неудач подряд", ""),
    f(Kind::Number, "breaker.open_seconds", "Исключать на, с", ""),
    f(Kind::Number, "breaker.health_interval", "Проверка раз в, с", "GET {base}/models. 0 — не проверять."),
    head("Протокол 1 (Telegram)"),
    note("Старый режим: сервер сам отправляет ответ в Telegram. Теряются форматирование, темы форума и память — включайте, только если без него не обойтись."),
    f(Kind::Bool, "protocol1.enabled", "Разрешить", ""),
    f(Kind::Secret, "protocol1.telegram_bot_token", "Токен бота", ""),
    f(Kind::Text, "protocol1.telegram_api", "Адрес Bot API", ""),
];

pub fn get<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |o, k| o.get(k))
}

fn set(v: &mut Value, path: &str, val: Value) {
    let mut cur = v;
    let keys: Vec<&str> = path.split('.').collect();
    for k in &keys[..keys.len() - 1] {
        if !cur.get(*k).is_some_and(Value::is_object) {
            cur[*k] = json!({});
        }
        cur = &mut cur[*k];
    }
    cur[keys[keys.len() - 1]] = val;
}

fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(", "),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Строки для окна из черновика и списка путей, заданных окружением.
pub fn rows(draft: &Value, locked: &[String]) -> Vec<FieldRow> {
    DEFS.iter()
        .map(|d| {
            let v = get(draft, d.path);
            let (kind, options, index) = match d.kind {
                Kind::Header => ("header", Vec::new(), 0),
                Kind::Note => ("note", Vec::new(), 0),
                Kind::Text | Kind::List => ("text", Vec::new(), 0),
                Kind::Number => ("number", Vec::new(), 0),
                Kind::Secret => ("secret", Vec::new(), 0),
                Kind::Bool => ("bool", Vec::new(), 0),
                Kind::Select(opts) => {
                    let cur = text_of(v);
                    ("select", opts.iter().map(|o| SharedString::from(*o)).collect(), opts.iter().position(|o| *o == cur).unwrap_or(0) as i32)
                }
            };
            FieldRow {
                kind: kind.into(),
                path: d.path.into(),
                label: t(d.label).into(),
                hint: if d.hint.is_empty() { SharedString::new() } else { t(d.hint).into() },
                value: text_of(v).into(),
                checked: v.and_then(Value::as_bool).unwrap_or(false),
                locked: !d.path.is_empty() && locked.iter().any(|l| l == d.path),
                options: model(options),
                index,
            }
        })
        .collect()
}

/// Правка текста/числа/списка. `Err` — значение не число (в черновик не пишется).
pub fn edit_text(draft: &mut Value, i: usize, text: &str) -> Result<(), String> {
    let Some(d) = DEFS.get(i) else { return Ok(()) };
    let val = match d.kind {
        Kind::Number => {
            let s = text.trim().replace(',', ".");
            if let Ok(n) = s.parse::<u64>() {
                json!(n)
            } else if let Ok(f) = s.parse::<f64>() {
                json!(f)
            } else {
                return Err(t("Нужно число"));
            }
        }
        Kind::List => Value::Array(text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| json!(s)).collect()),
        Kind::Text | Kind::Secret => json!(text),
        _ => return Ok(()),
    };
    set(draft, d.path, val);
    Ok(())
}

pub fn edit_bool(draft: &mut Value, i: usize, on: bool) {
    if let Some(d) = DEFS.get(i) {
        set(draft, d.path, json!(on));
    }
}

pub fn edit_select(draft: &mut Value, i: usize, idx: usize) {
    if let Some(Def { kind: Kind::Select(opts), path, .. }) = DEFS.get(i)
        && let Some(o) = opts.get(idx)
    {
        set(draft, path, json!(o));
    }
}
