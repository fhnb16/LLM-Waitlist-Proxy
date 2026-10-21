//! Один экземпляр приложения: именованный мьютекс; второй запуск передаёт первому
//! «show» через именованный канал и завершается.

use std::time::Duration;

const PIPE: &str = r"\\.\pipe\lmq-desktop";

#[cfg(windows)]
fn mutex_name() -> Vec<u16> {
    let user = std::env::var("USERNAME").unwrap_or_default();
    format!("Local\\LMQ-desktop-{user}").encode_utf16().chain(std::iter::once(0)).collect()
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
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(PIPE) {
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
            let server = match ServerOptions::new().first_pipe_instance(first).create(PIPE) {
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
