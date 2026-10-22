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
}

/// Интерфейсы с адресами, без loopback.
pub fn interfaces() -> Vec<Iface> {
    let mut out: Vec<Iface> = Vec::new();
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        if i.is_loopback() {
            continue;
        }
        let ip = i.ip();
        let e = match out.iter_mut().find(|x| x.name == i.name) {
            Some(e) => e,
            None => {
                out.push(Iface { name: i.name.clone(), ipv4: Vec::new(), ipv6: Vec::new() });
                out.last_mut().unwrap()
            }
        };
        if ip.is_ipv4() { e.ipv4.push(ip) } else { e.ipv6.push(ip) }
    }
    out
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
