//! Compatibilité de l'ordre du CRC des séquences de la poignée de main.
//!
//! Avant cette correction, nos modems émettaient le CRC des séquences INFO, MP
//! et V.90 poids faible en premier ; la norme (et tout vrai modem) le met
//! poids fort en premier. Un modem d'aujourd'hui (`CrcMode::Auto`) doit
//! s'entendre avec les deux : un modem d'avant (`Legacy`, une appli 0.1.6
//! ou un central pas encore mis à jour) et un vrai modem (`Standard`, un
//! U.S. Robotics derrière un adaptateur), dans les deux rôles, en V.34 et
//! en 56K.

use v34_core::{CrcMode, EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

struct Run {
    connected: [Option<(u32, u32)>; 2],
    got: [Vec<u8>; 2],
    log: Vec<String>,
}

fn run(p: LineParams, call_cfg: ModemConfig, ans_cfg: ModemConfig, seconds: f32, payload: [&[u8]; 2]) -> Run {
    let mut call = Modem::new(Role::Call, call_cfg);
    let mut ans = Modem::new(Role::Answer, ans_cfg);
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
    for blk in 0..(seconds * 8000.0 / B as f32) as usize {
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
                    ModemEvent::Disconnected { reason } => r.log.push(format!("[{:6} ms] {i} NO CARRIER {reason}", blk * 20)),
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

fn cfg(crc_mode: CrcMode) -> ModemConfig {
    ModemConfig { crc_mode, ..ModemConfig::default() }
}

/// Connexion et données dans les deux sens ; renvoie le journal.
fn check(label: &str, line: &str, call_cfg: ModemConfig, ans_cfg: ModemConfig) -> (Run, String) {
    let a: Vec<u8> = "Bonjour depuis l'appelant ! 0123456789 ".repeat(10).into_bytes();
    let b: Vec<u8> = "Salut, ici l'appelé. Le modem marche. ".repeat(10).into_bytes();
    let r = run(LineParams::preset(line).unwrap(), call_cfg, ans_cfg, 30.0, [&a, &b]);
    let dump = r.log.join("\n");
    assert!(r.connected[0].is_some() && r.connected[1].is_some(), "{label} : pas de CONNECT\n{dump}");
    assert_eq!(r.got[1], a, "{label} : appelant → appelé\n{dump}");
    assert_eq!(r.got[0], b, "{label} : appelé → appelant\n{dump}");
    eprintln!("{label} : CONNECT {:?} / {:?}, données OK", r.connected[0], r.connected[1]);
    (r, dump)
}

const SUIT_LA_NORME: &str = "suit la norme";

#[test]
fn v34_avec_un_modem_d_avant_dans_les_deux_roles() {
    for (label, c, a) in [("nouveau → ancien", CrcMode::Auto, CrcMode::Legacy), ("ancien → nouveau", CrcMode::Legacy, CrcMode::Auto)] {
        let (_, dump) = check(label, "Urbaine", cfg(c), cfg(a));
        assert!(!dump.contains(SUIT_LA_NORME), "{label} : rien ne doit changer face à un modem d'avant\n{dump}");
    }
}

#[test]
fn v34_avec_un_vrai_modem_dans_les_deux_roles() {
    for (label, c, a) in [("nouveau → vrai modem", CrcMode::Auto, CrcMode::Standard), ("vrai modem → nouveau", CrcMode::Standard, CrcMode::Auto)] {
        let (_, dump) = check(label, "Urbaine", cfg(c), cfg(a));
        assert!(dump.contains(SUIT_LA_NORME), "{label} : on doit adopter l'ordre de la norme\n{dump}");
    }
}

#[test]
fn v34_entre_modems_d_aujourd_hui_et_entre_vrais_modems() {
    check("nouveau → nouveau", "Urbaine", cfg(CrcMode::Auto), cfg(CrcMode::Auto));
    check("norme → norme", "Urbaine", cfg(CrcMode::Standard), cfg(CrcMode::Standard));
}

fn fai(crc_mode: CrcMode) -> ModemConfig {
    ModemConfig { digital: true, crc_mode, ..ModemConfig::default() }
}

fn check_56k(label: &str, abonne: CrcMode, central: CrcMode) -> String {
    let (r, dump) = check(label, "Parfaite", cfg(abonne), fai(central));
    let (_, down) = r.connected[0].unwrap();
    assert!(down > 33600, "{label} : pas de 56k ({down} bit/s)\n{dump}");
    dump
}

#[test]
fn v90_appli_d_avant_et_central_d_aujourd_hui() {
    let dump = check_56k("56k : abonné d'avant → central nouveau", CrcMode::Legacy, CrcMode::Auto);
    assert!(!dump.contains(SUIT_LA_NORME), "{dump}");
}

#[test]
fn v90_appli_d_aujourd_hui_et_central_d_avant() {
    let dump = check_56k("56k : abonné nouveau → central d'avant", CrcMode::Auto, CrcMode::Legacy);
    assert!(!dump.contains(SUIT_LA_NORME), "{dump}");
}

#[test]
fn v90_vrai_modem_56k_et_central_d_aujourd_hui() {
    let dump = check_56k("56k : vrai modem → central nouveau", CrcMode::Standard, CrcMode::Auto);
    assert!(dump.contains(SUIT_LA_NORME), "{dump}");
}

/// Témoin : un vrai modem et un modem d'avant ne s'entendent pas (c'était
/// le cas avant cette correction) ; sinon les essais ci-dessus ne prouveraient rien.
#[test]
fn temoin_vrai_modem_et_modem_d_avant_ne_s_entendent_pas() {
    let r = run(LineParams::preset("Urbaine").unwrap(), cfg(CrcMode::Standard), cfg(CrcMode::Legacy), 20.0, [b"x", b"y"]);
    assert!(r.connected[0].is_none() && r.connected[1].is_none(), "les deux ordres stricts ne devraient pas se comprendre");
    assert!(r.log.iter().any(|l| l.contains("INFO0")), "échec attendu à l'échange des INFO0");
}
