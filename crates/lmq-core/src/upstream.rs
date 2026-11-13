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

/// Кусок ответа по мере генерации — для песочницы и клиентов со `stream: true`.
#[derive(Debug, Clone)]
pub enum Delta {
    Text(String),
    /// `reasoning_content` / `reasoning`: в итоговый текст не идёт, но клиенту со стримом нужен.
    Reasoning(String),
    /// `delta.tool_calls` как прислал апстрим.
    ToolCalls(Value),
}

pub type DeltaTx = mpsc::UnboundedSender<Delta>;

#[derive(Debug, Clone)]
pub enum UpErr {
    /// Соединение не установлено или оборвано.
    Connect(String),
    /// Стрим оборвался, когда клиенту уже ушла часть ответа: другой апстрим начал бы заново.
    Interrupted(String),
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
            UpErr::Interrupted(e) => format!("upstream stream broke off mid-answer: {e}"),
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

/// Имя параметра из ответа апстрима, отклонившего его: Groq «property 'x' is unsupported»,
/// OpenAI «Unrecognized request argument supplied: x» / «Unsupported parameter: 'x'»,
/// serde «unknown field `x`» и похожие.
pub fn rejected_param(msg: &str) -> Option<String> {
    let lower = msg.to_lowercase();
    let quoted = |s: &str| -> Option<String> {
        let s = s.trim_start();
        let (open, close) = match s.chars().next()? {
            '\'' => ('\'', '\''),
            '"' => ('"', '"'),
            '`' => ('`', '`'),
            _ => {
                let name: String = s.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                return (!name.is_empty()).then_some(name);
            }
        };
        let rest = &s[open.len_utf8()..];
        let end = rest.find(close)?;
        Some(rest[..end].to_string())
    };
    // «… 'x' is unsupported» / «… 'x' is not supported» — имя перед фразой.
    for phrase in ["is unsupported", "is not supported", "is not allowed", "not permitted"] {
        if let Some(i) = lower.find(phrase) {
            let before = &msg[..i];
            let before = before.trim_end();
            if let Some(close) = before.chars().last().filter(|c| matches!(c, '\'' | '"' | '`')) {
                let inner = &before[..before.len() - close.len_utf8()];
                if let Some(open) = inner.rfind(close) {
                    return Some(inner[open + close.len_utf8()..].to_string());
                }
            }
        }
    }
    // «… argument supplied: x», «unsupported parameter: 'x'», «unknown field `x`».
    for marker in ["argument supplied:", "unsupported parameter:", "unknown parameter:", "unknown field", "unrecognized field", "extra field"] {
        if let Some(i) = lower.find(marker) {
            return quoted(&msg[i + marker.len()..]);
        }
    }
    None
}

/// Копия для журнала: строки `data:…` (картинки base64) заменены на `<data:image/jpeg, N bytes>`.
pub fn shrink_data_urls(v: &Value) -> Value {
    match v {
        Value::String(s) if s.starts_with("data:") && s.len() > 200 => Value::String(format!("<{}, {} bytes>", crate::admin::image_source(s), s.len())),
        Value::Array(a) => Value::Array(a.iter().map(shrink_data_urls).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), shrink_data_urls(v))).collect()),
        other => other.clone(),
    }
}

/// Эти поля без модели не обойдутся — их не убираем, даже если апстрим ругается.
const ESSENTIAL: &[&str] = &["model", "messages", "prompt", "stream"];

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

/// Как писать в журнал запрос к апстриму (`logging.requests`, `logging.content`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Trace<'a> {
    pub job_id: &'a str,
    pub requests: bool,
    pub content: bool,
}

