//! Мониторинг NVIDIA GPU через NVML. Библиотека (`nvml.dll` / `libnvidia-ml.so`)
//! загружается во время работы; если её нет, список устройств пуст и UI скрывает раздел.

use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct GpuInfo {
    pub index: u32,
    pub name: String,
    /// Загрузка ядра, %.
    pub util: u32,
    /// Загрузка контроллера памяти, %.
    pub mem_util: u32,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub temp_c: Option<u32>,
    pub power_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub fan: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct GpuState {
    pub available: bool,
    pub driver: Option<String>,
    pub error: Option<String>,
    pub devices: Vec<GpuInfo>,
    pub sampled_at: i64,
}

#[derive(Default)]
pub struct GpuMonitor {
    state: Arc<Mutex<GpuState>>,
}

impl GpuMonitor {
    pub fn snapshot(&self) -> GpuState {
        self.state.lock().clone()
    }

    /// Фоновый опрос раз в `period`. Без NVML задача сразу завершается.
    pub fn spawn(&self, period: std::time::Duration, shutdown: tokio_util::sync::CancellationToken) {
        let state = self.state.clone();
        std::thread::Builder::new().name("lmq-gpu".into()).spawn(move || sampler(state, period, shutdown)).ok();
    }
}

#[cfg(feature = "gpu")]
fn sampler(state: Arc<Mutex<GpuState>>, period: std::time::Duration, shutdown: tokio_util::sync::CancellationToken) {
    use nvml_wrapper::Nvml;
    use nvml_wrapper::enum_wrappers::device::TemperatureSensor;

    let nvml = match Nvml::init() {
        Ok(n) => n,
        Err(e) => {
            let mut s = state.lock();
            s.available = false;
            s.error = Some(format!("NVML недоступен: {e}"));
            return;
        }
    };
    let driver = nvml.sys_driver_version().ok();
    while !shutdown.is_cancelled() {
        let mut devices = Vec::new();
        let count = nvml.device_count().unwrap_or(0);
        for i in 0..count {
            let Ok(d) = nvml.device_by_index(i) else { continue };
            let util = d.utilization_rates().ok();
            let mem = d.memory_info().ok();
            devices.push(GpuInfo {
                index: i,
                name: d.name().unwrap_or_else(|_| format!("GPU {i}")),
                util: util.as_ref().map(|u| u.gpu).unwrap_or(0),
                mem_util: util.as_ref().map(|u| u.memory).unwrap_or(0),
                mem_used_mb: mem.as_ref().map(|m| m.used / 1_048_576).unwrap_or(0),
                mem_total_mb: mem.as_ref().map(|m| m.total / 1_048_576).unwrap_or(0),
                temp_c: d.temperature(TemperatureSensor::Gpu).ok(),
                power_w: d.power_usage().ok().map(|mw| mw as f64 / 1000.0),
                power_limit_w: d.enforced_power_limit().ok().map(|mw| mw as f64 / 1000.0),
                fan: d.fan_speed(0).ok(),
            });
        }
        {
            let mut s = state.lock();
            s.available = true;
            s.driver = driver.clone();
            s.error = None;
            s.devices = devices;
            s.sampled_at = crate::util::now_ms();
        }
        std::thread::sleep(period);
    }
}

#[cfg(not(feature = "gpu"))]
fn sampler(state: Arc<Mutex<GpuState>>, _period: std::time::Duration, _shutdown: tokio_util::sync::CancellationToken) {
    state.lock().error = Some("собрано без поддержки GPU".into());
}
