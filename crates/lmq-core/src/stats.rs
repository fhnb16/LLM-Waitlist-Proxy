//! Статистика: поминутные корзины по (модель, апстрим, клиент, тип чата) с гистограммами
//! ожидания и ответа. Текущая минута копится в памяти, завершённые пишутся в SQLite.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::store::Store;
use crate::util::now_ms;

/// Верхние границы корзин гистограммы, мс (последняя корзина — «больше»).
pub const BUCKETS_MS: [i64; 12] = [250, 500, 1_000, 2_000, 5_000, 10_000, 20_000, 30_000, 60_000, 120_000, 300_000, 600_000];
const NB: usize = BUCKETS_MS.len() + 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Cell {
    pub jobs: u64,
    pub ok: u64,
    pub errors: u64,
    pub wait_sum: i64,
    pub wait_max: i64,
    pub waits: u64,
    pub lat_sum: i64,
    pub lat_max: i64,
    pub lats: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Время генерации успешных ответов (для токенов в секунду), мс.
    pub gen_ms: i64,
    pub err: BTreeMap<String, u64>,
    pub wait_h: Vec<u64>,
    pub lat_h: Vec<u64>,
}

fn bucket(ms: i64) -> usize {
    BUCKETS_MS.iter().position(|b| ms <= *b).unwrap_or(BUCKETS_MS.len())
}

fn add_hist(h: &mut Vec<u64>, other: &[u64]) {
    if h.len() < NB {
        h.resize(NB, 0);
    }
    for (i, v) in other.iter().enumerate().take(NB) {
        h[i] += v;
    }
}

impl Cell {
    pub fn merge(&mut self, o: &Cell) {
        self.jobs += o.jobs;
        self.ok += o.ok;
        self.errors += o.errors;
        self.wait_sum += o.wait_sum;
        self.wait_max = self.wait_max.max(o.wait_max);
        self.waits += o.waits;
        self.lat_sum += o.lat_sum;
        self.lat_max = self.lat_max.max(o.lat_max);
        self.lats += o.lats;
        self.prompt_tokens += o.prompt_tokens;
        self.completion_tokens += o.completion_tokens;
        self.gen_ms += o.gen_ms;
        for (k, v) in &o.err {
            *self.err.entry(k.clone()).or_default() += v;
        }
        add_hist(&mut self.wait_h, &o.wait_h);
        add_hist(&mut self.lat_h, &o.lat_h);
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Key {
    pub model: String,
    pub upstream: String,
    pub client: String,
    pub chat_kind: String,
}

/// Итог одного задания для статистики.
#[derive(Clone, Debug)]
pub struct Sample<'a> {
    pub model: &'a str,
    pub upstream: Option<&'a str>,
    pub client: &'a str,
    pub chat_kind: &'a str,
    /// `None` — успех, иначе код ошибки.
    pub error_code: Option<&'a str>,
    pub wait_ms: Option<i64>,
    pub latency_ms: Option<i64>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Default)]
struct Current {
    minute: i64,
    cells: HashMap<Key, Cell>,
    depth_max: i64,
    running_max: i64,
    slots: i64,
}

pub struct Stats {
    store: Store,
    cur: Mutex<Current>,
    ewma: Mutex<HashMap<String, f64>>,
}

