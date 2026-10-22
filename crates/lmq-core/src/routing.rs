//! Таблица маршрутизации: модели → апстримы со слотами и выключателем (circuit breaker).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::config::{BreakerCfg, EngineCfg, EngineKind, ModelCfg, RouteCfg, Settings};
use crate::util::now_ms;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Breaker {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Clone, Debug, Serialize)]
pub struct HealthSnapshot {
    pub breaker: Breaker,
    pub failures: u32,
    pub open_until: Option<i64>,
    pub last_error: Option<String>,
    pub last_ok_at: Option<i64>,
    pub last_fail_at: Option<i64>,
    pub last_check_at: Option<i64>,
    pub check_latency_ms: Option<i64>,
    /// Результат последней фоновой проверки (`None` — ещё не проверялся).
    pub reachable: Option<bool>,
}

#[derive(Debug)]
struct HealthState {
    breaker: Breaker,
    failures: u32,
    open_until: i64,
    last_error: Option<String>,
    last_ok_at: Option<i64>,
    last_fail_at: Option<i64>,
    last_check_at: Option<i64>,
    check_latency_ms: Option<i64>,
    reachable: Option<bool>,
}

#[derive(Debug)]
pub struct Health {
    s: Mutex<HealthState>,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            s: Mutex::new(HealthState {
                breaker: Breaker::Closed,
                failures: 0,
                open_until: 0,
                last_error: None,
                last_ok_at: None,
                last_fail_at: None,
                last_check_at: None,
                check_latency_ms: None,
                reachable: None,
            }),
        }
    }
}

