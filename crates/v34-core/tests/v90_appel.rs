//! Appels complets avec le 56k : l'abonné (appelant, modem analogique) et le
//! FAI (appelé, raccordé en numérique). Même banc que `connect.rs`.

use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

struct Run {
    connected: [Option<(u32, u32)>; 2],
    got: [Vec<u8>; 2],
    log: Vec<String>,
}

fn fai() -> ModemConfig {
    ModemConfig { digital: true, ..ModemConfig::default() }
}

fn run_with(p: LineParams, abonne: ModemConfig, fai: ModemConfig, seconds: f32, payload: [&[u8]; 2]) -> Run {
    let mut call = Modem::new(Role::Call, abonne);
    let mut ans = Modem::new(Role::Answer, fai);
    // le sens montant passe par la boucle locale de l'abonné comme le
    // descendant (le FAI, lui, n'a pas de boucle analogique)
    let mut c2a = LineSim::new(p.clone());
    let mut a2c = LineSim::new(p.clone());
    let mut echo_c = EchoPath::new(p);
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
        a2c.process(&ans_tx, &mut call_rx);
        echo_c.process(&call_tx, &mut call_rx);
        for (i, m) in [&mut call, &mut ans].into_iter().enumerate() {
            for ev in m.drain_events() {
                match ev {
                    ModemEvent::Connected { tx_rate, rx_rate } => r.connected[i] = Some((tx_rate, rx_rate)),
                    ModemEvent::Data(d) => r.got[i].extend(d),
                    ModemEvent::Log { msg, .. } => r.log.push(format!("[{:6} ms] {} {msg}", blk * 20, if i == 0 { "ABONNÉ" } else { "FAI   " })),
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

fn echange(p: LineParams, abonne: ModemConfig, fai: ModemConfig) -> Run {
    let a: Vec<u8> = "Requête de l'abonné vers le FAI. ".repeat(20).into_bytes();
    let b: Vec<u8> = "Réponse du FAI, page web de 1998. ".repeat(40).into_bytes();
    let r = run_with(p, abonne, fai, 30.0, [&a, &b]);
    let dump = r.log.join("\n");
    assert!(r.connected[0].is_some() && r.connected[1].is_some(), "pas de CONNECT\n{dump}");
    assert_eq!(String::from_utf8_lossy(&r.got[1]), String::from_utf8_lossy(&a), "abonné → FAI\n{dump}");
    assert_eq!(String::from_utf8_lossy(&r.got[0]), String::from_utf8_lossy(&b), "FAI → abonné\n{dump}");
    r
}

/// Le 56k est négocié en V.8 ; la phase 2 se fait rôles inversés (l'abonné
/// tient le rôle de l'appelé du V.34), INFO0d compris.
#[test]
fn v8_annonce_le_56k_et_phase2_inversee() {
    let r = echange(LineParams::preset("Urbaine").unwrap(), ModemConfig::default(), fai());
    let dump = r.log.join("\n");
    assert!(dump.contains("nous serons le modem analogique"), "{dump}");
    assert!(dump.contains("nous serons le modem numérique"), "{dump}");
    eprintln!("CONNECT abonné {:?} / FAI {:?}", r.connected[0], r.connected[1]);
}

/// Sans 56k d'un côté ou de l'autre : V.34 classique, rôles normaux.
#[test]
fn sans_56k_v34_classique() {
    for (abonne, f) in [(ModemConfig { v90: false, ..ModemConfig::default() }, fai()), (ModemConfig::default(), ModemConfig::default())] {
        let r = echange(LineParams::preset("Urbaine").unwrap(), abonne, f);
        assert!(!r.log.join("\n").contains("modem analogique"));
    }
}
