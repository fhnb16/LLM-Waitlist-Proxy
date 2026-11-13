//! Диспетчер: выдаёт задания моделям по свободным слотам и исполняет их.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use tokio::sync::{OwnedSemaphorePermit, mpsc};
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::queue::{QItem, SyncCtx, TIER_GROUP, TIER_PRIVATE};
use crate::routing::{ModelRt, UpstreamRt};
use crate::store::SyncRecord;
use crate::upstream::{self, UpErr, UpOk};
use crate::util::{now_ms, strip_think};

pub async fn run(app: Arc<App>) {
    loop {
        dispatch_round(&app).await;
        tokio::select! {
            _ = app.queue.wake.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = app.shutdown.cancelled() => break,
        }
    }
}

/// Свободен ли апстрим: выключатель замкнут, движок (если привязан) готов.
pub fn upstream_usable(app: &App, up: &UpstreamRt) -> bool {
    let cfg = app.settings();
    if let Some(e) = &up.engine
        && !app.engines.is_ready(e)
    {
        return false;
    }
    up.health.available(&cfg.breaker)
}

fn pick_upstream(app: &App, m: &ModelRt, exclude: &HashSet<String>) -> Option<(Arc<UpstreamRt>, OwnedSemaphorePermit)> {
    let mut cands: Vec<&Arc<UpstreamRt>> = m.upstreams.iter().filter(|u| !exclude.contains(&u.id) && u.sem.available_permits() > 0).collect();
    cands.sort_by_key(|u| std::cmp::Reverse(u.sem.available_permits()));
    for u in cands {
        if !upstream_usable(app, u) {
            continue;
        }
        if let Ok(p) = u.sem.clone().try_acquire_owned() {
            return Some((u.clone(), p));
        }
    }
    None
}

async fn dispatch_round(app: &Arc<App>) {
    if app.paused.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let routing = app.routing();
    let sched = app.schedule();
    for model in app.queue.models_with_work() {
        let Some(m) = routing.models.get(&model).cloned() else {
            fail_orphans(app, &model).await;
            continue;
        };
        if sched.dispatch_paused(&model) {
            continue;
        }
        let cap = sched.slot_cap(&model);
        loop {
            let Some((up, permit)) = pick_upstream(app, &m, &HashSet::new()) else { break };
            let Some((item, cancel)) = app.queue.take_next(&model, cap, &up.id) else {
                drop(permit);
                break;
            };
            let app = app.clone();
            let m = m.clone();
            tokio::spawn(async move { run_item(app, m, up, permit, item, cancel).await });
        }
    }
}

/// Модель удалена из конфигурации — её задания завершаются ошибкой.
async fn fail_orphans(app: &Arc<App>, model: &str) {
    for mut item in app.queue.drain_model(model) {
        let msg = format!("model '{model}' was removed from configuration");
        match item.sync.take() {
            Some(mut s) => {
                if let Some(tx) = s.tx.take() {
                    let _ = tx.send(Err((503, "upstream_error".into(), msg)));
                }
            }
            None => {
                let _ = app.store.finish_err(&item.job_id, "failed", "upstream_error", &msg, None, now_ms()).await;
                app.emit_job(&item.job_id, "failed", model, item.chat_id, Some("upstream_error"));
            }
        }
    }
    app.delivery_wake.notify_one();
}

/// Снимает отметку «выполняется», даже если исполнитель упал.
struct FinishGuard<'a> {
    app: &'a App,
    job_id: String,
}

impl Drop for FinishGuard<'_> {
    fn drop(&mut self) {
        self.app.queue.finish(&self.job_id);
    }
}

async fn run_item(app: Arc<App>, model: Arc<ModelRt>, up: Arc<UpstreamRt>, permit: OwnedSemaphorePermit, mut item: QItem, cancel: CancellationToken) {
    let _guard = FinishGuard { app: &app, job_id: item.job_id.clone() };
    match item.sync.take() {
        Some(ctx) => run_sync(&app, &model, up, permit, item, ctx, cancel).await,
        None => run_async(&app, &model, up, permit, item, cancel).await,
    }
}