/// Один запрос к апстриму. `deltas` — куда слать куски ответа (песочница, клиент со стримом);
/// при нём запрос идёт стримом, даже если апстрим настроен без стрима: не умеющий стрим
/// апстрим ответит обычным JSON, и он уйдёт одним куском.
#[allow(clippy::too_many_arguments)]
pub async fn call(
    http: &reqwest::Client,
    up: &UpstreamRt,
    payload: &Map<String, Value>,
    model: &ModelCfg,
    timeout: Duration,
    cancel: &CancellationToken,
    deltas: Option<&DeltaTx>,
    trace: Trace<'_>,
) -> Result<UpOk, UpErr> {
    let stream = up.stream || deltas.is_some();
    let mut body = build_payload(payload, model, &up.target_model, stream);
    let mut dropped: Vec<String> = Vec::new();
    for k in up.drop_params.iter().chain(up.learned_drops.lock().iter()) {
        if !ESSENTIAL.contains(&k.as_str()) && body.remove(k).is_some() {
            dropped.push(k.clone());
        }
    }
    let started = Instant::now();
    let fut = async {
        // Отклонённый апстримом параметр убираем и повторяем (не больше 8 раз за запрос).
        for _ in 0..8 {
            if trace.requests {
                let msgs = body.get("messages").map(crate::admin::messages_summary).unwrap_or(Value::Null);
                let params: Vec<&String> = body.keys().filter(|k| *k != "messages").collect();
                if trace.content {
                    let full = shrink_data_urls(&Value::Object(body.clone())).to_string();
                    tracing::info!(job_id = trace.job_id, upstream = %up.id, url = %up.url, route = %crate::net::describe(&up.route),
                        params = ?params, dropped = ?dropped, messages = %msgs, body = %full, "upstream request");
                } else {
                    tracing::info!(job_id = trace.job_id, upstream = %up.id, url = %up.url, route = %crate::net::describe(&up.route),
                        params = ?params, dropped = ?dropped, messages = %msgs, "upstream request");
                }
            }
            let t = Instant::now();
            let mut req = http.post(&up.url).json(&body);
            if !up.api_key.is_empty() {
                req = req.bearer_auth(&up.api_key);
            }
            let resp = req.send().await.map_err(|e| UpErr::Connect(format!("{} ({})", short_reqwest(&e), crate::net::describe(&up.route))))?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                let msg = error_message(&text);
                if trace.requests {
                    tracing::info!(job_id = trace.job_id, upstream = %up.id, status = status.as_u16(), ms = t.elapsed().as_millis() as u64,
                        body = %truncate(&text, 2000), "upstream response");
                }
                if matches!(status.as_u16(), 400 | 422)
                    && let Some(p) = rejected_param(&msg).filter(|p| !ESSENTIAL.contains(&p.as_str()))
                    && body.remove(&p).is_some()
                {
                    tracing::warn!(job_id = trace.job_id, upstream = %up.id, param = %p, error = %msg, "upstream rejected parameter, retrying without it");
                    up.learned_drops.lock().insert(p.clone());
                    dropped.push(p);
                    continue;
                }
                return Err(UpErr::Status(status.as_u16(), msg));
            }
            if trace.requests {
                tracing::info!(job_id = trace.job_id, upstream = %up.id, status = status.as_u16(), ms = t.elapsed().as_millis() as u64, stream, "upstream response");
            }
            return if stream { read_stream(resp, &up.target_model, deltas).await } else { read_json(resp).await };
        }
        Err(UpErr::Bad("upstream rejected too many parameters".into()))
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
    truncate(&crate::netdiag::error_chain(e), 600)
}

async fn read_json(resp: reqwest::Response) -> Result<(Value, String, Option<Value>), UpErr> {
    let bytes = resp.bytes().await.map_err(|e| UpErr::Connect(short_reqwest(&e)))?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|e| UpErr::Bad(format!("not JSON: {e}")))?;
    parse_json(body)
}

/// Ответ без стрима: `choices` обязательны, `error` — ошибка апстрима.
fn parse_json(body: Value) -> Result<(Value, String, Option<Value>), UpErr> {
    if body.get("choices").is_none() {
        if body.get("error").is_some_and(|e| !e.is_null()) {
            return Err(UpErr::Bad(error_message(&body.to_string())));
        }
        return Err(UpErr::Bad("no choices in response".into()));
    }
    let text = extract_text(&body);
    let usage = body.get("usage").cloned().filter(|u| !u.is_null());
    Ok((body, text, usage))
}

/// Сборка ответа из событий SSE.
#[derive(Default)]
struct StreamAcc {
    text: String,
    reasoning: String,
    usage: Option<Value>,
    finish: Value,
    id: Value,
    /// Вызовы инструментов, собранные из `delta.tool_calls` по `index`.
    tools: Vec<Value>,
    /// Сколько строк `data:` разобрано. Ноль к концу ответа — апстрим прислал не SSE.
    events: usize,
    /// Ушёл ли получателю хоть один кусок.
    emitted: bool,
}

impl StreamAcc {
    fn emit(&mut self, deltas: Option<&DeltaTx>, d: Delta) {
        if let Some(tx) = deltas {
            let _ = tx.send(d);
            self.emitted = true;
        }
    }

