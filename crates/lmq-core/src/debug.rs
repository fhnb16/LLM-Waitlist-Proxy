//! Встроенный приёмник колбэков — эмулятор бота для отладки (`/admin/debug/callback`).
//! Показывает, что сервер отправил бы на `/webhook/llm`, и отвечает так, как настроено:
//! `200`, `403` или `500`, с задержкой.

use std::collections::VecDeque;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;

const CAP: usize = 200;

#[derive(Clone, Debug, Serialize)]
pub struct Received {
    pub id: u64,
    pub ts: i64,
    /// Токен из `X-Callback-Token` в замаскированном виде.
    pub token: Option<String>,
    pub content_type: Option<String>,
    pub body: Value,
    pub replied: u16,
    pub delay_ms: u64,
}

#[derive(Default)]
pub struct DebugSink {
    items: Mutex<VecDeque<Received>>,
    seq: Mutex<u64>,
}

impl DebugSink {
    pub fn record(&self, mut r: Received) -> Received {
        let mut seq = self.seq.lock();
        *seq += 1;
        r.id = *seq;
        let mut items = self.items.lock();
        if items.len() >= CAP {
            items.pop_front();
        }
        items.push_back(r.clone());
        r
    }

    pub fn list(&self) -> Vec<Received> {
        self.items.lock().iter().rev().cloned().collect()
    }

    pub fn clear(&self) {
        self.items.lock().clear();
    }
}
