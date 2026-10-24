//! Исходящие маршруты: через какой интерфейс и прокси идёт каждое направление
//! (модели, бот, Telegram). Нужен, когда, например, зарубежный апстрим доступен только
//! через VPN, а сервер бота — только напрямую.
//!
//! Интерфейс задаётся именем (`Ethernet`, `happ-tun`, `eth0`) или адресом. На Linux имя
//! привязывается через `SO_BINDTODEVICE`, на Windows и macOS — исходным адресом интерфейса:
//! Windows отправляет такие пакеты через интерфейс, которому принадлежит адрес.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;

use crate::app::VERSION;
use crate::config::RouteCfg;

#[derive(Clone, Debug, Serialize)]
pub struct Iface {
    pub name: String,
    pub ipv4: Vec<IpAddr>,
    pub ipv6: Vec<IpAddr>,
    /// Физический выход в интернет без туннеля (см. `uplinks`).
    pub uplink: bool,
}

/// Интерфейсы с адресами, без loopback.
pub fn interfaces() -> Vec<Iface> {
    let up = uplinks();
    let mut out: Vec<Iface> = Vec::new();
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        if i.is_loopback() {
            continue;
        }
        let ip = i.ip();
        let e = match out.iter_mut().find(|x| x.name == i.name) {
            Some(e) => e,
            None => {
                let uplink = up.iter().any(|u| u.eq_ignore_ascii_case(&i.name));
                out.push(Iface { name: i.name.clone(), ipv4: Vec::new(), ipv6: Vec::new(), uplink });
                out.last_mut().unwrap()
            }
        };
        if ip.is_ipv4() { e.ipv4.push(ip) } else { e.ipv6.push(ip) }
    }
    out
}

/// Признаки туннеля VPN в имени или описании адаптера.
const TUNNEL_MARKS: &[&str] = &["tun", "tap", "wintun", "wireguard", "wg", "vpn", "openvpn", "tailscale", "zerotier", "nordlynx", "outline", "happ"];

fn looks_like_tunnel(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    TUNNEL_MARKS.iter().any(|m| n.starts_with(m) || n.contains(&format!("-{m}")) || n.contains(&format!(" {m}")) || n.contains(&format!("{m}-")))
}

/// Физические интерфейсы с основным шлюзом (Ethernet, Wi-Fi, сотовый модем), а не туннели VPN:
/// через них машина видна снаружи, даже когда VPN в режиме TUN забирает системный маршрут.
/// Первым — интерфейс с наименьшей метрикой маршрута.
pub fn uplinks() -> Vec<String> {
    let mut list = uplinks_os();
    list.sort_by_key(|(metric, _)| *metric);
    let mut out: Vec<String> = Vec::new();
    for (_, name) in list {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Основной интерфейс без туннеля, если он есть.
pub fn primary_uplink() -> Option<String> {
    uplinks().into_iter().next()
}

#[cfg(windows)]
fn uplinks_os() -> Vec<(u32, String)> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
    };
    // Типы интерфейсов (ipifcons.h): Ethernet, Wi-Fi, мобильный широкополосный доступ.
    const KINDS: [u32; 4] = [6, 71, 243, 244];
    const IF_OPER_STATUS_UP: i32 = 1;
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size: u32 = 32 * 1024;
    // Буфер u64: выравнивание для структур адаптеров.
    let mut buf: Vec<u64> = Vec::new();
    let mut ok = false;
    for _ in 0..4 {
        buf = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: буфер размером `size` байт, функция пишет в него не больше и обновляет `size`.
        let r = unsafe { GetAdaptersAddresses(0, flags, std::ptr::null(), buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(), &mut size) };
        if r == 0 {
            ok = true;
            break;
        }
        if r != ERROR_BUFFER_OVERFLOW {
            break;
        }
    }
    if !ok {
        return Vec::new();
    }
    // SAFETY: строка UTF-16 с нулём на конце из структуры адаптера, живёт вместе с `buf`.
    let wide = |p: *mut u16| -> String {
        if p.is_null() {
            return String::new();
        }
        unsafe {
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        }
    };
    let mut out = Vec::new();
    let mut p = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while !p.is_null() {
        // SAFETY: связный список, который GetAdaptersAddresses построил внутри `buf`.
        let a = unsafe { &*p };
        let name = wide(a.FriendlyName);
        let desc = wide(a.Description);
        if a.OperStatus == IF_OPER_STATUS_UP
            && !a.FirstGatewayAddress.is_null()
            && KINDS.contains(&a.IfType)
            && !looks_like_tunnel(&name)
            && !looks_like_tunnel(&desc)
        {
            out.push((a.Ipv4Metric, name));
        }
        p = a.Next;
    }
    out
}

#[cfg(target_os = "linux")]
fn uplinks_os() -> Vec<(u32, String)> {
    default_routes(&std::fs::read_to_string("/proc/net/route").unwrap_or_default())
}