    /// Дописывает куски `delta.tool_calls`: `id`, `type`, имя — как пришли, `arguments` — склейкой.
    fn add_tools(&mut self, calls: &[Value]) {
        for (pos, tc) in calls.iter().enumerate() {
            let idx = tc.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(pos);
            while self.tools.len() <= idx {
                self.tools.push(json!({"id": "", "type": "function", "function": {"name": "", "arguments": ""}}));
            }
            let slot = &mut self.tools[idx];
            for k in ["id", "type"] {
                if let Some(v) = tc.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    slot[k] = json!(v);
                }
            }
            if let Some(f) = tc.get("function") {
                for k in ["name", "arguments"] {
                    if let Some(piece) = f.get(k).and_then(Value::as_str) {
                        let joined = format!("{}{piece}", slot["function"][k].as_str().unwrap_or(""));
                        slot["function"][k] = json!(joined);
                    }
                }
            }
        }
    }

    /// Одна строка SSE. `Ok(true)` — пришёл `[DONE]`.
    fn line(&mut self, raw: &[u8], deltas: Option<&DeltaTx>) -> Result<bool, UpErr> {
        let line = String::from_utf8_lossy(raw);
        let Some(data) = line.trim().strip_prefix("data:") else { return Ok(false) };
        let data = data.trim();
        if data == "[DONE]" {
            self.events += 1;
            return Ok(true);
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(false) };
        self.events += 1;
        if v.get("error").is_some_and(|e| !e.is_null()) {
            return Err(UpErr::Bad(error_message(&v.to_string())));
        }
        if self.id.is_null() {
            self.id = v.get("id").cloned().unwrap_or(Value::Null);
        }
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            self.usage = Some(u.clone());
        }
        let Some(c) = v.get("choices").and_then(|c| c.get(0)) else { return Ok(false) };
        // Кусок обычно в `delta`, но часть серверов шлёт целое `message` (последним событием или единственным).
        let field = |k: &str| ["delta", "message"].iter().find_map(|f| c.get(*f).and_then(|d| d.get(k)).filter(|x| !x.is_null()));
        let piece = field("content").and_then(content_to_string).or_else(|| c.get("text").and_then(Value::as_str).map(String::from));
        // Размышления (`reasoning_content` у llama-server и DeepSeek, `reasoning` у vLLM, Ollama, OpenRouter)
        // в текст для бота не идут (§3.1), но клиенту со стримом нужны, а пустой ответ ими объясняется.
        let reasoning = field("reasoning_content").or_else(|| field("reasoning")).and_then(Value::as_str).filter(|r| !r.is_empty()).map(String::from);
        let tools = field("tool_calls").and_then(Value::as_array).filter(|a| !a.is_empty()).cloned();
        let finish = c.get("finish_reason").filter(|f| !f.is_null()).cloned();
        if let Some(r) = reasoning {
            self.reasoning.push_str(&r);
            self.emit(deltas, Delta::Reasoning(r));
        }
        if let Some(p) = piece.filter(|p| !p.is_empty()) {
            self.text.push_str(&p);
            self.emit(deltas, Delta::Text(p));
        }
        if let Some(calls) = tools {
            self.add_tools(&calls);
            self.emit(deltas, Delta::ToolCalls(Value::Array(calls)));
        }
        if let Some(fr) = finish {
            self.finish = fr;
        }
        Ok(false)
    }

    fn finish(self, model: &str) -> (Value, String, Option<Value>) {
        let mut message = json!({"role": "assistant", "content": self.text});
        if !self.reasoning.is_empty() {
            message["reasoning_content"] = Value::String(self.reasoning);
        }
        if !self.tools.is_empty() {
            message["tool_calls"] = Value::Array(self.tools);
        }
        let body = json!({
            "id": self.id,
            "object": "chat.completion",
            "created": chrono::Utc::now().timestamp(),
            "model": model,
            "choices": [{"index": 0, "message": message, "finish_reason": if self.finish.is_null() { json!("stop") } else { self.finish }}],
            "usage": self.usage.clone().unwrap_or(Value::Null),
        });
        (body, self.text, self.usage)
    }
}

/// Ответ на запрос со `stream: true`. Не все апстримы его соблюдают: одни отвечают обычным
/// JSON (по заголовку или без него), другие не ставят перевод строки после последнего события.
async fn read_stream(resp: reqwest::Response, model: &str, deltas: Option<&DeltaTx>) -> Result<(Value, String, Option<Value>), UpErr> {
    // Обычный JSON вместо стрима уходит получателю одним куском.
    let send_whole = |r: (Value, String, Option<Value>)| {
        if let Some(tx) = deltas {
            for d in message_deltas(&r.0) {
                let _ = tx.send(d);
            }
        }
        r
    };
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    if content_type.contains("json") && !content_type.contains("event-stream") {
        return read_json(resp).await.map(send_whole);
    }
    let mut acc = StreamAcc::default();
    let raw = match read_events(resp, &mut acc, deltas).await {
        Ok(raw) => raw,
        // Часть ответа уже у получателя: повтор на другом апстриме склеил бы два ответа.
        Err(e @ (UpErr::Connect(_) | UpErr::Bad(_))) if acc.emitted => return Err(UpErr::Interrupted(e.message())),
        Err(e) => return Err(e),
    };
    if acc.events == 0 {
        return match serde_json::from_slice::<Value>(&raw) {
            Ok(body) => parse_json(body).map(send_whole),
            Err(_) if raw.iter().all(u8::is_ascii_whitespace) => Err(UpErr::Bad("empty response to a stream request".into())),
            Err(_) => Err(UpErr::Bad(format!("neither SSE nor JSON: {}", truncate(String::from_utf8_lossy(&raw).trim(), 300)))),
        };
    }
    Ok(acc.finish(model))
}

