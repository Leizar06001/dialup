//! Modem V.29 : 9600 bit/s (repli 7200 et 4800), semi-duplex, celui qui
//! transporte les pages des télécopieurs du groupe 3.
//!
//! - Porteuse 1700 Hz, 2400 bauds, constellation de 16 points : quatre
//!   phases sur les axes (amplitudes 3 et 5) et quatre en diagonale (√2 et
//!   3√2). Q1 choisit l'amplitude, Q2Q3Q4 le **changement** de phase
//!   (codage différentiel, par pas de 45°). À 7200 bit/s, Q1 = 0 ; à 4800,
//!   Q1 = 0 et Q4 = non(Q2 ⊕ Q3) : seulement des quarts de tour.
//! - Embrouilleur auto-synchronisant 1 + x⁻¹⁸ + x⁻²³.
//! - Séquence d'apprentissage : segment 1, 48 symboles de silence ;
//!   segment 2, 128 symboles A/B alternés ; segment 3, 384 symboles C ou D
//!   selon la suite pseudo-aléatoire 1 + x⁻⁶ + x⁻⁷ ; segment 4, 48
//!   symboles de 1 embrouillés (déjà en mode données).
//!
//! Le récepteur cale rythme, phase et gain sur l'alternance A/B, entraîne
//! l'égaliseur sur les points A, B, C, D, puis passe en décisions sur toute
//! la constellation. Il ne rend les bits qu'après les 1 du segment 4, à
//! partir du premier 0 : les données d'un télécopieur (TCF, page T.4)
//! commencent toujours par des 0.

use std::collections::VecDeque;
use std::f32::consts::{FRAC_PI_4, SQRT_2};

use crate::api::NOMINAL_RMS;
use crate::dsp::shaping::PassbandMod;
use crate::dsp::Cpx;
use crate::qamrx::QamRx;
use crate::train::{QAM_ALPHA, QAM_SPAN};

pub const BAUDS: f64 = 2400.0;
pub const PORTEUSE: f64 = 1700.0;
const SEG1: usize = 48;
const SEG2: usize = 128;
const SEG3: usize = 384;
const SEG4: usize = 48;
/// Symboles de 1 embrouillés après les données, avant de couper la porteuse
const FIN: usize = 24;

/// Débit V.29.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Debit {
    B9600,
    B7200,
    B4800,
}

impl Debit {
    pub fn bits_par_symbole(self) -> usize {
        match self {
            Debit::B9600 => 4,
            Debit::B7200 => 3,
            Debit::B4800 => 2,
        }
    }
    pub fn bit_s(self) -> u32 {
        self.bits_par_symbole() as u32 * 2400
    }
    /// Points d'apprentissage A, B, C, D (indice de phase, amplitude)
    fn apprentissage(self) -> [(u8, f32); 4] {
        match self {
            Debit::B9600 => [(4, 3.0), (7, 3.0 * SQRT_2), (0, 3.0), (3, 3.0 * SQRT_2)],
            Debit::B7200 => [(4, 3.0), (7, SQRT_2), (0, 3.0), (3, SQRT_2)],
            Debit::B4800 => [(4, 3.0), (6, 3.0), (0, 3.0), (2, 3.0)],
        }
    }
}

/// Mise à l'échelle : puissance moyenne 1 pour la constellation à 16 points
fn echelle() -> f32 {
    1.0 / 13.5f32.sqrt()
}

/// Changement de phase (en pas de 45°) selon Q2Q3Q4 (V.29, tableau 1).
const DELTA: [u8; 8] = [1, 0, 2, 3, 6, 7, 5, 4];
/// Et l'inverse : Q2Q3Q4 selon le changement de phase.
const INVERSE: [u8; 8] = [1, 0, 2, 3, 7, 6, 4, 5];

fn point(phase: u8, amplitude: f32) -> Cpx {
    Cpx::from_polar(amplitude * echelle(), phase as f32 * FRAC_PI_4)
}

