//! Преобразование данных ядра в структуры окна: KPI, графики, строки списков.
//! Все функции вызываются в потоке интерфейса (ModelRc не Send).

use std::collections::HashMap;
use std::fmt::Write as _;

use chrono::{Local, TimeZone};
use lmq_core::admin::Overview;
use lmq_core::config::Settings;
use lmq_core::engines::{EngineInfo, EngineLine, EngineStatus};
use lmq_core::logging::LogEntry;
use lmq_core::routing::Breaker;
use lmq_core::stats::{Point, Series};
use lmq_core::store::JobRow as CoreJob;
use slint::{ModelRc, SharedString, VecModel};

use super::i18n::{is_en, t, tf, tn};
use super::{BannerRow, Chart, ClientRow, EngineRow, GpuRow, JobRow, Kpi, LogLine, ModelRow, RunRow, UpstreamRow};

pub fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn ss(s: impl AsRef<str>) -> SharedString {
    SharedString::from(s.as_ref())
}

// ------------------------------------------------------------------ формат

pub mod fmt {
    use super::{is_en, t};

    fn dec(v: f64, digits: usize) -> String {
        let s = format!("{v:.digits$}");
        let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
        if is_en() { s } else { s.replace('.', ",") }
    }

    pub fn num(v: Option<f64>, digits: usize) -> String {
        match v {
            Some(v) if v.is_finite() => {
                let r = dec(v, digits);
                // Разряды: 12 345 / 12,345.
                let (int, frac) = match r.find([',', '.']) {
                    Some(i) => (&r[..i], &r[i..]),
                    None => (r.as_str(), ""),
                };
                let neg = int.starts_with('-');
                let digits_only = int.trim_start_matches('-');
                let mut out = String::new();
                for (i, ch) in digits_only.chars().enumerate() {
                    if i > 0 && (digits_only.len() - i) % 3 == 0 {
                        out.push(if is_en() { ',' } else { '\u{a0}' });
                    }
                    out.push(ch);
                }
                format!("{}{out}{frac}", if neg { "-" } else { "" })
            }
            _ => "—".into(),
        }
    }

    pub fn int(v: impl Into<f64>) -> String {
        num(Some(v.into()), 0)
    }

    pub fn ms(v: Option<f64>) -> String {
        let Some(v) = v.filter(|v| v.is_finite()) else { return "—".into() };
        if v > 0.0 && v < 10.0 {
            return format!("{} {}", dec(v, 1), t("мс"));
        }
        if v < 1000.0 {
            return format!("{} {}", v.round(), t("мс"));
        }
        if v < 60_000.0 {
            return format!("{} {}", dec(v / 1000.0, if v < 10_000.0 { 1 } else { 0 }), t("с"));
        }
        let m = (v / 60_000.0).floor();
        let s = ((v % 60_000.0) / 1000.0).round();
        if s > 0.0 { format!("{m} {} {s} {}", t("мин"), t("с")) } else { format!("{m} {}", t("мин")) }
    }

    pub fn pct(v: Option<f64>) -> String {
        match v {
            Some(v) => format!("{}%", dec(v * 100.0, if v < 0.1 { 1 } else { 0 })),
            None => "—".into(),
        }
    }

    pub fn mb(v: u64) -> String {
        if v >= 1024 { format!("{} {}", dec(v as f64 / 1024.0, 1), t("ГБ")) } else { format!("{v} {}", t("МБ")) }
    }

    pub fn uptime(s: i64) -> String {
        let d = s / 86_400;
        let h = (s % 86_400) / 3600;
        let m = (s % 3600) / 60;
        if d > 0 {
            format!("{d} {} {h} {}", t("д"), t("ч"))
        } else if h > 0 {
            format!("{h} {} {m} {}", t("ч"), t("мин"))
        } else {
            format!("{m} {}", t("мин"))
        }
    }
}

pub fn time(ts: i64, pattern: &str) -> String {
    Local.timestamp_millis_opt(ts).single().map(|d| d.format(pattern).to_string()).unwrap_or_default()
}

