//! Один экземпляр приложения на папку настроек: именованный мьютекс; второй запуск с
//! той же папкой передаёт первому «show» через именованный канал и завершается.
//! Установленный LMQ, портативная копия и `--home` другой папки работают параллельно.

use std::sync::OnceLock;
use std::time::Duration;

static KEY: OnceLock<String> = OnceLock::new();

/// Ключ экземпляра — короткий хэш папки настроек. Вызывается до остальных функций.
pub fn init(config_dir: &std::path::Path) {
    let path = config_dir.to_string_lossy().to_lowercase();
    let _ = KEY.set(lmq_core::util::sha256_hex(&path)[..12].to_string());
}

fn key() -> &'static str {
    KEY.get().map(String::as_str).unwrap_or("default")
}

#[cfg(windows)]
fn pipe_name() -> String {
    format!(r"\\.\pipe\lmq-desktop-{}", key())
}

#[cfg(windows)]
fn mutex_name() -> Vec<u16> {
    let user = std::env::var("USERNAME").unwrap_or_default();
    format!("Local\\LMQ-desktop-{user}-{}", key()).encode_utf16().chain(std::iter::once(0)).collect()
}

/// Захватывает мьютекс. `false` — приложение уже запущено. `wait` — подождать, пока
/// предыдущий экземпляр завершится (перезапуск в другом режиме).
#[cfg(windows)]
pub fn acquire(wait: Duration) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let deadline = std::time::Instant::now() + wait;
    let name = mutex_name();
    loop {
        // SAFETY: имя — нуль-терминированная UTF-16 строка.
        let h = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        // SAFETY: читаем код ошибки сразу после вызова.
        let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if !h.is_null() && !exists {
            // Дескриптор не закрываем: мьютекс живёт до конца процесса.
            return true;
        }
        if !h.is_null() {
            // SAFETY: закрываем свой лишний дескриптор.
            unsafe { CloseHandle(h) };
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(not(windows))]
pub fn acquire(_wait: Duration) -> bool {
    true
}

/// Просит запущенный экземпляр показать окно.
#[cfg(windows)]
pub fn signal_show() {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(pipe_name()) {
        let _ = f.write_all(b"show\n");
    }
}

#[cfg(not(windows))]
pub fn signal_show() {}

/// Слушает канал; на каждое «show» вызывает `on_show`.
#[cfg(windows)]
pub fn listen(rt: &tokio::runtime::Handle, on_show: impl Fn() + Send + Sync + 'static) {
    use tokio::io::AsyncReadExt;
    use tokio::net::windows::named_pipe::ServerOptions;
    rt.spawn(async move {
        let mut first = true;
        loop {
            let server = match ServerOptions::new().first_pipe_instance(first).create(pipe_name()) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "pipe create failed");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            first = false;
            if server.connect().await.is_err() {
                continue;
            }
            let mut s = server;
            let mut buf = [0u8; 64];
            if let Ok(n) = s.read(&mut buf).await
                && buf[..n].starts_with(b"show")
            {
                on_show();
            }
        }
    });
}

#[cfg(not(windows))]
pub fn listen(_rt: &tokio::runtime::Handle, _on_show: impl Fn() + Send + Sync + 'static) {}