/// Amplitude selon la phase (axe ou diagonale) et Q1.
fn amplitude(phase: u8, q1: bool) -> f32 {
    match (phase % 2 == 0, q1) {
        (true, false) => 3.0,
        (true, true) => 5.0,
        (false, false) => SQRT_2,
        (false, true) => 3.0 * SQRT_2,
    }
}

/// Les points de la constellation à ce débit : (point, phase, Q1).
fn constellation(d: Debit) -> Vec<(Cpx, u8, bool)> {
    let mut v = Vec::new();
    for p in 0..8u8 {
        for q1 in [false, true] {
            let garde = match d {
                Debit::B9600 => true,
                Debit::B7200 => !q1,
                Debit::B4800 => !q1 && p % 2 == 0,
            };
            if garde {
                v.push((point(p, amplitude(p, q1)), p, q1));
            }
        }
    }
    v
}

#[derive(Clone, Debug, Default)]
struct Embrouilleur {
    sr: u32,
}

impl Embrouilleur {
    fn embrouiller(&mut self, b: bool) -> bool {
        let o = b ^ ((self.sr >> 17) & 1 == 1) ^ ((self.sr >> 22) & 1 == 1);
        self.sr = (self.sr << 1) | o as u32;
        o
    }
    fn desembrouiller(&mut self, r: bool) -> bool {
        let o = r ^ ((self.sr >> 17) & 1 == 1) ^ ((self.sr >> 22) & 1 == 1);
        self.sr = (self.sr << 1) | r as u32;
        o
    }
}

// ───────────────────────── émission ─────────────────────────

/// Émetteur V.29 : apprentissage, données, puis porteuse coupée.
pub struct V29Tx {
    debit: Debit,
    m: PassbandMod,
    n: usize,
    prbs: u8,
    emb: Embrouilleur,
    phase: u8,
    bits: VecDeque<bool>,
    terminer: bool,
    fin: usize,
    /// Symboles nuls de vidange du filtre, après la fin
    vidange: usize,
}

impl V29Tx {
    pub fn new(debit: Debit) -> V29Tx {
        let mut m = PassbandMod::new(BAUDS, PORTEUSE, QAM_ALPHA, QAM_SPAN);
        m.gain = NOMINAL_RMS * SQRT_2;
        V29Tx { debit, m, n: 0, prbs: 0b0101010, emb: Embrouilleur::default(), phase: 0, bits: VecDeque::new(), terminer: false, fin: 0, vidange: 0 }
    }

    /// Données à émettre après l'apprentissage.
    pub fn envoyer(&mut self, bits: impl IntoIterator<Item = bool>) {
        self.bits.extend(bits);
    }

    /// Plus de données : la porteuse s'arrêtera une fois la file vidée.
    pub fn terminer(&mut self) {
        self.terminer = true;
    }

    /// Bits encore en file.
    pub fn en_attente(&self) -> usize {
        self.bits.len()
    }

    /// L'émission est finie (porteuse coupée, filtre vidé).
    pub fn fini(&self) -> bool {
        self.vidange > 2 * QAM_SPAN + 2
    }

    fn symbole(&mut self) -> Cpx {
        let n = self.n;
        self.n += 1;
        let [a, b, c, d] = self.debit.apprentissage();
        if n < SEG1 {
            return Cpx::ZERO;
        }
        if n < SEG1 + SEG2 {
            let (p, amp) = if (n - SEG1) % 2 == 0 { a } else { b };
            return point(p, amp);
        }
        if n < SEG1 + SEG2 + SEG3 {
            // suite pseudo-aléatoire 1 + x⁻⁶ + x⁻⁷ : 0 → C, 1 → D
            let bit = ((self.prbs >> 5) ^ (self.prbs >> 6)) & 1;
            self.prbs = ((self.prbs << 1) | bit) & 0x7F;
            let (p, amp) = if bit == 0 { c } else { d };
            self.phase = p;
            return point(p, amp);
        }
        // Données (le segment 4 et la fin : des 1)
        let en_donnees = n >= SEG1 + SEG2 + SEG3 + SEG4;
        let mut q = [false; 4];
        let bps = self.debit.bits_par_symbole();
        if en_donnees && self.bits.is_empty() && self.terminer {
            if self.fin >= FIN {
                self.vidange += 1;
                return Cpx::ZERO;
            }
            self.fin += 1;
        }
        // Bits utiles : Q1Q2Q3Q4 à 9600, Q2Q3Q4 à 7200, Q2Q3 à 4800
        let premier = if bps == 4 { 0 } else { 1 };
        for qi in q.iter_mut().skip(premier).take(bps) {
            let b = if en_donnees { self.bits.pop_front().unwrap_or(true) } else { true };
            *qi = self.emb.embrouiller(b);
        }
        if self.debit == Debit::B4800 {
            q[3] = !(q[1] ^ q[2]);
        }
        let code = (q[1] as usize) << 2 | (q[2] as usize) << 1 | q[3] as usize;
        self.phase = (self.phase + DELTA[code]) % 8;
        point(self.phase, amplitude(self.phase, q[0]))
    }

