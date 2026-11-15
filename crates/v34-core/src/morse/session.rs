//! Une liaison morse : notre manipulateur et notre écoute, en duplex, et
//! leur branchement sur le modem (après la négociation V.8).

use super::code::joli;
use super::ecoute::Ecoute;
use super::manipulateur::Manipulateur;
use super::{debit_equivalent, Cadence, MorseEvenement, MorseVue, F_APPELANT, F_APPELE, MPM, MPM_MAX, MPM_MIN};
use crate::api::*;
use crate::modem::{Modem, MS};
use crate::tx::TxMode;
use crate::v8::{Menu, NonNormalise};

/// Délai laissé pour émettre SK avant de raccrocher quoi qu'il arrive (4 s).
const DELAI_SK: u64 = 4000 * MS;
/// SK reçu : on raccroche un peu après (le temps que l'autre finisse).
const APRES_SK: u64 = 300 * MS;

pub struct Morse {
    pub mpm: u8,
    f_tx: f64,
    f_rx: f64,
    manip: Manipulateur,
    ecoute: Ecoute,
    n: u64,
    /// Raccrochage demandé : SK part, puis on raccroche (au plus tard à l'échéance)
    fin: Option<u64>,
    /// SK reçu : l'autre a fini, on raccroche à l'échéance
    sk_recu: Option<u64>,
    evenements: Vec<MorseEvenement>,
    /// Dernier signe reconnu (texte, code), pour l'afficheur
    dernier: (String, String),
}

/// Fréquences (émission, écoute) d'un rôle.
pub fn frequences(role: Role) -> (f64, f64) {
    match role {
        Role::Call => (F_APPELANT, F_APPELE),
        Role::Answer => (F_APPELE, F_APPELANT),
    }
}

impl Morse {
    pub fn new(role: Role, mpm: u8) -> Self {
        let mpm = mpm.clamp(MPM_MIN, MPM_MAX);
        let cadence = Cadence::paris(mpm);
        let (f_tx, f_rx) = frequences(role);
        Self {
            mpm,
            f_tx,
            f_rx,
            manip: Manipulateur::new(f_tx, cadence).avec_blanc(super::manipulateur::BLANC_INITIAL),
            // le bruit s'écoute à mi-chemin des deux tons
            ecoute: Ecoute::new(f_rx, (f_tx + f_rx) / 2.0, cadence),
            n: 0,
            fin: None,
            sk_recu: None,
            evenements: Vec::new(),
            dernier: (String::new(), String::new()),
        }
    }

    /// Du texte à émettre (UTF-8 ; procédures entre chevrons : `<KA>`, `<AR>`…).
    pub fn envoyer(&mut self, octets: &[u8]) {
        if self.fin.is_none() {
            self.manip.envoyer(octets);
        }
    }

    /// On raccroche : ce qui attend est oublié, SK part. Renvoie vrai si
    /// c'était déjà demandé (on raccroche alors sans attendre).
    pub fn raccrocher(&mut self) -> bool {
        if self.fin.is_some() {
            return true;
        }
        self.manip.couper();
        self.manip.envoyer(b"<SK>");
        self.fin = Some(self.n + DELAI_SK);
        false
    }

    pub fn process(&mut self, rx: &[f32], tx: &mut [f32]) {
        for (r, t) in rx.iter().zip(tx.iter_mut()) {
            *t = self.manip.echantillon();
            self.ecoute.pousser(*r);
            self.n += 1;
        }
        for s in self.manip.partis() {
            self.evenements.push(MorseEvenement::Emis { texte: s.texte.to_string(), code: s.code.to_string() });
        }
        for (texte, code) in self.ecoute.prendre() {
            if texte == "<SK>" && self.sk_recu.is_none() {
                self.sk_recu = Some(self.n + APRES_SK);
            }
            if !code.is_empty() {
                self.dernier = (texte.clone(), code.clone());
            }
            self.evenements.push(MorseEvenement::Recu { texte, code });
        }
    }

    pub fn evenements(&mut self) -> Vec<MorseEvenement> {
        std::mem::take(&mut self.evenements)
    }

    /// Faut-il raccrocher ? (et pourquoi)
    pub fn fini(&self) -> Option<&'static str> {
        if let Some(t) = self.fin {
            if self.manip.oisif() || self.n >= t {
                return Some("raccroché (SK envoyé)");
            }
        }
        match self.sk_recu {
            Some(t) if self.n >= t => Some("fin de contact (SK reçu)"),
            _ => None,
        }
    }

    pub fn vue(&self) -> MorseVue {
        let (tx_texte, tx_code, tx_fait) = match self.manip.courant() {
            Some((s, fait)) => (s.texte.to_string(), s.code.to_string(), fait),
            None => (String::new(), String::new(), 0),
        };
        MorseVue {
            mpm: self.mpm,
            f_tx: self.f_tx as f32,
            f_rx: self.f_rx as f32,
            tx_cle: self.manip.cle(),
            tx_texte,
            tx_code,
            tx_fait,
            tx_attente: self.manip.attente() as u32,
            rx_cle: self.ecoute.ton(),
            rx_code: self.ecoute.code().to_string(),
            rx_texte: self.dernier.0.clone(),
            rx_dernier_code: self.dernier.1.clone(),
            snr_db: self.ecoute.snr_db(),
        }
    }
}

