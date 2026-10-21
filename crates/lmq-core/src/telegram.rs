//! Протокол 1 (§9, для совместимости, выключен по умолчанию): сервер сам отправляет
//! ответ в Telegram. Токен бота в журнал не попадает: в ошибках остаётся только описание.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::config::Protocol1Cfg;

/// Предел длины сообщения Telegram.
const TG_LIMIT: usize = 4096;

pub async fn send_message(
    http: &reqwest::Client,
    cfg: &Protocol1Cfg,
    chat_id: i64,
    text: &str,
    reply_to: Option<i64>,
    thread_id: Option<i64>,
) -> Result<Value> {
    if cfg.telegram_bot_token.is_empty() {
        bail!("TELEGRAM_BOT_TOKEN is not set");
    }
    let text: String = if text.chars().count() > TG_LIMIT { text.chars().take(TG_LIMIT).collect() } else { text.to_string() };
    let mut body = json!({ "chat_id": chat_id, "text": text });
    if let Some(r) = reply_to {
        body["reply_parameters"] = json!({ "message_id": r, "allow_sending_without_reply": true });
    }
    if let Some(t) = thread_id {
        body["message_thread_id"] = json!(t);
    }
    let url = format!("{}/bot{}/sendMessage", cfg.telegram_api.trim_end_matches('/'), cfg.telegram_bot_token);
    let resp = http
        .post(&url)
        .timeout(std::time::Duration::from_secs(20))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("telegram request failed: {}", e.without_url()))?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() || v.get("ok") != Some(&Value::Bool(true)) {
        let desc = v.get("description").and_then(Value::as_str).unwrap_or("unknown error");
        bail!("telegram HTTP {}: {desc}", status.as_u16());
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}
