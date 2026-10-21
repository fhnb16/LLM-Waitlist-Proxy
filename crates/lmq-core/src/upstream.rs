//! Вызов OpenAI-совместимого апстрима.

use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::ModelCfg;
use crate::routing::UpstreamRt;

/// Поля очереди, которые в модель не передаются (§2.1).
pub const QUEUE_FIELDS: &[&str] = &["protocol", "delivery", "_delivery", "job_id", "chat_id", "reply_to", "thread_id", "callback"];

#[derive(Debug, Clone)]
pub struct UpOk {
    /// Ответ апстрима в форме OpenAI (при стриме — собранный).
    pub body: Value,
    pub text: String,
    pub usage: Option<Value>,
    pub latency_ms: i64,
}

#[derive(Debug, Clone)]
pub enum UpErr {
    /// Соединение не установлено или оборвано.
    Connect(String),
    /// Апстрим ответил ошибкой.
    Status(u16, String),
    Timeout(u64),
    Cancelled,
    /// Ответ не разобран.
    Bad(String),
}

impl UpErr {
    /// Переходить ли к следующему апстриму (§5.4: ошибка соединения или 5xx).
    pub fn failover(&self) -> bool {
        matches!(self, UpErr::Connect(_) | UpErr::Bad(_)) || matches!(self, UpErr::Status(s, _) if *s >= 500)
    }

    pub fn message(&self) -> String {
        match self {
            UpErr::Connect(e) => format!("upstream connection error: {e}"),
            UpErr::Status(s, b) => format!("upstream HTTP {s}: {b}"),
            UpErr::Timeout(t) => format!("upstream timeout after {t}s"),
            UpErr::Cancelled => "cancelled".into(),
            UpErr::Bad(e) => format!("bad upstream response: {e}"),
        }
    }
}

/// Тело для апстрима: присланное без полей очереди, `defaults` (только отсутствующие),
/// `force` (перекрывают), имя модели апстрима, `stream`.
pub fn build_payload(src: &Map<String, Value>, model: &ModelCfg, target_model: &str, stream: bool) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in src {
        if !QUEUE_FIELDS.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in &model.defaults {
        if !out.contains_key(k) || out.get(k) == Some(&Value::Null) {
            out.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in &model.force {
        out.insert(k.clone(), v.clone());
    }
    out.insert("model".into(), Value::String(target_model.to_string()));
    out.insert("stream".into(), Value::Bool(stream));
    if stream && !out.contains_key("stream_options") {
        out.insert("stream_options".into(), json!({"include_usage": true}));
    }
    if !stream {
        out.remove("stream_options");
    }
    out
}

fn content_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let s: String = parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str).or_else(|| p.as_str())).collect::<Vec<_>>().join("");
            Some(s)
        }
        _ => None,
    }
}

/// Текст ответа: `choices[0].message.content`, иначе `choices[0].text`.
/// `reasoning_content` не берётся (§3.1).
pub fn extract_text(body: &Value) -> String {
    let choice = body.get("choices").and_then(|c| c.get(0));
    let from_message = choice.and_then(|c| c.get("message")).and_then(|m| m.get("content")).and_then(content_to_string).filter(|s| !s.is_empty());
    from_message.or_else(|| choice.and_then(|c| c.get("text")).and_then(Value::as_str).map(String::from)).unwrap_or_default()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// Сообщение об ошибке апстрима: `error.message`, если есть, иначе начало тела.
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.get("message").and_then(Value::as_str).map(String::from).or_else(|| e.as_str().map(String::from))))
        .unwrap_or_else(|| truncate(body.trim(), 300))
}

/// Один запрос к апстриму. `deltas` — куда слать куски текста (песочница); при нём
/// запрос идёт стримом, даже если апстрим настроен без стрима.
pub async fn call(
    http: &reqwest::Client,
    up: &UpstreamRt,
    payload: &Map<String, Value>,
    model: &ModelCfg,
    timeout: Duration,
    cancel: &CancellationToken,
    deltas: Option<&mpsc::UnboundedSender<String>>,
) -> Result<UpOk, UpErr> {
    let stream = up.stream || deltas.is_some();
    let body = build_payload(payload, model, &up.target_model, stream);
    let started = Instant::now();
    let fut = async {
        let mut req = http.post(&up.url).json(&body);
        if !up.api_key.is_empty() {
            req = req.bearer_auth(&up.api_key);
        }
        let resp = req.send().await.map_err(|e| UpErr::Connect(short_reqwest(&e)))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(UpErr::Status(status.as_u16(), error_message(&text)));
        }
        if stream { read_stream(resp, &up.target_model, deltas).await } else { read_json(resp).await }
    };
    let res = tokio::select! {
        r = tokio::time::timeout(timeout, fut) => match r {
            Ok(r) => r,
            Err(_) => Err(UpErr::Timeout(timeout.as_secs())),
        },
        _ = cancel.cancelled() => Err(UpErr::Cancelled),
    };
    res.map(|(body, text, usage)| UpOk { body, text, usage, latency_ms: started.elapsed().as_millis() as i64 })
}

