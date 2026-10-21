//! Хранилище заданий: SQLite в режиме WAL.
//!
//! Состояние задания (`state`) — итог выполнения: `queued | running | done | failed |
//! expired | cancelled | superseded`. Доставка боту (`delivery`) — отдельно:
//! `none | pending | delivered | dead`. Наружу состояние `dead` показывается вместо
//! итога, если доставка исчерпана (§4.2).

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{FromRow, SqlitePool};

use crate::util::now_ms;

const MIGRATIONS: &[(i64, &str)] = &[(
    1,
    r#"
CREATE TABLE jobs (
    job_id           TEXT PRIMARY KEY,
    client           TEXT NOT NULL,
    model            TEXT NOT NULL,
    model_used       TEXT NOT NULL,
    chat_id          INTEGER NOT NULL,
    reply_to         INTEGER,
    thread_id        INTEGER,
    protocol         INTEGER NOT NULL DEFAULT 2,
    priority         INTEGER NOT NULL DEFAULT 2,
    payload          TEXT,
    callback_url     TEXT,
    callback_token   TEXT,
    state            TEXT NOT NULL,
    delivery         TEXT NOT NULL DEFAULT 'none',
    attempts         INTEGER NOT NULL DEFAULT 0,
    hops             INTEGER NOT NULL DEFAULT 0,
    delivery_attempts INTEGER NOT NULL DEFAULT 0,
    next_delivery_at INTEGER,
    created_at       INTEGER NOT NULL,
    started_at       INTEGER,
    finished_at      INTEGER,
    delivered_at     INTEGER,
    error            TEXT,
    error_code       TEXT,
    delivery_error   TEXT,
    usage            TEXT,
    result_text      TEXT,
    tg_result        TEXT,
    upstream_used    TEXT,
    queued_ms        INTEGER,
    latency_ms       INTEGER
);
CREATE INDEX jobs_state ON jobs(state, created_at);
CREATE INDEX jobs_delivery ON jobs(delivery, next_delivery_at);
CREATE INDEX jobs_chat ON jobs(chat_id, state);
CREATE INDEX jobs_finished ON jobs(finished_at);

CREATE TABLE stats_minute (
    minute      INTEGER NOT NULL,
    model       TEXT NOT NULL,
    upstream    TEXT NOT NULL,
    client      TEXT NOT NULL,
    chat_kind   TEXT NOT NULL,
    data        TEXT NOT NULL,
    PRIMARY KEY (minute, model, upstream, client, chat_kind)
);
CREATE TABLE stats_queue (
    minute      INTEGER PRIMARY KEY,
    depth_max   INTEGER NOT NULL,
    running_max INTEGER NOT NULL,
    slots       INTEGER NOT NULL
);
"#,
)];

#[derive(Clone, Debug)]
pub struct Store {
    pub pool: SqlitePool,
}

#[derive(Clone, Debug, Default, FromRow, Serialize)]
pub struct JobRow {
    pub job_id: String,
    pub client: String,
    pub model: String,
    pub model_used: String,
    pub chat_id: i64,
    pub reply_to: Option<i64>,
    pub thread_id: Option<i64>,
    pub protocol: i64,
    pub priority: i64,
    #[serde(skip)]
    pub payload: Option<String>,
    pub callback_url: Option<String>,
    #[serde(skip)]
    pub callback_token: Option<String>,
    pub state: String,
    pub delivery: String,
    pub attempts: i64,
    pub hops: i64,
    pub delivery_attempts: i64,
    pub next_delivery_at: Option<i64>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub delivered_at: Option<i64>,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub delivery_error: Option<String>,
    pub usage: Option<String>,
    #[serde(skip)]
    pub result_text: Option<String>,
    #[serde(skip)]
    pub tg_result: Option<String>,
    pub upstream_used: Option<String>,
    pub queued_ms: Option<i64>,
    pub latency_ms: Option<i64>,
}

