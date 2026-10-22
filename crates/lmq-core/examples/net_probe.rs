//! Проверка сетевого пути клиентом LMQ:
//! `cargo run -p lmq-core --example net_probe -- <url> [интерфейс] [прокси]`.
//! То же умеет `lmq-server check-url`.

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().expect("usage: net_probe <url> [interface] [proxy]");
    let route = lmq_core::config::RouteCfg { interface: args.next().unwrap_or_default(), proxy: args.next().unwrap_or_default() };
    let r = lmq_core::netdiag::probe_route(&url, std::time::Duration::from_secs(10), &route).await;
    println!("{}", serde_json::to_string_pretty(&r).unwrap());
}