    pub fn echantillon(&mut self) -> f32 {
        while self.m.needs_symbol() {
            let s = self.symbole();
            self.m.push_symbol(s);
        }
        self.m.next_sample()
    }
}

// ───────────────────────── réception ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Etat {
    /// Pas de porteuse
    Attente,
    /// Porteuse détectée vers le symbole k : on attend assez d'A/B pour se
    /// caler (numéro de l'essai)
    Calage(i64, u32),
    /// Égaliseur entraîné sur A, B, C, D jusqu'au symbole donné
    Apprentissage(i64),
    /// Décisions sur toute la constellation
    Donnees,
}

/// Récepteur V.29.
pub struct V29Rx {
    debit: Debit,
    rx: QamRx,
    etat: Etat,
    puissance: f32,
    dessus: u32,
    dessous: u32,
    dernier_k: i64,
    phase: u8,
    desemb: Embrouilleur,
    uns: u32,
    ouvert: bool,
    bits: Vec<bool>,
    porteuse_vue: bool,
    perdue: bool,
    points: Vec<(Cpx, u8, bool)>,
    /// Symboles décidés en mode données
    en_donnees_n: u32,
    /// Bruit de fond (puissance, suivi pendant l'attente)
    bruit: f32,
    /// Niveau du signal reçu (puissance, suivi pendant la réception)
    niveau: f32,
    /// Puissance lissée sur ~60 ms : un clic ne la fait pas remonter
    lente: f32,
    /// Erreur quadratique moyenne des décisions en données (diagnostic)
    pub evm: f32,
    /// Diagnostic : symbole de détection et corrélations des deux parités
    pub diag: (i64, f32, f32),
}

/// Seuils de porteuse (puissance moyenne ; le signal nominal fait 0,0625) :
/// au moins −33 dB sous le nominal, et nettement au-dessus du bruit de fond
const SEUIL_MIN: f32 = 3.0e-5;
/// Porteuse continue exigée avant de se caler (10 ms : pas un parasite)
const PRESENCE: u32 = 80;
/// Corrélation normalisée minimale avec l'alternance A/B
const CORRELATION_MIN: f32 = 0.35;

impl V29Rx {
    pub fn new(debit: Debit) -> V29Rx {
        V29Rx {
            debit,
            rx: QamRx::new(BAUDS, PORTEUSE),
            etat: Etat::Attente,
            puissance: 0.0,
            dessus: 0,
            dessous: 0,
            dernier_k: 0,
            phase: 0,
            desemb: Embrouilleur::default(),
            uns: 0,
            ouvert: false,
            bits: Vec::new(),
            porteuse_vue: false,
            perdue: false,
            points: constellation(debit),
            en_donnees_n: 0,
            // (rejoint le vrai bruit en ~60 ms, avant le premier signal)
            bruit: 1.0e-3,
            niveau: 0.0,
            lente: 0.0,
            evm: 1.0,
            diag: (0, 0.0, 0.0),
        }
    }

    /// La porteuse est apparue (et pas encore disparue).
    pub fn porteuse(&self) -> bool {
        self.porteuse_vue && !self.perdue
    }

    /// La porteuse est apparue puis a disparu : réception terminée.
    pub fn fini(&self) -> bool {
        self.perdue
    }

