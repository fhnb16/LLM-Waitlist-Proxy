//! Очередь в памяти (зеркало заданий `queued` из БД).
//!
//! Для каждой модели — три яруса: синхронные запросы, личные чаты, группы (при
//! `PRIVATE_FIRST`; иначе личные и группы в одном ярусе). Внутри яруса — кольцо чатов
//! (round-robin), у чата — FIFO. Чат, у которого задание выполняется, пропускается.

use std::collections::{HashMap, HashSet, VecDeque};

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Map, Value};
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;

pub const TIER_SYNC: u8 = 0;
pub const TIER_PRIVATE: u8 = 1;
pub const TIER_GROUP: u8 = 2;

/// Ответ синхронного запроса: тело OpenAI или (HTTP-код, код ошибки, сообщение).
pub type SyncResult = Result<Value, (u16, String, String)>;

pub struct SyncCtx {
    pub payload: Map<String, Value>,
    pub tx: Option<oneshot::Sender<SyncResult>>,
    /// Куски ответа по мере генерации (песочница, клиент со стримом).
    pub deltas: Option<crate::upstream::DeltaTx>,
    pub client: String,
    pub requested_model: String,
}

pub struct QItem {
    pub job_id: String,
    pub chat_id: i64,
    pub created_at: i64,
    pub client: String,
    pub sync: Option<SyncCtx>,
}

#[derive(Default)]
struct Tier {
    ring: VecDeque<i64>,
    chats: HashMap<i64, VecDeque<QItem>>,
}

impl Tier {
    fn len(&self) -> usize {
        self.chats.values().map(VecDeque::len).sum()
    }

    fn push(&mut self, it: QItem) {
        let q = self.chats.entry(it.chat_id).or_default();
        if q.is_empty() {
            self.ring.push_back(it.chat_id);
        }
        q.push_back(it);
    }

    fn remove(&mut self, job_id: &str) -> Option<QItem> {
        let mut found = None;
        for (chat, q) in self.chats.iter_mut() {
            if let Some(pos) = q.iter().position(|i| i.job_id == job_id) {
                found = Some((*chat, q.remove(pos)?));
                break;
            }
        }
        let (chat, item) = found?;
        if self.chats.get(&chat).is_some_and(VecDeque::is_empty) {
            self.chats.remove(&chat);
            self.ring.retain(|c| *c != chat);
        }
        Some(item)
    }

    /// Следующее задание по кругу чатов, пропуская занятые.
    fn take(&mut self, active: &HashSet<i64>) -> Option<QItem> {
        for _ in 0..self.ring.len() {
            let chat = self.ring.pop_front()?;
            if active.contains(&chat) {
                self.ring.push_back(chat);
                continue;
            }
            let q = self.chats.get_mut(&chat)?;
            let item = q.pop_front();
            if q.is_empty() {
                self.chats.remove(&chat);
            } else {
                self.ring.push_back(chat);
            }
            return item;
        }
        None
    }
}

#[derive(Default)]
struct ModelQueue {
    sync: VecDeque<QItem>,
    tiers: [Tier; 2],
}