/// Интерфейсы маршрутов по умолчанию из `/proc/net/route` (Destination 00000000; столбец 6 —
/// метрика), без туннелей и мостов контейнеров.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn default_routes(text: &str) -> Vec<(u32, String)> {
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<&str> = l.split_whitespace().collect();
            (c.len() > 6 && c[1] == "00000000").then(|| (c[6].parse().unwrap_or(0), c[0].to_string()))
        })
        .filter(|(_, n)| !looks_like_tunnel(n) && !["docker", "veth", "br-", "virbr"].iter().any(|p| n.starts_with(p)))
        .collect()
}

#[cfg(not(any(windows, target_os = "linux")))]
fn uplinks_os() -> Vec<(u32, String)> {
    Vec::new()
}

#[derive(Clone, Debug, Serialize)]
pub struct ProxyHint {
    pub url: String,
    /// Чей это обычно порт — подсказка, а не проверка.
    pub label: &'static str,
}

/// Локальные прокси VPN-клиентов, которые сейчас принимают соединения (xray/v2rayN, Clash,
/// sing-box, Tor): для выпадающего списка «прокси».
pub async fn detect_local_proxies() -> Vec<ProxyHint> {
    const KNOWN: &[(u16, &str, &str)] = &[
        (10808, "socks5h", "xray / v2rayN / Happ, SOCKS"),
        (10809, "http", "xray / v2rayN / Happ, HTTP"),
        (1080, "socks5h", "SOCKS"),
        (2080, "socks5h", "sing-box / NekoBox, mixed"),
        (7890, "http", "Clash, mixed"),
        (7891, "socks5h", "Clash, SOCKS"),
        (9050, "socks5h", "Tor"),
        (8889, "http", "HTTP"),
    ];
    let checks = KNOWN.iter().map(|(port, scheme, label)| async move {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], *port));
        let open = matches!(tokio::time::timeout(Duration::from_millis(250), tokio::net::TcpStream::connect(addr)).await, Ok(Ok(_)));
        open.then(|| ProxyHint { url: format!("{scheme}://127.0.0.1:{port}"), label })
    });
    futures::future::join_all(checks).await.into_iter().flatten().collect()
}

fn is_system(s: &str) -> bool {
    let s = s.trim();
    s.is_empty() || s.eq_ignore_ascii_case("system") || s.eq_ignore_ascii_case("default")
}

/// Прокси не задан явно: пусто, `none`, `direct` или `system`.
fn is_none_proxy(s: &str) -> bool {
    let s = s.trim();
    s.is_empty() || s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("direct") || s.eq_ignore_ascii_case("system")
}

/// Как выходить в сеть: системный маршрут, интерфейс по имени (Linux) или исходный адрес.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Bind {
    System,
    #[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
    Device(String),
    Addr(IpAddr),
}

impl std::fmt::Display for Bind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bind::System => f.write_str("system"),
            Bind::Device(d) => write!(f, "dev {d}"),
            Bind::Addr(a) => write!(f, "{a}"),
        }
    }
}

/// Интерфейс из настроек → привязка. Имя ищется без учёта регистра; у интерфейса берётся
/// IPv4 (если его нет — первый IPv6).
pub fn resolve_bind(spec: &str) -> Result<Bind, String> {
    if is_system(spec) {
        return Ok(Bind::System);
    }
    let spec = spec.trim();
    if let Ok(ip) = spec.parse::<IpAddr>() {
        return Ok(Bind::Addr(ip));
    }
    let list = interfaces();
    let Some(i) = list.iter().find(|i| i.name.eq_ignore_ascii_case(spec)) else {
        let names: Vec<&str> = list.iter().map(|i| i.name.as_str()).collect();
        return Err(format!("интерфейс '{spec}' не найден (есть: {})", names.join(", ")));
    };
    if cfg!(any(target_os = "linux", target_os = "android")) {
        return Ok(Bind::Device(i.name.clone()));
    }
    match i.ipv4.first().or(i.ipv6.first()) {
        Some(ip) => Ok(Bind::Addr(*ip)),
        None => Err(format!("у интерфейса '{spec}' нет адреса — он отключён?")),
    }
}

