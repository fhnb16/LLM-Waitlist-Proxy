//! Диагностика сетевого пути до апстрима или бота через выбранный маршрут (интерфейс,
//! прокси): DNS, TCP до каждого адреса, HTTP-запрос и внешний IP, с которого маршрут
//! выходит в интернет. Запрос — `GET` без тела: ничего не отправляет ни модели, ни боту.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::RouteCfg;
use crate::net::{Bind, Kind, NetPool, describe, resolve_bind};

/// Сервис, который возвращает внешний IP обычным текстом.
pub const IP_ECHO: &str = "https://api.ipify.org";

#[derive(Debug, Serialize)]
pub struct AddrProbe {
    pub addr: String,
    pub family: &'static str,
    pub ok: bool,
    pub ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Probe {
    pub url: String,
    pub route: String,
    pub host: String,
    pub port: u16,
    pub dns_ms: u64,
    pub dns_error: Option<String>,
    /// Пусто, если запрос идёт через прокси: адрес назначения тогда разрешает прокси.
    pub addrs: Vec<AddrProbe>,
    /// HTTP-статус: любой ответ значит, что сервер достижим.
    pub http_status: Option<u16>,
    pub http_ms: u64,
    pub http_error: Option<String>,
    /// Внешний IP этого маршрута (через VPN или напрямую).
    pub exit_ip: Option<String>,
    /// Короткий вывод для человека.
    pub verdict: String,
}

/// Полная цепочка причин ошибки: `error sending request: client error (Connect): …`.
pub fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(inner) = src {
        let t = inner.to_string();
        if !s.contains(&t) {
            s = format!("{s}: {t}");
        }
        src = inner.source();
    }
    s
}

fn ms(t: Instant) -> u64 {
    t.elapsed().as_millis() as u64
}

/// TCP-подключение с исходным адресом `bind` (выход через конкретный интерфейс).
async fn connect_from(addr: SocketAddr, bind: Option<IpAddr>) -> std::io::Result<tokio::net::TcpStream> {
    let Some(ip) = bind else { return tokio::net::TcpStream::connect(addr).await };
    if ip.is_ipv4() != addr.is_ipv4() {
        return Err(std::io::Error::other("адрес другого семейства, чем у интерфейса"));
    }
    let sock = if addr.is_ipv4() { tokio::net::TcpSocket::new_v4()? } else { tokio::net::TcpSocket::new_v6()? };
    sock.bind(SocketAddr::new(ip, 0))?;
    sock.connect(addr).await
}

pub async fn probe_route(url: &str, timeout: Duration, route: &RouteCfg) -> Probe {
    let mut p = Probe {
        url: url.to_string(),
        route: describe(route),
        host: String::new(),
        port: 0,
        dns_ms: 0,
        dns_error: None,
        addrs: Vec::new(),
        http_status: None,
        http_ms: 0,
        http_error: None,
        exit_ip: None,
        verdict: String::new(),
    };
    let parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(e) => {
            p.verdict = format!("неверный адрес: {e}");
            return p;
        }
    };
    p.host = parsed.host_str().unwrap_or_default().to_string();
    p.port = parsed.port_or_known_default().unwrap_or(443);
    let bind = match resolve_bind(&route.interface) {
        Ok(b) => b,
        Err(e) => {
            p.verdict = e;
            return p;
        }
    };
    let via_proxy = !route.proxy.trim().is_empty() && !["none", "direct", "system"].contains(&route.proxy.trim().to_ascii_lowercase().as_str());

    if !via_proxy {
        let t = Instant::now();
        match tokio::time::timeout(timeout, tokio::net::lookup_host((p.host.as_str(), p.port))).await {
            Ok(Ok(addrs)) => {
                p.dns_ms = ms(t);
                let bind_ip = match &bind {
                    Bind::Addr(ip) => Some(*ip),
                    _ => None,
                };
                for a in addrs.collect::<Vec<_>>() {
                    let t = Instant::now();
                    let (ok, error) = match tokio::time::timeout(timeout, connect_from(a, bind_ip)).await {
                        Ok(Ok(_)) => (true, None),
                        Ok(Err(e)) => (false, Some(e.to_string())),
                        Err(_) => (false, Some(format!("нет ответа за {} с", timeout.as_secs()))),
                    };
                    p.addrs.push(AddrProbe { addr: a.to_string(), family: if a.is_ipv6() { "IPv6" } else { "IPv4" }, ok, ms: ms(t), error });
                }
            }
            Ok(Err(e)) => {
                p.dns_ms = ms(t);
                p.dns_error = Some(e.to_string());
            }
            Err(_) => {
                p.dns_ms = ms(t);
                p.dns_error = Some(format!("нет ответа DNS за {} с", timeout.as_secs()));
            }
        }
    }

    // Отдельный пул: проверка не должна влиять на клиентов работающего сервера.
    let pool = NetPool::default();
    match pool.client(route, Kind::NoRedirect) {
        Ok(c) => {
            let t = Instant::now();
            match c.get(url).timeout(timeout * 2).send().await {
                Ok(r) => p.http_status = Some(r.status().as_u16()),
                Err(e) => p.http_error = Some(error_chain(&e)),
            }
            p.http_ms = ms(t);
            if let Ok(Ok(r)) = tokio::time::timeout(timeout, c.get(IP_ECHO).send()).await
                && r.status().is_success()
                && let Ok(ip) = r.text().await
            {
                p.exit_ip = Some(ip.trim().chars().take(64).collect());
            }
        }
        Err(e) => p.http_error = Some(e),
    }
    p.verdict = verdict(&p, via_proxy);
    p
}

fn verdict(p: &Probe, via_proxy: bool) -> String {
    let exit = p.exit_ip.as_deref().map(|ip| format!(" Выход в интернет: {ip}.")).unwrap_or_default();
    if let Some(e) = &p.dns_error {
        return format!("DNS не нашёл {}: {e}. Проверьте подключение, DNS или VPN.{exit}", p.host);
    }
    let v4_ok = p.addrs.iter().any(|a| a.family == "IPv4" && a.ok);
    let has_v6 = p.addrs.iter().any(|a| a.family == "IPv6");
    let v6_ok = p.addrs.iter().any(|a| a.family == "IPv6" && a.ok);
    match (p.http_status, &p.http_error) {
        (Some(s), _) => format!("Сервер достижим: HTTP {s} за {} мс.{exit}", p.http_ms),
        (None, Some(e)) if via_proxy => format!("Через прокси запрос не прошёл: {e}. Проверьте, что прокси запущен и пускает к {}.{exit}", p.host),
        (None, Some(e)) if v4_ok && has_v6 && !v6_ok => {
            format!("TCP по IPv4 проходит, IPv6 нет, а HTTP-запрос не прошёл: {e}. Проверьте IPv6 и VPN.{exit}")
        }
        (None, Some(e)) if !p.addrs.is_empty() && !p.addrs.iter().any(|a| a.ok) => {
            format!("Ни один адрес {} не принимает соединения: {e}. Хост недоступен по этому маршруту (VPN, файрвол, блокировка).{exit}", p.host)
        }
        (None, Some(e)) => format!("Соединение есть, но HTTP-запрос не прошёл: {e}. Часто это VPN, который рвёт соединение, или TLS.{exit}"),
        (None, None) => "нет данных".into(),
    }
}