impl ModelQueue {
    fn len(&self) -> usize {
        self.sync.len() + self.tiers.iter().map(Tier::len).sum::<usize>()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RunningJob {
    pub job_id: String,
    pub model: String,
    pub chat_id: i64,
    pub client: String,
    pub upstream: String,
    pub started_at: i64,
    pub sync: bool,
    #[serde(skip)]
    pub cancel: CancellationToken,
}

#[derive(Default)]
struct State {
    models: HashMap<String, ModelQueue>,
    /// job_id → (модель, чат, ярус) для заданий в очереди.
    index: HashMap<String, (String, i64, u8, i64)>,
    per_chat: HashMap<i64, u32>,
    active_chats: HashSet<i64>,
    running: HashMap<String, RunningJob>,
    running_per_model: HashMap<String, u32>,
}

#[derive(Default)]
pub struct Queue {
    st: Mutex<State>,
    pub wake: Notify,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ModelLoad {
    pub queued: usize,
    pub running: u32,
}

impl Queue {
    pub fn push(&self, model: &str, tier: u8, item: QItem) {
        {
            let mut st = self.st.lock();
            if item.sync.is_none() {
                st.index.insert(item.job_id.clone(), (model.to_string(), item.chat_id, tier, item.created_at));
                *st.per_chat.entry(item.chat_id).or_default() += 1;
            }
            let mq = st.models.entry(model.to_string()).or_default();
            match tier {
                TIER_SYNC => mq.sync.push_back(item),
                TIER_PRIVATE => mq.tiers[0].push(item),
                _ => mq.tiers[1].push(item),
            }
        }
        self.wake.notify_one();
    }

    /// Убирает задание из очереди (не выполняющееся).
    pub fn remove(&self, job_id: &str) -> Option<(String, QItem)> {
        let mut st = self.st.lock();
        let (model, chat, tier, _) = st.index.remove(job_id)?;
        if let Some(n) = st.per_chat.get_mut(&chat) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                st.per_chat.remove(&chat);
            }
        }
        let mq = st.models.get_mut(&model)?;
        let item = match tier {
            TIER_PRIVATE => mq.tiers[0].remove(job_id),
            _ => mq.tiers[1].remove(job_id),
        };
        item.map(|i| (model, i))
    }

    pub fn remove_sync(&self, job_id: &str) -> bool {
        let mut st = self.st.lock();
        for mq in st.models.values_mut() {
            if let Some(pos) = mq.sync.iter().position(|i| i.job_id == job_id) {
                mq.sync.remove(pos);
                return true;
            }
        }
        false
    }

    pub fn is_queued(&self, job_id: &str) -> bool {
        self.st.lock().index.contains_key(job_id)
    }

    pub fn total_queued(&self) -> usize {
        self.st.lock().index.len()
    }

    pub fn queued_in_chat(&self, chat_id: i64) -> u32 {
        self.st.lock().per_chat.get(&chat_id).copied().unwrap_or(0)
    }

    pub fn queued_in_model(&self, model: &str) -> usize {
        self.st.lock().models.get(model).map(ModelQueue::len).unwrap_or(0)
    }

    /// Задания чата, которые ещё не начаты (для вытеснения).
    pub fn queued_of_chat(&self, chat_id: i64) -> Vec<String> {
        self.st.lock().index.iter().filter(|(_, v)| v.1 == chat_id).map(|(k, _)| k.clone()).collect()
    }

    /// Позиция в очереди (0 — следующее), приблизительно по ярусу и времени постановки.
    pub fn position(&self, job_id: &str) -> Option<usize> {
        let st = self.st.lock();
        let (model, _, tier, created) = st.index.get(job_id)?.clone();
        let mq = st.models.get(&model)?;
        let mut ahead = mq.sync.len();
        for (t, tq) in mq.tiers.iter().enumerate() {
            let t = t as u8 + 1;
            for q in tq.chats.values() {
                for it in q {
                    if t < tier || (t == tier && it.created_at < created) {
                        ahead += 1;
                    }
                }
            }
        }
        Some(ahead)
    }

    pub fn models_with_work(&self) -> Vec<String> {
        self.st.lock().models.iter().filter(|(_, q)| q.len() > 0).map(|(k, _)| k.clone()).collect()
    }

    /// Следующее задание модели. `cap` — предел одновременных заданий модели (расписание).
    /// Регистрирует его как выполняющееся и занимает чат.
    pub fn take_next(&self, model: &str, cap: Option<u32>, upstream: &str) -> Option<(QItem, CancellationToken)> {
        let mut st = self.st.lock();
        let running = st.running_per_model.get(model).copied().unwrap_or(0);
        if cap.is_some_and(|c| running >= c) {
            return None;
        }
        let State { models, active_chats, .. } = &mut *st;
        let mq = models.get_mut(model)?;
        let item = mq.sync.pop_front().or_else(|| mq.tiers[0].take(active_chats)).or_else(|| mq.tiers[1].take(active_chats))?;
        if item.sync.is_none() {
            if let Some((_, chat, _, _)) = st.index.remove(&item.job_id)
                && let Some(n) = st.per_chat.get_mut(&chat)
            {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    st.per_chat.remove(&chat);
                }
            }
            st.active_chats.insert(item.chat_id);
        }
        let cancel = CancellationToken::new();
        *st.running_per_model.entry(model.to_string()).or_default() += 1;
        st.running.insert(
            item.job_id.clone(),
            RunningJob {
                job_id: item.job_id.clone(),
                model: model.to_string(),
                chat_id: item.chat_id,
                client: item.client.clone(),
                upstream: upstream.to_string(),
                started_at: crate::util::now_ms(),
                sync: item.sync.is_some(),
                cancel: cancel.clone(),
            },
        );
        Some((item, cancel))
    }

    /// Задание закончилось: освобождает чат и счётчик модели.
    pub fn finish(&self, job_id: &str) {
        {
            let mut st = self.st.lock();
            if let Some(r) = st.running.remove(job_id) {
                if let Some(n) = st.running_per_model.get_mut(&r.model) {
                    *n = n.saturating_sub(1);
                }
                if !r.sync {
                    st.active_chats.remove(&r.chat_id);
                }
            }
        }
        self.wake.notify_one();
    }

    pub fn set_running_upstream(&self, job_id: &str, upstream: &str) {
        if let Some(r) = self.st.lock().running.get_mut(job_id) {
            r.upstream = upstream.to_string();
        }
    }

