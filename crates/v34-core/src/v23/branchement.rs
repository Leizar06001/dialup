//! Le Minitel branché sur le modem : décroché en serveur, reconnaissance du
//! sifflement par l'appelant, puis l'appel V.23 à la place de la liaison de
//! données (comme la télécopie, qui passe elle aussi à côté du V.34).

use crate::api::*;
use crate::modem::{Modem, St};

use super::session::{Cote, Evenement, Minitel};

impl Modem {
    /// Appelé, en serveur Minitel : silence, ANS pur, porteuse 1300 Hz.
    pub(crate) fn repondre_minitel(&mut self) {
        self.reset_link();
        self.role = Role::Answer;
        self.status.role = Role::Answer;
        self.status.leds.oh = true;
        self.set_phase(Phase::Minitel);
        self.minitel = Some(Box::new(Minitel::serveur(self.cfg.answer_silence_ms)));
    }

    /// Octets à envoyer, raccrochage.
    pub(crate) fn commande_minitel(&mut self, cmd: ModemCommand) {
        let Some(m) = self.minitel.as_mut() else { return };
        match cmd {
            ModemCommand::Send(b) => {
                self.status.bytes_tx += b.len() as u64;
                m.envoyer(&b);
            }
            // Le serveur laisse d'abord partir son dernier écran ; le Minitel
            // (CONNEXION/FIN, ATH), lui, coupe net, et un second raccrochage aussi.
            ModemCommand::Hangup if m.cote() == Cote::Serveur && !m.vidange() => m.raccrocher(),
            ModemCommand::Hangup => m.couper("raccroché"),
            _ => {}
        }
        if m.fini() {
            let raison = m.raison().unwrap_or("raccroché").to_string();
            self.fin_minitel(&raison);
        }
    }

    /// Appelant, en attente de l'ANSam : un ANS pur puis le sifflement à
    /// 1300 Hz ? C'est un serveur Minitel : on passe en V.23.
    pub(crate) fn ecouter_minitel(&mut self, r: f32) -> bool {
        let Some(e) = self.ecoute_v23.as_mut() else { return false };
        if !matches!(self.st, St::CallWaitAnsam | St::CallTe | St::CallCm) {
            self.ecoute_v23 = None;
            return false;
        }
        let trouve = e.push(r);
        let ans_ms = e.ans_ms();
        if !trouve {
            // 2100 Hz depuis longtemps, et toujours pas d'ANSam : c'est un ANS tout simple
            if ans_ms >= 800 && self.st == St::CallWaitAnsam && e.annoncer_ans() {
                self.rx_name("ANS");
                self.log(LogLevel::Info, "2100 Hz sans modulation : un ANS tout simple (V.25), pas l'ANSam du V.8…");
            }
            return false;
        }
        self.reset_link();
        self.role = Role::Call;
        self.status.role = Role::Call;
        self.status.leds.oh = true;
        self.log(LogLevel::Info, &format!("ANS pur ({:.1} s) puis porteuse 1300 Hz : c'est un serveur Minitel ! On passe en V.23 (1200/75)", ans_ms as f32 / 1000.0));
        self.set_phase(Phase::Minitel);
        self.rx_name("porteuse 1300 Hz");
        self.minitel = Some(Box::new(Minitel::terminal()));
        true
    }

    pub(crate) fn process_minitel(&mut self, rx: &[f32], tx: &mut [f32]) {
        let Some(m) = self.minitel.as_mut() else { return };
        m.process(rx, tx);
        self.n += rx.len() as u64;
        let evs = m.evenements();
        let (emis, recu) = (m.emission(), m.reception());
        let leds = (m.connecte(), m.emet(), m.recoit());
        let fini = m.fini().then(|| m.raison().unwrap_or("raccroché").to_string());
        let cote = m.cote();
        (self.status.leds.cd, self.status.leds.sd, self.status.leds.rd) = leds;
        for e in evs {
            match e {
                Evenement::Log(niveau, msg) => self.log(niveau, &msg),
                Evenement::Emet(nom) => self.tx_name(nom),
                Evenement::Entend(nom) => self.rx_name(nom),
                Evenement::Connecte { tx, rx } => {
                    let (f_tx, f_rx) = if cote == Cote::Serveur { (1300.0, 390.0) } else { (390.0, 1300.0) };
                    let p = &mut self.status.params;
                    p.v8_modes = vec!["V.23".into()];
                    (p.data_rate_tx, p.data_rate_rx) = (Some(tx), Some(rx));
                    (p.symbol_rate_tx, p.symbol_rate_rx) = (Some(tx), Some(rx));
                    (p.carrier_tx_hz, p.carrier_rx_hz) = (Some(f_tx), Some(f_rx));
                    self.params_changed();
                    self.events.push(ModemEvent::Connected { tx_rate: tx, rx_rate: rx });
                }
                Evenement::Donnees(d) => {
                    self.status.bytes_rx += d.len() as u64;
                    self.events.push(ModemEvent::Data(d));
                }
            }
        }
        // afficheur et karaoké : ce qui passe vraiment (sans événement à
        // chaque page, comme pour la télécopie)
        if self.status.tx_signal != emis {
            self.status.tx_signal = emis.to_string();
        }
        if self.status.rx_signal != recu {
            self.status.rx_signal = recu.to_string();
        }
        if let Some(raison) = fini {
            self.fin_minitel(&raison);
        }
    }

    /// Fin d'un appel Minitel : on raccroche, et on le dit (le lien réseau
    /// libère alors l'appel au central).
    fn fin_minitel(&mut self, raison: &str) {
        self.minitel = None;
        self.log(LogLevel::Info, &format!("Minitel : fin de la communication ({raison})"));
        self.hangup(raison);
        if !self.events.iter().any(|e| matches!(e, ModemEvent::Disconnected { .. })) {
            self.events.push(ModemEvent::Disconnected { reason: raison.to_string() });
        }
    }
}