/// Vitesse convenue : celle de l'appelant, ramenée à ce qu'on sait faire.
fn vitesse(proposee: u8) -> u8 {
    proposee.clamp(MPM_MIN, MPM.max(MPM_MIN))
}

impl Modem {
    /// Appelé, CM reçu : le morse est-il proposé, et l'accepte-t-on (DIP 4) ?
    /// Renvoie les services non normalisés à mettre dans le JM.
    pub(crate) fn morse_pour_jm(&mut self, cm: &Menu) -> Vec<NonNormalise> {
        for ns in cm.nsf.iter().filter(|n| n.mpm_morse().is_none()) {
            let f = String::from_utf8_lossy(&ns.fournisseur).into_owned();
            self.log(LogLevel::Detail, &format!("CM : service non normalisé inconnu (pays T.35 {:02X?}, fournisseur « {f} ») — ignoré", ns.pays));
        }
        let Some(proposee) = cm.morse() else { return Vec::new() };
        if !self.cfg.morse {
            self.log(LogLevel::Info, "CM : l'appelant propose le morse (service non normalisé TINYVERSE) — refusé (DIP 4 sur OFF), on continue en V.34");
            return Vec::new();
        }
        let mpm = vitesse(proposee);
        self.morse_convenu = Some(mpm);
        self.v90 = false;
        self.log(LogLevel::Info, &format!("CM : l'appelant propose le morse (service non normalisé TINYVERSE, France) — accepté, {mpm} mots/minute"));
        self.status.params.v8_modes.insert(0, "Morse".into());
        vec![NonNormalise::morse(mpm)]
    }

    /// Appelant, JM reçu : l'appelé a-t-il accepté notre morse ?
    pub(crate) fn morse_dans_jm(&mut self, jm: &Menu) {
        if !self.cfg.morse {
            return;
        }
        match jm.morse() {
            Some(mpm) => {
                let mpm = vitesse(mpm);
                self.morse_convenu = Some(mpm);
                self.v90 = false;
                self.log(LogLevel::Info, &format!("JM : le correspondant accepte le morse (TINYVERSE) — {mpm} mots/minute"));
                self.status.params.v8_modes.retain(|m| m != "V.90");
                self.status.params.v8_modes.insert(0, "Morse".into());
            }
            None => self.log(LogLevel::Info, "JM sans service non normalisé : pas de morse en face, on continue en V.34"),
        }
    }

    /// Fin de V.8 (CJ et 75 ms de silence) : la liaison morse commence.
    pub(crate) fn demarrer_morse(&mut self, mpm: u8) {
        // plus aucun détecteur de la négociation
        self.fsk_rx = None;
        self.ansam_det = None;
        self.info_rx = None;
        self.tone_rx = None;
        self.tone_presence = None;
        self.probe = None;
        self.cng = None;
        self.set_tx(TxMode::Silence, "silence");
        let m = Morse::new(self.role, mpm);
        let debit = debit_equivalent(m.mpm);
        self.set_phase(Phase::Morse);
        self.status.leds.cd = true;
        self.status.params.data_rate_tx = Some(debit);
        self.status.params.data_rate_rx = Some(debit);
        self.params_changed();
        let p = Cadence::paris(m.mpm).point_ms();
        self.log(LogLevel::Info, &format!("CONNECT {} MPM MORSE — émission à {:.0} Hz, écoute à {:.0} Hz, point de {p:.0} ms (cadence PARIS)", m.mpm, m.f_tx, m.f_rx));
        self.status.morse = Some(m.vue());
        self.morse = Some(Box::new(m));
        self.events.push(ModemEvent::Connected { tx_rate: debit, rx_rate: debit });
    }

    /// Commandes en morse : le texte part au manipulateur ; raccrocher
    /// envoie SK d'abord. Renvoie la commande si elle n'est pas pour le morse.
    pub(crate) fn commande_morse(&mut self, cmd: ModemCommand) -> Option<ModemCommand> {
        let Some(m) = self.morse.as_mut() else { return Some(cmd) };
        match cmd {
            ModemCommand::Send(octets) => {
                self.status.bytes_tx += octets.len() as u64;
                m.envoyer(&octets);
                None
            }
            ModemCommand::Hangup => {
                if m.raccrocher() {
                    self.hangup("raccroché");
                } else {
                    self.log(LogLevel::Info, "On raccroche : SK (fin de contact) part d'abord");
                }
                None
            }
            // ni réentraînement ni renégociation : c'est du morse
            ModemCommand::Retrain | ModemCommand::Renegotiate => None,
            autre => Some(autre),
        }
    }