pub fn ago(ts: i64, now: i64) -> String {
    let d = now - ts;
    if d < 5000 {
        t("только что")
    } else if d < 60_000 {
        tf("{n} с назад", &[("n", &(d / 1000))])
    } else if d < 3_600_000 {
        tf("{n} мин назад", &[("n", &(d / 60_000))])
    } else if d < 86_400_000 {
        tf("{n} ч назад", &[("n", &(d / 3_600_000))])
    } else {
        time(ts, "%d.%m %H:%M")
    }
}

// ----------------------------------------------------------------- графики

/// Округлённый вверх максимум шкалы; для целых — чётный, чтобы середина была целой.
fn nice_max(raw: f64, integer: bool) -> f64 {
    // NaN и неположительные значения — шкала по умолчанию.
    if raw.is_nan() || raw <= 0.0 {
        return if integer { 2.0 } else { 1.0 };
    }
    let mag = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 2.5, 4.0, 5.0, 6.0, 8.0, 10.0] {
        let v = m * mag;
        if v + 1e-9 >= raw && (!integer || ((v / 2.0).fract() == 0.0 && v >= 2.0)) {
            return v;
        }
    }
    10.0 * mag
}

fn xpos(i: usize, n: usize) -> f64 {
    (i as f64 + 0.5) / n.max(1) as f64 * 1000.0
}

fn ypos(v: f64, max: f64) -> f64 {
    100.0 - (v / max).clamp(0.0, 1.0) * 100.0
}

fn line_path(vals: &[Option<f64>], max: f64) -> String {
    let n = vals.len();
    let mut s = String::new();
    let mut pen = false;
    for (i, v) in vals.iter().enumerate() {
        match v {
            Some(v) => {
                let (x, y) = (xpos(i, n), ypos(*v, max));
                let next = vals.get(i + 1).is_some_and(Option::is_some);
                if !pen && !next {
                    // Одиночная точка: короткий штрих, иначе её не видно.
                    let _ = write!(s, "M{:.1} {y:.2} L{:.1} {y:.2} ", x - 4.0, x + 4.0);
                } else {
                    let _ = write!(s, "{}{x:.1} {y:.2} ", if pen { "L" } else { "M" });
                }
                pen = true;
            }
            None => pen = false,
        }
    }
    s
}

fn area_path(vals: &[Option<f64>], max: f64) -> String {
    let n = vals.len();
    let mut s = String::new();
    let mut run: Vec<(f64, f64)> = Vec::new();
    let flush = |run: &mut Vec<(f64, f64)>, s: &mut String| {
        if run.len() >= 2 {
            let _ = write!(s, "M{:.1} 100 ", run[0].0);
            for (x, y) in run.iter() {
                let _ = write!(s, "L{x:.1} {y:.2} ");
            }
            let _ = write!(s, "L{:.1} 100 Z ", run[run.len() - 1].0);
        }
        run.clear();
    };
    for (i, v) in vals.iter().enumerate() {
        match v {
            Some(v) => run.push((xpos(i, n), ypos(*v, max))),
            None => flush(&mut run, &mut s),
        }
    }
    flush(&mut run, &mut s);
    s
}

/// Столбцы; `base` — высота уже нарисованного снизу (стопка).
fn bar_path(vals: &[f64], base: Option<&[f64]>, max: f64) -> String {
    let n = vals.len().max(1);
    let w = 1000.0 / n as f64;
    let bw = (w * 0.66).max(1.0);
    let mut s = String::new();
    for (i, v) in vals.iter().enumerate() {
        if *v <= 0.0 {
            continue;
        }
        let b = base.map(|b| b[i]).unwrap_or(0.0);
        let y0 = ypos(b, max);
        let y1 = ypos(b + v, max);
        let x = i as f64 * w + (w - bw) / 2.0;
        let _ = write!(s, "M{x:.1} {y1:.2}h{bw:.1}V{y0:.2}h{:.1}Z ", -bw);
    }
    s
}

