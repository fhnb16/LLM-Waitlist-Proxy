//! Поля экрана «Настройки» — те же разделы и подписи, что в веб-панели
//! (`web/js/screens/settings.js`). Правки копятся в черновике и сохраняются кнопкой.

use serde_json::{Value, json};
use slint::SharedString;

use super::FieldRow;
use super::i18n::t;
use super::view::model;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Plain,
    Secs,
    Hours,
}

/// Целое число: диапазон, готовые значения для списка, единица для подписи,
/// подпись нуля («не проверять»).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Num {
    pub min: i64,
    pub max: i64,
    pub presets: &'static [i64],
    pub unit: Unit,
    pub zero: &'static str,
}

const fn num(min: i64, max: i64, presets: &'static [i64], unit: Unit) -> Kind {
    Kind::Number(Num { min, max, presets, unit, zero: "" })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Header,
    Note,
    Text,
    Number(Num),
    Bool,
    Secret,
    List,
    Select(&'static [&'static str]),
    /// Интерфейс: список интерфейсов системы и ручной ввод.
    Iface,
    /// Прокси: «как в системе», «напрямую», найденные локальные прокси и ручной ввод.
    Proxy,
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

/// Служебный «путь» заметки, в которую подставляется список интерфейсов системы.
const IFACES: &str = "@interfaces";

