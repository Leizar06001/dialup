//! Un appel Minitel, d'un bout à l'autre, côté serveur (l'appelé, qui
//! répond par l'ANS puis siffle sa porteuse 1300 Hz) ou côté Minitel
//! (l'appelant, qui reconnaît ce sifflement et répond par sa porteuse
//! 390 Hz). Tout est en échantillons à 8 kHz : [`Minitel::process`] prend ce
//! qui arrive de la ligne et remplit ce qui part, comme le modem de données.

use crate::api::LogLevel;
use crate::dsp::tone::{PowerMeter, ToneDetector};

use super::fsk::{Ans, Emetteur, Recepteur, Recu, Voie};

const MS: u32 = 8;
/// ANS (V.25 : 3,3 ± 0,7 s ; on prend le bas de la fourchette)
const ANS_MS: u32 = 2600;
/// Silence entre l'ANS et la porteuse (V.25 : 75 ± 20 ms)
const PAUSE_MS: u32 = 75;
/// Porteuse de retour confirmée côté serveur
const RETOUR_MS: u32 = 150;
/// Le Minitel annonce CONNECT après avoir sifflé sa porteuse ce temps-là
const ACCROCHE_MS: u32 = 300;
/// Plus de porteuse depuis ce temps : NO CARRIER (les coupures plus brèves
/// d'une ligne d'orage sont pardonnées)
const PERTE_MS: u32 = 600;
/// Le serveur attend la porteuse de retour au plus…
const ATTENTE_RETOUR_MS: u32 = 15_000;
/// Raccrochage : les derniers caractères partent d'abord (au plus…)
const VIDANGE_MAX_MS: u32 = 20_000;
/// Voyant RD : allumé ce temps après un caractère reçu
const RD_MS: u32 = 120;

/// De quel côté de la ligne on est.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cote {
    /// Le serveur (Télétel) : émet à 1200 bit/s, reçoit à 75
    Serveur,
    /// Le Minitel : reçoit à 1200 bit/s, émet à 75
    Terminal,
}

