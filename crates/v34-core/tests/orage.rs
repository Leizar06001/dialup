//! Ligne « Orage » (SNR 20 dB, 3 clics/s, coupures) : grâce aux reprises
//! (répétition des INFO0, réentraînements), la connexion doit aboutir dans
//! la grande majorité des cas. Sur une ligne aussi bruitée, l'issue d'une
//! graine dépend de chaque milliseconde du dialogue : 24 graines, pour que
//! le test mesure la robustesse plutôt que la chance d'une poignée d'entre
//! elles.
use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

#[test]
fn orage_connects_most_of_the_time() {
    let p = LineParams::preset("Orage").unwrap();
    let mut summary = Vec::new();
    for seed in 1..=24u64 {
        let mut m = [Modem::new(Role::Call, ModemConfig::default()), Modem::new(Role::Answer, ModemConfig::default())];
        let mut lines = [LineSim::with_seed(p.clone(), seed), LineSim::with_seed(p.clone(), seed * 7919)];
        let mut echo = [EchoPath::new(p.clone()), EchoPath::new(p.clone())];
        m[0].command(ModemCommand::Dial);
        m[1].command(ModemCommand::Answer);
        let (mut rx, mut tx) = ([[0.0f32; 160]; 2], [[0.0f32; 160]; 2]);
        let mut outcome = String::from("rien");
        let mut connected = [false; 2];
        'run: for blk in 0..(30 * 50) {
            for i in 0..2 { let r = rx[i]; m[i].process(&r, &mut tx[i]); }
            let (t0, t1) = (tx[0], tx[1]);
            lines[0].process(&t0, &mut rx[1]); echo[1].process(&t1, &mut rx[1]);
            lines[1].process(&t1, &mut rx[0]); echo[0].process(&t0, &mut rx[0]);
            for i in 0..2 {
                for ev in m[i].drain_events() {
                    match ev {
                        ModemEvent::Connected { tx_rate, .. } => { connected[i] = true; if connected[0] && connected[1] { outcome = format!("CONNECT {tx_rate} à {} s", blk / 50); break 'run; } }
                        ModemEvent::Log { msg, .. } if msg.contains("Échec") || msg.contains("Reprise") || msg.contains("répétition") => { outcome = format!("{} [{}] {msg}", if i == 0 { "APPELANT" } else { "APPELÉ" }, blk / 50); if msg.contains("Échec") { break 'run; } }
                        _ => {}
                    }
                }
            }
        }
        summary.push(format!("graine {seed:2} : {outcome}"));
    }
    eprintln!("{}", summary.join("\n"));
    let ok = summary.iter().filter(|l| l.contains("CONNECT")).count();
    // ~80 % mesurés sur 80 graines ; 70 % exigés, pour laisser la marge du
    // hasard sur 24
    assert!(ok >= 17, "seulement {ok}/24 connexions sur la ligne Orage\n{}", summary.join("\n"));
}