impl Health {
    /// Можно ли сейчас отдавать задания. Разомкнутый выключатель по истечении паузы
    /// переходит в полуоткрытое состояние: одно задание проверяет апстрим.
    pub fn available(&self, cfg: &BreakerCfg) -> bool {
        if !cfg.enabled {
            return true;
        }
        let mut s = self.s.lock();
        match s.breaker {
            Breaker::Closed | Breaker::HalfOpen => true,
            Breaker::Open => {
                if now_ms() >= s.open_until {
                    s.breaker = Breaker::HalfOpen;
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn breaker(&self) -> Breaker {
        self.s.lock().breaker
    }

    /// Успешный запрос. Возвращает `true`, если выключатель замкнулся (апстрим вернулся).
    pub fn on_success(&self) -> bool {
        let mut s = self.s.lock();
        let was_open = s.breaker != Breaker::Closed;
        s.breaker = Breaker::Closed;
        s.failures = 0;
        s.last_ok_at = Some(now_ms());
        s.reachable = Some(true);
        was_open
    }

    /// Неудача (соединение, 5xx, таймаут). Возвращает `true`, если выключатель разомкнулся.
    pub fn on_failure(&self, err: &str, cfg: &BreakerCfg) -> bool {
        let mut s = self.s.lock();
        s.failures += 1;
        s.last_error = Some(err.to_string());
        s.last_fail_at = Some(now_ms());
        if !cfg.enabled {
            return false;
        }
        let should_open = s.breaker == Breaker::HalfOpen || s.failures >= cfg.failure_threshold.max(1);
        if should_open && s.breaker != Breaker::Open {
            s.breaker = Breaker::Open;
            s.open_until = now_ms() + (cfg.open_seconds as i64) * 1000;
            return true;
        }
        if s.breaker == Breaker::Open {
            s.open_until = now_ms() + (cfg.open_seconds as i64) * 1000;
        }
        false
    }

    /// Итог фоновой проверки доступности.
    pub fn on_check(&self, ok: bool, latency_ms: i64, err: Option<String>) {
        let mut s = self.s.lock();
        s.last_check_at = Some(now_ms());
        s.check_latency_ms = Some(latency_ms);
        s.reachable = Some(ok);
        if ok {
            if s.breaker == Breaker::Open {
                s.breaker = Breaker::HalfOpen;
            }
        } else if let Some(e) = err {
            s.last_error = Some(e);
        }
    }

    pub fn snapshot(&self) -> HealthSnapshot {
        let s = self.s.lock();
        HealthSnapshot {
            breaker: s.breaker,
            failures: s.failures,
            open_until: (s.breaker == Breaker::Open).then_some(s.open_until),
            last_error: s.last_error.clone(),
            last_ok_at: s.last_ok_at,
            last_fail_at: s.last_fail_at,
            last_check_at: s.last_check_at,
            check_latency_ms: s.check_latency_ms,
            reachable: s.reachable,
        }
    }

    /// Ручной сброс из панели.
    pub fn reset(&self) {
        let mut s = self.s.lock();
        s.breaker = Breaker::Closed;
        s.failures = 0;
    }
}

#[derive(Debug)]
pub struct UpstreamRt {
    pub id: String,
    pub model: String,
    pub index: usize,
    pub url: String,
    /// Имя модели, которое ждёт апстрим.
    pub target_model: String,
    pub api_key: String,
    pub slots: u32,
    pub stream: bool,
    pub timeout: Option<Duration>,
    pub engine: Option<String>,
    pub sem: Arc<Semaphore>,
    pub health: Arc<Health>,
    /// Маршрут: свой у апстрима, иначе `network.upstream`.
    pub route: RouteCfg,
    /// Параметры из настроек, которые не отправляются.
    pub drop_params: Vec<String>,
    /// Параметры, которые апстрим отклонил сам; переживают перезагрузку настроек.
    pub learned_drops: Arc<parking_lot::Mutex<std::collections::BTreeSet<String>>>,
}

impl UpstreamRt {
    pub fn busy(&self) -> u32 {
        self.slots.saturating_sub(self.sem.available_permits() as u32)
    }

    /// `…/v1/chat/completions` → `…/v1`.
    pub fn base_url(&self) -> String {
        let u = self.url.trim_end_matches('/');
        u.strip_suffix("/chat/completions").unwrap_or(u).to_string()
    }
}

#[derive(Debug)]
pub struct ModelRt {
    pub name: String,
    pub cfg: ModelCfg,
    pub upstreams: Vec<Arc<UpstreamRt>>,
}

impl ModelRt {
    pub fn slots(&self) -> u32 {
        self.upstreams.iter().map(|u| u.slots).sum()
    }
}

#[derive(Debug, Default)]
pub struct Routing {
    pub models: BTreeMap<String, Arc<ModelRt>>,
    pub aliases: BTreeMap<String, String>,
}

/// Адрес и число слотов апстрима, привязанного к движку.
pub fn engine_endpoint(e: &EngineCfg) -> (String, u32) {
    let host = if e.llama.host.is_empty() || e.llama.host == "0.0.0.0" { "127.0.0.1" } else { &e.llama.host };
    let port = match (e.kind, e.llama.port) {
        (EngineKind::Ollama, 0) => 11434,
        (EngineKind::Vllm, 0) => 8000,
        (EngineKind::Koboldcpp, 0) => 5001,
        (_, 0) => 8080,
        (_, p) => p,
    };
    let slots = match e.kind {
        EngineKind::LlamaCpp => e.llama.parallel.max(1),
        _ => 1,
    };
    (format!("http://{host}:{port}/v1/chat/completions"), slots)
}

impl Routing {
    /// Собирает таблицу. Семафоры и здоровье апстримов с тем же id переносятся из `prev`
    /// (семафор — только если число слотов не изменилось).
    pub fn build(s: &Settings, prev: Option<&Routing>) -> Routing {
        let mut models = BTreeMap::new();
        for (name, m) in &s.models {
            if !m.enabled {
                continue;
            }
            let mut ups = Vec::new();
            for (i, u) in m.upstreams.iter().enumerate() {
                let id = if u.name.trim().is_empty() { format!("{name}#{i}") } else { u.name.trim().to_string() };
                let engine = s.engines.iter().find(|e| !u.engine.is_empty() && e.id == u.engine);
                let (url, slots) = match engine {
                    Some(e) => {
                        let (eu, es) = engine_endpoint(e);
                        (if u.url.trim().is_empty() { eu } else { u.url.clone() }, if u.slots > 1 { u.slots } else { es })
                    }
                    None => (u.url.clone(), u.slots.max(1)),
                };
                let old = prev.and_then(|p| p.models.get(name)).and_then(|pm| pm.upstreams.iter().find(|x| x.id == id));
                let sem = match old {
                    Some(o) if o.slots == slots => o.sem.clone(),
                    _ => Arc::new(Semaphore::new(slots as usize)),
                };
                let health = old.map(|o| o.health.clone()).unwrap_or_default();
                let learned_drops = old.map(|o| o.learned_drops.clone()).unwrap_or_default();
                let own = RouteCfg { interface: u.interface.clone(), proxy: u.proxy.clone() };
                ups.push(Arc::new(UpstreamRt {
                    id,
                    model: name.clone(),
                    index: i,
                    url,
                    target_model: if u.model.trim().is_empty() { name.clone() } else { u.model.trim().to_string() },
                    api_key: u.api_key.clone(),
                    slots,
                    stream: u.stream,
                    timeout: (u.timeout > 0).then(|| Duration::from_secs(u.timeout)),
                    engine: (!u.engine.is_empty()).then(|| u.engine.clone()),
                    sem,
                    health,
                    route: own.inherit(&s.network.upstream),
                    drop_params: u.drop_params.iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect(),
                    learned_drops,
                }));
            }
            models.insert(name.clone(), Arc::new(ModelRt { name: name.clone(), cfg: m.clone(), upstreams: ups }));
        }
        let aliases = s.aliases.iter().filter(|(_, t)| models.contains_key(*t)).map(|(a, t)| (a.clone(), t.clone())).collect();
        Routing { models, aliases }
    }

    /// Модель по имени или алиасу.
    pub fn resolve(&self, name: &str) -> Option<Arc<ModelRt>> {
        if let Some(m) = self.models.get(name) {
            return Some(m.clone());
        }
        self.aliases.get(name).and_then(|t| self.models.get(t)).cloned()
    }

    pub fn upstreams(&self) -> impl Iterator<Item = &Arc<UpstreamRt>> {
        self.models.values().flat_map(|m| m.upstreams.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaker_opens_and_half_opens() {
        let h = Health::default();
        let cfg = BreakerCfg { enabled: true, failure_threshold: 2, open_seconds: 0, health_interval: 0 };
        assert!(h.available(&cfg));
        assert!(!h.on_failure("e", &cfg));
        assert!(h.on_failure("e", &cfg));
        assert_eq!(h.breaker(), Breaker::Open);
        // open_seconds = 0: сразу полуоткрыт.
        assert!(h.available(&cfg));
        assert_eq!(h.breaker(), Breaker::HalfOpen);
        // В полуоткрытом одна неудача снова размыкает.
        assert!(h.on_failure("e", &cfg));
        assert!(h.available(&cfg));
        assert!(h.on_success());
        assert_eq!(h.breaker(), Breaker::Closed);
    }

    #[test]
    fn breaker_blocks_while_open() {
        let h = Health::default();
        let cfg = BreakerCfg { enabled: true, failure_threshold: 1, open_seconds: 60, health_interval: 0 };
        h.on_failure("e", &cfg);
        assert!(!h.available(&cfg));
        let off = BreakerCfg { enabled: false, ..cfg };
        assert!(h.available(&off));
    }
}