    /// Un bloc audio en morse.
    pub(crate) fn process_morse(&mut self, rx: &[f32], tx: &mut [f32]) {
        let Some(m) = self.morse.as_mut() else { return };
        m.process(rx, tx);
        self.n += rx.len() as u64;
        let evs = m.evenements();
        let vue = m.vue();
        let fini = m.fini();
        // SD : touche baissée ; RD : le ton d'en face ; CD : la liaison
        self.status.leds.sd = vue.tx_cle;
        self.status.leds.rd = vue.rx_cle;
        self.status.leds.cd = true;
        // Afficheur TX/RX et karaoké : ce qui part, ce qui arrive
        self.status.tx_signal = if vue.tx_texte.is_empty() { "silence".into() } else { format!("Morse {} {}", vue.tx_texte, joli(&vue.tx_code)) };
        self.status.rx_signal = if vue.rx_cle || !vue.rx_code.is_empty() { format!("Morse {}", joli(&vue.rx_code)) } else { "silence".into() };
        for e in evs {
            let donnees = match &e {
                // le texte reçu est aussi un flux d'octets (ligne de test, terminal)
                MorseEvenement::Recu { texte, .. } if texte != "<SK>" => Some(texte.clone().into_bytes()),
                _ => None,
            };
            self.events.push(ModemEvent::Morse(e));
            if let Some(d) = donnees {
                self.status.bytes_rx += d.len() as u64;
                self.events.push(ModemEvent::Data(d));
            }
        }
        self.status.morse = Some(vue);
        if let Some(raison) = fini {
            self.hangup(raison);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deux sessions face à face, sans ligne : tout passe, dans les deux sens à la fois.
    #[test]
    fn duplex_sans_ligne() {
        let mut a = Morse::new(Role::Call, 20);
        let mut b = Morse::new(Role::Answer, 20);
        a.envoyer("<KA>BONJOUR<AR>".as_bytes());
        b.envoyer("<KA>SALUT À TOI<AR>".as_bytes());
        let (mut ta, mut tb) = ([0.0f32; 160], [0.0f32; 160]);
        let (mut ra, mut rb) = ([0.0f32; 160], [0.0f32; 160]);
        let (mut lu_a, mut lu_b) = (String::new(), String::new());
        for _ in 0..(20 * 50) {
            a.process(&ra, &mut ta);
            b.process(&rb, &mut tb);
            // chacun entend l'autre… et son propre écho, à peine plus faible
            for i in 0..160 {
                ra[i] = tb[i] + 0.8 * ta[i];
                rb[i] = ta[i] + 0.8 * tb[i];
            }
            for (lu, s) in [(&mut lu_a, &mut a), (&mut lu_b, &mut b)] {
                for e in s.evenements() {
                    if let MorseEvenement::Recu { texte, .. } = e {
                        lu.push_str(&texte);
                    }
                }
            }
        }
        assert_eq!(lu_b, "<KA>BONJOUR<AR>");
        assert_eq!(lu_a, "<KA>SALUT À TOI<AR>");
        assert_eq!(a.vue().f_tx, 800.0);
        assert_eq!(b.vue().f_tx, 1600.0);
    }

    #[test]
    fn raccrocher_envoie_sk() {
        let mut a = Morse::new(Role::Call, 20);
        let mut b = Morse::new(Role::Answer, 20);
        a.envoyer(b"UN TRES LONG MESSAGE QUI NE PARTIRA PAS EN ENTIER");
        let (mut ta, mut tb, mut r) = ([0.0f32; 160], [0.0f32; 160], [0.0f32; 160]);
        a.process(&r, &mut ta);
        assert!(!a.raccrocher(), "première demande : SK d'abord");
        let mut fin_a = None;
        for k in 0..(10 * 50) {
            a.process(&r, &mut ta);
            b.process(&ta, &mut tb);
            r.fill(0.0);
            if fin_a.is_none() {
                fin_a = a.fini().map(|r| (k, r));
            }
            if b.fini().is_some() {
                break;
            }
        }
        let (k, raison) = fin_a.expect("A raccroche après SK");
        assert!(k < 3 * 50, "SK en moins de 3 s ({k} blocs)");
        assert_eq!(raison, "raccroché (SK envoyé)");
        assert_eq!(b.fini(), Some("fin de contact (SK reçu)"));
        assert!(a.raccrocher(), "deuxième demande : tout de suite");
    }
}