/// Читает события SSE в `acc`. Возвращает тело целиком, если событий не нашлось (вдруг это JSON).
async fn read_events(resp: reqwest::Response, acc: &mut StreamAcc, deltas: Option<&DeltaTx>) -> Result<Vec<u8>, UpErr> {
    let mut raw: Vec<u8> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| UpErr::Connect(short_reqwest(&e)))?;
        if acc.events == 0 {
            raw.extend_from_slice(&chunk);
        }
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            if acc.line(&line, deltas)? {
                return Ok(Vec::new());
            }
        }
        if acc.events > 0 && !raw.is_empty() {
            raw = Vec::new();
        }
    }
    // Последняя строка без перевода строки.
    if !buf.is_empty() {
        acc.line(&buf, deltas)?;
    }
    Ok(raw)
}

/// Куски, которыми целый ответ (`choices[0].message`) уходит получателю стрима.
pub fn message_deltas(body: &Value) -> Vec<Delta> {
    let msg = body.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("message"));
    let mut out = Vec::new();
    if let Some(r) = msg.and_then(|m| m.get("reasoning_content").or_else(|| m.get("reasoning"))).and_then(Value::as_str).filter(|r| !r.is_empty()) {
        out.push(Delta::Reasoning(r.to_string()));
    }
    let text = extract_text(body);
    if !text.is_empty() {
        out.push(Delta::Text(text));
    }
    if let Some(calls) = msg.and_then(|m| m.get("tool_calls")).and_then(Value::as_array).filter(|a| !a.is_empty()) {
        let calls = calls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut c = c.clone();
                if let Some(o) = c.as_object_mut() {
                    o.entry("index").or_insert(json!(i));
                }
                c
            })
            .collect();
        out.push(Delta::ToolCalls(Value::Array(calls)));
    }
    out
}

/// Подробность к `empty_answer`: чем модель была занята вместо ответа.
pub fn empty_answer_reason(body: &Value) -> String {
    let choice = body.get("choices").and_then(|c| c.get(0));
    let finish = choice.and_then(|c| c.get("finish_reason")).and_then(Value::as_str).unwrap_or("");
    let msg = choice.and_then(|c| c.get("message"));
    let reasoned = msg.and_then(|m| m.get("reasoning_content").or_else(|| m.get("reasoning"))).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty())
        || extract_text(body).contains("<think>");
    let base = "model returned an empty answer";
    match (reasoned, finish) {
        (true, "length") => format!("{base}: reasoning used up max_tokens, raise max_tokens or turn thinking off"),
        (true, _) => format!("{base}: only reasoning, no content"),
        (false, "") | (false, "stop") => base.to_string(),
        (false, f) => format!("{base} (finish_reason: {f})"),
    }
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
    fn rejected_param_messages() {
        // Из журнала (Groq).
        assert_eq!(rejected_param("property 'mirostat' is unsupported").as_deref(), Some("mirostat"));
        assert_eq!(rejected_param("property 'chat_template_kwargs' is unsupported").as_deref(), Some("chat_template_kwargs"));
        // OpenAI.
        assert_eq!(rejected_param("Unrecognized request argument supplied: top_k").as_deref(), Some("top_k"));
        assert_eq!(rejected_param("Unsupported parameter: 'logit_bias' is not supported with this model.").as_deref(), Some("logit_bias"));
        // serde / Rust-серверы.
        assert_eq!(rejected_param("Failed to deserialize the JSON body: unknown field `repeat_penalty`").as_deref(), Some("repeat_penalty"));
        assert_eq!(rejected_param("model not found"), None);
        assert_eq!(rejected_param("rate limit reached"), None);
    }

    #[test]
    fn data_urls_are_shortened_for_logs() {
        let big = format!("data:image/jpeg;base64,{}", "A".repeat(5000));
        let v = json!({"messages": [{"role": "user", "content": [{"type": "image_url", "image_url": {"url": big}}, {"type": "text", "text": "что тут?"}]}]});
        let s = shrink_data_urls(&v).to_string();
        assert!(s.contains("<data:image/jpeg, 5023 bytes>"), "{s}");
        assert!(s.contains("что тут?") && s.len() < 300);
    }

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
