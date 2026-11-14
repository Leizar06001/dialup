//! Un appel de télécopie, d'un bout à l'autre (T.30, méthode « non ECM ») :
//!
//! ```text
//! émetteur (appelant)                      récepteur (appelé)
//!   CNG 1100 Hz ………………………………→
//!                                ←……………… CED 2100 Hz (3 s)
//!                                ←……………… CSI + DIS (V.21)
//!   TSI + DCS (V.21) ……………………→
//!   TCF (V.29, 1,5 s de 0) ………→
//!                                ←……………… CFR (ou FTT : on réessaie moins vite)
//!   page (V.29, T.4) ……………………→     (imprimée au fil de l'eau)
//!   EOP (V.21) ……………………………………→
//!                                ←……………… MCF
//!   DCN (V.21) ……………………………………→     raccroché
//! ```
//!
//! Tout est en échantillons à 8 kHz : [`Fax::process`] prend ce qui arrive
//! de la ligne et remplit ce qui part, comme le modem de données.

use std::f32::consts::SQRT_2;

use crate::api::NOMINAL_RMS;
use crate::dsp::fsk::{FskDemod, FskMod, V21Channel};
use crate::dsp::osc::Osc;

use super::hdlc;
use super::t30::{self, Trame};
use super::t4;
use super::v29::{Debit, V29Rx, V29Tx};

const MS: usize = 8;
/// Silence entre deux salves (T.30 : 75 ± 20 ms)
const PAUSE: usize = 75 * MS;
/// Attente d'une réponse (T4 : 3 s)
const T4: usize = 3000 * MS;
/// Attente du correspondant au début (T1 : 35 s)
const T1: usize = 35_000 * MS;
/// Préambule V.21 : 1 s de fanions
const PREAMBULE: usize = 38;
/// Durée du TCF
const TCF_MS: usize = 1500;
const ESSAIS: u32 = 3;
const AMPLITUDE: f32 = NOMINAL_RMS * SQRT_2;