struct Spec<'a> {
    title: String,
    l1: String,
    l2: String,
    v1: Vec<Option<f64>>,
    v2: Option<Vec<Option<f64>>>,
    bars: bool,
    status: bool,
    area: bool,
    integer: bool,
    fmt: &'a dyn Fn(Option<f64>) -> String,
}

fn x_labels(ts: &[i64], range: usize) -> [String; 3] {
    let pat = match range {
        3 => "%d.%m %H:00",
        4 => "%d.%m",
        _ => "%H:%M",
    };
    match ts.len() {
        0 => [String::new(), String::new(), String::new()],
        n => [time(ts[0], pat), time(ts[n / 2], pat), time(ts[n - 1], pat)],
    }
}

fn build(spec: Spec<'_>, ts: &[i64], range: usize) -> Chart {
    let has = |v: &Vec<Option<f64>>| v.iter().any(|x| x.is_some_and(|x| x > 0.0));
    let empty = !has(&spec.v1) && !spec.v2.as_ref().is_some_and(has);
    let raw = if spec.bars && spec.status {
        spec.v1.iter().zip(spec.v2.as_deref().unwrap_or(&[])).map(|(a, b)| a.unwrap_or(0.0) + b.unwrap_or(0.0)).fold(0.0, f64::max)
    } else {
        spec.v1.iter().chain(spec.v2.iter().flatten()).flatten().copied().fold(0.0, f64::max)
    };
    let max = nice_max(raw, spec.integer);
    let (p1, a1, p2) = if spec.bars {
        let a: Vec<f64> = spec.v1.iter().map(|x| x.unwrap_or(0.0)).collect();
        let b: Vec<f64> = spec.v2.as_ref().map(|v| v.iter().map(|x| x.unwrap_or(0.0)).collect()).unwrap_or_default();
        let p2 = if b.is_empty() { String::new() } else { bar_path(&b, spec.status.then_some(&a[..]), max) };
        (bar_path(&a, None, max), String::new(), p2)
    } else {
        (
            line_path(&spec.v1, max),
            if spec.area { area_path(&spec.v1, max) } else { String::new() },
            spec.v2.as_ref().map(|v| line_path(v, max)).unwrap_or_default(),
        )
    };
    let f = spec.fmt;
    let tips: Vec<SharedString> = (0..spec.v1.len())
        .map(|i| {
            let mut s = if ts.is_empty() { String::new() } else { time(ts[i], "%d.%m %H:%M") };
            let mut add = |label: &str, v: Option<f64>| {
                if !s.is_empty() {
                    s.push('\n');
                }
                let _ = write!(s, "{label}: {}", f(v));
            };
            add(&spec.l1, spec.v1[i]);
            if let Some(v2) = &spec.v2 {
                add(&spec.l2, v2[i]);
            }
            ss(s)
        })
        .collect();
    let [x0, x1, x2] = x_labels(ts, range);
    Chart {
        summary: ss(format!("{}: {}", t("максимум"), f(Some(raw)))),
        title: ss(&spec.title),
        empty,
        bars: spec.bars,
        status: spec.status,
        y_max: ss(if empty { String::new() } else { f(Some(max)) }),
        y_mid: ss(if empty { String::new() } else { f(Some(max / 2.0)) }),
        x0: ss(x0),
        x1: ss(x1),
        x2: ss(x2),
        l1: ss(&spec.l1),
        l2: ss(&spec.l2),
        p1: ss(p1),
        a1: ss(a1),
        p2: ss(p2),
        n: spec.v1.len() as i32,
        tips: model(tips),
    }
}

fn col(pts: &[Point], f: impl Fn(&Point) -> Option<f64>) -> Vec<Option<f64>> {
    pts.iter().map(f).collect()
}