/// TCP-соединение с `addr` через привязку `bind` (интерфейс или его адрес).
pub async fn connect_via(addr: std::net::SocketAddr, bind: &Bind) -> std::io::Result<tokio::net::TcpStream> {
    let sock = if addr.is_ipv4() { tokio::net::TcpSocket::new_v4()? } else { tokio::net::TcpSocket::new_v6()? };
    match bind {
        Bind::System => {}
        Bind::Addr(ip) if ip.is_ipv4() != addr.is_ipv4() => return Err(std::io::Error::other("адрес другого семейства, чем у интерфейса")),
        Bind::Addr(ip) => sock.bind(std::net::SocketAddr::new(*ip, 0))?,
        #[cfg(any(target_os = "linux", target_os = "android"))]
        Bind::Device(d) => sock.bind_device(Some(d.as_bytes()))?,
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        Bind::Device(_) => {}
    }
    let stream = sock.connect(addr).await?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// Прокси без пароля — для журнала.
pub fn redact_proxy(p: &str) -> String {
    match reqwest::Url::parse(p) {
        Ok(mut u) if u.password().is_some() || !u.username().is_empty() => {
            let _ = u.set_username("***");
            let _ = u.set_password(None);
            u.to_string()
        }
        _ => p.to_string(),
    }
}

/// Маршрут словами: `интерфейс 192.168.1.228, прокси socks5://127.0.0.1:10808`.
pub fn describe(route: &RouteCfg) -> String {
    let iface =
        if is_system(&route.interface) { "системный маршрут".to_string() } else { format!("интерфейс {}", route.interface.trim()) };
    if is_none_proxy(&route.proxy) { iface } else { format!("{iface}, прокси {}", redact_proxy(route.proxy.trim())) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Модели и прочие запросы: редиректы разрешены.
    General,
    /// Колбэки боту: без редиректов (защита от SSRF).
    NoRedirect,
}

type Key = (Kind, Bind, String);

/// HTTP-клиенты по маршрутам. Клиент пересоздаётся, если у интерфейса сменился адрес
/// (переподключение VPN, DHCP): адрес — часть ключа.
#[derive(Default)]
pub struct NetPool {
    cache: Mutex<HashMap<Key, reqwest::Client>>,
}

impl NetPool {
    pub fn client(&self, route: &RouteCfg, kind: Kind) -> Result<reqwest::Client, String> {
        let bind = resolve_bind(&route.interface)?;
        // Пусто — как в системе (переменные HTTP(S)_PROXY), `none` — напрямую, иначе — адрес прокси.
        let proxy = route.proxy.trim().to_string();
        let key = (kind, bind.clone(), proxy.clone());
        if let Some(c) = self.cache.lock().get(&key) {
            return Ok(c.clone());
        }
        let mut b =
            reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).pool_idle_timeout(Duration::from_secs(90)).user_agent(format!("LMQ/{VERSION}"));
        if kind == Kind::NoRedirect {
            b = b.redirect(reqwest::redirect::Policy::none());
        }
        match &bind {
            Bind::System => {}
            Bind::Addr(ip) => b = b.local_address(*ip),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            Bind::Device(d) => b = b.interface(d),
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            Bind::Device(_) => {}
        }
        let follows_system = proxy.is_empty() || proxy.eq_ignore_ascii_case("system");
        if !follows_system {
            if is_none_proxy(&proxy) {
                b = b.no_proxy();
            } else {
                let p = reqwest::Proxy::all(&proxy).map_err(|e| format!("прокси '{}': {e}", redact_proxy(&proxy)))?;
                b = b.proxy(p);
            }
        }
        let c = b.build().map_err(|e| format!("HTTP-клиент для маршрута ({}): {e}", describe(route)))?;
        let mut cache = self.cache.lock();
        // Старые клиенты прежних адресов не копятся бесконечно.
        if cache.len() > 32 {
            cache.clear();
        }
        cache.insert(key, c.clone());
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_and_routes() {
        assert_eq!(resolve_bind("").unwrap(), Bind::System);
        assert_eq!(resolve_bind("System").unwrap(), Bind::System);
        assert_eq!(resolve_bind("127.0.0.1").unwrap(), Bind::Addr("127.0.0.1".parse().unwrap()));
        assert!(resolve_bind("no-such-iface-lmq").unwrap_err().contains("не найден"));
        let r = RouteCfg { interface: String::new(), proxy: "socks5://u:secret@127.0.0.1:1080".into() };
        assert!(!describe(&r).contains("secret"));
        let own = RouteCfg { interface: "eth0".into(), proxy: String::new() };
        let base = RouteCfg { interface: "tun0".into(), proxy: "socks5://p:1".into() };
        assert_eq!(own.inherit(&base), RouteCfg { interface: "eth0".into(), proxy: "socks5://p:1".into() });
    }

    #[test]
    fn tunnels_are_not_uplinks() {
        for n in ["happ-tun", "tun0", "wg0", "Wintun Userspace Tunnel", "TAP-Windows Adapter V9", "Fortinet SSL VPN Virtual Ethernet Adapter", "tailscale0"] {
            assert!(looks_like_tunnel(n), "{n}");
        }
        for n in ["Ethernet", "eth0", "enp3s0", "Wi-Fi", "Intel(R) Wi-Fi 6 AX201 160MHz", "Realtek PCIe GbE Family Controller", "Беспроводная сеть"]
        {
            assert!(!looks_like_tunnel(n), "{n}");
        }
        let route = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\n\
                     tun0\t00000000\t00000000\t0001\t0\t0\t0\t00000000\n\
                     eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\n\
                     eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\n";
        assert_eq!(default_routes(route), vec![(100, "eth0".to_string())]);
    }

    #[test]
    fn pool_reuses_clients_and_rejects_bad_proxy() {
        let pool = NetPool::default();
        let r = RouteCfg::default();
        assert!(pool.client(&r, Kind::General).is_ok());
        assert!(pool.client(&r, Kind::NoRedirect).is_ok());
        assert_eq!(pool.cache.lock().len(), 2);
        assert!(pool.client(&r, Kind::General).is_ok());
        assert_eq!(pool.cache.lock().len(), 2);
        let bad = RouteCfg { interface: String::new(), proxy: "::not a url::".into() };
        assert!(pool.client(&bad, Kind::General).is_err());
    }
}
