//! Sonde de diagnostic du service « web 1998 » (sans modem) :
//!     cargo run --release -p v34-server --example sonde1998 -- '/p?u=http://68k.news/'
//! Affiche le statut, le type et le corps (ou sa taille pour une image).
use v34_server::config;
use v34_server::web1998::Web1998;

fn main() {
    let w = Web1998::from_config(&config::Web1998::default(), Vec::new()).expect("service actif");
    for target in std::env::args().skip(1) {
        let t0 = std::time::Instant::now();
        let r = w.handle(&target);
        eprintln!("== {target} → {} {} ({} o, {:.1} s)", r.status, r.content_type, r.body.len(), t0.elapsed().as_secs_f32());
        if r.content_type.starts_with("text/") {
            println!("{}", String::from_utf8_lossy(&r.body));
        }
    }
}