    /// Прерывает выполняющееся задание. `true` — оно выполнялось.
    pub fn cancel_running(&self, job_id: &str) -> bool {
        match self.st.lock().running.get(job_id) {
            Some(r) => {
                r.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn is_running(&self, job_id: &str) -> bool {
        self.st.lock().running.contains_key(job_id)
    }

    /// Задания, ждущие дольше `max_wait_ms`: убираются из очереди и возвращаются.
    pub fn take_expired(&self, now: i64, max_wait_ms: i64) -> Vec<(String, String, QItem)> {
        let ids: Vec<String> = {
            let st = self.st.lock();
            st.index.iter().filter(|(_, v)| now - v.3 > max_wait_ms).map(|(k, _)| k.clone()).collect()
        };
        ids.into_iter().filter_map(|id| self.remove(&id).map(|(m, it)| (id, m, it))).collect()
    }

    pub fn load(&self) -> HashMap<String, ModelLoad> {
        let st = self.st.lock();
        let mut out: HashMap<String, ModelLoad> = HashMap::new();
        for (m, q) in &st.models {
            out.entry(m.clone()).or_default().queued = q.len();
        }
        for (m, n) in &st.running_per_model {
            out.entry(m.clone()).or_default().running = *n;
        }
        out
    }

    pub fn running(&self) -> Vec<RunningJob> {
        self.st.lock().running.values().cloned().collect()
    }

    pub fn total_running(&self) -> usize {
        self.st.lock().running.len()
    }

    /// Все задания модели убираются из очереди (модель удалена из конфигурации).
    pub fn drain_model(&self, model: &str) -> Vec<QItem> {
        let ids: Vec<String> = {
            let st = self.st.lock();
            st.index.iter().filter(|(_, v)| v.0 == model).map(|(k, _)| k.clone()).collect()
        };
        let mut out: Vec<QItem> = ids.into_iter().filter_map(|id| self.remove(&id).map(|(_, it)| it)).collect();
        let mut st = self.st.lock();
        if let Some(mq) = st.models.get_mut(model) {
            out.extend(mq.sync.drain(..));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(id: &str, chat: i64, t: i64) -> QItem {
        QItem { job_id: id.into(), chat_id: chat, created_at: t, client: "c".into(), sync: None }
    }

    #[test]
    fn one_chat_one_job_and_round_robin() {
        let q = Queue::default();
        for i in 0..10 {
            q.push("m", TIER_GROUP, it(&format!("a{i}"), -1, i));
        }
        q.push("m", TIER_GROUP, it("b0", -2, 100));
        let (first, _) = q.take_next("m", None, "u").unwrap();
        assert_eq!(first.job_id, "a0");
        // Чат A занят — следующее из чата B, хотя оно поставлено позже.
        let (second, _) = q.take_next("m", None, "u").unwrap();
        assert_eq!(second.job_id, "b0");
        assert!(q.take_next("m", None, "u").is_none());
        q.finish("a0");
        assert_eq!(q.take_next("m", None, "u").unwrap().0.job_id, "a1");
    }

    #[test]
    fn private_before_group_and_sync_first() {
        let q = Queue::default();
        q.push("m", TIER_GROUP, it("g", -5, 1));
        q.push("m", TIER_PRIVATE, it("p", 5, 2));
        q.push(
            "m",
            TIER_SYNC,
            QItem {
                job_id: "s".into(),
                chat_id: 0,
                created_at: 3,
                client: "c".into(),
                sync: Some(SyncCtx { payload: Map::new(), tx: None, deltas: None, client: "c".into(), requested_model: "m".into() }),
            },
        );
        assert_eq!(q.take_next("m", None, "u").unwrap().0.job_id, "s");
        assert_eq!(q.take_next("m", None, "u").unwrap().0.job_id, "p");
        assert_eq!(q.take_next("m", None, "u").unwrap().0.job_id, "g");
    }

    #[test]
    fn positions_counts_and_expiry() {
        let q = Queue::default();
        q.push("m", TIER_GROUP, it("a", -1, 1));
        q.push("m", TIER_GROUP, it("b", -2, 2));
        q.push("m", TIER_PRIVATE, it("p", 3, 3));
        assert_eq!(q.position("p"), Some(0));
        assert_eq!(q.position("b"), Some(2));
        assert_eq!(q.queued_in_chat(-1), 1);
        assert_eq!(q.total_queued(), 3);
        let exp = q.take_expired(1000, 998);
        assert_eq!(exp.len(), 1);
        assert_eq!(exp[0].0, "a");
        assert_eq!(q.total_queued(), 2);
    }

    #[test]
    fn slot_cap_limits_running() {
        let q = Queue::default();
        q.push("m", TIER_GROUP, it("a", -1, 1));
        q.push("m", TIER_GROUP, it("b", -2, 2));
        assert!(q.take_next("m", Some(1), "u").is_some());
        assert!(q.take_next("m", Some(1), "u").is_none());
    }
}