pub enum ExecErr {
    Cancelled,
    Failed { code: &'static str, msg: String },
}

/// Ждёт слот другого апстрима модели (не из `tried`).
async fn acquire_other(app: &App, model: &ModelRt, tried: &HashSet<String>, cancel: &CancellationToken) -> Option<(Arc<UpstreamRt>, OwnedSemaphorePermit)> {
    if let Some(h) = pick_upstream(app, model, tried) {
        return Some(h);
    }
    let cand = model.upstreams.iter().find(|u| !tried.contains(&u.id) && upstream_usable(app, u))?.clone();
    tokio::select! {
        r = tokio::time::timeout(Duration::from_secs(30), cand.sem.clone().acquire_owned()) => match r {
            Ok(Ok(p)) => Some((cand, p)),
            _ => None,
        },
        _ = cancel.cancelled() => None,
    }
}

/// Запрос к модели с перебором апстримов (ошибка соединения или 5xx → следующий) и
/// повтором всего круга до `UPSTREAM_RETRIES` раз.
pub async fn execute(
    app: &App,
    job_id: &str,
    model: &ModelRt,
    first: (Arc<UpstreamRt>, OwnedSemaphorePermit),
    payload: &Map<String, Value>,
    cancel: &CancellationToken,
    deltas: Option<&mpsc::UnboundedSender<String>>,
) -> Result<(UpOk, Arc<UpstreamRt>), ExecErr> {
    let cfg = app.settings();
    let retries = cfg.queue.upstream_retries;
    let mut held = Some(first);
    let mut last = String::from("no upstream available");
    for attempt in 0..=retries {
        let mut tried: HashSet<String> = HashSet::new();
        loop {
            let (up, permit) = match held.take() {
                Some(h) => h,
                None => match acquire_other(app, model, &tried, cancel).await {
                    Some(h) => h,
                    None => break,
                },
            };
            if cancel.is_cancelled() {
                return Err(ExecErr::Cancelled);
            }
            tried.insert(up.id.clone());
            app.queue.set_running_upstream(job_id, &up.id);
            let timeout = up.timeout.unwrap_or(Duration::from_secs(cfg.queue.upstream_timeout.max(1)));
            let trace = upstream::Trace { job_id, requests: cfg.logging.requests, content: cfg.logging.content };
            let res = match app.upstream_client(&up) {
                Ok(http) => upstream::call(&http, &up, payload, &model.cfg, timeout, cancel, deltas, trace).await,
                Err(e) => Err(UpErr::Connect(format!("route: {e}"))),
            };
            drop(permit);
            match res {
                Ok(ok) => {
                    if up.health.on_success() {
                        app.emit_upstream(&up, "closed", None);
                    }
                    return Ok((ok, up));
                }
                Err(UpErr::Cancelled) => return Err(ExecErr::Cancelled),
                Err(e @ UpErr::Timeout(_)) => {
                    app.note_upstream_failure(&up, &e);
                    return Err(ExecErr::Failed { code: "upstream_timeout", msg: e.message() });
                }
                Err(e) if e.failover() => {
                    app.note_upstream_failure(&up, &e);
                    last = e.message();
                    tracing::warn!(job_id, upstream = %up.id, error = %last, "upstream failed, trying next");
                }
                Err(e) => {
                    app.metrics_upstream_error(&model.name, "upstream_error");
                    return Err(ExecErr::Failed { code: "upstream_error", msg: e.message() });
                }
            }
        }
        if attempt < retries {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                _ = cancel.cancelled() => return Err(ExecErr::Cancelled),
            }
        }
    }
    Err(ExecErr::Failed { code: "upstream_error", msg: last })
}