    /// Les données ont commencé (les 1 du segment 4 sont passés).
    pub fn en_donnees(&self) -> bool {
        self.ouvert
    }

    /// SNR estimé par l'égaliseur (dB).
    pub fn snr_db(&self) -> f32 {
        self.rx.snr_db()
    }

    /// Bits reçus depuis le dernier appel.
    pub fn prendre(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.bits)
    }

    pub fn push(&mut self, x: f32) {
        if self.perdue {
            return;
        }
        self.puissance += 0.05 * (x * x - self.puissance);
        if self.etat == Etat::Attente && self.dessus == 0 {
            // bruit de fond : part de haut, suit vite vers le bas et
            // lentement vers le haut (un clic ne le fait pas monter)
            let k = if self.puissance < self.bruit { 0.01 } else { 0.0005 };
            self.bruit += k * (self.puissance - self.bruit);
        }
        let seuil_on = SEUIL_MIN.max(4.0 * self.bruit);
        // Pendant la réception, la porteuse est perdue 10 dB sous son niveau
        let seuil_off = if matches!(self.etat, Etat::Apprentissage(_) | Etat::Donnees) { (0.1 * self.niveau).max(2.5 * self.bruit).max(0.4 * SEUIL_MIN) } else { 0.4 * seuil_on };
        // Le niveau de référence se mesure pendant l'apprentissage seulement
        // (ensuite, le bruit d'après la fin l'entraînerait vers le bas)
        if matches!(self.etat, Etat::Apprentissage(_)) {
            self.niveau += 0.001 * (self.puissance - self.niveau);
        }
        // (le clic d'un parasite est bref : il compte à peine dans la
        // puissance lente, qui décide de la fin de porteuse)
        self.lente += 0.002 * ((x * x).min(4.0 * self.niveau.max(SEUIL_MIN)) - self.lente);
        let en_reception = matches!(self.etat, Etat::Apprentissage(_) | Etat::Donnees);
        if self.puissance > seuil_on {
            self.dessus += 1;
        } else if self.puissance < seuil_off {
            self.dessus = 0;
        }
        let eteinte = if en_reception { self.lente < seuil_off } else { self.puissance < seuil_off };
        if eteinte {
            self.dessous += 1;
        } else {
            self.dessous = 0;
        }
        if self.etat == Etat::Attente && self.dessus >= PRESENCE {
            self.etat = Etat::Calage(self.dernier_k, 0);
        } else if self.etat != Etat::Attente && self.dessous >= 400 {
            // 50 ms sans porteuse (puissance lente) : fin de la transmission ;
            // une coupure plus brève de la ligne n'arrête pas la réception
            self.perdue = true;
            return;
        }
        let Some(s) = self.rx.push(x) else { return };
        self.dernier_k = s.k;
        match self.etat {
            Etat::Attente => {}
            Etat::Calage(k0, essai) => {
                // Rythme, phase et gain sur 40 symboles A/B alternés : la
                // fenêtre (trois essais) et la parité qui corrèlent le mieux
                const N: i64 = 40;
                if s.k < k0 + 24 + N {
                    return;
                }
                let [a, b, ..] = self.debit.apprentissage();
                let (pa, pb) = (point(a.0, a.1), point(b.0, b.1));
                let refs = |debut: i64, pair: bool| -> Vec<(i64, Cpx)> {
                    (debut..debut + N).map(|k| (k, if ((k - debut) % 2 == 0) == pair { pa } else { pb })).collect()
                };
                let mut meilleur = (0.0f32, k0, true);
                for debut in [k0 - 8, k0 + 8, k0 + 24] {
                    for pair in [true, false] {
                        let c = self.rx.correlation(&refs(debut, pair));
                        if c > meilleur.0 {
                            meilleur = (c, debut, pair);
                        }
                    }
                }
                let (c, debut, pair) = meilleur;
                self.diag = (k0, c, (debut - k0) as f32);
                if c < CORRELATION_MIN {
                    // Pas l'alternance A/B (un parasite dans la fenêtre, ou
                    // une fausse alerte) : on réessaie un peu plus loin tant
                    // que le segment 2 peut encore être là
                    self.etat = if essai < 6 { Etat::Calage(k0 + 16, essai + 1) } else { Etat::Attente };
                    self.dessus = 0;
                    return;
                }
                self.porteuse_vue = true;
                self.niveau = self.puissance;
                self.lente = self.puissance;
                self.rx.fine_timing(&refs(debut, pair));
                self.rx.start_ls();
                self.etat = Etat::Apprentissage(debut + 400);
            }
            Etat::Apprentissage(jusqua) => {
                let [a, b, c, d] = self.debit.apprentissage();
                let r = [a, b, c, d].iter().map(|&(p, amp)| point(p, amp)).min_by(|u, v| (*u - s.z).norm_sqr().total_cmp(&(*v - s.z).norm_sqr())).unwrap_or(Cpx::ZERO);
                if (r - s.z).norm_sqr() < 0.5 {
                    self.rx.adapt(s.z, r);
                }
                if s.k >= jusqua {
                    // Coefficients optimaux (moindres carrés) sur tout
                    // l'apprentissage, puis un pas plus fin pour suivre la ligne
                    self.rx.finish_ls(64);
                    self.rx.mu = 0.08;
                    self.etat = Etat::Donnees;
                    self.phase = self.decider(s.z).1;
                }
            }
            Etat::Donnees => {
                let (r, p, q1) = self.decider(s.z);
                let e = (r - s.z).norm_sqr();
                // Un parasite (clic, coupure) donne une erreur aberrante :
                // on n'adapte pas l'égaliseur dessus, sinon il diverge
                if e < (4.0 * self.evm).max(0.02) {
                    self.rx.adapt(s.z, r);
                }
                self.evm += 0.002 * (e.min(1.0) - self.evm);
                let delta = (p + 8 - self.phase) % 8;
                self.phase = p;
                let code = INVERSE[delta as usize];
                let q = [q1, code & 4 != 0, code & 2 != 0, code & 1 != 0];
                // Sur une ligne très bruitée, les 1 du segment 4 peuvent ne
                // jamais passer intacts : on ouvre quand même (le TCF et le
                // décodeur T.4 se resynchronisent tout seuls)
                self.en_donnees_n += 1;
                if self.en_donnees_n > 160 {
                    self.ouvert = true;
                }
                let bps = self.debit.bits_par_symbole();
                let premier = if bps == 4 { 0 } else { 1 };
                for &b in &q[premier..premier + bps] {
                    let d = self.desemb.desembrouiller(b);
                    if self.ouvert {
                        self.bits.push(d);
                    } else if d {
                        self.uns += 1;
                    } else if self.uns >= 40 {
                        // premier 0 après les 1 du segment 4 : les données
                        self.ouvert = true;
                        self.bits.push(d);
                    } else {
                        self.uns = 0;
                    }
                }
            }
        }
    }

    /// Point le plus proche : (point, phase, Q1).
    fn decider(&self, z: Cpx) -> (Cpx, u8, bool) {
        let mut best = self.points[0];
        let mut dmin = f32::MAX;
        for &pt in &self.points {
            let d = (pt.0 - z).norm_sqr();
            if d < dmin {
                dmin = d;
                best = pt;
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::{LineParams, LineSim};

    /// Bits pseudo-aléatoires reproductibles (commençant par un 0, comme
    /// les données d'un télécopieur)
    fn donnees(n: usize) -> Vec<bool> {
        let mut x = 0x1234_5678u32;
        std::iter::once(false).chain((1..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x & 1 == 1
            }))
            .collect()
    }

    /// Émet `bits` à ce débit à travers la ligne ; rend ce qui a été reçu.
    fn liaison(debit: Debit, bits: &[bool], ligne: Option<&str>) -> (Vec<bool>, f32) {
        let (r, _, evm) = liaison_diag(debit, bits, ligne);
        (r, -10.0 * evm.max(1e-9).log10())
    }

    fn liaison_diag(debit: Debit, bits: &[bool], ligne: Option<&str>) -> (Vec<bool>, (i64, f32, f32), f32) {
        let mut tx = V29Tx::new(debit);
        tx.envoyer(bits.iter().copied());
        tx.terminer();
        let mut rx = V29Rx::new(debit);
        let mut sim = ligne.map(|l| LineSim::new(LineParams::preset(l).unwrap()));
        let mut recu = Vec::new();
        // un peu de silence avant (le bruit de la ligne seulement)
        for _ in 0..800 {
            let mut o = [0.0f32];
            if let Some(s) = sim.as_mut() {
                s.process(&[0.0], &mut o);
            }
            rx.push(o[0]);
        }
        let mut n = 0;
        let mut evm = 1.0;
        while !rx.fini() && n < 8000 * 60 {
            let x = if tx.fini() { 0.0 } else { tx.echantillon() };
            let y = match sim.as_mut() {
                Some(s) => {
                    let mut o = [0.0f32];
                    s.process(&[x], &mut o);
                    o[0]
                }
                None => x,
            };
            rx.push(y);
            let avant = recu.len();
            recu.extend(rx.prendre());
            if avant < bits.len() / 2 && recu.len() >= bits.len() / 2 {
                evm = rx.evm;
            }
            n += 1;
        }
        if std::env::var_os("V29_NIVEAUX").is_some() {
            eprintln!("  fin : n = {n}, puissance {:.2e}, lente {:.2e}, bruit {:.2e}, niveau {:.2e}, dessous {}, état {:?}", rx.puissance, rx.lente, rx.bruit, rx.niveau, rx.dessous, rx.etat);
        }
        (recu, rx.diag, evm)
    }

    /// Erreurs, au meilleur alignement (à quelques bits près au début,
    /// comme le TCF et le T.4 qui se resynchronisent tout seuls) ; la fin
    /// (1 de fin de porteuse) est ignorée.
    fn erreurs(envoye: &[bool], recu: &[bool]) -> usize {
        let compte = |dec: usize| {
            let r = recu.get(dec..).unwrap_or(&[]);
            let n = envoye.len().min(r.len());
            envoye[..n].iter().zip(&r[..n]).filter(|(a, b)| a != b).count() + (envoye.len() - n)
        };
        (0..64).map(compte).min().unwrap_or(envoye.len())
    }

    #[test]
    fn aller_retour_sans_ligne() {
        for d in [Debit::B9600, Debit::B7200, Debit::B4800] {
            let bits = donnees(9600);
            let (recu, snr) = liaison(d, &bits, None);
            assert_eq!(erreurs(&bits, &recu), 0, "{d:?} : SNR {snr:.1} dB, {} bits reçus", recu.len());
        }
    }

    #[test]
    #[ignore = "mesure : V29_LIGNE=Orage cargo test -p v34-core --lib orage -- --ignored --nocapture"]
    fn orage() {
        let ligne = std::env::var("V29_LIGNE").unwrap_or_else(|_| "Orage".into());
        for d in [Debit::B9600, Debit::B7200, Debit::B4800] {
            let bits = donnees(9600 * 3);
            let (recu, diag, evm) = liaison_diag(d, &bits, Some(&ligne));
            eprintln!("V.29 {} sur {ligne} : {} erreurs sur {} bits ({} reçus), EVM {:.1} dB, calage {diag:?}", d.bit_s(), erreurs(&bits, &recu), bits.len(), recu.len(), -10.0 * evm.max(1e-9).log10());
        }
    }

    #[test]
    fn a_travers_les_lignes() {
        for ligne in ["Parfaite", "Urbaine", "Campagne"] {
            let bits = donnees(9600 * 3);
            let (recu, snr) = liaison(Debit::B9600, &bits, Some(ligne));
            let e = erreurs(&bits, &recu);
            eprintln!("V.29 9600 sur {ligne} : {e} erreurs sur {} bits, SNR {snr:.1} dB", bits.len());
            // Campagne a des parasites (0,2 par seconde) : une salve d'erreurs
            // est possible selon le tirage ; les autres lignes, aucune erreur
            let max = if ligne == "Campagne" { bits.len() / 100 } else { 0 };
            assert!(e <= max, "{ligne} : {e} erreurs, SNR {snr:.1} dB");
        }
    }
}
