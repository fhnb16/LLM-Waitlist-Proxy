//! SOCKS5-прокси LMQ: слушает заданный порт и выводит соединения через выбранный интерфейс
//! (VPN, Ethernet, IP). Имена разрешает сам прокси — клиенту `socks5h://`. Поддерживается
//! CONNECT (TCP); вход без пароля или по логину и паролю (RFC 1929). Пароль и передаваемые
//! данные в журнал не пишутся; адрес назначения — только у неудачных соединений и на debug.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::app::App;
use crate::net::{Bind, connect_via, resolve_bind};
use crate::util::{ct_eq, now_ms};

/// Сколько ждать приветствия, входа и команды клиента.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// Сколько ждать соединения с адресом назначения (каждым из адресов имени).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Больше одновременных соединений не принимается.
const MAX_ACTIVE: u64 = 1024;

// Коды ответа SOCKS5 (RFC 1928, 6).
const REP_OK: u8 = 0x00;
const REP_FAILURE: u8 = 0x01;
const REP_NET_UNREACHABLE: u8 = 0x03;
const REP_HOST_UNREACHABLE: u8 = 0x04;
const REP_REFUSED: u8 = 0x05;
const REP_TTL_EXPIRED: u8 = 0x06;
const REP_CMD_UNSUPPORTED: u8 = 0x07;
const REP_ATYP_UNSUPPORTED: u8 = 0x08;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    /// Адрес, на котором прокси слушает.
    pub listening: Option<String>,
    /// Почему не слушает (порт занят, неверный адрес).
    pub listen_error: Option<String>,
    pub active: u64,
    /// Соединений установлено.
    pub connections: u64,
    /// Не удалось подключиться к адресу назначения (DNS, интерфейс, отказ).
    pub failed: u64,
    /// Отказы прокси: неверный пароль, другая версия протокола, команда кроме CONNECT.
    pub rejected: u64,
    /// Байт от клиентов в сеть и обратно (считаются по ходу передачи).
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
}

/// Состояние прокси: адрес, счётчики с запуска.
#[derive(Default)]
pub struct Socks {
    stats: Mutex<Snapshot>,
    active: AtomicU64,
    bytes_up: AtomicU64,
    bytes_down: AtomicU64,
    /// Внешний IP для ссылок: (интерфейс, IP, когда узнали) — чтобы не спрашивать каждый раз.
    public_ip: Mutex<Option<(String, String, i64)>>,
}

/// Сколько помнить внешний IP.
const PUBLIC_IP_TTL_MS: i64 = 5 * 60_000;

impl Socks {
    /// Внешний IP для ссылок через интерфейс без туннеля (запомненный на 5 минут).
    pub async fn public_ip(&self, net: &crate::net::NetPool, interface: &str, refresh: bool) -> Result<String, String> {
        if !refresh
            && let Some((i, ip, at)) = self.public_ip.lock().clone()
            && i == interface
            && now_ms() - at < PUBLIC_IP_TTL_MS
        {
            return Ok(ip);
        }
        let http = net.client(&route_of(interface), crate::net::Kind::General)?;
        let ip = async {
            let r = http.get(crate::netdiag::IP_ECHO).timeout(Duration::from_secs(8)).send().await?.error_for_status()?;
            r.text().await
        }
        .await
        .map_err(|e| format!("внешний IP не определён: {}", crate::netdiag::error_chain(&e)))?;
        let ip: String = ip.trim().chars().take(64).collect();
        *self.public_ip.lock() = Some((interface.to_string(), ip.clone(), now_ms()));
        Ok(ip)
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut s = self.stats.lock().clone();
        s.active = self.active.load(Ordering::Relaxed);
        s.bytes_up = self.bytes_up.load(Ordering::Relaxed);
        s.bytes_down = self.bytes_down.load(Ordering::Relaxed);
        s
    }

    pub fn bound(&self) -> Option<SocketAddr> {
        self.stats.lock().listening.as_deref().and_then(|a| a.parse().ok())
    }

    fn set_bound(&self, addr: Option<SocketAddr>, error: Option<String>) {
        let mut s = self.stats.lock();
        s.listening = addr.map(|a| a.to_string());
        s.listen_error = error;
    }

