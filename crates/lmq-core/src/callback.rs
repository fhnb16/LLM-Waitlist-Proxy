//! Доставка результата боту (§3): `POST callback.url` с `X-Callback-Token`.
//!
//! `2xx` — доставлено (тексты стираются); `403` — без повторов, `dead`;
//! сеть, `5xx`, `429` — повторы с паузами 2, 5, 15, 30, 60 с, не больше `CALLBACK_RETRIES`.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use url::Url;

use crate::app::App;
use crate::config::CallbackCfg;
use crate::events::Event;
use crate::store::JobRow;
use crate::util::now_ms;

const DELAYS: [i64; 5] = [2, 5, 15, 30, 60];

pub fn retry_delay_s(attempt: i64) -> i64 {
    DELAYS[((attempt - 1).max(0) as usize).min(DELAYS.len() - 1)]
}

fn is_local_host(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        url::Host::Ipv6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80,
    }
}

fn host_matches(host: &str, pattern: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let pattern = pattern.trim().trim_end_matches('.').to_ascii_lowercase();
    if pattern.is_empty() {
        return false;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return host.ends_with(&format!(".{suffix}"));
    }
    // В списке может стоять host:port или полный URL — берём только хост.
    let pattern = Url::parse(&pattern).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| pattern.split(':').next().unwrap_or("").to_string());
    host == pattern.trim_start_matches('[').trim_end_matches(']')
}

/// Проверка `callback.url` (§7): только `https://` (или `http://` для localhost и частных
/// сетей при `CALLBACK_ALLOW_HTTP_PRIVATE`) и только хосты из белого списка.
pub fn validate_url(raw: &str, cfg: &CallbackCfg) -> Result<Url, String> {
    let u = Url::parse(raw.trim()).map_err(|_| "callback.url is not a valid URL".to_string())?;
    if !u.username().is_empty() || u.password().is_some() {
        return Err("callback.url must not contain credentials".into());
    }
    let host = u.host().ok_or_else(|| "callback.url has no host".to_string())?;
    match u.scheme() {
        "https" => {}
        "http" if cfg.allow_http_private && is_local_host(&host) => {}
        "http" => return Err("callback.url must use https".into()),
        _ => return Err("callback.url must use https".into()),
    }
    let host_s = match &host {
        url::Host::Domain(d) => d.to_string(),
        url::Host::Ipv4(ip) => IpAddr::V4(*ip).to_string(),
        url::Host::Ipv6(ip) => IpAddr::V6(*ip).to_string(),
    };
    let allowed: Vec<String> = if !cfg.allowed_hosts.is_empty() {
        cfg.allowed_hosts.clone()
    } else {
        Url::parse(&cfg.url).ok().and_then(|b| b.host_str().map(String::from)).into_iter().collect()
    };
    if allowed.iter().any(|p| host_matches(&host_s, p)) { Ok(u) } else { Err(format!("callback host '{host_s}' is not in CALLBACK_ALLOWED_HOSTS")) }
}

/// Тело колбэка протокола 2 (§3.1).
pub fn build_body(row: &JobRow) -> Value {
    let usage: Value = row.usage.as_deref().and_then(|u| serde_json::from_str(u).ok()).unwrap_or(Value::Null);
    if row.state == "done" {
        json!({
            "job_id": row.job_id,
            "chat_id": row.chat_id,
            "reply_to": row.reply_to,
            "thread_id": row.thread_id,
            "text": row.result_text,
            "model": row.model_used,
            "usage": usage,
            "queued_ms": row.queued_ms,
            "latency_ms": row.latency_ms,
        })
    } else {
        json!({
            "job_id": row.job_id,
            "chat_id": row.chat_id,
            "reply_to": row.reply_to,
            "thread_id": row.thread_id,
            "text": Value::Null,
            "error": row.error.clone().unwrap_or_else(|| row.state.clone()),
            "error_code": row.error_code.clone().unwrap_or_else(|| row.state.clone()),
            "model": row.model_used,
            "queued_ms": row.queued_ms,
            "latency_ms": row.latency_ms,
        })
    }
}