/// Ce qui se passe, pour le modem.
#[derive(Clone, Debug, PartialEq)]
pub enum Evenement {
    Log(LogLevel, String),
    /// Signal émis (étapes de l'établissement seulement)
    Emet(&'static str),
    /// Signal reconnu en réception
    Entend(&'static str),
    Connecte { tx: u32, rx: u32 },
    Donnees(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Etat {
    // serveur
    Silence,
    Ans,
    Pause,
    Porteuse,
    // Minitel
    Accroche,
    // les deux
    Connecte,
    Vidange,
    Fini,
}

pub struct Minitel {
    cote: Cote,
    etat: Etat,
    /// Échantillons depuis l'entrée dans l'état
    n: u32,
    silence: u32,
    ans: Ans,
    tx: Emetteur,
    rx: Recepteur,
    evenements: Vec<Evenement>,
    /// Caractères reçus, livrés au plus tard à CONNECT
    recus: Vec<u8>,
    connecte: bool,
    fin: Option<String>,
    emis: &'static str,
    entendu: &'static str,
    /// Échantillons depuis le dernier caractère reçu
    depuis_recu: u32,
    /// Raccrochage : échantillons depuis que la file est vide
    marge: u32,
    pub erreurs_parite: u32,
    pub caracteres_recus: u64,
}

impl Minitel {
    /// Le serveur décroche : `silence_ms` de silence, puis l'ANS.
    pub fn serveur(silence_ms: u32) -> Minitel {
        let mut m = Minitel::new(Cote::Serveur, Etat::Silence);
        m.silence = silence_ms.max(200) * MS;
        m.log(LogLevel::Info, &format!("Décroché en serveur Minitel (V.23) : {} ms de silence, puis l'ANS", silence_ms.max(200)));
        m
    }

    /// Le Minitel vient de reconnaître le sifflement : il répond.
    pub fn terminal() -> Minitel {
        let mut m = Minitel::new(Cote::Terminal, Etat::Accroche);
        m.tx.porteuse = true;
        m.emis = "porteuse 390 Hz";
        m.evenements.push(Evenement::Emet("porteuse 390 Hz"));
        m.entendu = "porteuse 1300 Hz";
        m.log(LogLevel::Info, "Porteuse de retour : 390 Hz (V.23, voie à 75 bit/s)");
        m
    }

    fn new(cote: Cote, etat: Etat) -> Minitel {
        let (voie_tx, voie_rx) = match cote {
            Cote::Serveur => (Voie::Aller, Voie::Retour),
            Cote::Terminal => (Voie::Retour, Voie::Aller),
        };
        let on_ms = if cote == Cote::Serveur { 60 } else { 30 };
        Minitel {
            cote,
            etat,
            n: 0,
            silence: 0,
            ans: Ans::default(),
            tx: Emetteur::new(voie_tx),
            rx: Recepteur::new(voie_rx, on_ms, PERTE_MS),
            evenements: Vec::new(),
            recus: Vec::new(),
            connecte: false,
            fin: None,
            emis: "silence",
            entendu: "silence",
            depuis_recu: u32::MAX,
            marge: 0,
            erreurs_parite: 0,
            caracteres_recus: 0,
        }
    }

    pub fn cote(&self) -> Cote {
        self.cote
    }

    /// (débit émis, débit reçu) en bit/s
    pub fn debits(&self) -> (u32, u32) {
        match self.cote {
            Cote::Serveur => (1200, 75),
            Cote::Terminal => (75, 1200),
        }
    }

    pub fn evenements(&mut self) -> Vec<Evenement> {
        std::mem::take(&mut self.evenements)
    }

    /// Nom de ce qui part en ce moment (afficheur, karaoké).
    pub fn emission(&self) -> &'static str {
        self.emis
    }

    /// Nom de ce qui arrive en ce moment.
    pub fn reception(&self) -> &'static str {
        self.entendu
    }

    /// Raccrochage en cours (la file se vide).
    pub fn vidange(&self) -> bool {
        self.etat == Etat::Vidange
    }

    pub fn connecte(&self) -> bool {
        self.connecte
    }

    pub fn fini(&self) -> bool {
        self.etat == Etat::Fini
    }

    /// Raison de la fin (quand c'est fini).
    pub fn raison(&self) -> Option<&str> {
        self.fin.as_deref()
    }

    /// Un caractère part en ce moment (voyant SD).
    pub fn emet(&self) -> bool {
        self.tx.emet()
    }

    /// Un caractère vient d'arriver (voyant RD).
    pub fn recoit(&self) -> bool {
        self.depuis_recu < RD_MS * MS
    }

    /// Caractères qui attendent de partir.
    pub fn en_attente(&self) -> usize {
        self.tx.en_attente()
    }

    /// Caractères à envoyer (7 bits).
    pub fn envoyer(&mut self, octets: &[u8]) {
        if !matches!(self.etat, Etat::Fini) {
            self.tx.envoyer(octets);
        }
    }

    /// Raccrocher : ce qui est en file part d'abord, puis la porteuse se tait.
    pub fn raccrocher(&mut self) {
        match self.etat {
            Etat::Connecte | Etat::Accroche | Etat::Porteuse if !self.tx.vide() => self.aller(Etat::Vidange),
            Etat::Vidange | Etat::Fini => {}
            _ => self.finir("raccroché"),
        }
    }

    /// Coupe tout de suite (l'appel est perdu, ou on raccroche une seconde fois).
    pub fn couper(&mut self, raison: &str) {
        if self.etat != Etat::Fini {
            self.finir(raison);
        }
    }

    fn log(&mut self, niveau: LogLevel, msg: &str) {
        self.evenements.push(Evenement::Log(niveau, msg.to_string()));
    }

    fn aller(&mut self, e: Etat) {
        self.etat = e;
        self.n = 0;
    }

    fn finir(&mut self, raison: &str) {
        self.tx.porteuse = false;
        self.emis = "silence";
        self.entendu = "silence";
        self.fin = Some(raison.to_string());
        self.aller(Etat::Fini);
    }

    fn emet_signal(&mut self, nom: &'static str) {
        self.emis = nom;
        self.evenements.push(Evenement::Emet(nom));
    }

    fn connecter(&mut self) {
        let (tx, rx) = self.debits();
        self.connecte = true;
        self.aller(Etat::Connecte);
        self.evenements.push(Evenement::Connecte { tx, rx });
        if !self.recus.is_empty() {
            self.evenements.push(Evenement::Donnees(std::mem::take(&mut self.recus)));
        }
    }

    pub fn process(&mut self, rx: &[f32], tx: &mut [f32]) {
        for (x, y) in rx.iter().zip(tx.iter_mut()) {
            *y = self.step(*x);
        }
        if self.connecte && !self.recus.is_empty() {
            self.evenements.push(Evenement::Donnees(std::mem::take(&mut self.recus)));
        }
        // afficheur : on sait si des caractères passent ou si la porteuse est au repos
        if self.connecte && !self.fini() {
            let (voie_tx, voie_rx) = match self.cote {
                Cote::Serveur => (("V.23 1200 bit/s", "porteuse 1300 Hz"), ("V.23 75 bit/s", "porteuse 390 Hz")),
                Cote::Terminal => (("V.23 75 bit/s", "porteuse 390 Hz"), ("V.23 1200 bit/s", "porteuse 1300 Hz")),
            };
            self.emis = if self.tx.vide() { voie_tx.1 } else { voie_tx.0 };
            self.entendu = if self.recoit() { voie_rx.0 } else { voie_rx.1 };
        }
    }

    fn step(&mut self, x: f32) -> f32 {
        self.n = self.n.saturating_add(1);
        self.depuis_recu = self.depuis_recu.saturating_add(1);
        if self.etat == Etat::Fini {
            return 0.0;
        }
        match self.rx.push(x) {
            Some(Recu::Caractere(c)) => self.recu(c),
            // caractère abîmé : le Minitel l'affiche comme un pavé (SUB)
            Some(Recu::Parite(_)) => {
                self.erreurs_parite += 1;
                self.recu(0x1A);
            }
            Some(Recu::Cadrage) | None => {}
        }
        match self.etat {
            Etat::Silence => {
                if self.n >= self.silence {
                    self.emet_signal("ANS");
                    self.log(LogLevel::Info, "Envoi de l'ANS : 2100 Hz pur, sans modulation (V.25), pendant 2,6 s");
                    self.aller(Etat::Ans);
                }
                0.0
            }
            Etat::Ans => {
                if self.n >= ANS_MS * MS {
                    self.emis = "silence";
                    self.aller(Etat::Pause);
                    return 0.0;
                }
                self.ans.next_sample()
            }
            Etat::Pause => {
                if self.n >= PAUSE_MS * MS {
                    self.tx.porteuse = true;
                    self.emet_signal("porteuse 1300 Hz");
                    self.log(LogLevel::Info, "Porteuse V.23 : 1300 Hz, le sifflement du Minitel — en attente de la porteuse de retour (390 Hz)");
                    self.aller(Etat::Porteuse);
                }
                0.0
            }
            Etat::Porteuse => {
                if self.rx.present_ms() >= RETOUR_MS {
                    self.entendu = "porteuse 390 Hz";
                    self.evenements.push(Evenement::Entend("porteuse 390 Hz"));
                    self.log(LogLevel::Info, "Porteuse de retour 390 Hz reçue : un Minitel est en ligne");
                    self.log(LogLevel::Info, "CONNECT 1200/75 (V.23 : 1200 bit/s vers le Minitel, 75 bit/s depuis son clavier)");
                    self.connecter();
                } else if self.n >= ATTENTE_RETOUR_MS * MS {
                    self.log(LogLevel::Warn, "Pas de porteuse de retour : personne au bout du fil ?");
                    self.finir("pas de porteuse de retour (390 Hz)");
                    return 0.0;
                }
                self.tx.next_sample()
            }
            Etat::Accroche => {
                if self.n >= ACCROCHE_MS * MS && self.rx.porteuse() {
                    self.log(LogLevel::Info, "CONNECT 1200/75 (V.23 : 1200 bit/s vers l'écran, 75 bit/s depuis le clavier)");
                    self.connecter();
                } else if self.rx.absent_ms() >= PERTE_MS {
                    self.log(LogLevel::Warn, "Le sifflement s'est tu avant la connexion");
                    self.finir("porteuse 1300 Hz perdue");
                    return 0.0;
                }
                self.tx.next_sample()
            }
            Etat::Connecte | Etat::Vidange => {
                if self.rx.absent_ms() >= PERTE_MS {
                    let qui = if self.cote == Cote::Serveur { "Le Minitel a raccroché (porteuse 390 Hz perdue)" } else { "Le serveur a raccroché (porteuse 1300 Hz perdue)" };
                    self.log(LogLevel::Info, qui);
                    self.finir("porteuse perdue");
                    return 0.0;
                }
                // raccrochage : on laisse partir le dernier bit d'arrêt (20 ms)
                if self.etat == Etat::Vidange {
                    self.marge = if self.tx.vide() { self.marge + 1 } else { 0 };
                    if self.marge > 20 * MS || self.n >= VIDANGE_MAX_MS * MS {
                        self.finir("raccroché");
                        return 0.0;
                    }
                }
                self.tx.next_sample()
            }
            Etat::Fini => 0.0,
        }
    }

    fn recu(&mut self, c: u8) {
        self.depuis_recu = 0;
        self.caracteres_recus += 1;
        // avant la connexion : ce n'est qu'au serveur qu'on peut parler
        if matches!(self.etat, Etat::Connecte | Etat::Vidange | Etat::Accroche | Etat::Porteuse) {
            self.recus.push(c);
        }
    }
}

/// L'oreille du modem appelant pendant qu'il attend un ANSam : un ANS pur
/// (2100 Hz sans modulation, au moins une seconde) suivi d'une porteuse à
/// 1300 Hz, c'est un serveur Minitel. Un modem V.34 ne siffle jamais à
/// 1300 Hz (il enchaîne sur V.8, à 980/1180 et 1650/1850 Hz).
#[derive(Clone, Debug)]
pub struct EcouteMinitel {
    t2100: ToneDetector,
    t1300: ToneDetector,
    total: PowerMeter,
    /// Échantillons d'ANS entendus (au total)
    ans: u32,
    /// Échantillons depuis le dernier échantillon d'ANS
    depuis_ans: u32,
    porteuse: u32,
    annonce: bool,
}

impl Default for EcouteMinitel {
    fn default() -> Self {
        Self::new()
    }
}

impl EcouteMinitel {
    pub fn new() -> EcouteMinitel {
        // fenêtres de 20 ms : leurs zéros tombent tous les 50 Hz, donc
        // 1300 Hz est invisible pour le détecteur à 2100 Hz et inversement
        EcouteMinitel { t2100: ToneDetector::new(2100.0, 160), t1300: ToneDetector::new(1300.0, 160), total: PowerMeter::new(160), ans: 0, depuis_ans: u32::MAX, porteuse: 0, annonce: false }
    }

    /// Vrai au moment où le serveur Minitel est reconnu.
    pub fn push(&mut self, x: f32) -> bool {
        self.t2100.push(x);
        self.t1300.push(x);
        let p = self.total.push(x);
        let domine = |a: f32| a > crate::modem::TONE_MIN_AMP && a * a * 0.5 > 0.6 * p;
        if domine(self.t2100.amplitude()) {
            self.ans += 1;
            self.depuis_ans = 0;
        } else {
            self.depuis_ans = self.depuis_ans.saturating_add(1);
        }
        let apres_ans = self.ans >= 1000 * MS && self.depuis_ans < 1500 * MS;
        if apres_ans && domine(self.t1300.amplitude()) {
            self.porteuse += 1;
        } else {
            self.porteuse = 0;
        }
        self.porteuse >= 150 * MS
    }

    /// Vrai la première fois seulement (pour annoncer l'ANS une fois).
    pub fn annoncer_ans(&mut self) -> bool {
        !std::mem::replace(&mut self.annonce, true)
    }

    /// Durée d'ANS entendue (ms).
    pub fn ans_ms(&self) -> u32 {
        self.ans / MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deux bouts reliés directement : établissement, puis des caractères
    /// dans les deux sens.
    #[test]
    fn serveur_et_minitel_se_parlent() {
        let mut s = Minitel::serveur(400);
        let mut ecoute = EcouteMinitel::new();
        let mut m: Option<Minitel> = None;
        let (mut a, mut b) = (vec![0.0f32; 160], vec![0.0f32; 160]);
        let (mut vers_m, mut vers_s) = (vec![0.0f32; 160], vec![0.0f32; 160]);
        let mut recu_m = Vec::new();
        let mut recu_s = Vec::new();
        let mut connect = [None, None];
        for bloc in 0..400 {
            s.process(&vers_s, &mut a);
            match m.as_mut() {
                Some(m) => m.process(&vers_m, &mut b),
                None => {
                    b.fill(0.0);
                    if vers_m.iter().any(|&x| ecoute.push(x)) {
                        m = Some(Minitel::terminal());
                    }
                }
            }
            vers_m.copy_from_slice(&a);
            vers_s.copy_from_slice(&b);
            for e in s.evenements() {
                match e {
                    Evenement::Connecte { .. } => {
                        connect[0] = Some(bloc);
                        s.envoyer(b"\x0c3615 ORACLE");
                    }
                    Evenement::Donnees(d) => recu_s.extend(d),
                    _ => {}
                }
            }
            if let Some(m) = m.as_mut() {
                for e in m.evenements() {
                    match e {
                        Evenement::Connecte { tx, rx } => {
                            assert_eq!((tx, rx), (75, 1200));
                            connect[1] = Some(bloc);
                            m.envoyer(b"\x13\x41");
                        }
                        Evenement::Donnees(d) => recu_m.extend(d),
                        _ => {}
                    }
                }
            }
        }
        let (cs, cm) = (connect[0].expect("CONNECT serveur"), connect[1].expect("CONNECT Minitel"));
        // silence 0,4 s + ANS 2,6 s + pause + 150 ms d'écoute + 300 ms : ≈ 3,5 s
        assert!((170..=190).contains(&cm), "CONNECT du Minitel au bloc {cm}");
        assert!(cs <= cm);
        assert_eq!(recu_m, b"\x0c3615 ORACLE");
        assert_eq!(recu_s, b"\x13\x41");
    }

    #[test]
    fn un_ansam_n_est_pas_un_serveur_minitel() {
        let mut g = crate::v8::AnsamGen::new();
        let mut e = EcouteMinitel::new();
        for _ in 0..40_000 {
            assert!(!e.push(g.next_sample()));
        }
    }
}
