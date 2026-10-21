//! Метрики Prometheus (§4.6): `queue_jobs{state}`, `queue_wait_seconds`,
//! `upstream_latency_seconds{model}`, `upstream_errors_total{model,code}`,
//! `callback_failures_total`, плюс здоровье апстримов, движков и GPU.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

const BOUNDS_S: [f64; 12] = [0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 60.0, 120.0, 300.0, 600.0];

#[derive(Clone, Default)]
struct Hist {
    counts: [u64; 12],
    count: u64,
    sum: f64,
}

impl Hist {
    fn observe(&mut self, secs: f64) {
        for (i, b) in BOUNDS_S.iter().enumerate() {
            if secs <= *b {
                self.counts[i] += 1;
            }
        }
        self.count += 1;
        self.sum += secs;
    }

    fn render(&self, out: &mut String, name: &str, labels: &str) {
        let sep = if labels.is_empty() { "" } else { "," };
        for (i, b) in BOUNDS_S.iter().enumerate() {
            let _ = writeln!(out, "{name}_bucket{{{labels}{sep}le=\"{b}\"}} {}", self.counts[i]);
        }
        let _ = writeln!(out, "{name}_bucket{{{labels}{sep}le=\"+Inf\"}} {}", self.count);
        let l = if labels.is_empty() { String::new() } else { format!("{{{labels}}}") };
        let _ = writeln!(out, "{name}_sum{l} {}", self.sum);
        let _ = writeln!(out, "{name}_count{l} {}", self.count);
    }
}

#[derive(Default)]
pub struct Metrics {
    wait: Mutex<Hist>,
    latency: Mutex<BTreeMap<String, Hist>>,
    upstream_errors: Mutex<BTreeMap<(String, String), u64>>,
    pub callback_failures: AtomicU64,
    finished: Mutex<BTreeMap<String, u64>>,
}

pub fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " ")
}

impl Metrics {
    pub fn observe_wait(&self, ms: i64) {
        self.wait.lock().observe(ms as f64 / 1000.0);
    }

    pub fn observe_latency(&self, model: &str, ms: i64) {
        self.latency.lock().entry(model.to_string()).or_default().observe(ms as f64 / 1000.0);
    }

    pub fn upstream_error(&self, model: &str, code: &str) {
        *self.upstream_errors.lock().entry((model.to_string(), code.to_string())).or_default() += 1;
    }

    pub fn finished(&self, outcome: &str) {
        *self.finished.lock().entry(outcome.to_string()).or_default() += 1;
    }

    pub fn render_into(&self, out: &mut String) {
        out.push_str("# HELP queue_wait_seconds Time jobs spent waiting in the queue.\n# TYPE queue_wait_seconds histogram\n");
        self.wait.lock().render(out, "queue_wait_seconds", "");
        out.push_str("# HELP upstream_latency_seconds Upstream answer time.\n# TYPE upstream_latency_seconds histogram\n");
        for (m, h) in self.latency.lock().iter() {
            h.render(out, "upstream_latency_seconds", &format!("model=\"{}\"", esc(m)));
        }
        out.push_str("# HELP upstream_errors_total Upstream errors by model and code.\n# TYPE upstream_errors_total counter\n");
        for ((m, c), n) in self.upstream_errors.lock().iter() {
            let _ = writeln!(out, "upstream_errors_total{{model=\"{}\",code=\"{}\"}} {n}", esc(m), esc(c));
        }
        out.push_str("# HELP callback_failures_total Failed callback attempts.\n# TYPE callback_failures_total counter\n");
        let _ = writeln!(out, "callback_failures_total {}", self.callback_failures.load(Ordering::Relaxed));
        out.push_str("# HELP queue_jobs_finished_total Finished jobs by outcome since start.\n# TYPE queue_jobs_finished_total counter\n");
        for (o, n) in self.finished.lock().iter() {
            let _ = writeln!(out, "queue_jobs_finished_total{{outcome=\"{}\"}} {n}", esc(o));
        }
    }
}