pub fn minute_of(ms: i64) -> i64 {
    ms.div_euclid(60_000)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Range {
    M5,
    H1,
    H24,
    D7,
    D30,
}

impl Range {
    pub fn parse(s: &str) -> Self {
        match s {
            "5m" => Self::M5,
            "24h" | "1d" => Self::H24,
            "7d" => Self::D7,
            "30d" => Self::D30,
            _ => Self::H1,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::M5 => "5m",
            Self::H1 => "1h",
            Self::H24 => "24h",
            Self::D7 => "7d",
            Self::D30 => "30d",
        }
    }
    /// (длина диапазона в минутах, корзина в минутах)
    fn shape(self) -> (i64, i64) {
        match self {
            Self::M5 => (5, 1),
            Self::H1 => (60, 1),
            Self::H24 => (1440, 15),
            Self::D7 => (10080, 120),
            Self::D30 => (43200, 720),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    None,
    Model,
    Upstream,
    Client,
    ChatKind,
}

impl Group {
    pub fn parse(s: &str) -> Self {
        match s {
            "model" => Self::Model,
            "upstream" => Self::Upstream,
            "client" => Self::Client,
            "chat_kind" | "chat" => Self::ChatKind,
            _ => Self::None,
        }
    }
    fn key(self, k: &Key) -> &str {
        match self {
            Self::None => "",
            Self::Model => &k.model,
            Self::Upstream => &k.upstream,
            Self::Client => &k.client,
            Self::ChatKind => &k.chat_kind,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Query {
    pub range: Option<Range>,
    pub group: Option<Group>,
    pub model: Option<String>,
    pub upstream: Option<String>,
    pub client: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Point {
    pub t: i64,
    pub jobs: u64,
    pub ok: u64,
    pub errors: u64,
    pub wait_avg: Option<f64>,
    pub wait_p50: Option<f64>,
    pub wait_p95: Option<f64>,
    pub lat_avg: Option<f64>,
    pub lat_p50: Option<f64>,
    pub lat_p95: Option<f64>,
    pub tokens: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub tps: Option<f64>,
    pub error_rate: Option<f64>,
    pub depth: Option<i64>,
    pub running: Option<i64>,
    pub slots: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GroupSeries {
    pub key: String,
    pub totals: Point,
    pub points: Vec<Point>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Series {
    pub range: &'static str,
    pub bucket_s: i64,
    pub from: i64,
    pub to: i64,
    pub points: Vec<Point>,
    pub totals: Point,
    pub errors: BTreeMap<String, u64>,
    pub wait_hist: Vec<u64>,
    pub lat_hist: Vec<u64>,
    pub buckets_ms: Vec<i64>,
    pub groups: Vec<GroupSeries>,
}

fn percentile(h: &[u64], p: f64, max: i64) -> Option<f64> {
    let total: u64 = h.iter().sum();
    if total == 0 {
        return None;
    }
    let target = (total as f64 * p).ceil().max(1.0);
    let mut cum = 0u64;
    for (i, &c) in h.iter().enumerate() {
        if c == 0 {
            continue;
        }
        if (cum + c) as f64 >= target {
            let lo = if i == 0 { 0 } else { BUCKETS_MS[i - 1] } as f64;
            let hi = BUCKETS_MS.get(i).copied().unwrap_or(max.max(BUCKETS_MS[BUCKETS_MS.len() - 1])) as f64;
            let frac = (target - cum as f64) / c as f64;
            return Some((lo + (hi - lo) * frac).min(max.max(1) as f64));
        }
        cum += c;
    }
    None
}

fn point_of(t: i64, c: &Cell) -> Point {
    let tokens = c.prompt_tokens + c.completion_tokens;
    Point {
        t,
        jobs: c.jobs,
        ok: c.ok,
        errors: c.errors,
        wait_avg: (c.waits > 0).then(|| c.wait_sum as f64 / c.waits as f64),
        wait_p50: percentile(&c.wait_h, 0.5, c.wait_max),
        wait_p95: percentile(&c.wait_h, 0.95, c.wait_max),
        lat_avg: (c.lats > 0).then(|| c.lat_sum as f64 / c.lats as f64),
        lat_p50: percentile(&c.lat_h, 0.5, c.lat_max),
        lat_p95: percentile(&c.lat_h, 0.95, c.lat_max),
        tokens,
        prompt_tokens: c.prompt_tokens,
        completion_tokens: c.completion_tokens,
        tps: (c.gen_ms > 0 && c.completion_tokens > 0).then(|| c.completion_tokens as f64 / (c.gen_ms as f64 / 1000.0)),
        error_rate: (c.jobs > 0).then(|| c.errors as f64 / c.jobs as f64),
        depth: None,
        running: None,
        slots: None,
    }
}

impl Stats {
    pub fn new(store: Store) -> Self {
        Self { store, cur: Mutex::new(Current { minute: minute_of(now_ms()), ..Default::default() }), ewma: Mutex::new(HashMap::new()) }
    }

    pub fn record(&self, s: &Sample<'_>) {
        let key = Key {
            model: s.model.to_string(),
            upstream: s.upstream.unwrap_or("-").to_string(),
            client: s.client.to_string(),
            chat_kind: s.chat_kind.to_string(),
        };
        let mut cur = self.cur.lock();
        let c = cur.cells.entry(key).or_insert_with(|| Cell { wait_h: vec![0; NB], lat_h: vec![0; NB], ..Default::default() });
        c.jobs += 1;
        match s.error_code {
            None => {
                c.ok += 1;
                c.prompt_tokens += s.prompt_tokens;
                c.completion_tokens += s.completion_tokens;
                if let Some(l) = s.latency_ms {
                    c.gen_ms += l;
                }
            }
            Some(code) => {
                c.errors += 1;
                *c.err.entry(code.to_string()).or_default() += 1;
            }
        }
        if let Some(w) = s.wait_ms {
            c.waits += 1;
            c.wait_sum += w;
            c.wait_max = c.wait_max.max(w);
            c.wait_h[bucket(w)] += 1;
        }
        if let Some(l) = s.latency_ms {
            c.lats += 1;
            c.lat_sum += l;
            c.lat_max = c.lat_max.max(l);
            c.lat_h[bucket(l)] += 1;
        }
        drop(cur);
        if s.error_code.is_none()
            && let Some(l) = s.latency_ms
        {
            let mut e = self.ewma.lock();
            let v = e.entry(s.model.to_string()).or_insert(l as f64);
            *v = *v * 0.8 + l as f64 * 0.2;
        }
    }

    /// Снимок очереди (раз в секунду из диспетчера): в минуту пишется максимум.
    pub fn sample_queue(&self, depth: i64, running: i64, slots: i64) {
        let mut cur = self.cur.lock();
        cur.depth_max = cur.depth_max.max(depth);
        cur.running_max = cur.running_max.max(running);
        cur.slots = slots;
    }

    /// Сглаженное время ответа модели, мс (для ETA). Без данных — 20 с.
    pub fn avg_latency(&self, model: &str) -> f64 {
        self.ewma.lock().get(model).copied().unwrap_or(20_000.0)
    }

    /// Сбрасывает завершённую минуту в базу. `force` — сбросить и текущую (при выходе).
    pub async fn flush(&self, force: bool) -> Result<()> {
        let now_min = minute_of(now_ms());
        let taken = {
            let mut cur = self.cur.lock();
            if cur.minute == now_min && !force {
                return Ok(());
            }
            let t = std::mem::take(&mut *cur);
            cur.minute = now_min;
            t
        };
        if taken.cells.is_empty() && taken.depth_max == 0 && taken.running_max == 0 {
            return Ok(());
        }
        let mut tx = self.store.pool.begin().await?;
        for (k, cell) in &taken.cells {
            let old: Option<(String,)> =
                sqlx::query_as("SELECT data FROM stats_minute WHERE minute = ? AND model = ? AND upstream = ? AND client = ? AND chat_kind = ?")
                    .bind(taken.minute)
                    .bind(&k.model)
                    .bind(&k.upstream)
                    .bind(&k.client)
                    .bind(&k.chat_kind)
                    .fetch_optional(&mut *tx)
                    .await?;
            let mut merged = cell.clone();
            if let Some((data,)) = old
                && let Ok(prev) = serde_json::from_str::<Cell>(&data)
            {
                merged.merge(&prev);
            }
            sqlx::query(
                "INSERT INTO stats_minute (minute, model, upstream, client, chat_kind, data) VALUES (?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(minute, model, upstream, client, chat_kind) DO UPDATE SET data = excluded.data",
            )
            .bind(taken.minute)
            .bind(&k.model)
            .bind(&k.upstream)
            .bind(&k.client)
            .bind(&k.chat_kind)
            .bind(serde_json::to_string(&merged)?)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO stats_queue (minute, depth_max, running_max, slots) VALUES (?, ?, ?, ?) \
             ON CONFLICT(minute) DO UPDATE SET depth_max = MAX(depth_max, excluded.depth_max), \
             running_max = MAX(running_max, excluded.running_max), slots = excluded.slots",
        )
        .bind(taken.minute)
        .bind(taken.depth_max)
        .bind(taken.running_max)
        .bind(taken.slots)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn cleanup(&self, retention_days: u64) -> Result<()> {
        let cutoff = minute_of(now_ms()) - (retention_days.max(1) as i64) * 1440;
        sqlx::query("DELETE FROM stats_minute WHERE minute < ?").bind(cutoff).execute(&self.store.pool).await?;
        sqlx::query("DELETE FROM stats_queue WHERE minute < ?").bind(cutoff).execute(&self.store.pool).await?;
        Ok(())
    }

    async fn load(&self, from_min: i64) -> Result<(Vec<(i64, Key, Cell)>, Vec<(i64, i64, i64, i64)>)> {
        let rows: Vec<(i64, String, String, String, String, String)> =
            sqlx::query_as("SELECT minute, model, upstream, client, chat_kind, data FROM stats_minute WHERE minute >= ?")
                .bind(from_min)
                .fetch_all(&self.store.pool)
                .await?;
        let mut cells: Vec<(i64, Key, Cell)> = rows
            .into_iter()
            .filter_map(|(m, model, upstream, client, chat_kind, data)| {
                serde_json::from_str::<Cell>(&data).ok().map(|c| (m, Key { model, upstream, client, chat_kind }, c))
            })
            .collect();
        let mut queue: Vec<(i64, i64, i64, i64)> = sqlx::query_as("SELECT minute, depth_max, running_max, slots FROM stats_queue WHERE minute >= ?")
            .bind(from_min)
            .fetch_all(&self.store.pool)
            .await?;
        let cur = self.cur.lock();
        for (k, c) in &cur.cells {
            cells.push((cur.minute, k.clone(), c.clone()));
        }
        queue.push((cur.minute, cur.depth_max, cur.running_max, cur.slots));
        Ok((cells, queue))
    }

    pub async fn query(&self, q: &Query) -> Result<Series> {
        let range = q.range.unwrap_or(Range::H1);
        let group = q.group.unwrap_or(Group::None);
        let (len, step) = range.shape();
        let now_min = minute_of(now_ms());
        let end = now_min - now_min.rem_euclid(step) + step; // исключительная граница
        let start = end - len.div_euclid(step).max(1) * step;
        let n = ((end - start) / step) as usize;
        let (cells, queue) = self.load(start).await?;

        let mut total_buckets = vec![Cell::default(); n];
        let mut total = Cell::default();
        let mut groups: BTreeMap<String, (Cell, Vec<Cell>)> = BTreeMap::new();
        for (m, k, c) in cells.iter() {
            if *m < start || *m >= end {
                continue;
            }
            if q.model.as_deref().is_some_and(|x| x != k.model)
                || q.upstream.as_deref().is_some_and(|x| x != k.upstream)
                || q.client.as_deref().is_some_and(|x| x != k.client)
            {
                continue;
            }
            let idx = ((m - start) / step) as usize;
            total_buckets[idx].merge(c);
            total.merge(c);
            if group != Group::None {
                let g = groups.entry(group.key(k).to_string()).or_insert_with(|| (Cell::default(), vec![Cell::default(); n]));
                g.0.merge(c);
                g.1[idx].merge(c);
            }
        }
        let mut points: Vec<Point> = total_buckets.iter().enumerate().map(|(i, c)| point_of((start + i as i64 * step) * 60_000, c)).collect();
        // Очередь: максимум за корзину.
        for (m, depth, running, slots) in queue {
            if m < start || m >= end {
                continue;
            }
            let p = &mut points[((m - start) / step) as usize];
            p.depth = Some(p.depth.unwrap_or(0).max(depth));
            p.running = Some(p.running.unwrap_or(0).max(running));
            p.slots = Some(slots.max(p.slots.unwrap_or(0)));
        }
        let mut totals = point_of(start * 60_000, &total);
        totals.depth = points.iter().filter_map(|p| p.depth).max();
        totals.running = points.iter().filter_map(|p| p.running).max();
        let mut wait_hist = total.wait_h.clone();
        let mut lat_hist = total.lat_h.clone();
        wait_hist.resize(NB, 0);
        lat_hist.resize(NB, 0);
        let groups = groups
            .into_iter()
            .map(|(key, (tot, b))| GroupSeries {
                key,
                totals: point_of(start * 60_000, &tot),
                points: b.iter().enumerate().map(|(i, c)| point_of((start + i as i64 * step) * 60_000, c)).collect(),
            })
            .collect();
        Ok(Series {
            range: range.name(),
            bucket_s: step * 60,
            from: start * 60_000,
            to: end * 60_000,
            points,
            errors: total.err.clone(),
            totals,
            wait_hist,
            lat_hist,
            buckets_ms: BUCKETS_MS.to_vec(),
            groups,
        })
    }

    /// Средние ожидание и ответ по моделям за последние `minutes` минут.
    pub async fn window(&self, minutes: i64) -> Result<BTreeMap<String, Point>> {
        let from = minute_of(now_ms()) - minutes + 1;
        let (cells, _) = self.load(from).await?;
        let mut by: BTreeMap<String, Cell> = BTreeMap::new();
        for (m, k, c) in cells {
            if m >= from {
                by.entry(k.model.clone()).or_default().merge(&c);
            }
        }
        Ok(by.into_iter().map(|(k, c)| (k, point_of(from * 60_000, &c))).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn aggregates_and_flushes() {
        let store = Store::open_memory().await.unwrap();
        let st = Stats::new(store);
        for i in 0..10 {
            st.record(&Sample {
                model: "m",
                upstream: Some("m#0"),
                client: "bot",
                chat_kind: "group",
                error_code: if i == 9 { Some("upstream_error") } else { None },
                wait_ms: Some(1000 * i),
                latency_ms: Some(2000),
                prompt_tokens: 10,
                completion_tokens: 20,
            });
        }
        st.sample_queue(4, 2, 2);
        let s = st.query(&Query { range: Some(Range::H1), group: Some(Group::Model), ..Default::default() }).await.unwrap();
        assert_eq!(s.totals.jobs, 10);
        assert_eq!(s.totals.errors, 1);
        assert_eq!(s.errors.get("upstream_error"), Some(&1));
        assert_eq!(s.groups.len(), 1);
        assert!(s.totals.tps.unwrap() > 0.0);
        assert_eq!(s.points.len(), 60);
        st.flush(true).await.unwrap();
        let s2 = st.query(&Query::default()).await.unwrap();
        assert_eq!(s2.totals.jobs, 10);
        assert_eq!(s2.totals.depth, Some(4));
        // Повторный сброс той же минуты сливается, а не затирает.
        st.record(&Sample {
            model: "m",
            upstream: None,
            client: "bot",
            chat_kind: "group",
            error_code: Some("expired"),
            wait_ms: Some(1),
            latency_ms: None,
            prompt_tokens: 0,
            completion_tokens: 0,
        });
        st.flush(true).await.unwrap();
        let s3 = st.query(&Query::default()).await.unwrap();
        assert_eq!(s3.totals.jobs, 11);
    }

    #[test]
    fn percentiles_are_sane() {
        let mut h = vec![0u64; NB];
        h[bucket(900)] = 50;
        h[bucket(9000)] = 50;
        let p50 = percentile(&h, 0.5, 9000).unwrap();
        let p95 = percentile(&h, 0.95, 9000).unwrap();
        assert!(p50 <= 1000.0 && p95 > 5000.0);
    }
}
