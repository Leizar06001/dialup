//! Renégociation, réentraînement, raccrochage propre et reprise après un
//! épisode de bruit très fort.

use v34_core::{EchoPath, LineParams, LineSim, Modem, ModemCommand, ModemConfig, ModemEvent, Role};

const B: usize = 160;

struct Sim {
    m: [Modem; 2],
    lines: [LineSim; 2],
    echo: [EchoPath; 2],
    rx: [[f32; B]; 2],
    tx: [[f32; B]; 2],
    t_ms: u64,
    connects: [u32; 2],
    got: [Vec<u8>; 2],
    log: Vec<String>,
    disconnected: [Option<String>; 2],
}

impl Sim {
    fn new(preset: &str) -> Self {
        let p = LineParams::preset(preset).unwrap();
        let mut s = Sim {
            m: [Modem::new(Role::Call, ModemConfig::default()), Modem::new(Role::Answer, ModemConfig::default())],
            lines: [LineSim::new(p.clone()), LineSim::new(p.clone())],
            echo: [EchoPath::new(p.clone()), EchoPath::new(p)],
            rx: [[0.0; B]; 2],
            tx: [[0.0; B]; 2],
            t_ms: 0,
            connects: [0, 0],
            got: [Vec::new(), Vec::new()],
            log: Vec::new(),
            disconnected: [None, None],
        };
        s.m[0].command(ModemCommand::Dial);
        s.m[1].command(ModemCommand::Answer);
        s
    }

    fn set_line(&mut self, p: LineParams) {
        for l in &mut self.lines {
            l.set_params(p.clone());
        }
    }

    fn run_ms(&mut self, ms: u64) {
        for _ in 0..ms / 20 {
            for i in 0..2 {
                let (rx, tx) = (self.rx[i], &mut self.tx[i]);
                self.m[i].process(&rx, tx);
            }
            // ligne 0 : appelant → appelé ; ligne 1 : appelé → appelant
            let (t0, t1) = (self.tx[0], self.tx[1]);
            self.lines[0].process(&t0, &mut self.rx[1]);
            self.echo[1].process(&t1, &mut self.rx[1]);
            self.lines[1].process(&t1, &mut self.rx[0]);
            self.echo[0].process(&t0, &mut self.rx[0]);
            self.t_ms += 20;
            for i in 0..2 {
                for ev in self.m[i].drain_events() {
                    match ev {
                        ModemEvent::Connected { .. } => self.connects[i] += 1,
                        ModemEvent::Data(d) => self.got[i].extend(d),
                        ModemEvent::Log { msg, .. } => self.log.push(format!("[{:6}] {} {msg}", self.t_ms, if i == 0 { "APPELANT" } else { "APPELÉ  " })),
                        ModemEvent::Disconnected { reason } => self.disconnected[i] = Some(reason),
                        _ => {}
                    }
                }
            }
        }
    }

    /// Attend la connexion des deux côtés.
    fn connect(&mut self) {
        for _ in 0..200 {
            if self.connects[0] > 0 && self.connects[1] > 0 {
                return;
            }
            self.run_ms(100);
        }
        panic!("pas de CONNECT\n{}", self.log.join("\n"));
    }

    /// Envoie un message dans les deux sens et vérifie qu'il arrive.
    fn exchange(&mut self, tag: &str, max_ms: u64) {
        let a = format!("appelant:{tag};").into_bytes();
        let b = format!("appelé:{tag};").into_bytes();
        self.got = [Vec::new(), Vec::new()];
        self.m[0].command(ModemCommand::Send(a.clone()));
        self.m[1].command(ModemCommand::Send(b.clone()));
        let mut t = 0;
        while t < max_ms {
            self.run_ms(100);
            t += 100;
            if self.got[1].ends_with(&a) && self.got[0].ends_with(&b) {
                return;
            }
        }
        panic!(
            "« {tag} » non reçu (reçu par l'appelé : {:?}, par l'appelant : {:?})\n{}",
            String::from_utf8_lossy(&self.got[1]),
            String::from_utf8_lossy(&self.got[0]),
            self.log.join("\n")
        );
    }

    fn logged(&self, needle: &str) -> bool {
        self.log.iter().any(|l| l.contains(needle))
    }
}

#[test]
fn renegotiation_on_demand() {
    let mut s = Sim::new("Urbaine");
    s.connect();
    s.exchange("avant", 3000);
    s.m[0].command(ModemCommand::Renegotiate);
    s.run_ms(3000);
    assert!(s.logged("Renégociation terminée"), "{}", s.log.join("\n"));
    s.exchange("après", 5000);
}

#[test]
fn retrain_on_demand() {
    let mut s = Sim::new("Campagne");
    s.connect();
    s.exchange("avant", 3000);
    s.m[1].command(ModemCommand::Retrain);
    for _ in 0..100 {
        s.run_ms(100);
        if s.connects[0] >= 2 && s.connects[1] >= 2 {
            break;
        }
    }
    assert!(s.connects[0] >= 2 && s.connects[1] >= 2, "pas de nouvelle connexion après réentraînement\n{}", s.log.join("\n"));
    s.exchange("après", 6000);
}

#[test]
fn clean_hangup() {
    let mut s = Sim::new("Parfaite");
    s.connect();
    s.m[0].command(ModemCommand::Hangup);
    s.run_ms(2000);
    assert!(s.disconnected[0].is_some() && s.disconnected[1].is_some(), "{}", s.log.join("\n"));
    assert!(s.logged("Raccrochage propre terminé"), "{}", s.log.join("\n"));
}

#[test]
fn recovers_after_heavy_noise() {
    // Le scénario signalé : bruit très fort pendant la connexion, puis retour
    // à une ligne propre → les données doivent repasser.
    let mut s = Sim::new("Parfaite");
    s.connect();
    s.exchange("avant", 3000);
    let mut noisy = LineParams::preset("Parfaite").unwrap();
    noisy.snr_db = 12.0;
    s.set_line(noisy);
    s.run_ms(4000);
    s.set_line(LineParams::preset("Parfaite").unwrap());
    s.exchange("après le bruit", 20_000);
    assert!(s.disconnected.iter().all(|d| d.is_none()), "{}", s.log.join("\n"));
    for l in s.log.iter().filter(|l| ["erreurs", "Renégociation", "réentraîn", "Réentraîn", "améliorée", "CONNECT"].iter().any(|k| l.contains(k))) {
        eprintln!("{l}");
    }
}

#[test]
fn rate_goes_up_when_line_improves() {
    let mut s = Sim::new("Campagne");
    s.connect();
    s.exchange("ligne moyenne", 4000);
    s.set_line(LineParams::preset("Parfaite").unwrap());
    let before = s.connects;
    s.run_ms(16_000);
    assert!(s.logged("améliorée"), "pas de renégociation vers le haut\n{}", s.log.join("\n"));
    assert!(s.connects[0] > before[0], "{}", s.log.join("\n"));
    s.exchange("ligne propre", 5000);
    for l in s.log.iter().filter(|l| l.contains("améliorée") || l.contains("Renégociation terminée")) {
        eprintln!("{l}");
    }
}