pub fn charts(s: &Series, range: usize) -> Vec<Chart> {
    let pts = &s.points;
    let ts: Vec<i64> = pts.iter().map(|p| p.t).collect();
    let count = |v: Option<f64>| fmt::num(v, 0);
    let ms = |v: Option<f64>| fmt::ms(v);
    let tps = |v: Option<f64>| fmt::num(v, 1);
    vec![
        build(
            Spec {
                title: t("Очередь"),
                l1: t("в очереди"),
                l2: t("выполняется"),
                v1: col(pts, |p| p.depth.map(|v| v as f64)),
                v2: Some(col(pts, |p| p.running.map(|v| v as f64))),
                bars: false,
                status: false,
                area: true,
                integer: true,
                fmt: &count,
            },
            &ts,
            range,
        ),
        build(
            Spec {
                title: t("Задания"),
                l1: t("успешно"),
                l2: t("ошибки"),
                v1: col(pts, |p| Some(p.ok as f64)),
                v2: Some(col(pts, |p| Some(p.errors as f64))),
                bars: true,
                status: true,
                area: false,
                integer: true,
                fmt: &count,
            },
            &ts,
            range,
        ),
        build(
            Spec {
                title: t("Время ожидания"),
                l1: "p50".into(),
                l2: "p95".into(),
                v1: col(pts, |p| p.wait_p50),
                v2: Some(col(pts, |p| p.wait_p95)),
                bars: false,
                status: false,
                area: false,
                integer: false,
                fmt: &ms,
            },
            &ts,
            range,
        ),
        build(
            Spec {
                title: t("Время ответа модели"),
                l1: "p50".into(),
                l2: "p95".into(),
                v1: col(pts, |p| p.lat_p50),
                v2: Some(col(pts, |p| p.lat_p95)),
                bars: false,
                status: false,
                area: false,
                integer: false,
                fmt: &ms,
            },
            &ts,
            range,
        ),
        build(
            Spec {
                title: t("Скорость генерации, токенов/с"),
                l1: t("токенов/с"),
                l2: String::new(),
                v1: col(pts, |p| p.tps),
                v2: None,
                bars: false,
                status: false,
                area: true,
                integer: false,
                fmt: &tps,
            },
            &ts,
            range,
        ),
        wait_hist(s),
    ]
}

fn bucket_label(i: usize, b: &[i64]) -> String {
    let ms = |v: i64| fmt::ms(Some(v as f64));
    match b.get(i) {
        Some(v) if i == 0 => format!("≤ {}", ms(*v)),
        Some(v) => format!("{}–{}", ms(b[i - 1]), ms(*v)),
        None => format!("> {}", ms(*b.last().unwrap_or(&0))),
    }
}

/// Распределение ожидания по корзинам.
pub fn wait_hist(s: &Series) -> Chart {
    let vals: Vec<Option<f64>> = s.wait_hist.iter().map(|v| Some(*v as f64)).collect();
    let n = vals.len();
    let labels: Vec<String> = (0..n).map(|i| bucket_label(i, &s.buckets_ms)).collect();
    let count = |v: Option<f64>| fmt::num(v, 0);
    let mut c = build(
        Spec {
            title: t("Распределение ожидания"),
            l1: t("заданий"),
            l2: String::new(),
            v1: vals,
            v2: None,
            bars: true,
            status: false,
            area: false,
            integer: true,
            fmt: &count,
        },
        &[],
        0,
    );
    if n > 0 {
        c.x0 = ss(&labels[0]);
        c.x1 = ss(&labels[n / 2]);
        c.x2 = ss(&labels[n - 1]);
        let tips: Vec<SharedString> =
            labels.iter().zip(&s.wait_hist).map(|(l, v)| ss(format!("{l}\n{}", tn(*v as i64, "задание", "задания", "заданий")))).collect();
        c.tips = model(tips);
    }
    c
}

/// Лёгкий спарклайн для KPI (100×20).
fn spark(vals: &[Option<f64>]) -> String {
    let max = vals.iter().flatten().copied().fold(0.0, f64::max);
    if max <= 0.0 || vals.len() < 2 {
        return String::new();
    }
    let n = vals.len();
    let mut s = String::new();
    let mut pen = false;
    for (i, v) in vals.iter().enumerate() {
        match v {
            Some(v) => {
                let x = i as f64 / (n - 1) as f64 * 100.0;
                let y = 19.0 - v / max * 18.0;
                let _ = write!(s, "{}{x:.1} {y:.2} ", if pen { "L" } else { "M" });
                pen = true;
            }
            None => pen = false,
        }
    }
    s
}

