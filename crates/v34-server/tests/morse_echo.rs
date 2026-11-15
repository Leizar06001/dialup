//! La ligne de test parle morse : un abonné seul (DIP 4 sur ON) appelle le
//! 08 00 00 00 00 à travers le central, en UDP et en loi A ; la ligne de test
//! accepte le morse proposé en V.8, dit bonjour, et renvoie ce qu'elle lit.
//!
//! Le test tourne en temps réel (~30 s), d'où `#[ignore]` :
//!     cargo test -p v34-server --test morse_echo -- --ignored --nocapture

use std::time::{Duration, Instant};

use v34_core::morse::MorseEvenement;
use v34_core::{Modem, ModemCommand, ModemConfig, ModemEvent, Phase, Role};
use v34_net::auth::PasswordHash;
use v34_net::central::{CallState, CentralClient, CentralConfig, CentralEvent};
use v34_net::{Pacer, FRAME_SAMPLES};
use v34_server::log::{set_level, Level};
use v34_server::{Config, Server, ServerHandle};

const USER: (&str, &str) = ("morse", "mot-de-passe-morse");
const ECHO: &str = "0800000000";
const MESSAGE: &str = "<KA>SOS DE F5TINY<AR>";

fn start_server() -> ServerHandle {
    set_level(Level::Error);
    let h = PasswordHash::new(USER.1, 1000);
    let toml = format!(
        r#"
        [serveur]
        ecoute = "127.0.0.1:0"
        journal = "error"
        [modems]
        delai_decroche_ms = 100
        [[abonnes]]
        nom = "{}"
        numero = "0700000000"
        mot_de_passe_hash = "{h}"
        "#,
        USER.0
    );
    Server::bind(Config::from_toml(&toml).expect("configuration de test")).unwrap().spawn().unwrap()
}

#[test]
#[ignore = "temps réel (~30 s)"]
fn la_ligne_de_test_repond_en_morse() {
    let server = start_server();
    let mut c = CentralClient::connect(CentralConfig::new(server.local_addr().to_string(), USER.0, USER.1)).unwrap();
    let mut modem = Modem::new(Role::Call, ModemConfig { morse: true, ..ModemConfig::default() });
    let mut pacer = Pacer::new();
    let (mut rx, mut tx) = (vec![0.0f32; FRAME_SAMPLES], vec![0.0f32; FRAME_SAMPLES]);
    let (mut dialed, mut connecte, mut lu, mut journal) = (false, None, String::new(), Vec::new());
    let t0 = Instant::now();
    let mut next = Instant::now();
    let mut due = 0.0;
    while t0.elapsed() < Duration::from_secs(60) {
        let _ = c.poll();
        while let Some(ev) = c.next_event() {
            match ev {
                CentralEvent::Registered { .. } if !dialed => {
                    dialed = true;
                    c.dial(ECHO).unwrap();
                }
                CentralEvent::Answered { .. } => modem.command(ModemCommand::Dial),
                CentralEvent::HungUp { reason, .. } => journal.push(format!("raccroché : {reason:?}")),
                _ => {}
            }
        }
        let active = matches!(c.call_state(), CallState::Active { .. });
        due += 1.0 + pacer.update(c.jitter_level());
        while due >= 1.0 {
            if active {
                c.recv_block(&mut rx);
            } else {
                rx.fill(0.0);
            }
            modem.process(&rx, &mut tx);
            if active {
                let _ = c.send_block(&tx);
            }
            due -= 1.0;
        }
        for ev in modem.drain_events() {
            match ev {
                ModemEvent::Connected { tx_rate, .. } => {
                    connecte = Some((tx_rate, modem.status().phase, t0.elapsed().as_secs_f32()));
                    // en duplex : on parle pendant qu'elle dit bonjour
                    modem.command(ModemCommand::Send(MESSAGE.as_bytes().to_vec()));
                }
                ModemEvent::Morse(MorseEvenement::Recu { texte, .. }) => lu.push_str(&texte),
                ModemEvent::Log { msg, .. } => journal.push(msg),
                ModemEvent::Disconnected { reason } => journal.push(format!("NO CARRIER {reason}")),
                _ => {}
            }
        }
        if lu.contains(MESSAGE) {
            break;
        }
        next += Duration::from_millis(20);
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
    }
    let _ = c.hangup();
    eprintln!("CONNECT : {connecte:?}\nlu : {lu}\nen {:.1} s", t0.elapsed().as_secs_f32());
    let (debit, phase, _) = connecte.unwrap_or_else(|| panic!("pas de CONNECT\n{journal:#?}"));
    assert_eq!((debit, phase), (16, Phase::Morse), "{journal:#?}");
    assert!(journal.iter().any(|l| l.contains("le correspondant accepte le morse")), "{journal:#?}");
    // l'accueil, puis notre message qui revient
    assert!(lu.starts_with(&String::from_utf8_lossy(v34_server::services::ECHO_MORSE).into_owned()), "{lu}");
    assert!(lu.ends_with(MESSAGE), "{lu}");
}