pub static DEFS: &[Def] = &[
    head("Сервер"),
    note("Смена адреса или лимитов перезапускает слушатель внутри процесса."),
    f(Kind::Text, "server.listen", "Адрес API", "Наружу — только через обратный прокси с HTTPS."),
    f(Kind::Text, "server.admin_listen", "Адрес панели", "Пусто — панель выключена."),
    f(num(1, 512, &[4, 8, 16, 32, 64], Unit::Plain), "server.max_body_mb", "Предел тела, МБ", ""),
    f(num(1, 100_000, &[100, 200, 500, 1000], Unit::Plain), "server.max_messages", "Сообщений в задании", ""),
    f(num(1, 1_048_576, &[128, 256, 512, 1024, 2048], Unit::Plain), "server.max_string_kb", "Длина текста, КБ", "Картинки base64 не считаются."),
    f(num(1, 3600, &[10, 30, 60, 120], Unit::Secs), "server.body_read_timeout", "Чтение тела, с", "Защита от медленных клиентов."),
    head("Очередь и лимиты"),
    f(num(1, 1_000_000, &[50, 100, 200, 500, 1000], Unit::Plain), "queue.max_queue", "Всего в очереди", ""),
    f(num(1, 10_000, &[1, 2, 3, 5, 10], Unit::Plain), "queue.max_queue_per_chat", "На один чат", "Сверх — 429 с Retry-After."),
    f(
        num(1, 86_400, &[60, 120, 300, 600, 1800], Unit::Secs),
        "queue.max_wait",
        "Ждать не дольше, с",
        "Не начатое за это время задание — expired, бот промолчит.",
    ),
    f(num(1, 86_400, &[30, 60, 120, 300, 600], Unit::Secs), "queue.upstream_timeout", "Ответ модели, с", ""),
    f(num(1, 3600, &[15, 30, 60, 120], Unit::Secs), "queue.sync_timeout", "Синхронный режим, с", "«Проверить» и «Песочница» в боте."),
    f(num(0, 10, &[0, 1, 2, 3, 5], Unit::Plain), "queue.upstream_retries", "Повторов при ошибке", ""),
    f(Kind::Bool, "queue.supersede_same_chat", "Вытеснять старое задание чата", "Не начатое задание чата завершается superseded, если пришло новое."),
    f(Kind::Bool, "queue.private_first", "Личные чаты раньше групп", ""),
    head("Доставка боту"),
    note("callback.url каждого задания проверяется по белому списку хостов (защита от SSRF)."),
    f(Kind::Text, "callback.url", "Адрес колбэка", "Если задание пришло без callback."),
    f(Kind::Secret, "callback.token", "Токен колбэка", "Заголовок X-Callback-Token."),
    f(Kind::List, "callback.allowed_hosts", "Разрешённые хосты", "Пусто — только хост адреса колбэка."),
    f(Kind::Bool, "callback.allow_http_private", "http:// для localhost и частных сетей", ""),
    f(num(0, 50, &[0, 3, 6, 10, 20], Unit::Plain), "callback.retries", "Повторов доставки", "Паузы 2, 5, 15, 30, 60 с."),
    f(num(1, 120, &[5, 10, 15, 30, 60], Unit::Secs), "callback.timeout", "Таймаут, с", ""),
    head("Хранение и журнал"),
    f(Kind::Text, "storage.db_path", "База данных", "Меняется после перезапуска."),
    f(Kind::Text, "storage.models_config", "Внешний файл моделей", "Формат §5.4. Пусто — модели из настроек."),
    f(num(1, 87_600, &[6, 24, 72, 168, 720], Unit::Hours), "storage.retention_hours", "Хранить задания, ч", ""),
    f(num(1, 3650, &[7, 14, 30, 90, 365], Unit::Plain), "storage.stats_retention_days", "Хранить статистику, дн", ""),
    f(Kind::Select(LEVELS), "logging.level", "Уровень журнала", ""),
    f(Kind::Bool, "logging.content", "Писать тексты сообщений", "Персональные данные. Оставьте выключенным."),
    f(num(1, 3650, &[3, 7, 14, 30, 90], Unit::Plain), "logging.keep_days", "Файлы журнала, дн", ""),
    f(
        Kind::Bool,
        "logging.requests",
        "Подробный журнал запросов",
        "Строение запроса бота, что ушло модели, ответы апстрима и бота. Тексты — только вместе с «Писать тексты».",
    ),
    f(Kind::Bool, "metrics.enabled", "Метрики Prometheus", "GET /metrics на адресе API."),
    f(Kind::Secret, "metrics.token", "Токен метрик", "Пусто — /metrics только с localhost."),
    head("Сеть и маршруты"),
    note(
        "Через какой интерфейс или прокси ходит каждое направление. Например: модели — через VPN (happ-tun или socks5-прокси VPN-клиента), бот — напрямую (Ethernet). Пусто — системный маршрут.",
    ),
    Def { kind: Kind::Note, path: IFACES, label: "Интерфейсы", hint: "" },
    f(Kind::Iface, "network.upstream.interface", "Модели: интерфейс", "Имя (Ethernet, happ-tun, eth0) или IP. У апстрима можно задать свой."),
    f(Kind::Proxy, "network.upstream.proxy", "Модели: прокси", "socks5://, socks5h://, http://. none — напрямую."),
    f(Kind::Iface, "network.callback.interface", "Бот: интерфейс", "Колбэки на адрес бота."),
    f(Kind::Proxy, "network.callback.proxy", "Бот: прокси", ""),
    f(Kind::Iface, "network.telegram.interface", "Telegram: интерфейс", "Только для протокола 1."),
    f(Kind::Proxy, "network.telegram.proxy", "Telegram: прокси", ""),
    head("Выключатель апстримов"),
    note("Упавший апстрим временно исключается; фоновая проверка возвращает его, когда он ожил."),
    f(Kind::Bool, "breaker.enabled", "Включён", ""),
    f(num(1, 100, &[1, 2, 3, 5, 10], Unit::Plain), "breaker.failure_threshold", "Неудач подряд", ""),
    f(num(1, 86_400, &[10, 30, 60, 120, 300], Unit::Secs), "breaker.open_seconds", "Исключать на, с", ""),
    f(
        Kind::Number(Num { min: 0, max: 86_400, presets: &[0, 5, 15, 30, 60], unit: Unit::Secs, zero: "не проверять" }),
        "breaker.health_interval",
        "Проверка раз в, с",
        "GET {base}/models. 0 — не проверять.",
    ),
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

/// Подпись готового значения: `300 — 5 мин`, `72 — 3 дн`, `0 — не проверять`.
fn preset_label(n: i64, spec: &Num) -> String {
    if n == 0 && !spec.zero.is_empty() {
        return format!("0 — {}", t(spec.zero));
    }
    let sub = match spec.unit {
        Unit::Secs if n > 0 => Some(super::view::fmt::ms(Some(n as f64 * 1000.0))),
        Unit::Hours if n >= 48 && n % 24 == 0 => Some(format!("{} {}", n / 24, t("дн"))),
        _ => None,
    };
    match sub {
        Some(s) => format!("{n} — {s}"),
        None => n.to_string(),
    }
}

type Options = (Vec<SharedString>, Vec<SharedString>, Vec<SharedString>);

fn iface_options() -> Options {
    let mut o: Options = (vec!["".into()], vec![t("Системный маршрут").into()], vec!["".into()]);
    for i in lmq_core::net::interfaces() {
        o.0.push(i.name.clone().into());
        o.1.push("".into());
        o.2.push(i.ipv4.iter().chain(i.ipv6.iter().take(1)).map(|a| a.to_string()).collect::<Vec<_>>().join(", ").into());
    }
    o
}

fn proxy_options(found: &[lmq_core::net::ProxyHint]) -> Options {
    let mut o: Options = (vec!["".into(), "none".into()], vec![t("как в системе").into(), "none".into()], vec!["HTTP(S)_PROXY".into(), t("напрямую").into()]);
    for p in found {
        o.0.push(p.url.clone().into());
        o.1.push("".into());
        o.2.push(super::i18n::tf("найден: {what}", &[("what", &p.label)]).into());
    }
    o
}

/// Строки для окна из черновика, путей, заданных окружением, и найденных локальных прокси.
pub fn rows(draft: &Value, locked: &[String], proxies: &[lmq_core::net::ProxyHint]) -> Vec<FieldRow> {
    let ifaces = iface_options();
    let proxy = proxy_options(proxies);
    DEFS.iter()
        .map(|d| {
            let v = get(draft, d.path);
            let mut options: Options = Default::default();
            let mut index = 0;
            let (mut minimum, mut maximum) = (-1e15_f32, 1e15_f32);
            let mut placeholder = String::new();
            let kind = match d.kind {
                Kind::Header => "header",
                Kind::Note => "note",
                Kind::Text | Kind::List => "text",
                Kind::Secret => "secret",
                Kind::Bool => "bool",
                Kind::Number(spec) => {
                    minimum = spec.min as f32;
                    maximum = spec.max as f32;
                    options.0 = spec.presets.iter().map(|n| n.to_string().into()).collect();
                    options.1 = spec.presets.iter().map(|n| preset_label(*n, &spec).into()).collect();
                    options.2 = spec.presets.iter().map(|_| SharedString::new()).collect();
                    "number"
                }
                Kind::Iface => {
                    options = ifaces.clone();
                    placeholder = t("системный маршрут");
                    "combo"
                }
                Kind::Proxy => {
                    options = proxy.clone();
                    placeholder = t("как в системе");
                    "combo"
                }
                Kind::Select(opts) => {
                    let cur = text_of(v);
                    options.0 = opts.iter().map(|o| SharedString::from(*o)).collect();
                    index = opts.iter().position(|o| *o == cur).unwrap_or(0) as i32;
                    "select"
                }
            };
            let label = if d.path == IFACES {
                let list: Vec<String> = lmq_core::net::interfaces()
                    .into_iter()
                    .map(|i| format!("{} ({})", i.name, i.ipv4.iter().chain(i.ipv6.iter().take(1)).map(|a| a.to_string()).collect::<Vec<_>>().join(", ")))
                    .collect();
                format!("{}: {}", t(d.label), list.join(" · "))
            } else {
                t(d.label)
            };
            FieldRow {
                kind: kind.into(),
                path: if d.path == IFACES { "".into() } else { d.path.into() },
                label: label.into(),
                hint: if d.hint.is_empty() { SharedString::new() } else { t(d.hint).into() },
                value: text_of(v).into(),
                checked: v.and_then(Value::as_bool).unwrap_or(false),
                locked: !d.path.is_empty() && locked.iter().any(|l| l == d.path),
                options: model(options.0),
                labels: model(options.1),
                hints: model(options.2),
                index,
                minimum,
                maximum,
                placeholder: placeholder.into(),
            }
        })
        .collect()
}

/// Правка текста/числа/списка. `Err` — значение не подходит (в черновик не пишется).
pub fn edit_text(draft: &mut Value, i: usize, text: &str) -> Result<(), String> {
    let Some(d) = DEFS.get(i) else { return Ok(()) };
    let val = match d.kind {
        Kind::Number(spec) => {
            let n: i64 = text.trim().parse().map_err(|_| t("Нужно целое число"))?;
            if n < spec.min {
                return Err(super::i18n::tf("Не меньше {n}", &[("n", &spec.min)]));
            }
            if n > spec.max {
                return Err(super::i18n::tf("Не больше {n}", &[("n", &spec.max)]));
            }
            json!(n)
        }
        Kind::List => Value::Array(text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(|s| json!(s)).collect()),
        Kind::Text | Kind::Secret => json!(text),
        Kind::Iface | Kind::Proxy => json!(text.trim()),
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