fn short_reqwest(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(inner) = src {
        s = format!("{s}: {inner}");
        src = inner.source();
    }
    truncate(&s, 300)
}

async fn read_json(resp: reqwest::Response) -> Result<(Value, String, Option<Value>), UpErr> {
    let bytes = resp.bytes().await.map_err(|e| UpErr::Connect(short_reqwest(&e)))?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|e| UpErr::Bad(format!("not JSON: {e}")))?;
    if body.get("choices").is_none() {
        if let Some(err) = body.get("error") {
            return Err(UpErr::Bad(error_message(&err.to_string())));
        }
        return Err(UpErr::Bad("no choices in response".into()));
    }
    let text = extract_text(&body);
    let usage = body.get("usage").cloned().filter(|u| !u.is_null());
    Ok((body, text, usage))
}

async fn read_stream(resp: reqwest::Response, model: &str, deltas: Option<&mpsc::UnboundedSender<String>>) -> Result<(Value, String, Option<Value>), UpErr> {
    let mut text = String::new();
    let mut usage: Option<Value> = None;
    let mut finish: Value = Value::Null;
    let mut id = Value::Null;
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    let mut done = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| UpErr::Connect(short_reqwest(&e)))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                done = true;
                break;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(err) = v.get("error") {
                return Err(UpErr::Bad(error_message(&err.to_string())));
            }
            if id.is_null() {
                id = v.get("id").cloned().unwrap_or(Value::Null);
            }
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                usage = Some(u.clone());
            }
            if let Some(c) = v.get("choices").and_then(|c| c.get(0)) {
                let piece = c
                    .get("delta")
                    .and_then(|d| d.get("content"))
                    .and_then(content_to_string)
                    .or_else(|| c.get("text").and_then(Value::as_str).map(String::from));
                if let Some(p) = piece.filter(|p| !p.is_empty()) {
                    if let Some(tx) = deltas {
                        let _ = tx.send(p.clone());
                    }
                    text.push_str(&p);
                }
                if let Some(fr) = c.get("finish_reason").filter(|f| !f.is_null()) {
                    finish = fr.clone();
                }
            }
        }
        if done {
            break;
        }
    }
    let body = json!({
        "id": id,
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model,
        "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": if finish.is_null() { json!("stop") } else { finish }}],
        "usage": usage.clone().unwrap_or(Value::Null),
    });
    Ok((body, text, usage))
}

/// Фоновая проверка доступности: `GET {base}/models`. Любой ответ, кроме 5xx и обрыва, —
/// апстрим жив (401 тоже значит «сервер на месте»).
pub async fn check(http: &reqwest::Client, up: &UpstreamRt) -> (bool, i64, Option<String>) {
    let url = format!("{}/models", up.base_url());
    let started = Instant::now();
    let mut req = http.get(&url).timeout(Duration::from_secs(5));
    if !up.api_key.is_empty() {
        req = req.bearer_auth(&up.api_key);
    }
    let ms = || started.elapsed().as_millis() as i64;
    match req.send().await {
        Ok(r) if r.status().is_server_error() => (false, ms(), Some(format!("HTTP {}", r.status().as_u16()))),
        Ok(_) => (true, ms(), None),
        Err(e) => (false, ms(), Some(short_reqwest(&e))),
    }
}

/// Токены из `usage`.
pub fn usage_tokens(usage: Option<&Value>) -> (u64, u64) {
    let get = |k: &str| usage.and_then(|u| u.get(k)).and_then(Value::as_u64).unwrap_or(0);
    (get("prompt_tokens"), get("completion_tokens"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_rules() {
        let src: Map<String, Value> = serde_json::from_value(json!({
            "model": "qwen3-27b", "messages": [], "temperature": 0.7, "top_k": 20,
            "protocol": 2, "delivery": "bot", "job_id": "x", "chat_id": 1, "reply_to": 2, "thread_id": null,
            "callback": {"url": "u", "token": "t"}, "chat_template_kwargs": {"enable_thinking": true}
        }))
        .unwrap();
        let mut m = ModelCfg::default();
        m.defaults.insert("max_tokens".into(), json!(400));
        m.defaults.insert("temperature".into(), json!(0.1));
        m.force.insert("chat_template_kwargs".into(), json!({"enable_thinking": false}));
        let p = build_payload(&src, &m, "qwen3-27b-q4_k_m", false);
        assert_eq!(p["model"], "qwen3-27b-q4_k_m");
        assert_eq!(p["max_tokens"], 400);
        assert_eq!(p["temperature"], 0.7);
        assert_eq!(p["top_k"], 20);
        assert_eq!(p["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(p["stream"], false);
        for f in QUEUE_FIELDS {
            assert!(!p.contains_key(*f), "{f} leaked");
        }
    }

    #[test]
    fn text_extraction() {
        assert_eq!(extract_text(&json!({"choices":[{"message":{"content":"hi","reasoning_content":"r"}}]})), "hi");
        assert_eq!(extract_text(&json!({"choices":[{"message":{"content":""},"text":"t"}]})), "t");
        assert_eq!(extract_text(&json!({"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}]})), "ab");
        assert_eq!(extract_text(&json!({})), "");
    }
}