async fn run_async(app: &Arc<App>, model: &Arc<ModelRt>, up: Arc<UpstreamRt>, permit: OwnedSemaphorePermit, item: QItem, cancel: CancellationToken) {
    let now = now_ms();
    let row = match app.store.mark_running(&item.job_id, now).await {
        Ok(Some(r)) => r,
        Ok(None) => return, // уже отменено или вытеснено
        Err(e) => {
            tracing::error!(job_id = %item.job_id, error = %e, "mark running failed");
            return;
        }
    };
    let chat_kind = if row.chat_id > 0 { "private" } else { "group" };
    let wait_ms = now - row.created_at;
    app.emit_job(&row.job_id, "running", &model.name, row.chat_id, None);
    tracing::info!(
        job_id = %row.job_id, client = %row.client, model = %model.name, chat_id = row.chat_id,
        state = "running", wait_ms, upstream = %up.id, "job started"
    );
    let payload: Map<String, Value> = row.payload.as_deref().and_then(|p| serde_json::from_str(p).ok()).unwrap_or_default();
    let res = execute(app, &row.job_id, model, (up, permit), &payload, &cancel, None).await;
    let fin = now_ms();
    let log_content = app.settings().logging.content;
    match res {
        Ok((ok, up)) => {
            let (pt, ct) = upstream::usage_tokens(ok.usage.as_ref());
            if strip_think(&ok.text).trim().is_empty() {
                let reason = upstream::empty_answer_reason(&ok.body);
                let _ = app.store.finish_err(&row.job_id, "failed", "empty_answer", &reason, Some(ok.latency_ms), fin).await;
                app.record(&model.name, Some(&up.id), &row.client, chat_kind, Some("empty_answer"), Some(wait_ms), Some(ok.latency_ms), 0, 0);
                app.emit_job(&row.job_id, "failed", &model.name, row.chat_id, Some("empty_answer"));
                tracing::warn!(job_id = %row.job_id, model = %model.name, upstream = %up.id, latency_ms = ok.latency_ms, error_code = "empty_answer", error = %reason, "job failed");
            } else {
                let usage = ok.usage.as_ref().map(|u| u.to_string());
                if let Err(e) = app.store.finish_ok(&row.job_id, &ok.text, usage.as_deref(), &up.id, &model.name, ok.latency_ms, fin).await {
                    tracing::error!(job_id = %row.job_id, error = %e, "store result failed");
                }
                app.record(&model.name, Some(&up.id), &row.client, chat_kind, None, Some(wait_ms), Some(ok.latency_ms), pt, ct);
                app.emit_job(&row.job_id, "done", &model.name, row.chat_id, None);
                if log_content {
                    tracing::info!(
                        job_id = %row.job_id, client = %row.client, model = %model.name, chat_id = row.chat_id, state = "done",
                        wait_ms, latency_ms = ok.latency_ms, tokens = pt + ct, text = %ok.text, "job done"
                    );
                } else {
                    tracing::info!(
                        job_id = %row.job_id, client = %row.client, model = %model.name, chat_id = row.chat_id, state = "done",
                        wait_ms, latency_ms = ok.latency_ms, tokens = pt + ct, "job done"
                    );
                }
            }
        }
        Err(ExecErr::Cancelled) => {
            let _ = app.store.finish_err(&row.job_id, "cancelled", "cancelled", "cancelled via API", None, fin).await;
            app.record(&model.name, None, &row.client, chat_kind, Some("cancelled"), Some(wait_ms), None, 0, 0);
            app.emit_job(&row.job_id, "cancelled", &model.name, row.chat_id, Some("cancelled"));
            tracing::info!(job_id = %row.job_id, model = %model.name, state = "cancelled", "job cancelled while running");
        }
        Err(ExecErr::Failed { code, msg }) => {
            // Резерв: следующая модель из цепочки fallback.
            if let Some(next) = fallback_target(app, model, row.hops)
                && app.store.requeue(&row.job_id, &next, row.hops + 1).await.is_ok()
            {
                let tier = if row.priority == 1 { TIER_PRIVATE } else { TIER_GROUP };
                app.record(&model.name, None, &row.client, chat_kind, Some(code), Some(wait_ms), None, 0, 0);
                app.queue.push(
                    &next,
                    tier,
                    QItem { job_id: row.job_id.clone(), chat_id: row.chat_id, created_at: row.created_at, client: row.client.clone(), sync: None },
                );
                app.emit_job(&row.job_id, "queued", &next, row.chat_id, None);
                tracing::warn!(job_id = %row.job_id, model = %model.name, fallback = %next, error_code = code, error = %msg, "job moved to fallback model");
                return;
            }
            let _ = app.store.finish_err(&row.job_id, "failed", code, &msg, None, fin).await;
            app.record(&model.name, None, &row.client, chat_kind, Some(code), Some(wait_ms), None, 0, 0);
            app.emit_job(&row.job_id, "failed", &model.name, row.chat_id, Some(code));
            tracing::warn!(job_id = %row.job_id, client = %row.client, model = %model.name, chat_id = row.chat_id, state = "failed", error_code = code, error = %msg, "job failed");
        }
    }
    app.delivery_wake.notify_one();
}