// --------------------------------------------------------------------- обзор

pub fn kpis(o: &Overview, s: Option<&Series>) -> Vec<Kpi> {
    let slots: u32 = o.models.iter().map(|m| m.slots).sum();
    let h1 = &o.last_1h;
    let pts: &[Point] = s.map(|s| &s.points[..]).unwrap_or(&[]);
    let sp = |f: &dyn Fn(&Point) -> Option<f64>| ss(spark(&pts.iter().map(f).collect::<Vec<_>>()));
    let pending = o.counts.get("pending_delivery").copied().unwrap_or(0);
    let err = h1.error_rate.unwrap_or(0.0);
    vec![
        Kpi {
            title: ss(t("В очереди")),
            value: ss(fmt::int(o.queued as f64)),
            unit: ss(""),
            sub: ss(tf("доставляется: {n}", &[("n", &pending)])),
            spark: sp(&|p| p.depth.map(|v| v as f64)),
            bad: false,
        },
        Kpi {
            title: ss(t("Выполняется")),
            value: ss(fmt::int(o.running as f64)),
            unit: ss(if slots > 0 { format!("/ {slots}") } else { String::new() }),
            sub: ss(if slots > 0 { tf("загрузка {p}", &[("p", &fmt::pct(Some(o.running as f64 / slots as f64)))]) } else { t("нет слотов") }),
            spark: sp(&|p| p.running.map(|v| v as f64)),
            bad: false,
        },
        Kpi {
            title: ss(t("Заданий за час")),
            value: ss(fmt::int(h1.jobs as f64)),
            unit: ss(""),
            sub: ss(if h1.jobs > 0 { tf("ошибок {p}", &[("p", &fmt::pct(Some(err)))]) } else { "—".into() }),
            spark: sp(&|p| Some(p.jobs as f64)),
            bad: err > 0.2,
        },
        Kpi {
            title: ss(t("Ожидание p95")),
            value: ss(fmt::ms(h1.wait_p95)),
            unit: ss(""),
            sub: ss(tf("среднее {v}", &[("v", &fmt::ms(h1.wait_avg))])),
            spark: sp(&|p| p.wait_p95),
            bad: false,
        },
        Kpi {
            title: ss(t("Ответ p95")),
            value: ss(fmt::ms(h1.lat_p95)),
            unit: ss(""),
            sub: ss(tf("среднее {v}", &[("v", &fmt::ms(h1.lat_avg))])),
            spark: sp(&|p| p.lat_p95),
            bad: false,
        },
        Kpi {
            title: ss(t("Токены/с")),
            value: ss(fmt::num(h1.tps, 1)),
            unit: ss(""),
            sub: ss(tf("токенов за час: {n}", &[("n", &fmt::int(h1.tokens as f64))])),
            spark: sp(&|p| p.tps),
            bad: false,
        },
    ]
}

pub fn status_line(o: &Overview) -> String {
    [if o.paused { t("Выдача на паузе") } else { t("Работает") }, fmt::uptime(o.uptime_s), tf("профиль {p}", &[("p", &o.profile)]), format!("v{}", o.version)]
        .join(" · ")
}