/// Ce qui se passe, pour l'affichage.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FaxEvenement {
    /// Étape de la procédure (« CNG », « DIS reçu », « DCS 9600 »…)
    Etape(String),
    /// Numéro du correspondant (CSI ou TSI)
    Correspondant(String),
    /// Une ligne de la page reçue, décodée (récepteur)
    Ligne(Vec<bool>),
    /// Lignes envoyées (émetteur)
    Progression { faites: usize, total: usize },
    /// Résolution de la page qui arrive (récepteur) : fine = 7,7 lignes/mm
    Resolution { fine: bool },
    Fin { ok: bool, raison: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleFax {
    Emetteur,
    Recepteur,
}

/// Ce qui part sur la ligne.
enum Sortie {
    Silence,
    Ton(Osc),
    V21(FskMod, hdlc::Emetteur),
    V29(Box<V29Tx>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Etat {
    // émetteur
    Cng,
    AvantTcf,
    Tcf,
    AttenteCfr,
    AvantPage,
    Page,
    AvantEop,
    EnvoiEop,
    AttenteMcf,
    EnvoiDcn,
    // récepteur
    SilenceInitial,
    Ced,
    AvantDis,
    EnvoiDis,
    AttenteDcs,
    RecoitTcf,
    AvantReponseTcf,
    EnvoiReponseTcf,
    RecoitPage,
    AttenteEop,
    AvantMcf,
    EnvoiMcf,
    AttenteDcn,
    Fini,
}

pub struct Fax {
    role: RoleFax,
    etat: Etat,
    /// Échantillons depuis l'entrée dans l'état
    n: usize,
    /// Depuis le début
    total: usize,
    sortie: Sortie,
    numero: String,
    v21: FskDemod,
    hdlc: hdlc::Recepteur,
    recues: Vec<Trame>,
    evenements: Vec<FaxEvenement>,
    debit: Debit,
    fine: bool,
    essais: u32,
    // émetteur
    page_bits: Vec<bool>,
    /// Bit de fin de chaque ligne dans `page_bits`
    fins_lignes: Vec<usize>,
    lignes_faites: usize,
    // récepteur
    rx29: Option<Box<V29Rx>>,
    tcf: Vec<bool>,
    tcf_ok: bool,
    t4: Option<t4::Decodeur>,
    page: Vec<Vec<bool>>,
    /// Émetteur qui abandonne : la raison, dite après le DCN
    abandon: Option<&'static str>,
    /// Nom de ce qui part en ce moment (karaoké, afficheur)
    emis: &'static str,
    /// Fanions V.21 vus récemment (échantillon du dernier)
    v21_vu: usize,
}

impl Fax {
    fn new(role: RoleFax, numero: &str, etat: Etat) -> Fax {
        Fax {
            role,
            etat,
            n: 0,
            total: 0,
            sortie: Sortie::Silence,
            numero: numero.to_string(),
            v21: FskDemod::new(V21Channel::High),
            hdlc: hdlc::Recepteur::new(),
            recues: Vec::new(),
            evenements: Vec::new(),
            debit: Debit::B9600,
            fine: true,
            essais: 0,
            page_bits: Vec::new(),
            fins_lignes: Vec::new(),
            lignes_faites: 0,
            rx29: None,
            tcf: Vec::new(),
            tcf_ok: false,
            t4: None,
            page: Vec::new(),
            abandon: None,
            emis: "silence",
            v21_vu: 0,
        }
    }

    /// Appelant : envoie cette page (lignes de 1728 points, vrai = noir).
    /// `fine` : 7,7 lignes par mm (sinon 3,85).
    pub fn emetteur(page: &[Vec<bool>], numero: &str, fine: bool) -> Fax {
        let mut f = Fax::new(RoleFax::Emetteur, numero, Etat::Cng);
        f.fine = fine;
        // Page codée d'avance, avec la position de chaque fin de ligne
        let mut bits = Vec::new();
        bits.extend([false; 11].iter().chain([true].iter()));
        for l in page {
            t4::coder_ligne(l, &mut bits);
            bits.extend([false; 11].iter().chain([true].iter()));
            f.fins_lignes.push(bits.len());
        }
        for _ in 0..5 {
            bits.extend([false; 11].iter().chain([true].iter()));
        }
        f.page_bits = bits;
        f.sortie = Sortie::Ton(Osc::new(1100.0));
        f.emis = "CNG";
        f.evenement(FaxEvenement::Etape("CNG".into()));
        f
    }

    /// Appelé : reçoit une page. `apres_ansam` : le modem a déjà fait
    /// entendre son 2100 Hz (ANSam), on passe directement au DIS.
    pub fn recepteur(numero: &str, apres_ansam: bool) -> Fax {
        let mut f = Fax::new(RoleFax::Recepteur, numero, if apres_ansam { Etat::AvantDis } else { Etat::SilenceInitial });
        if apres_ansam {
            f.evenement(FaxEvenement::Etape("CNG entendu : mode fax".into()));
        }
        f
    }

    pub fn role(&self) -> RoleFax {
        self.role
    }

    pub fn fini(&self) -> bool {
        self.etat == Etat::Fini
    }

    /// Débit V.29 en cours de négociation ou retenu.
    pub fn debit(&self) -> Debit {
        self.debit
    }

    /// Page reçue (récepteur), ligne par ligne.
    pub fn page(&self) -> &[Vec<bool>] {
        &self.page
    }

    /// Nom de l'étape en cours, pour l'affichage.
    pub fn etape(&self) -> &'static str {
        match self.etat {
            Etat::Cng => "CNG",
            Etat::AvantTcf | Etat::Tcf | Etat::RecoitTcf => "TCF",
            Etat::AttenteCfr | Etat::AvantReponseTcf | Etat::EnvoiReponseTcf => "CFR",
            Etat::AvantPage | Etat::Page | Etat::RecoitPage => "PAGE",
            Etat::AvantEop | Etat::EnvoiEop | Etat::AttenteEop => "EOP",
            Etat::AttenteMcf | Etat::AvantMcf | Etat::EnvoiMcf => "MCF",
            Etat::EnvoiDcn | Etat::AttenteDcn => "DCN",
            Etat::SilenceInitial | Etat::Ced => "CED",
            Etat::AvantDis | Etat::EnvoiDis | Etat::AttenteDcs => "DIS",
            Etat::Fini => "FIN",
        }
    }

    /// L'appel est coupé en cours de route : la fin, pour l'affichage. Une
    /// page déjà reçue reste reçue (l'appelant a pu raccrocher juste après
    /// notre MCF, avant que son DCN nous parvienne).
    pub fn couper(&mut self) {
        if self.fini() {
            return;
        }
        // Émetteur qui a déjà eu sa confirmation (MCF) : le correspondant a
        // raccroché pendant notre DCN, la page est bien partie
        if self.etat == Etat::EnvoiDcn && self.abandon.is_none() {
            let ok = self.tcf_ok;
            self.finir(ok, if ok { "page transmise" } else { "page transmise, mais mal reçue" });
            return;
        }
        let recue = self.role == RoleFax::Recepteur && !self.page.is_empty();
        let raison = if recue { "page reçue" } else { "appel coupé" };
        self.finir(recue, raison);
    }

    pub fn evenements(&mut self) -> Vec<FaxEvenement> {
        std::mem::take(&mut self.evenements)
    }

    fn evenement(&mut self, e: FaxEvenement) {
        self.evenements.push(e);
    }

    fn etape_ev(&mut self, s: impl Into<String>) {
        self.evenements.push(FaxEvenement::Etape(s.into()));
    }

    fn aller(&mut self, e: Etat) {
        self.etat = e;
        self.n = 0;
    }

    fn finir(&mut self, ok: bool, raison: &str) {
        self.sortie = Sortie::Silence;
        self.evenement(FaxEvenement::Fin { ok, raison: raison.into() });
        self.aller(Etat::Fini);
    }

    /// Une salve V.21 : préambule puis les trames (la dernière est finale).
    fn salve(&mut self, trames: &[(u8, Vec<u8>)]) {
        let mut e = hdlc::Emetteur::default();
        e.preambule(PREAMBULE);
        for (i, (fcf, fif)) in trames.iter().enumerate() {
            e.trame(&t30::construire(*fcf, fif, i + 1 == trames.len()));
        }
        self.sortie = Sortie::V21(FskMod::new(V21Channel::High), e);
        self.emis = trames.last().map_or("V.21", |(f, _)| t30::nom(*f));
        let noms: Vec<&str> = trames.iter().map(|(f, _)| t30::nom(*f)).collect();
        self.etape_ev(format!("→ {}", noms.join(" + ")));
    }

    /// Ce qui part sur la ligne : « CNG », « CED », le nom de la trame V.21
    /// (« DIS », « DCS »…), « TCF », « PAGE », ou « silence ».
    pub fn emission(&self) -> &'static str {
        match self.sortie {
            Sortie::Silence => "silence",
            _ => self.emis,
        }
    }

    /// Ce qui arrive : « TCF » ou « PAGE » (V.29), « V.21 » (trames),
    /// ou rien.
    pub fn reception(&self) -> &'static str {
        if self.rx29.as_ref().is_some_and(|r| r.porteuse()) {
            return if self.etat == Etat::RecoitTcf { "TCF" } else { "PAGE" };
        }
        if self.emet() || self.total.saturating_sub(self.v21_vu) > 200 * MS {
            return "";
        }
        "V.21"
    }

    fn emet(&self) -> bool {
        !matches!(self.sortie, Sortie::Silence)
    }

    /// Fin de l'émission en cours ?
    fn sortie_finie(&self) -> bool {
        match &self.sortie {
            Sortie::Silence => true,
            Sortie::Ton(_) => false,
            Sortie::V21(m, e) => e.vide() && m.needs_bit(),
            Sortie::V29(t) => t.fini(),
        }
    }

    fn echantillon(&mut self) -> f32 {
        match &mut self.sortie {
            Sortie::Silence => 0.0,
            Sortie::Ton(o) => AMPLITUDE * o.next_cos(),
            Sortie::V21(m, e) => {
                if m.needs_bit() {
                    // après la dernière trame, des fanions… non : des 1 (repos)
                    let b = e.bit().unwrap_or(true);
                    m.set_bit(b);
                }
                AMPLITUDE * m.next_sample()
            }
            Sortie::V29(t) => {
                if t.fini() {
                    0.0
                } else {
                    t.echantillon()
                }
            }
        }
    }

    /// Ce qui arrive de la ligne, quand on n'émet pas.
    fn ecouter(&mut self, x: f32) {
        if let Some(b) = self.v21.push(x) {
            let avant = self.hdlc.fanions;
            let trame = self.hdlc.bit(b);
            if self.hdlc.fanions != avant {
                self.v21_vu = self.total;
            }
            if let Some(octets) = trame {
                if let Some(t) = t30::lire(&octets) {
                    self.etape_ev(format!("← {}", t30::nom(t.fcf)));
                    self.recues.push(t);
                }
            }
        }
        if let Some(rx) = self.rx29.as_mut() {
            rx.push(x);
            let bits = rx.prendre();
            match self.etat {
                Etat::RecoitTcf => self.tcf.extend(bits),
                Etat::RecoitPage => {
                    if let Some(d) = self.t4.as_mut() {
                        for b in bits {
                            match d.bit(b) {
                                Some(t4::Sortie::Ligne { pixels, .. }) => {
                                    self.page.push(pixels.clone());
                                    self.evenements.push(FaxEvenement::Ligne(pixels));
                                }
                                Some(t4::Sortie::FinDePage) | None => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Dernière salve reçue complète (trame finale) : on la prend.
    fn salve_recue(&mut self) -> Option<Vec<Trame>> {
        if self.recues.last().is_some_and(|t| t.finale) {
            Some(std::mem::take(&mut self.recues))
        } else {
            None
        }
    }

    pub fn process(&mut self, rx: &[f32], tx: &mut [f32]) {
        for (i, o) in tx.iter_mut().enumerate() {
            *o = self.echantillon();
            if !self.emet() {
                self.ecouter(rx.get(i).copied().unwrap_or(0.0));
            }
            self.n += 1;
            self.total += 1;
            self.avancer();
        }
    }

    fn avancer(&mut self) {
        match self.etat {
            Etat::Fini => {}
            // ───────── émetteur ─────────
            Etat::Cng => {
                // 0,5 s de 1100 Hz, 3 s de silence, en écoutant le DIS
                let cycle = self.n % (3500 * MS);
                if cycle == 0 {
                    self.sortie = Sortie::Ton(Osc::new(1100.0));
                    self.emis = "CNG";
                } else if cycle == 500 * MS {
                    self.sortie = Sortie::Silence;
                }
                if let Some(s) = self.salve_recue() {
                    if let Some(dis) = s.iter().find(|t| t.code() == t30::DIS) {
                        if let Some(csi) = s.iter().find(|t| t.code() == t30::CSI) {
                            let c = t30::ident_lire(&csi.fif);
                            self.evenement(FaxEvenement::Correspondant(c));
                        }
                        self.debit = if t30::dis_v29(&dis.fif) { Debit::B9600 } else { Debit::B4800 };
                        self.sortie = Sortie::Silence;
                        self.essais = 0;
                        // la salve DCS, puis le TCF (état AvantTcf)
                        self.envoyer_dcs();
                    }
                } else if self.total > T1 {
                    self.finir(false, "pas de télécopieur au bout du fil");
                }
            }
            Etat::AvantTcf => {
                if matches!(self.sortie, Sortie::V21(..)) {
                    if self.sortie_finie() {
                        self.sortie = Sortie::Silence;
                        self.n = 0;
                    }
                } else if self.n >= PAUSE {
                    let mut t = V29Tx::new(self.debit);
                    t.envoyer(std::iter::repeat_n(false, self.debit.bit_s() as usize * TCF_MS / 1000));
                    t.terminer();
                    self.sortie = Sortie::V29(Box::new(t));
                    self.emis = "TCF";
                    self.etape_ev(format!("→ TCF ({} bit/s)", self.debit.bit_s()));
                    self.aller(Etat::Tcf);
                }
            }
            Etat::Tcf => {
                if self.sortie_finie() {
                    self.sortie = Sortie::Silence;
                    self.recues.clear();
                    self.aller(Etat::AttenteCfr);
                }
            }
            Etat::AttenteCfr => {
                if let Some(s) = self.salve_recue() {
                    if s.iter().any(|t| t.code() == t30::CFR) {
                        self.aller(Etat::AvantPage);
                    } else if s.iter().any(|t| t.code() == t30::FTT) {
                        // apprentissage raté : on réessaie moins vite
                        self.debit = match self.debit {
                            Debit::B9600 => Debit::B7200,
                            Debit::B7200 => Debit::B4800,
                            Debit::B4800 => {
                                self.abandonner("ligne trop mauvaise, même à 4800 bit/s");
                                return;
                            }
                        };
                        self.essais = 0;
                        self.envoyer_dcs();
                    }
                } else if self.n >= T4 {
                    self.essais += 1;
                    if self.essais >= ESSAIS {
                        self.finir(false, "pas de réponse au DCS");
                    } else {
                        self.envoyer_dcs();
                    }
                }
            }
            Etat::AvantPage => {
                if self.n >= PAUSE {
                    let mut t = V29Tx::new(self.debit);
                    t.envoyer(self.page_bits.iter().copied());
                    t.terminer();
                    self.sortie = Sortie::V29(Box::new(t));
                    self.emis = "PAGE";
                    self.etape_ev(format!("→ page ({} bit/s)", self.debit.bit_s()));
                    self.aller(Etat::Page);
                }
            }
            Etat::Page => {
                if let Sortie::V29(t) = &self.sortie {
                    let partis = self.page_bits.len().saturating_sub(t.en_attente());
                    let faites = self.fins_lignes.partition_point(|&f| f <= partis);
                    if faites != self.lignes_faites {
                        self.lignes_faites = faites;
                        if faites % 16 == 0 || faites == self.fins_lignes.len() {
                            self.evenement(FaxEvenement::Progression { faites, total: self.fins_lignes.len() });
                        }
                    }
                }
                if self.sortie_finie() {
                    self.sortie = Sortie::Silence;
                    self.essais = 0;
                    self.aller(Etat::AvantEop);
                }
            }
            Etat::AvantEop => {
                if self.n >= PAUSE {
                    self.salve(&[(t30::EOP | 1, vec![])]);
                    self.aller(Etat::EnvoiEop);
                }
            }
            Etat::EnvoiEop => {
                if self.sortie_finie() {
                    self.sortie = Sortie::Silence;
                    self.recues.clear();
                    self.aller(Etat::AttenteMcf);
                }
            }
            Etat::AttenteMcf => {
                if let Some(s) = self.salve_recue() {
                    if s.iter().any(|t| matches!(t.code(), t30::MCF | t30::RTN)) {
                        let ok = s.iter().any(|t| t.code() == t30::MCF);
                        self.salve(&[(t30::DCN | 1, vec![])]);
                        self.tcf_ok = ok;
                        self.aller(Etat::EnvoiDcn);
                    }
                } else if self.n >= T4 {
                    self.essais += 1;
                    if self.essais >= ESSAIS {
                        self.finir(false, "page envoyée, mais pas de confirmation");
                    } else {
                        self.aller(Etat::AvantEop);
                    }
                }
            }
            Etat::EnvoiDcn => {
                if matches!(self.sortie, Sortie::V21(..)) {
                    if self.sortie_finie() {
                        self.sortie = Sortie::Silence;
                        self.n = 0;
                    }
                    return;
                }
                // un peu de silence : le DCN arrive avant qu'on raccroche
                if self.n >= 300 * MS {
                    if let Some(r) = self.abandon {
                        self.finir(false, r);
                    } else {
                        let ok = self.tcf_ok;
                        self.finir(ok, if ok { "page transmise" } else { "page transmise, mais mal reçue" });
                    }
                }
            }
            // ───────── récepteur ─────────
            Etat::SilenceInitial => {
                if self.n >= 200 * MS {
                    self.sortie = Sortie::Ton(Osc::new(2100.0));
                    self.emis = "CED";
                    self.etape_ev("CED");
                    self.aller(Etat::Ced);
                }
            }
            Etat::Ced => {
                if self.n >= 3000 * MS {
                    self.sortie = Sortie::Silence;
                    self.aller(Etat::AvantDis);
                }
            }
            Etat::AvantDis => {
                if self.n >= PAUSE {
                    let csi = t30::fif_ident(&self.numero);
                    self.salve(&[(t30::CSI, csi), (t30::DIS, t30::fif_dis())]);
                    self.aller(Etat::EnvoiDis);
                }
            }
            Etat::EnvoiDis => {
                if self.sortie_finie() {
                    self.sortie = Sortie::Silence;
                    self.recues.clear();
                    self.aller(Etat::AttenteDcs);
                }
            }
            Etat::AttenteDcs => {
                if let Some(s) = self.salve_recue() {
                    if s.iter().any(|t| t.code() == t30::DCN) {
                        self.finir(false, "l'appelant a abandonné (ligne trop mauvaise)");
                        return;
                    }
                    self.lire_dcs(&s);
                } else if self.n >= T4 {
                    if self.total > T1 {
                        self.finir(false, "pas de réponse de l'appelant");
                    } else {
                        self.aller(Etat::AvantDis);
                    }
                }
            }
            Etat::RecoitTcf => {
                // On juge dès que les 1,5 s sont là (sans attendre que la
                // porteuse s'éteigne : sur une ligne à parasites, c'est long)
                let attendu = self.debit.bit_s() as usize * TCF_MS / 1000;
                let fini = self.rx29.as_ref().is_some_and(|r| r.fini()) || self.tcf.len() >= attendu;
                if fini || self.n >= 4000 * MS {
                    self.rx29 = None;
                    self.tcf_ok = tcf_bon(&self.tcf, self.debit);
                    self.etape_ev(format!("TCF {} ({} bits)", if self.tcf_ok { "bon" } else { "mauvais" }, self.tcf.len()));
                    self.aller(Etat::AvantReponseTcf);
                } else if let Some(s) = self.salve_recue() {
                    // DCS répété (notre CFR perdu ?)
                    self.lire_dcs(&s);
                }
            }
            Etat::AvantReponseTcf => {
                if self.n >= PAUSE {
                    let fcf = if self.tcf_ok { t30::CFR } else { t30::FTT };
                    self.salve(&[(fcf, vec![])]);
                    self.aller(Etat::EnvoiReponseTcf);
                }
            }
            Etat::EnvoiReponseTcf => {
                if self.sortie_finie() {
                    self.sortie = Sortie::Silence;
                    self.recues.clear();
                    if self.tcf_ok {
                        self.rx29 = Some(Box::new(V29Rx::new(self.debit)));
                        self.t4 = Some(t4::Decodeur::default());
                        self.page.clear();
                        self.aller(Etat::RecoitPage);
                    } else {
                        self.aller(Etat::AttenteDcs);
                    }
                }
            }
            Etat::RecoitPage => {
                let (vue, fini) = self.rx29.as_ref().map_or((false, true), |r| (r.porteuse() || r.fini(), r.fini()));
                if fini {
                    self.rx29 = None;
                    self.etape_ev(format!("page reçue : {} lignes", self.page.len()));
                    self.aller(Etat::AttenteEop);
                } else if let Some(s) = self.salve_recue() {
                    if s.iter().any(|t| t.code() == t30::DCS) {
                        // l'appelant n'a pas eu notre CFR : il recommence
                        self.lire_dcs(&s);
                    } else if s.iter().any(|t| matches!(t.code(), t30::EOP | t30::MPS)) {
                        // fin de page annoncée avant qu'on ait vu la porteuse s'éteindre
                        self.rx29 = None;
                        self.etape_ev(format!("page reçue : {} lignes", self.page.len()));
                        self.aller(Etat::AvantMcf);
                    }
                } else if !vue && self.n >= 2 * T4 {
                    self.finir(false, "la page n'est jamais arrivée");
                }
            }
            Etat::AttenteEop => {
                if let Some(s) = self.salve_recue() {
                    if s.iter().any(|t| matches!(t.code(), t30::EOP | t30::MPS)) {
                        self.aller(Etat::AvantMcf);
                    }
                } else if self.n >= 2 * T4 {
                    self.finir(!self.page.is_empty(), "page reçue (fin de procédure absente)");
                }
            }
            Etat::AvantMcf => {
                if self.n >= PAUSE {
                    self.salve(&[(t30::MCF, vec![])]);
                    self.aller(Etat::EnvoiMcf);
                }
            }
            Etat::EnvoiMcf => {
                if self.sortie_finie() {
                    self.sortie = Sortie::Silence;
                    self.recues.clear();
                    self.aller(Etat::AttenteDcn);
                }
            }
            Etat::AttenteDcn => {
                if let Some(s) = self.salve_recue() {
                    if s.iter().any(|t| t.code() == t30::DCN) {
                        self.finir(true, "page reçue");
                    } else if s.iter().any(|t| matches!(t.code(), t30::EOP | t30::MPS)) {
                        // notre MCF perdu : on le redit
                        self.aller(Etat::AvantMcf);
                    }
                } else if self.n >= T4 {
                    self.finir(true, "page reçue");
                }
            }
        }
    }

    /// L'émetteur renonce : il le dit (DCN) avant de raccrocher.
    fn abandonner(&mut self, raison: &'static str) {
        self.abandon = Some(raison);
        self.salve(&[(t30::DCN | 1, vec![])]);
        self.aller(Etat::EnvoiDcn);
    }

    fn envoyer_dcs(&mut self) {
        let tsi = t30::fif_ident(&self.numero);
        self.salve(&[(t30::TSI | 1, tsi), (t30::DCS | 1, t30::fif_dcs(self.debit, self.fine))]);
        self.etape_ev(format!("DCS {} bit/s", self.debit.bit_s()));
        self.aller(Etat::AvantTcf);
    }

    /// Récepteur : un DCS (et son TSI) est arrivé ; on se prépare au TCF.
    fn lire_dcs(&mut self, s: &[Trame]) {
        if let Some(tsi) = s.iter().find(|t| t.code() == t30::TSI) {
            let c = t30::ident_lire(&tsi.fif);
            self.evenement(FaxEvenement::Correspondant(c));
        }
        let Some((debit, fine)) = s.iter().find(|t| t.code() == t30::DCS).and_then(|t| t30::dcs_lire(&t.fif)) else { return };
        self.debit = debit;
        self.fine = fine;
        self.evenement(FaxEvenement::Resolution { fine });
        self.etape_ev(format!("DCS reçu : {} bit/s", debit.bit_s()));
        self.rx29 = Some(Box::new(V29Rx::new(debit)));
        self.tcf.clear();
        self.aller(Etat::RecoitTcf);
    }
}

/// Le TCF est-il bon ? Au moins 60 % des 1,5 s reçus, et presque aucun 1
/// (une erreur peut en donner trois, par l'embrouilleur).
fn tcf_bon(bits: &[bool], debit: Debit) -> bool {
    let attendu = debit.bit_s() as usize * TCF_MS / 1000;
    if bits.len() < attendu * 6 / 10 {
        return false;
    }
    // le début (fin du segment 4) et la toute fin (porteuse qui s'éteint)
    // ne comptent pas
    let utiles = &bits[bits.len() / 10..bits.len() * 9 / 10];
    let uns = utiles.iter().filter(|&&b| b).count();
    uns <= utiles.len() / 500 + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::{LineParams, LineSim};

    fn page_test(n: usize) -> Vec<Vec<bool>> {
        (0..n).map(|y| (0..t4::LARGEUR).map(|x| (x + y) % 97 < 9 || (300..340).contains(&x)).collect()).collect()
    }

    /// Un appel de fax complet entre deux appareils, à travers une ligne
    /// dans chaque sens ; rend (fin émetteur, fin récepteur, page reçue,
    /// débit, étapes du récepteur).
    fn appel(ligne: Option<&str>, page: &[Vec<bool>], apres_ansam: bool) -> (FaxEvenement, FaxEvenement, Vec<Vec<bool>>, Debit, Vec<String>) {
        let mut e = Fax::emetteur(page, "0100000001", true);
        let mut r = Fax::recepteur("0100000002", apres_ansam);
        let p = ligne.map(|l| LineParams::preset(l).unwrap());
        let (mut l1, mut l2) = (p.clone().map(LineSim::new), p.map(LineSim::new));
        let (mut a, mut b) = (vec![0.0f32; 160], vec![0.0f32; 160]);
        let (mut fe, mut fr) = (None, None);
        let mut etapes = Vec::new();
        for _ in 0..(120 * 50) {
            // a : ce que l'émetteur a émis au tour précédent, à travers la ligne
            let mut va = vec![0.0f32; 160];
            let mut vb = vec![0.0f32; 160];
            match (&mut l1, &mut l2) {
                (Some(x), Some(y)) => {
                    x.process(&a, &mut va);
                    y.process(&b, &mut vb);
                }
                _ => {
                    va.copy_from_slice(&a);
                    vb.copy_from_slice(&b);
                }
            }
            e.process(&vb, &mut a);
            r.process(&va, &mut b);
            for ev in e.evenements() {
                if let FaxEvenement::Fin { .. } = ev {
                    fe = Some(ev);
                }
            }
            for ev in r.evenements() {
                match ev {
                    FaxEvenement::Fin { .. } => fr = Some(ev),
                    FaxEvenement::Etape(s) => etapes.push(s),
                    _ => {}
                }
            }
            if e.fini() && r.fini() {
                break;
            }
        }
        let rien = FaxEvenement::Fin { ok: false, raison: "pas fini".into() };
        (fe.unwrap_or(rien.clone()), fr.unwrap_or(rien), r.page().to_vec(), e.debit(), etapes)
    }

    #[test]
    fn appel_complet_sans_ligne() {
        let page = page_test(200);
        let (fe, fr, recue, debit, etapes) = appel(None, &page, false);
        assert_eq!(fe, FaxEvenement::Fin { ok: true, raison: "page transmise".into() }, "{etapes:?}");
        assert_eq!(fr, FaxEvenement::Fin { ok: true, raison: "page reçue".into() });
        assert_eq!(debit, Debit::B9600);
        assert_eq!(recue, page);
    }

    #[test]
    fn appel_complet_a_travers_une_ligne() {
        let page = page_test(120);
        let (fe, fr, recue, debit, etapes) = appel(Some("Urbaine"), &page, true);
        assert!(matches!(fe, FaxEvenement::Fin { ok: true, .. }), "{fe:?} {etapes:?}");
        assert!(matches!(fr, FaxEvenement::Fin { ok: true, .. }), "{fr:?}");
        assert_eq!(debit, Debit::B9600);
        assert_eq!(recue, page);
    }

    #[test]
    #[ignore = "mesure : FAX_LIGNE=Orage cargo test -p v34-core --lib fax_sur -- --ignored --nocapture"]
    fn fax_sur_une_ligne() {
        let l = std::env::var("FAX_LIGNE").unwrap_or_else(|_| "Longue distance".into());
        let page = page_test(150);
        let (fe, fr, recue, debit, etapes) = appel(Some(&l), &page, true);
        let justes = recue.iter().zip(&page).filter(|(a, b)| a == b).count();
        eprintln!("{l} : {fe:?} / {fr:?}, {} bit/s, {justes}/{} lignes justes sur {} reçues\n{etapes:?}", debit.bit_s(), page.len(), recue.len());
    }
}