impl JobRow {
    /// Состояние для API (§4.2): `dead`, если доставка исчерпана.
    pub fn public_state(&self) -> &str {
        if self.delivery == "dead" { "dead" } else { &self.state }
    }
}

#[derive(Clone, Debug)]
pub struct NewJob {
    pub job_id: String,
    pub client: String,
    pub model: String,
    pub model_used: String,
    pub chat_id: i64,
    pub reply_to: Option<i64>,
    pub thread_id: Option<i64>,
    pub protocol: i64,
    pub priority: i64,
    pub payload: String,
    pub callback_url: String,
    pub callback_token: String,
    pub created_at: i64,
}

/// Задание для восстановления очереди после старта.
#[derive(Clone, Debug, FromRow)]
pub struct QueuedLite {
    pub job_id: String,
    pub client: String,
    pub model_used: String,
    pub chat_id: i64,
    pub priority: i64,
    pub created_at: i64,
}

#[derive(Clone, Debug, Default)]
pub struct JobFilter {
    pub state: Option<String>,
    pub query: Option<String>,
    pub model: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

const JOB_COLUMNS: &str = "job_id, client, model, model_used, chat_id, reply_to, thread_id, protocol, priority, \
    payload, callback_url, callback_token, state, delivery, attempts, hops, delivery_attempts, next_delivery_at, \
    created_at, started_at, finished_at, delivered_at, error, error_code, delivery_error, usage, result_text, \
    tg_result, upstream_used, queued_ms, latency_ms";

pub const FINAL_STATES: &[&str] = &["done", "failed", "expired", "cancelled", "superseded"];

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).ok();
        }
        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(10))
            .foreign_keys(true);
        Self::open_with(opts).await.with_context(|| format!("open database {}", path.display()))
    }

    /// База в памяти (для тестов).
    pub async fn open_memory() -> Result<Self> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?;
        let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await?;
        let s = Self { pool };
        s.migrate().await?;
        Ok(s)
    }

    async fn open_with(opts: SqliteConnectOptions) -> Result<Self> {
        let pool = SqlitePoolOptions::new().max_connections(6).acquire_timeout(Duration::from_secs(10)).connect_with(opts).await?;
        let s = Self { pool };
        s.migrate().await?;
        Ok(s)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)").execute(&self.pool).await?;
        for (version, sql) in MIGRATIONS {
            let done: Option<(i64,)> =
                sqlx::query_as("SELECT version FROM schema_migrations WHERE version = ?").bind(version).fetch_optional(&self.pool).await?;
            if done.is_some() {
                continue;
            }
            let mut tx = self.pool.begin().await?;
            sqlx::raw_sql(sql).execute(&mut *tx).await.with_context(|| format!("migration {version}"))?;
            sqlx::query("INSERT INTO schema_migrations(version, applied_at) VALUES (?, ?)").bind(version).bind(now_ms()).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        Ok(())
    }

    pub async fn ping(&self) -> bool {
        sqlx::query("SELECT 1").execute(&self.pool).await.is_ok()
    }

    /// Вставка нового задания. `false` — такой `job_id` уже есть.
    pub async fn insert_job(&self, j: &NewJob) -> Result<bool> {
        let r = sqlx::query(
            "INSERT INTO jobs (job_id, client, model, model_used, chat_id, reply_to, thread_id, protocol, priority, \
             payload, callback_url, callback_token, state, delivery, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', 'none', ?) ON CONFLICT(job_id) DO NOTHING",
        )
        .bind(&j.job_id)
        .bind(&j.client)
        .bind(&j.model)
        .bind(&j.model_used)
        .bind(j.chat_id)
        .bind(j.reply_to)
        .bind(j.thread_id)
        .bind(j.protocol)
        .bind(j.priority)
        .bind(&j.payload)
        .bind(&j.callback_url)
        .bind(&j.callback_token)
        .bind(j.created_at)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    pub async fn get_job(&self, job_id: &str) -> Result<Option<JobRow>> {
        Ok(sqlx::query_as::<_, JobRow>(&format!("SELECT {JOB_COLUMNS} FROM jobs WHERE job_id = ?")).bind(job_id).fetch_optional(&self.pool).await?)
    }

    /// После перезапуска: `running` → `queued`, если попытки не исчерпаны, иначе — ошибка.
    pub async fn recover(&self, max_attempts: i64) -> Result<(u64, u64)> {
        let now = now_ms();
        let failed = sqlx::query(
            "UPDATE jobs SET state = 'failed', error = 'interrupted by restart', error_code = 'upstream_error', \
             finished_at = ?, delivery = 'pending', next_delivery_at = ? WHERE state = 'running' AND attempts >= ?",
        )
        .bind(now)
        .bind(now)
        .bind(max_attempts)
        .execute(&self.pool)
        .await?
        .rows_affected();
        let requeued = sqlx::query("UPDATE jobs SET state = 'queued', started_at = NULL WHERE state = 'running'").execute(&self.pool).await?.rows_affected();
        Ok((requeued, failed))
    }

    pub async fn load_queued(&self) -> Result<Vec<QueuedLite>> {
        Ok(sqlx::query_as::<_, QueuedLite>(
            "SELECT job_id, client, model_used, chat_id, priority, created_at FROM jobs WHERE state = 'queued' ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Задание начато. Возвращает строку (с payload) или `None`, если задание уже не в очереди.
    pub async fn mark_running(&self, job_id: &str, now: i64) -> Result<Option<JobRow>> {
        let r = sqlx::query(
            "UPDATE jobs SET state = 'running', started_at = ?, attempts = attempts + 1, \
             queued_ms = ? - created_at WHERE job_id = ? AND state = 'queued'",
        )
        .bind(now)
        .bind(now)
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_job(job_id).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn finish_ok(&self, job_id: &str, text: &str, usage: Option<&str>, upstream: &str, model_used: &str, latency_ms: i64, now: i64) -> Result<()> {
        sqlx::query(
            "UPDATE jobs SET state = 'done', result_text = ?, usage = ?, upstream_used = ?, model_used = ?, \
             latency_ms = ?, finished_at = ?, error = NULL, error_code = NULL, delivery = 'pending', \
             next_delivery_at = ? WHERE job_id = ?",
        )
        .bind(text)
        .bind(usage)
        .bind(upstream)
        .bind(model_used)
        .bind(latency_ms)
        .bind(now)
        .bind(now)
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Завершает задание с ошибкой (`failed`, `expired`, `cancelled`, `superseded`) и
    /// ставит доставку ошибки боту. Только из незавершённых состояний.
    pub async fn finish_err(&self, job_id: &str, state: &str, code: &str, error: &str, latency_ms: Option<i64>, now: i64) -> Result<bool> {
        let r = sqlx::query(
            "UPDATE jobs SET state = ?, error_code = ?, error = ?, latency_ms = COALESCE(?, latency_ms), \
             finished_at = ?, delivery = 'pending', next_delivery_at = ? \
             WHERE job_id = ? AND state IN ('queued', 'running')",
        )
        .bind(state)
        .bind(code)
        .bind(error)
        .bind(latency_ms)
        .bind(now)
        .bind(now)
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    /// Возвращает задание в очередь (резервная модель).
    pub async fn requeue(&self, job_id: &str, model_used: &str, hops: i64) -> Result<()> {
        sqlx::query("UPDATE jobs SET state = 'queued', model_used = ?, hops = ?, started_at = NULL WHERE job_id = ?")
            .bind(model_used)
            .bind(hops)
            .bind(job_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn due_deliveries(&self, now: i64, limit: i64) -> Result<Vec<JobRow>> {
        Ok(sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {JOB_COLUMNS} FROM jobs WHERE delivery = 'pending' AND next_delivery_at <= ? \
             ORDER BY next_delivery_at LIMIT ?"
        ))
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn next_delivery_at(&self) -> Result<Option<i64>> {
        let r: Option<(Option<i64>,)> = sqlx::query_as("SELECT MIN(next_delivery_at) FROM jobs WHERE delivery = 'pending'").fetch_optional(&self.pool).await?;
        Ok(r.and_then(|x| x.0))
    }

    /// Доставлено: тексты, payload и токен колбэка стираются (§5.1, §7).
    pub async fn mark_delivered(&self, job_id: &str, now: i64) -> Result<()> {
        sqlx::query(
            "UPDATE jobs SET delivery = 'delivered', delivered_at = ?, delivery_attempts = delivery_attempts + 1, \
             delivery_error = NULL, payload = NULL, result_text = NULL, callback_token = NULL, tg_result = NULL \
             WHERE job_id = ?",
        )
        .bind(now)
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn schedule_redelivery(&self, job_id: &str, attempts: i64, next_at: i64, err: &str) -> Result<()> {
        sqlx::query("UPDATE jobs SET delivery_attempts = ?, next_delivery_at = ?, delivery_error = ? WHERE job_id = ?")
            .bind(attempts)
            .bind(next_at)
            .bind(err)
            .bind(job_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn mark_dead(&self, job_id: &str, attempts: i64, err: &str) -> Result<()> {
        sqlx::query("UPDATE jobs SET delivery = 'dead', delivery_attempts = ?, delivery_error = ?, payload = NULL WHERE job_id = ?")
            .bind(attempts)
            .bind(err)
            .bind(job_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_tg_result(&self, job_id: &str, msg: &str) -> Result<()> {
        sqlx::query("UPDATE jobs SET tg_result = ? WHERE job_id = ?").bind(msg).bind(job_id).execute(&self.pool).await?;
        Ok(())
    }

    /// Повторная доставка «мёртвого» задания.
    pub async fn redeliver(&self, job_id: &str) -> Result<bool> {
        let r = sqlx::query(
            "UPDATE jobs SET delivery = 'pending', delivery_attempts = 0, next_delivery_at = ?, delivery_error = NULL \
             WHERE job_id = ? AND delivery = 'dead'",
        )
        .bind(now_ms())
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    pub async fn redeliver_all_dead(&self) -> Result<u64> {
        Ok(sqlx::query(
            "UPDATE jobs SET delivery = 'pending', delivery_attempts = 0, next_delivery_at = ?, delivery_error = NULL \
             WHERE delivery = 'dead'",
        )
        .bind(now_ms())
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    pub async fn list_jobs(&self, f: &JobFilter) -> Result<(Vec<JobRow>, i64)> {
        let mut where_ = vec!["1 = 1".to_string()];
        let mut binds: Vec<String> = Vec::new();
        if let Some(s) = f.state.as_deref().filter(|s| !s.is_empty()) {
            if s == "dead" {
                where_.push("delivery = 'dead'".into());
            } else if s == "pending_delivery" {
                where_.push("delivery = 'pending'".into());
            } else if s == "active" {
                where_.push("state IN ('queued', 'running')".into());
            } else {
                where_.push("state = ? AND delivery != 'dead'".into());
                binds.push(s.to_string());
            }
        }
        if let Some(m) = f.model.as_deref().filter(|s| !s.is_empty()) {
            where_.push("(model = ? OR model_used = ?)".into());
            binds.push(m.to_string());
            binds.push(m.to_string());
        }
        if let Some(q) = f.query.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            where_.push("(job_id LIKE ? OR CAST(chat_id AS TEXT) LIKE ? OR client LIKE ?)".into());
            let like = format!("%{q}%");
            binds.extend([like.clone(), like.clone(), like]);
        }
        let w = where_.join(" AND ");
        let count_sql = format!("SELECT COUNT(*) FROM jobs WHERE {w}");
        let mut cq = sqlx::query_as::<_, (i64,)>(&count_sql);
        for b in &binds {
            cq = cq.bind(b);
        }
        let total = cq.fetch_one(&self.pool).await?.0;
        let sql = format!("SELECT {JOB_COLUMNS} FROM jobs WHERE {w} ORDER BY created_at DESC LIMIT ? OFFSET ?");
        let mut q = sqlx::query_as::<_, JobRow>(&sql);
        for b in &binds {
            q = q.bind(b);
        }
        let limit = if f.limit <= 0 { 100 } else { f.limit.min(1000) };
        let rows = q.bind(limit).bind(f.offset.max(0)).fetch_all(&self.pool).await?;
        Ok((rows, total))
    }

    pub async fn count_by_state(&self) -> Result<Vec<(String, i64)>> {
        Ok(sqlx::query_as::<_, (String, i64)>(
            "SELECT CASE WHEN delivery = 'dead' THEN 'dead' WHEN delivery = 'pending' AND state NOT IN ('queued','running') \
             THEN 'pending_delivery' ELSE state END AS s, COUNT(*) FROM jobs GROUP BY s",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Чистка: завершённые и доставленные (или «мёртвые») старше срока — вместе с текстами.
    pub async fn cleanup(&self, retention_hours: u64) -> Result<u64> {
        let cutoff = now_ms() - (retention_hours as i64) * 3_600_000;
        Ok(sqlx::query("DELETE FROM jobs WHERE finished_at IS NOT NULL AND finished_at < ? AND delivery IN ('delivered', 'dead', 'none')")
            .bind(cutoff)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }

    /// Ручная очистка из панели: удаляет задания в перечисленных публичных состояниях.
    pub async fn purge(&self, states: &[String]) -> Result<u64> {
        let mut total = 0;
        for s in states {
            let sql = match s.as_str() {
                "dead" => "DELETE FROM jobs WHERE delivery = 'dead'".to_string(),
                "delivered" => "DELETE FROM jobs WHERE delivery = 'delivered'".to_string(),
                st if FINAL_STATES.contains(&st) => {
                    format!("DELETE FROM jobs WHERE state = '{st}' AND delivery IN ('delivered', 'dead', 'none')")
                }
                _ => continue,
            };
            total += sqlx::query(&sql).execute(&self.pool).await?.rows_affected();
        }
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, chat: i64) -> NewJob {
        NewJob {
            job_id: id.into(),
            client: "bot".into(),
            model: "m".into(),
            model_used: "m".into(),
            chat_id: chat,
            reply_to: Some(1),
            thread_id: None,
            protocol: 2,
            priority: 2,
            payload: "{}".into(),
            callback_url: "https://bot/webhook/llm".into(),
            callback_token: "enc:x".into(),
            created_at: now_ms(),
        }
    }

    #[tokio::test]
    async fn insert_is_idempotent() {
        let s = Store::open_memory().await.unwrap();
        assert!(s.insert_job(&job("a", -1)).await.unwrap());
        assert!(!s.insert_job(&job("a", -1)).await.unwrap());
        assert_eq!(s.load_queued().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn lifecycle_wipes_texts_after_delivery() {
        let s = Store::open_memory().await.unwrap();
        s.insert_job(&job("a", -1)).await.unwrap();
        let r = s.mark_running("a", now_ms()).await.unwrap().unwrap();
        assert_eq!(r.attempts, 1);
        s.finish_ok("a", "hello", None, "m#0", "m", 10, now_ms()).await.unwrap();
        let due = s.due_deliveries(now_ms() + 1, 10).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].result_text.as_deref(), Some("hello"));
        s.mark_delivered("a", now_ms()).await.unwrap();
        let r = s.get_job("a").await.unwrap().unwrap();
        assert!(r.result_text.is_none() && r.payload.is_none() && r.callback_token.is_none());
        assert_eq!(r.delivery, "delivered");
    }

    #[tokio::test]
    async fn recover_requeues_running() {
        let s = Store::open_memory().await.unwrap();
        s.insert_job(&job("a", -1)).await.unwrap();
        s.mark_running("a", now_ms()).await.unwrap();
        let (requeued, failed) = s.recover(2).await.unwrap();
        assert_eq!((requeued, failed), (1, 0));
        assert_eq!(s.get_job("a").await.unwrap().unwrap().state, "queued");
    }
}