pub fn banners(o: &Overview) -> Vec<BannerRow> {
    let mut v = Vec::new();
    let mut push = |kind: &str, title: String, text: String| v.push(BannerRow { kind: ss(kind), title: ss(title), text: ss(text) });
    if let Some(e) = &o.bound.client_error {
        push("bad", t("API не слушает"), e.clone());
    }
    if let Some(e) = &o.bound.admin_error {
        push("bad", t("Панель не слушает"), e.clone());
    }
    if o.paused {
        push("warn", t("Выдача заданий на паузе"), t("Задания принимаются и ждут в очереди; MAX_WAIT продолжает действовать."));
    }
    if o.clients == 0 {
        push("warn", t("Нет клиентов API"), t("Бот получит 401, пока не создан ключ."));
    }
    for r in &o.schedule.active {
        let what = match r.action.as_str() {
            "pause_intake" => t("приём на паузе"),
            "pause_dispatch" => t("выдача на паузе"),
            "limit_slots" => tf("не больше {n} одновременно", &[("n", &r.slots)]),
            other => other.to_string(),
        };
        let models = if r.models.is_empty() { String::new() } else { format!(" · {}", r.models.join(", ")) };
        push(
            "info",
            tf("Расписание: {name}", &[("name", &r.name)]),
            format!("{what}{models} · {}", tf("ещё {t}", &[("t", &fmt::ms(Some(r.ends_in as f64 * 1000.0)))])),
        );
    }
    let down: Vec<String> = o.models.iter().flat_map(|m| m.upstreams.iter().filter(|u| u.health.breaker == Breaker::Open).map(|u| u.id.clone())).collect();
    if !down.is_empty() {
        push("bad", t("Апстримы исключены выключателем"), down.join(", "));
    }
    v
}

/// Результат проверки модели: идёт ли проверка и текст итога.
pub type Tests = HashMap<String, (bool, String)>;

