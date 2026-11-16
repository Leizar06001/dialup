//! Trace une graine de la ligne « Orage » (tests/orage.rs), journal complet.
//!     cargo run --release -p v34-core --example orage_graine -- <graine>
use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

fn main() {
    let seed: u64 = std::env::args().nth(1).map_or(2, |s| s.parse().unwrap());
    let p = LineParams::preset("Orage").unwrap();
    let mut m = [Modem::new(Role::Call, ModemConfig::default()), Modem::new(Role::Answer, ModemConfig::default())];
    let mut lines = [LineSim::with_seed(p.clone(), seed), LineSim::with_seed(p.clone(), seed * 7919)];
    let mut echo = [EchoPath::new(p.clone()), EchoPath::new(p.clone())];
    m[0].command(ModemCommand::Dial);
    m[1].command(ModemCommand::Answer);
    let (mut rx, mut tx) = ([[0.0f32; 160]; 2], [[0.0f32; 160]; 2]);
    for blk in 0..(30 * 50) {
        for i in 0..2 {
            let r = rx[i];
            m[i].process(&r, &mut tx[i]);
        }
        let (t0, t1) = (tx[0], tx[1]);
        lines[0].process(&t0, &mut rx[1]);
        echo[1].process(&t1, &mut rx[1]);
        lines[1].process(&t1, &mut rx[0]);
        echo[0].process(&t0, &mut rx[0]);
        for i in 0..2 {
            for ev in m[i].drain_events() {
                match ev {
                    ModemEvent::Log { msg, .. } => println!("[{:6} ms] {} {msg}", blk * 20, if i == 0 { "APPELANT" } else { "APPELÉ  " }),
                    ModemEvent::Connected { tx_rate, rx_rate } => println!("[{:6} ms] {i} CONNECT {tx_rate}/{rx_rate}", blk * 20),
                    _ => {}
                }
            }
        }
    }
}
