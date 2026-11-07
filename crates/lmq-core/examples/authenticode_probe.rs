//! Подпись Authenticode файлов глазами автообновления (`authenticode::check`):
//! `cargo run -p lmq-core --example authenticode_probe -- <файл>…`

fn main() {
    for a in std::env::args().skip(1) {
        println!("{a}: {:?}", lmq_core::authenticode::check(std::path::Path::new(&a)));
    }
}