pub fn models(o: &Overview, s: &Settings, tests: &Tests) -> Vec<ModelRow> {
    let mut rows = Vec::new();
    for (name, cfg) in &s.models {
        let ov = o.models.iter().find(|m| &m.name == name);
        let (testing, test) = tests.get(name).cloned().unwrap_or_default();
        let ups: Vec<UpstreamRow> = ov
            .map(|m| {
                m.upstreams
                    .iter()
                    .map(|u| {
                        let engine_ready = u.engine_status.is_none_or(|e| e == EngineStatus::Ready);
                        let (kind, state) = match u.health.breaker {
                            Breaker::Open => ("bad", t("исключён выключателем")),
                            Breaker::HalfOpen => ("busy", t("проверяется")),
                            Breaker::Closed if !engine_ready => ("busy", t("движок не готов")),
                            Breaker::Closed if u.health.reachable == Some(false) => ("bad", t("недоступен")),
                            Breaker::Closed if u.busy > 0 => ("run", t("в работе")),
                            Breaker::Closed => ("ok", t("готов")),
                        };
                        UpstreamRow {
                            id: ss(&u.id),
                            url: ss(&u.url),
                            slots: u.slots as i32,
                            busy: u.busy as i32,
                            kind: ss(kind),
                            state: ss(state),
                            error: ss(u.health.last_error.clone().unwrap_or_default()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (kind, state) = match ov {
            _ if !cfg.enabled => ("off", t("выключена")),
            Some(m) if !m.available => ("bad", t("недоступна")),
            Some(m) if m.running > 0 => ("run", t("работает")),
            Some(_) => ("ok", t("доступна")),
            None => ("off", t("не загружена")),
        };
        rows.push(ModelRow {
            name: ss(name),
            description: ss(&cfg.description),
            queued: ov.map(|m| m.queued as i32).unwrap_or(0),
            running: ov.map(|m| m.running as i32).unwrap_or(0),
            slots: ov.map(|m| m.slots as i32).unwrap_or(0),
            busy: ov.map(|m| m.busy as i32).unwrap_or(0),
            kind: ss(kind),
            state: ss(state),
            enabled: cfg.enabled,
            aliases: ss(ov.map(|m| m.aliases.join(", ")).unwrap_or_default()),
            fallback: ss(cfg.fallback.join(" → ")),
            upstreams: model(ups),
            test: ss(test),
            testing,
        });
    }
    rows
}

pub fn running(o: &Overview) -> Vec<RunRow> {
    let mut rj = o.running_jobs.clone();
    rj.sort_by_key(|j| j.started_at);
    rj.iter()
        .map(|j| {
            let mut meta = vec![j.model.clone(), j.upstream.clone()];
            if j.chat_id != 0 {
                meta.push(format!("chat {}", j.chat_id));
            }
            RunRow {
                id: ss(if j.sync { t("синхронный запрос") } else { j.job_id.clone() }),
                meta: ss(meta.join(" · ")),
                elapsed: ss(fmt::ms(Some((o.now - j.started_at) as f64))),
            }
        })
        .collect()
}

pub fn engine_state(s: EngineStatus) -> (&'static str, String) {
    match s {
        EngineStatus::Ready => ("run", t("готов")),
        EngineStatus::Starting => ("busy", t("запускается")),
        EngineStatus::Stopping => ("busy", t("останавливается")),
        EngineStatus::Crashed => ("bad", t("упал, перезапуск")),
        EngineStatus::Failed => ("bad", t("остановлен после сбоев")),
        EngineStatus::Stopped => ("off", t("остановлен")),
    }
}

pub fn engines(list: &[EngineInfo], now: i64) -> Vec<EngineRow> {
    list.iter()
        .map(|e| {
            let (kind, state) = engine_state(e.status);
            let kind_name = serde_json::to_value(e.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
            let mut meta = vec![kind_name];
            if let Some(pid) = e.pid {
                meta.push(format!("PID {pid}"));
            }
            if let Some(s) = e.ready_at.or(e.started_at) {
                meta.push(tf("запущен {t}", &[("t", &ago(s, now))]));
            }
            if e.restarts > 0 {
                meta.push(tf("перезапусков: {n}", &[("n", &e.restarts)]));
            }
            EngineRow {
                id: ss(&e.id),
                name: ss(&e.name),
                kind: ss(kind),
                state: ss(state),
                meta: ss(meta.join(" · ")),
                endpoint: ss(&e.endpoint),
                command: ss(e.command.join(" ")),
                last_exit: ss(e.last_exit.clone().unwrap_or_default()),
                running: !matches!(e.status, EngineStatus::Stopped | EngineStatus::Failed),
            }
        })
        .collect()
}

pub fn gpus(o: &Overview) -> Vec<GpuRow> {
    if !o.gpu.available {
        return Vec::new();
    }
    o.gpu
        .devices
        .iter()
        .map(|d| {
            let mem_f = if d.mem_total_mb > 0 { d.mem_used_mb as f32 / d.mem_total_mb as f32 } else { 0.0 };
            let temp = d.temp_c.map(|c| {
                let mut s = format!("{c} °C");
                if let Some(p) = d.power_w {
                    let _ = write!(s, " · {} {}", fmt::num(Some(p), 0), t("Вт"));
                }
                s
            });
            GpuRow {
                name: ss(&d.name),
                util: ss(format!("{}%", d.util)),
                util_f: d.util as f32 / 100.0,
                mem: ss(format!("{} / {}", fmt::mb(d.mem_used_mb), fmt::mb(d.mem_total_mb))),
                mem_f,
                temp: ss(temp.unwrap_or_default()),
                temp_f: d.temp_c.unwrap_or(0) as f32 / 100.0,
                hot: d.temp_c.is_some_and(|c| c >= 83),
            }
        })
        .collect()
}

// -------------------------------------------------------------------- очередь

pub const JOB_FILTERS: [&str; 11] = ["", "active", "queued", "running", "done", "failed", "dead", "pending_delivery", "expired", "cancelled", "superseded"];

pub fn job_state(s: &str) -> (&'static str, String) {
    match s {
        "queued" => ("busy", t("в очереди")),
        "running" => ("run", t("выполняется")),
        "done" => ("ok", t("готово")),
        "failed" => ("bad", t("ошибка")),
        "expired" => ("off", t("истекло")),
        "cancelled" => ("off", t("отменено")),
        "superseded" => ("off", t("вытеснено")),
        "dead" => ("bad", t("не доставлено")),
        "pending_delivery" => ("busy", t("доставляется")),
        _ => ("off", s.to_string()),
    }
}

pub fn jobs(rows: &[CoreJob], now: i64) -> Vec<JobRow> {
    rows.iter()
        .map(|j| {
            let st = if j.delivery == "pending" && matches!(j.state.as_str(), "done" | "failed") { "pending_delivery" } else { j.public_state() };
            let (kind, label) = job_state(st);
            let mut meta = Vec::new();
            if j.chat_id != 0 {
                meta.push(format!("chat {}", j.chat_id));
            } else {
                meta.push(t("синхронный"));
            }
            meta.push(j.client.clone());
            if !j.model_used.is_empty() && j.model_used != j.model {
                meta.push(format!("→ {}", j.model_used));
            }
            let mut timing = Vec::new();
            if let Some(w) = j.queued_ms {
                timing.push(format!("{} {}", t("ждал"), fmt::ms(Some(w as f64))));
            }
            if let Some(l) = j.latency_ms {
                timing.push(format!("{} {}", t("ответ"), fmt::ms(Some(l as f64))));
            }
            let error = j.error_code.clone().or_else(|| if j.delivery == "dead" { j.delivery_error.clone() } else { None }).unwrap_or_default();
            JobRow {
                id: ss(&j.job_id),
                model: ss(&j.model),
                meta: ss(meta.join(" · ")),
                kind: ss(kind),
                state: ss(label),
                when: ss(ago(j.created_at, now)),
                timing: ss(timing.join("\n")),
                error: ss(error),
                can_cancel: matches!(j.state.as_str(), "queued" | "running"),
                can_redeliver: j.delivery == "dead",
            }
        })
        .collect()
}

// ------------------------------------------------------------------- журналы

pub fn log_line(e: &LogEntry) -> LogLine {
    let mut text = e.message.clone();
    for (k, v) in &e.fields {
        let v = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let _ = write!(text, "  {k}={v}");
    }
    LogLine { time: ss(time(e.ts, "%H:%M:%S")), level: ss(e.level.to_uppercase()), text: ss(text) }
}

pub fn engine_line(l: &EngineLine) -> LogLine {
    LogLine { time: ss(time(l.ts, "%H:%M:%S")), level: ss(if l.stream == "sys" { "LMQ".to_string() } else { l.level.to_uppercase() }), text: ss(&l.text) }
}

// ------------------------------------------------------------------- клиенты

pub fn clients(list: &[lmq_core::admin::ClientView]) -> Vec<ClientRow> {
    list.iter()
        .map(|c| ClientRow {
            name: ss(&c.name),
            note: ss(&c.note),
            enabled: c.enabled,
            created: ss(if c.created_at > 0 { time(c.created_at, "%d.%m.%Y") } else { "—".into() }),
            from_env: c.from_env,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_max_rounds_up() {
        assert_eq!(nice_max(0.0, true), 2.0);
        assert_eq!(nice_max(3.0, true), 4.0);
        assert_eq!(nice_max(7.0, true), 8.0);
        assert_eq!(nice_max(1234.0, false), 2000.0);
        assert_eq!(nice_max(0.42, false), 0.5);
        assert_eq!(nice_max(f64::NAN, false), 1.0);
    }

    #[test]
    fn paths_break_on_gaps_and_show_single_points() {
        let p = line_path(&[Some(1.0), Some(2.0), None, Some(1.0)], 2.0);
        assert_eq!(p.matches('M').count(), 2, "{p}");
        assert!(p.contains("L"), "{p}");
        let single = line_path(&[None, Some(1.0), None], 2.0);
        assert!(single.starts_with('M') && single.contains('L'), "{single}");
        assert!(area_path(&[Some(1.0), None], 2.0).is_empty());
        assert!(area_path(&[Some(1.0), Some(1.0)], 2.0).ends_with("Z "));
    }

    #[test]
    fn stacked_bars_start_on_top_of_base() {
        let base = [2.0, 0.0];
        let top = bar_path(&[1.0, 1.0], Some(&base), 4.0);
        // Над столбцом высотой 2 из 4 вторая серия начинается с y = 50.
        assert!(top.contains("V50.00"), "{top}");
    }
}