    fn error(&self, failed: bool, msg: String) {
        let mut s = self.stats.lock();
        if failed {
            s.failed += 1;
        } else {
            s.rejected += 1;
        }
        s.last_error = Some(msg);
        s.last_error_at = Some(now_ms());
    }
}

/// Уменьшает счётчик активных соединений, как бы ни закончилось соединение.
struct ActiveGuard<'a>(&'a AtomicU64);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Поднимает слушатель и перезапускает его при смене `socks.enabled` / `socks.listen`.
pub async fn run(app: Arc<App>) {
    loop {
        if app.shutdown.is_cancelled() {
            break;
        }
        let mut epoch = app.socks_epoch.subscribe();
        epoch.borrow_and_update();
        let cfg = app.settings().socks.clone();
        if !cfg.enabled {
            app.socks.set_bound(None, None);
            if wait_change(&app, &mut epoch).await {
                continue;
            }
            break;
        }
        let addr: SocketAddr = match cfg.listen.trim().parse() {
            Ok(a) => a,
            Err(_) => {
                app.socks.set_bound(None, Some(format!("неверный адрес {}", cfg.listen)));
                if wait_change(&app, &mut epoch).await {
                    continue;
                }
                break;
            }
        };
        let listener = match TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                let msg = format!("cannot listen on {addr}: {e}");
                tracing::error!(error = %msg, "socks proxy: bind failed");
                app.socks.set_bound(None, Some(msg));
                tokio::select! {
                    _ = epoch.changed() => {}
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                    _ = app.shutdown.cancelled() => break,
                }
                continue;
            }
        };
        let local = listener.local_addr().ok();
        app.socks.set_bound(local, None);
        if !addr.ip().is_loopback() && cfg.username.is_empty() {
            tracing::warn!(addr = %addr, "socks proxy listens beyond localhost without a password: anyone who reaches the port can use it");
        }
        tracing::info!(addr = %local.map(|a| a.to_string()).unwrap_or_default(), interface = %crate::net::describe(&route_of(&cfg.interface)), "socks proxy listening");
        loop {
            tokio::select! {
                r = listener.accept() => match r {
                    Ok((stream, peer)) => {
                        if app.socks.active.load(Ordering::Relaxed) >= MAX_ACTIVE {
                            tracing::warn!(peer = %peer, "socks proxy: too many connections, dropped");
                            continue;
                        }
                        app.socks.active.fetch_add(1, Ordering::Relaxed);
                        tokio::spawn(handle(app.clone(), stream, peer));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "socks proxy: accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                },
                _ = epoch.changed() => break,
                _ = app.shutdown.cancelled() => break,
            }
        }
        app.socks.set_bound(None, None);
    }
}

/// Ждёт смены настроек прокси (`true`) или остановки LMQ (`false`).
async fn wait_change(app: &App, epoch: &mut tokio::sync::watch::Receiver<u64>) -> bool {
    tokio::select! {
        _ = epoch.changed() => true,
        _ = app.shutdown.cancelled() => false,
    }
}

/// Маршрут «интерфейс, без прокси».
pub fn route_of(interface: &str) -> crate::config::RouteCfg {
    crate::config::RouteCfg { interface: interface.to_string(), proxy: "none".into() }
}

/// `socks5h://логин:пароль@хост:порт`; без `with_password` — только логин.
pub fn link(c: &crate::config::SocksCfg, host: &str, port: u16, with_password: bool) -> String {
    let h = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
    let Ok(mut u) = url::Url::parse(&format!("socks5h://{h}:{port}")) else { return format!("socks5h://{h}:{port}") };
    if !c.username.is_empty() {
        let _ = u.set_username(&c.username);
        if with_password && !c.password.is_empty() {
            let _ = u.set_password(Some(&c.password));
        }
    }
    u.to_string()
}

/// Ссылка, которая открывает настройку прокси в Telegram: `https://t.me/socks?server=…`.
/// (`t.me/proxy` — для MTProto-прокси с `secret`, SOCKS5 — `t.me/socks`.)
pub fn telegram_link(c: &crate::config::SocksCfg, host: &str, port: u16) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    q.append_pair("server", host.trim_start_matches('[').trim_end_matches(']')).append_pair("port", &port.to_string());
    if !c.username.is_empty() {
        q.append_pair("user", &c.username).append_pair("pass", &c.password);
    }
    format!("https://t.me/socks?{}", q.finish())
}