pub async fn run(app: Arc<App>) {
    let inflight: Arc<Mutex<HashSet<String>>> = Arc::default();
    let sem = Arc::new(Semaphore::new(8));
    loop {
        let now = now_ms();
        match app.store.due_deliveries(now, 64).await {
            Ok(rows) => {
                for row in rows {
                    if !inflight.lock().insert(row.job_id.clone()) {
                        continue;
                    }
                    let Ok(permit) = sem.clone().acquire_owned().await else { return };
                    let app = app.clone();
                    let inflight = inflight.clone();
                    tokio::spawn(async move {
                        let id = row.job_id.clone();
                        deliver(&app, row).await;
                        inflight.lock().remove(&id);
                        drop(permit);
                        app.delivery_wake.notify_one();
                    });
                }
            }
            Err(e) => tracing::error!(error = %e, "delivery query failed"),
        }
        let next = app.store.next_delivery_at().await.ok().flatten();
        let wait = next.map(|t| (t - now_ms()).clamp(200, 5_000)).unwrap_or(5_000) as u64;
        tokio::select! {
            _ = app.delivery_wake.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
            _ = app.shutdown.cancelled() => break,
        }
    }
}

async fn deliver(app: &Arc<App>, row: JobRow) {
    let cfg = app.settings();
    let now = now_ms();
    let url = row.callback_url.clone().filter(|u| !u.is_empty());
    let body = if row.protocol == 1 && row.state == "done" {
        match telegram_result(app, &row).await {
            Ok(msg) => json!({ "result": msg }),
            Err(e) => {
                schedule_retry(app, &row, &format!("telegram: {e}"), now).await;
                return;
            }
        }
    } else {
        build_body(&row)
    };
    let Some(url) = url else {
        if row.protocol == 1 {
            // Протокол 1 без колбэка: ответ уже в Telegram.
            let _ = app.store.mark_delivered(&row.job_id, now_ms()).await;
        } else {
            let _ = app.store.mark_dead(&row.job_id, row.delivery_attempts, "no callback url").await;
        }
        return;
    };
    let token = match row.callback_token.as_deref() {
        Some(t) => app.cipher.decrypt_str(t).unwrap_or_default(),
        None => String::new(),
    };
    let route = crate::net::describe(&cfg.network.callback);
    let http = match app.callback_client() {
        Ok(c) => c,
        Err(e) => {
            schedule_retry(app, &row, &format!("route: {e}"), now).await;
            return;
        }
    };
    let mut req = http.post(&url).timeout(Duration::from_secs(cfg.callback.timeout.max(1))).json(&body);
    if !token.is_empty() {
        req = req.header("X-Callback-Token", token);
    }
    let attempt = row.delivery_attempts + 1;
    let host = reqwest::Url::parse(&url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_default();
    if cfg.logging.requests {
        let shown = if cfg.logging.content { body.to_string() } else { callback_shape(&body).to_string() };
        tracing::info!(job_id = %row.job_id, url = %url, route = %route, attempt, body = %shown, "callback request");
    }
    let t = std::time::Instant::now();
    let res = req.send().await;
    if cfg.logging.requests
        && let Ok(r) = &res
    {
        tracing::info!(job_id = %row.job_id, status = r.status().as_u16(), ms = t.elapsed().as_millis() as u64, "callback response");
    }
    match res {
        Ok(r) if r.status().is_success() => {
            if let Err(e) = app.store.mark_delivered(&row.job_id, now_ms()).await {
                tracing::error!(job_id = %row.job_id, error = %e, "mark delivered failed");
            }
            tracing::info!(
                job_id = %row.job_id, client = %row.client, model = %row.model_used, chat_id = row.chat_id,
                state = %row.state, attempt, "callback delivered"
            );
            app.events.emit(Event::Job {
                job_id: row.job_id.clone(),
                state: row.state.clone(),
                model: row.model_used.clone(),
                chat_id: row.chat_id,
                error_code: row.error_code.clone(),
            });
        }
        Ok(r) if r.status().as_u16() == 403 => {
            let _ = app.store.mark_dead(&row.job_id, attempt, "callback_forbidden").await;
            tracing::warn!(job_id = %row.job_id, client = %row.client, "callback forbidden (403): check callback token");
            app.events.emit(Event::Job {
                job_id: row.job_id.clone(),
                state: "dead".into(),
                model: row.model_used.clone(),
                chat_id: row.chat_id,
                error_code: Some("callback_forbidden".into()),
            });
        }
        Ok(r) => {
            let status = r.status().as_u16();
            let text = r.text().await.unwrap_or_default();
            let excerpt: String = text.trim().chars().take(300).collect();
            let err = if excerpt.is_empty() { format!("HTTP {status}") } else { format!("HTTP {status}: {excerpt}") };
            schedule_retry(app, &row, &err, now).await
        }
        Err(e) => {
            // Полная цепочка причин и маршрут: «connect timed out … (интерфейс Ethernet)».
            let chain = crate::netdiag::error_chain(&e.without_url());
            schedule_retry(app, &row, &format!("network to {host}: {chain} ({route}, {} ms)", t.elapsed().as_millis()), now).await
        }
    }
}

/// Тело колбэка без текста ответа — для подробного журнала при `LOG_CONTENT=false`.
fn callback_shape(body: &Value) -> Value {
    let mut b = body.clone();
    if let Some(o) = b.as_object_mut() {
        for k in ["text", "result"] {
            if let Some(v) = o.get_mut(k) {
                let n = v.as_str().map(|s| s.chars().count()).unwrap_or(0);
                *v = json!(format!("<{n} chars>"));
            }
        }
    }
    b
}

async fn schedule_retry(app: &Arc<App>, row: &JobRow, err: &str, now: i64) {
    let retries = app.settings().callback.retries as i64;
    let attempt = row.delivery_attempts + 1;
    if attempt > retries {
        let _ = app.store.mark_dead(&row.job_id, attempt, &format!("callback_failed: {err}")).await;
        tracing::warn!(job_id = %row.job_id, client = %row.client, attempts = attempt, error = %err, "callback failed, job is dead");
        app.metrics_callback_failure();
        app.events.emit(Event::Job {
            job_id: row.job_id.clone(),
            state: "dead".into(),
            model: row.model_used.clone(),
            chat_id: row.chat_id,
            error_code: Some("callback_failed".into()),
        });
    } else {
        let next = now + retry_delay_s(attempt) * 1000;
        let _ = app.store.schedule_redelivery(&row.job_id, attempt, next, err).await;
        app.metrics_callback_failure();
        tracing::info!(job_id = %row.job_id, attempt, error = %err, retry_in_s = retry_delay_s(attempt), "callback retry scheduled");
    }
}

/// Протокол 1: сообщение в Telegram (один раз), результат сохраняется для повторов колбэка.
async fn telegram_result(app: &Arc<App>, row: &JobRow) -> anyhow::Result<Value> {
    if let Some(saved) = row.tg_result.as_deref() {
        return Ok(serde_json::from_str(saved)?);
    }
    let cfg = app.settings();
    let http = app.telegram_client().map_err(|e| anyhow::anyhow!("route: {e}"))?;
    let msg = crate::telegram::send_message(&http, &cfg.protocol1, row.chat_id, row.result_text.as_deref().unwrap_or(""), row.reply_to, row.thread_id).await?;
    app.store.save_tg_result(&row.job_id, &msg.to_string()).await?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(url: &str, hosts: &[&str], http: bool) -> CallbackCfg {
        CallbackCfg { url: url.into(), allowed_hosts: hosts.iter().map(|s| s.to_string()).collect(), allow_http_private: http, ..Default::default() }
    }

    #[test]
    fn ssrf_rules() {
        let c = cfg("https://bot.example.com/webhook/llm", &[], false);
        assert!(validate_url("https://bot.example.com/webhook/llm", &c).is_ok());
        assert!(validate_url("https://evil.example.com/x", &c).is_err());
        assert!(validate_url("http://bot.example.com/x", &c).is_err());
        assert!(validate_url("https://user:pw@bot.example.com/x", &c).is_err());
        assert!(validate_url("ftp://bot.example.com/x", &c).is_err());

        let c = cfg("", &["*.example.org", "api.test:443"], false);
        assert!(validate_url("https://a.example.org/x", &c).is_ok());
        assert!(validate_url("https://example.org/x", &c).is_err());
        assert!(validate_url("https://api.test/x", &c).is_ok());

        let none = cfg("", &[], false);
        assert!(validate_url("https://bot.example.com/x", &none).is_err());

        let local = cfg("", &["127.0.0.1", "localhost", "10.0.0.5"], true);
        assert!(validate_url("http://127.0.0.1:8801/admin/debug/callback", &local).is_ok());
        assert!(validate_url("http://10.0.0.5/x", &local).is_ok());
        let local_off = cfg("", &["127.0.0.1"], false);
        assert!(validate_url("http://127.0.0.1/x", &local_off).is_err());
    }

    #[test]
    fn delays() {
        assert_eq!(retry_delay_s(1), 2);
        assert_eq!(retry_delay_s(2), 5);
        assert_eq!(retry_delay_s(5), 60);
        assert_eq!(retry_delay_s(9), 60);
    }
}