/// Следующая резервная модель (`fallback[hops]`), если она есть в таблице.
pub fn fallback_target(app: &App, model: &ModelRt, hops: i64) -> Option<String> {
    let routing = app.routing();
    model.cfg.fallback.iter().skip(hops.max(0) as usize).find_map(|f| routing.resolve(f).map(|m| m.name.clone())).filter(|n| n != &model.name)
}

async fn run_sync(
    app: &Arc<App>,
    model: &Arc<ModelRt>,
    up: Arc<UpstreamRt>,
    permit: OwnedSemaphorePermit,
    item: QItem,
    mut ctx: SyncCtx,
    cancel: CancellationToken,
) {
    let started = now_ms();
    let wait_ms = started - item.created_at;
    app.emit_job(&item.job_id, "running", &model.name, 0, None);
    let res = execute(app, &item.job_id, model, (up, permit), &ctx.payload, &cancel, ctx.deltas.as_ref()).await;
    // Итог попадает в журнал и в историю очереди (без текстов), как у обычных заданий.
    let mut rec = SyncRecord {
        job_id: &item.job_id,
        client: &ctx.client,
        model: &ctx.requested_model,
        model_used: &model.name,
        state: "done",
        created_at: item.created_at,
        started_at: Some(started),
        finished_at: now_ms(),
        queued_ms: Some(wait_ms),
        ..Default::default()
    };
    let usage_text;
    let up_id;
    let fail_msg;
    let result = match res {
        Ok((ok, up)) => {
            let (pt, ct) = upstream::usage_tokens(ok.usage.as_ref());
            app.record(&model.name, Some(&up.id), &ctx.client, "sync", None, Some(wait_ms), Some(ok.latency_ms), pt, ct);
            let images = crate::admin::image_count(ctx.payload.get("messages"));
            if strip_think(&ok.text).trim().is_empty() {
                // Синхронному клиенту ответ уходит как есть, но в журнале причина должна быть видна.
                tracing::warn!(job_id = %item.job_id, client = %ctx.client, model = %model.name, upstream = %up.id, error = %upstream::empty_answer_reason(&ok.body), "sync answer is empty");
            }
            tracing::info!(
                job_id = %item.job_id, client = %ctx.client, model = %model.name, chat_id = 0, state = "done", upstream = %up.id,
                wait_ms, latency_ms = ok.latency_ms, tokens = pt + ct, images, "sync request done"
            );
            usage_text = ok.usage.as_ref().map(|u| u.to_string());
            up_id = up.id.clone();
            rec.usage = usage_text.as_deref();
            rec.upstream = Some(&up_id);
            rec.latency_ms = Some(ok.latency_ms);
            let mut body = ok.body;
            if let Some(o) = body.as_object_mut() {
                o.insert("model".into(), Value::String(ctx.requested_model.clone()));
            }
            Ok(body)
        }
        Err(ExecErr::Cancelled) => {
            tracing::info!(job_id = %item.job_id, client = %ctx.client, model = %model.name, chat_id = 0, state = "cancelled", wait_ms, "sync request cancelled");
            rec.state = "cancelled";
            rec.error_code = Some("cancelled");
            Err((504, "cancelled".to_string(), "request cancelled".to_string()))
        }
        Err(ExecErr::Failed { code, msg }) => {
            app.record(&model.name, None, &ctx.client, "sync", Some(code), Some(wait_ms), None, 0, 0);
            tracing::warn!(
                job_id = %item.job_id, client = %ctx.client, model = %model.name, chat_id = 0, state = "failed",
                wait_ms, error_code = code, error = %msg, "sync request failed"
            );
            fail_msg = msg.clone();
            rec.state = "failed";
            rec.error_code = Some(code);
            rec.error = Some(&fail_msg);
            let status = if code == "upstream_timeout" { 504 } else { 502 };
            Err((status, code.to_string(), msg))
        }
    };
    if let Err(e) = app.store.insert_sync_record(&rec).await {
        tracing::error!(job_id = %item.job_id, error = %e, "store sync record failed");
    }
    app.emit_job(&item.job_id, rec.state, &model.name, 0, rec.error_code);
    if let Some(tx) = ctx.tx.take() {
        let _ = tx.send(result);
    }
}
