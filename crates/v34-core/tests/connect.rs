//! Test de bout en bout : deux modems reliés par une ligne simulée,
//! négociation complète puis échange de données dans les deux sens.

use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

struct Run {
    connected: [Option<(u32, u32)>; 2],
    got: [Vec<u8>; 2],
    log: Vec<String>,
}

fn run(preset: &str, seconds: f32, payload: [&[u8]; 2]) -> Run {
    run_with(LineParams::preset(preset).expect("préréglage"), seconds, payload)
}

fn run_with(p: LineParams, seconds: f32, payload: [&[u8]; 2]) -> Run {
    let mut call = Modem::new(Role::Call, ModemConfig::default());
    let mut ans = Modem::new(Role::Answer, ModemConfig::default());
    let mut c2a = LineSim::new(p.clone());
    let mut a2c = LineSim::new(p.clone());
    let mut echo_c = EchoPath::new(p.clone());
    let mut echo_a = EchoPath::new(p);
    call.command(ModemCommand::Dial);
    ans.command(ModemCommand::Answer);
    const B: usize = 160;
    let (mut call_rx, mut ans_rx) = ([0.0f32; B], [0.0f32; B]);
    let (mut call_tx, mut ans_tx) = ([0.0f32; B], [0.0f32; B]);
    let mut r = Run { connected: [None, None], got: [Vec::new(), Vec::new()], log: Vec::new() };
    let mut sent = [false, false];
    let blocks = (seconds * 8000.0 / B as f32) as usize;
    for blk in 0..blocks {
        call.process(&call_rx, &mut call_tx);
        ans.process(&ans_rx, &mut ans_tx);
        c2a.process(&call_tx, &mut ans_rx);
        echo_a.process(&ans_tx, &mut ans_rx);
        a2c.process(&ans_tx, &mut call_rx);
        echo_c.process(&call_tx, &mut call_rx);
        for (i, m) in [&mut call, &mut ans].into_iter().enumerate() {
            for ev in m.drain_events() {
                match ev {
                    ModemEvent::Connected { tx_rate, rx_rate } => r.connected[i] = Some((tx_rate, rx_rate)),
                    ModemEvent::Data(d) => r.got[i].extend(d),
                    ModemEvent::Log { msg, .. } => r.log.push(format!("[{:6} ms] {} {msg}", blk * 20, if i == 0 { "APPELANT" } else { "APPELÉ  " })),
                    ModemEvent::Disconnected { reason } => r.log.push(format!("[{:6} ms] {} NO CARRIER {reason}", blk * 20, i)),
                    _ => {}
                }
            }
            if r.connected[i].is_some() && !sent[i] {
                sent[i] = true;
                m.command(ModemCommand::Send(payload[i].to_vec()));
            }
        }
        if r.got[0].len() >= payload[1].len() && r.got[1].len() >= payload[0].len() && sent[0] && sent[1] {
            break;
        }
    }
    r
}

fn check(preset: &str) {
    let a: Vec<u8> = "Bonjour depuis l'appelant ! 0123456789 ".repeat(20).into_bytes();
    let b: Vec<u8> = "Salut, ici l'appelé. Le modem marche. ".repeat(20).into_bytes();
    let r = run(preset, 25.0, [&a, &b]);
    let dump = r.log.join("\n");
    assert!(r.connected[0].is_some() && r.connected[1].is_some(), "{preset} : pas de CONNECT\n{dump}");
    assert_eq!(String::from_utf8_lossy(&r.got[1]), String::from_utf8_lossy(&a), "{preset} : appelant → appelé\n{dump}");
    assert_eq!(String::from_utf8_lossy(&r.got[0]), String::from_utf8_lossy(&b), "{preset} : appelé → appelant\n{dump}");
    eprintln!("{preset} : CONNECT {:?} / {:?} — données OK dans les deux sens", r.connected[0], r.connected[1]);
}

#[test]
fn data_over_perfect_line() {
    check("Parfaite");
}

#[test]
fn data_over_urban_line() {
    check("Urbaine");
}

#[test]
fn data_over_country_line() {
    check("Campagne");
}

#[test]
fn data_over_long_distance_line() {
    check("Longue distance");
}

/// Liaison par Internet : les tampons de gigue des deux côtés ajoutent
/// ~150 ms chacun, soit un aller-retour de l'ordre de 400 ms.
#[test]
fn data_over_long_network_delay() {
    let mut p = LineParams::preset("Parfaite").expect("préréglage");
    p.delay_ms = 200.0;
    let a: Vec<u8> = "Bonjour par Internet ! ".repeat(20).into_bytes();
    let b: Vec<u8> = "Reçu, 400 ms plus tard. ".repeat(20).into_bytes();
    let r = run_with(p, 30.0, [&a, &b]);
    let dump = r.log.join("\n");
    assert!(r.connected[0].is_some() && r.connected[1].is_some(), "pas de CONNECT\n{dump}");
    assert_eq!(r.got[1], a, "appelant → appelé\n{dump}");
    assert_eq!(r.got[0], b, "appelé → appelant\n{dump}");
    eprintln!("aller-retour 400 ms : CONNECT {:?} / {:?}", r.connected[0], r.connected[1]);
}