/// Чем закончилось рукопожатие, если соединение не установлено.
enum HsError {
    /// Отказ прокси (пароль, протокол, команда).
    Rejected(String),
    /// Не удалось подключиться к адресу назначения.
    Failed(String),
    /// Клиент ушёл или прислал обрывок — считать нечего.
    Io,
}

impl From<io::Error> for HsError {
    fn from(_: io::Error) -> Self {
        HsError::Io
    }
}

async fn handle(app: Arc<App>, mut client: TcpStream, peer: SocketAddr) {
    let _guard = ActiveGuard(&app.socks.active);
    let (target, remote) = match tokio::time::timeout(HANDSHAKE_TIMEOUT + CONNECT_TIMEOUT, handshake(&app, &mut client)).await {
        Ok(Ok(x)) => x,
        Ok(Err(HsError::Rejected(m))) => {
            tracing::info!(peer = %peer, reason = %m, "socks proxy: rejected");
            app.socks.error(false, m);
            return;
        }
        Ok(Err(HsError::Failed(m))) => {
            tracing::info!(peer = %peer, error = %m, "socks proxy: connect failed");
            app.socks.error(true, m);
            return;
        }
        Ok(Err(HsError::Io)) | Err(_) => return,
    };
    app.socks.stats.lock().connections += 1;
    tracing::debug!(peer = %peer, target = %target, "socks proxy: connected");
    let (cr, cw) = client.into_split();
    let (rr, rw) = remote.into_split();
    let up = pump(cr, rw, &app.socks.bytes_up);
    let down = pump(rr, cw, &app.socks.bytes_down);
    tokio::pin!(up, down);
    // Конец данных с одной стороны — ждём, пока закончит другая; ошибка (обрыв) закрывает обе.
    tokio::select! {
        r = &mut up => if r.is_ok() { let _ = down.await; },
        r = &mut down => if r.is_ok() { let _ = up.await; },
    }
}

/// Перекачивает данные в одну сторону и сразу считает байты: счётчики растут по ходу
/// передачи и не теряются, если соединение оборвалось. `Ok` — другая сторона закончила
/// передачу (конец данных), `Err` — обрыв.
async fn pump(mut from: tokio::net::tcp::OwnedReadHalf, mut to: tokio::net::tcp::OwnedWriteHalf, counter: &AtomicU64) -> io::Result<()> {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = from.read(&mut buf).await?;
        if n == 0 {
            // Полузакрытие: передаём конец данных дальше, обратное направление ещё работает.
            let _ = to.shutdown().await;
            return Ok(());
        }
        to.write_all(&buf[..n]).await?;
        counter.fetch_add(n as u64, Ordering::Relaxed);
    }
}

async fn reply(s: &mut TcpStream, code: u8, bound: Option<SocketAddr>) -> io::Result<()> {
    let addr = bound.unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0)));
    let mut out = vec![5, code, 0];
    match addr.ip() {
        IpAddr::V4(ip) => {
            out.push(1);
            out.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            out.push(4);
            out.extend_from_slice(&ip.octets());
        }
    }
    out.extend_from_slice(&addr.port().to_be_bytes());
    s.write_all(&out).await
}

async fn read_string(s: &mut TcpStream) -> io::Result<Vec<u8>> {
    let len = s.read_u8().await? as usize;
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Приветствие, вход, команда CONNECT и соединение с адресом назначения.
async fn handshake(app: &App, s: &mut TcpStream) -> Result<(String, TcpStream), HsError> {
    let cfg = app.settings().socks.clone();
    let mut head = [0u8; 2];
    tokio::time::timeout(HANDSHAKE_TIMEOUT, s.read_exact(&mut head)).await.map_err(|_| HsError::Io)??;
    if head[0] != 5 {
        return Err(HsError::Rejected(format!("не SOCKS5 (версия {})", head[0])));
    }
    let mut methods = vec![0u8; head[1] as usize];
    s.read_exact(&mut methods).await?;
    let need_auth = !cfg.username.is_empty();
    let method = if need_auth { 2 } else { 0 };
    if !methods.contains(&method) {
        s.write_all(&[5, 0xFF]).await?;
        let why = if need_auth {
            "клиент не предложил вход по логину и паролю"
        } else {
            "клиент не предложил вход без пароля"
        };
        return Err(HsError::Rejected(why.into()));
    }
    s.write_all(&[5, method]).await?;

    if need_auth {
        let ver = s.read_u8().await?;
        let user = read_string(s).await?;
        let pass = read_string(s).await?;
        // Оба сравнения выполняются всегда: время не выдаёт, что именно не совпало.
        let user_ok = ct_eq(&user, cfg.username.as_bytes());
        let pass_ok = ct_eq(&pass, cfg.password.as_bytes());
        let ok = ver == 1 && user_ok & pass_ok;
        s.write_all(&[1, if ok { 0 } else { 1 }]).await?;
        if !ok {
            return Err(HsError::Rejected("неверный логин или пароль".into()));
        }
    }

    let mut req = [0u8; 4];
    s.read_exact(&mut req).await?;
    if req[0] != 5 {
        return Err(HsError::Rejected(format!("не SOCKS5 (версия {})", req[0])));
    }
    let host = match req[3] {
        1 => {
            let mut b = [0u8; 4];
            s.read_exact(&mut b).await?;
            IpAddr::from(b).to_string()
        }
        3 => String::from_utf8_lossy(&read_string(s).await?).into_owned(),
        4 => {
            let mut b = [0u8; 16];
            s.read_exact(&mut b).await?;
            IpAddr::from(b).to_string()
        }
        other => {
            reply(s, REP_ATYP_UNSUPPORTED, None).await?;
            return Err(HsError::Rejected(format!("неизвестный тип адреса {other}")));
        }
    };
    let port = s.read_u16().await?;
    if req[1] != 1 {
        reply(s, REP_CMD_UNSUPPORTED, None).await?;
        let cmd = match req[1] {
            2 => "BIND".to_string(),
            3 => "UDP ASSOCIATE".to_string(),
            c => c.to_string(),
        };
        return Err(HsError::Rejected(format!("команда {cmd} не поддерживается, только CONNECT (TCP)")));
    }
    let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };

    let bind = match resolve_bind(&cfg.interface) {
        Ok(b) => b,
        Err(e) => {
            reply(s, REP_NET_UNREACHABLE, None).await?;
            return Err(HsError::Failed(e));
        }
    };
    let addrs: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host.as_str(), port))).await {
            Ok(Ok(a)) => a.collect(),
            Ok(Err(e)) => {
                reply(s, REP_HOST_UNREACHABLE, None).await?;
                return Err(HsError::Failed(format!("DNS не нашёл {host}: {e}")));
            }
            Err(_) => {
                reply(s, REP_HOST_UNREACHABLE, None).await?;
                return Err(HsError::Failed(format!("DNS не ответил за {} с: {host}", CONNECT_TIMEOUT.as_secs())));
            }
        },
    };
    // С привязкой к адресу интерфейса годятся только адреса того же семейства.
    let usable: Vec<SocketAddr> = match &bind {
        Bind::Addr(ip) => addrs.iter().copied().filter(|a| a.is_ipv4() == ip.is_ipv4()).collect(),
        _ => addrs.clone(),
    };
    if usable.is_empty() {
        reply(s, REP_HOST_UNREACHABLE, None).await?;
        return Err(HsError::Failed(format!("{target}: нет адресов того же семейства (IPv4/IPv6), что у интерфейса")));
    }
    let mut last = (REP_FAILURE, String::new());
    for a in usable {
        match tokio::time::timeout(CONNECT_TIMEOUT, connect_via(a, &bind)).await {
            Ok(Ok(remote)) => {
                reply(s, REP_OK, remote.local_addr().ok()).await?;
                return Ok((target, remote));
            }
            Ok(Err(e)) => {
                let code = match e.kind() {
                    io::ErrorKind::ConnectionRefused => REP_REFUSED,
                    io::ErrorKind::NetworkUnreachable => REP_NET_UNREACHABLE,
                    io::ErrorKind::HostUnreachable => REP_HOST_UNREACHABLE,
                    io::ErrorKind::TimedOut => REP_TTL_EXPIRED,
                    _ => REP_FAILURE,
                };
                last = (code, format!("{a}: {e}"));
            }
            Err(_) => last = (REP_TTL_EXPIRED, format!("{a}: нет ответа за {} с", CONNECT_TIMEOUT.as_secs())),
        }
    }
    reply(s, last.0, None).await?;
    Err(HsError::Failed(format!("{target} через {bind}: {}", last.1)))
}
