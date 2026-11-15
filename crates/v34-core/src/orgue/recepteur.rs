//! Le récepteur : échantillons → bits.
//!
//! 1. **Recherche** : la première note du motif est un signal périodique
//!    de période connue W₁ ; on guette l'autocorrélation normalisée au
//!    retard W₁ (insensible au niveau, au canal, au décalage de fréquence).
//! 2. **Calage** : filtre adapté (gabarit analytique du motif) autour du
//!    passage de la note 1 à la note 2 : l'instant exact du motif.
//! 3. **Mesure** : à la fin du motif, on vérifie les autres changements de
//!    note, puis, sur chaque note tenue : décalage de fréquence (rotation
//!    d'une période à la suivante), canal et bruit de chaque harmonique →
//!    chargement en bits pour l'émetteur d'en face.
//! 4. **Données** : le calendrier des symboles est connu (la mélodie) ;
//!    une TFD d'une période par symbole, au rang des harmoniques, après
//!    avoir retiré le décalage de fréquence (mélange complexe : pas besoin
//!    de transformée de Hilbert, l'image négative tombe hors des cases
//!    utiles). Décisions cohérentes avec suivi de la phase commune
//!    (boucle du 2ᵉ ordre) et du canal de chaque harmonique ; le symbole de
//!    référence de chaque note recale tout.

use std::f32::consts::PI;

use super::chargement::{base_donnees, Chargement};
use super::constellation::{gray, maq_decider, mdp, mdp_decider};
use super::emetteur::{motif_analytique, SymboleConnu};
use super::partition::{debut_motif, duree_motif, harmoniques, note_du_motif, periode, reference, Degre, Ordonnanceur, Symbole, ARTICULATION, CROCHE, DEGRES, FE, GARDE, MOTIF};
use super::{decalage_donnees, Duplex, Mode, Voix};
use crate::api::NOMINAL_RMS;
use crate::dsp::Cpx;

/// Fenêtre du filtre adapté autour d'un changement de note (échantillons).
const GABARIT: usize = 384;
/// Seuil de l'autocorrélation périodique pour la détection.
const SEUIL_PERIODICITE: f64 = 0.5;
/// Seuil du filtre adapté (corrélation normalisée).
const SEUIL_CALAGE: f32 = 0.4;
/// Recul de la fenêtre TFD par rapport au calage (échantillons) : laisse
/// un peu de garde aux précurseurs du canal.
pub const RECUL: usize = 12;

/// Résultat du calage sur le motif d'en face.
#[derive(Clone, Debug, PartialEq)]
pub struct Synchro {
    /// Début du motif (temps absolu du récepteur)
    pub debut_motif: usize,
    /// Corrélation du filtre adapté au calage
    pub correlation: f32,
    /// Changements de note retrouvés (sur `MOTIF.len() - 2`)
    pub verifies: usize,
    /// Décalage de fréquence estimé (Hz)
    pub decalage_hz: f32,
}

/// Mesures du canal sur le motif : par degré, par harmonique.
#[derive(Clone, Debug, Default)]
pub struct Mesure {
    pub canal: Vec<Vec<Cpx>>,
    /// SNR mesuré sur la moitié tenue de chaque note (bruit seul)
    pub snr_db: Vec<Vec<f32>>,
    /// Signal / (bruit + interférence) mesuré sur le tremblant (symboles
    /// connus découpés comme les données)
    pub sinr_db: Vec<Vec<f32>>,
    /// SINR lissé en fréquence (toutes les notes mises en commun), avec
    /// les planchers : c'est lui qui décide du chargement
    pub snr_lisse_db: Vec<Vec<f32>>,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub symboles: usize,
    pub effacements: usize,
    /// Somme des erreurs quadratiques / puissance attendue (pour le SNR mesuré en données)
    err: f64,
    sig: f64,
    /// Par degré, par harmonique : (Σ|erreur|², Σ|point|²) des décisions
    pub par_harmonique: Vec<Vec<(f64, f64)>>,
}

impl Stats {
    /// SNR moyen mesuré sur les décisions en données (dB).
    pub fn snr_donnees_db(&self) -> f32 {
        if self.err <= 0.0 {
            return 99.0;
        }
        (10.0 * (self.sig / self.err).log10()) as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Etat {
    Recherche,
    Calage { detection: usize },
    Mesure,
    Attente,
    Donnees,
    Echec,
}

/// Autocorrélation glissante au retard `w` sur `l` échantillons.
#[derive(Clone, Debug)]
struct Periodicite {
    w: usize,
    l: usize,
    sxy: f64,
    sxx: f64,
    syy: f64,
    duree: usize,
}

pub struct Recepteur {
    /// La voix écoutée (celle d'en face)
    pub voix: Voix,
    pub duplex: Duplex,
    pub mode: Mode,
    buf: Vec<f32>,
    base: usize,
    /// Temps absolu : échantillons reçus
    pub t: usize,
    pub etat: Etat,
    arme: usize,
    det: Periodicite,
    gabarit: Vec<Cpx>,
    journal: Vec<SymboleConnu>,
    pub synchro: Option<Synchro>,
    pub mesure: Option<Mesure>,
    chargement: Option<Chargement>,
    /// Marge (dB) du chargement
    pub marge_db: f32,
    /// Début des données d'en face (temps absolu du récepteur)
    pub t_donnees: usize,
    ordo: Ordonnanceur,
    prochain: Option<Symbole>,
    canal: Vec<Vec<Cpx>>,
    bases: Vec<Vec<Cpx>>,
    osc: Vec<Vec<Cpx>>,
    idx: Vec<u32>,
    phi: f64,
    t_phi: usize,
    omega: f64,
    bits: Vec<u8>,
    pub stats: Stats,
}

impl Recepteur {
    pub fn new(voix: Voix, duplex: Duplex, mode: Mode) -> Self {
        let w1 = periode(voix, note_du_motif(0).expect("le motif commence par une note seule"));
        let (gabarit, journal) = motif_analytique(voix, duplex, mode);
        Self {
            voix,
            duplex,
            mode,
            buf: Vec::new(),
            base: 0,
            t: 0,
            etat: Etat::Recherche,
            arme: 0,
            det: Periodicite { w: w1, l: 2 * w1, sxy: 0.0, sxx: 0.0, syy: 0.0, duree: 0 },
            gabarit,
            journal,
            synchro: None,
            mesure: None,
            chargement: None,
            marge_db: 3.0,
            t_donnees: 0,
            ordo: Ordonnanceur::new(voix),
            prochain: None,
            canal: Vec::new(),
            bases: Vec::new(),
            osc: DEGRES.iter().map(|&d| osc(periode(voix, d))).collect(),
            idx: Vec::new(),
            phi: 0.0,
            t_phi: 0,
            omega: 0.0,
            bits: Vec::new(),
            stats: Stats::default(),
        }
    }

    /// N'écoute pas avant l'instant `t` (on se tait sur notre propre écho).
    pub fn armer_a(&mut self, t: usize) {
        self.arme = t;
    }

    /// Chargement choisi pour l'émetteur d'en face (après la mesure).
    pub fn chargement(&self) -> Option<&Chargement> {
        self.chargement.as_ref()
    }

    /// Bits reçus depuis le dernier appel.
    pub fn prendre(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.bits)
    }

    #[inline]
    fn r(&self, t: usize) -> f32 {
        self.buf[t - self.base]
    }

    pub fn traiter(&mut self, x: &[f32]) {
        for &v in x {
            self.buf.push(v);
            self.t += 1;
            if self.etat == Etat::Recherche {
                self.detecter();
            }
        }
        loop {
            let avant = self.etat;
            match self.etat {
                Etat::Recherche | Etat::Echec => {}
                Etat::Calage { detection } => self.caler(detection),
                Etat::Mesure => self.mesurer(),
                Etat::Attente => {
                    if self.t >= self.t_donnees {
                        self.etat = Etat::Donnees;
                    }
                }
                Etat::Donnees => self.demoduler(),
            }
            if self.etat == avant {
                break;
            }
        }
        self.elaguer();
    }

    fn elaguer(&mut self) {
        let garder = match self.etat {
            Etat::Recherche => self.t.saturating_sub(8000),
            Etat::Calage { .. } | Etat::Mesure => return,
            Etat::Attente => self.t_donnees.saturating_sub(64).min(self.t.saturating_sub(8000)),
            Etat::Donnees => match &self.prochain {
                Some(s) => (self.t_donnees + s.fenetre).saturating_sub(RECUL + 64),
                None => self.t.saturating_sub(8000),
            },
            Etat::Echec => self.t,
        }
        .min(self.t);
        if garder > self.base + 16000 {
            self.buf.drain(..garder - self.base);
            self.base = garder;
        }
    }

    // ───────────── 1. recherche ─────────────

    fn detecter(&mut self) {
        let t = self.t - 1;
        let (w, l) = (self.det.w, self.det.l);
        if t < self.arme + w + l || t < self.base + w + l {
            return;
        }
        let (x, y) = (self.r(t) as f64, self.r(t - w) as f64);
        let (xo, yo) = (self.r(t - l) as f64, self.r(t - l - w) as f64);
        let d = &mut self.det;
        d.sxy += x * y - xo * yo;
        d.sxx += x * x - xo * xo;
        d.syy += y * y - yo * yo;
        if t == self.arme + w + l {
            // premier passage : sommes exactes
            let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
            for i in 0..l {
                let (a, b) = (self.buf[t - i - self.base] as f64, self.buf[t - i - w - self.base] as f64);
                sxy += a * b;
                sxx += a * a;
                syy += b * b;
            }
            self.det.sxy = sxy;
            self.det.sxx = sxx;
            self.det.syy = syy;
        }
        let d = &mut self.det;
        let plancher = (NOMINAL_RMS as f64 * 1e-3).powi(2) * l as f64;
        let rho = d.sxy / (d.sxx * d.syy).sqrt().max(1e-30);
        if rho > SEUIL_PERIODICITE && d.sxx > plancher {
            d.duree += 1;
        } else {
            d.duree = 0;
        }
        if d.duree >= w {
            d.duree = 0;
            self.etat = Etat::Calage { detection: t };
        }
    }

    // ───────────── 2. calage ─────────────

    /// Corrélation normalisée entre le signal reçu à partir de `t` et le
    /// gabarit à partir de `g`, sur `n` échantillons.
    fn correler(&self, t: usize, g: usize, n: usize) -> f32 {
        let mut acc = Cpx::ZERO;
        let (mut er, mut eg) = (0.0f32, 0.0f32);
        for i in 0..n {
            let x = self.r(t + i);
            let gi = self.gabarit[g + i];
            acc += gi.conj() * x;
            er += x * x;
            eg += gi.norm_sqr();
        }
        acc.abs() / (er * eg / 2.0).sqrt().max(1e-20)
    }

    /// Corrélation au changement de note `i → i+1` du motif supposé
    /// commencer à `t_motif`.
    fn correler_passage(&self, t_motif: usize, i: usize) -> f32 {
        let c = debut_motif(i + 1);
        self.correler(t_motif + c - GABARIT / 2, c - GABARIT / 2, GABARIT)
    }

    fn caler(&mut self, detection: usize) {
        let w1 = self.det.w;
        // τ : instant du passage de la note 1 à la note 2 (temps du récepteur)
        let lo = detection.saturating_sub(4 * w1).max(self.base + GABARIT / 2).max(CROCHE);
        let hi = detection + CROCHE;
        if self.t < hi + GABARIT / 2 + 1 {
            return;
        }
        let mut meilleur = (0.0f32, 0usize);
        for tau in lo..=hi {
            let c = self.correler(tau - GABARIT / 2, CROCHE - GABARIT / 2, GABARIT);
            if c > meilleur.0 {
                meilleur = (c, tau);
            }
        }
        if meilleur.0 < SEUIL_CALAGE {
            self.recommencer(detection + w1);
            return;
        }
        let t_motif = meilleur.1 - CROCHE;
        self.synchro = Some(Synchro { debut_motif: t_motif, correlation: meilleur.0, verifies: 0, decalage_hz: 0.0 });
        self.etat = Etat::Mesure;
    }

    fn recommencer(&mut self, des: usize) {
        self.det.duree = 0;
        self.arme = des.max(self.arme);
        self.det.sxy = 0.0;
        self.det.sxx = 0.0;
        self.det.syy = 0.0;
        self.synchro = None;
        self.etat = Etat::Recherche;
        // les sommes seront recalculées exactement au réarmement
        if self.t > self.arme + self.det.w + self.det.l {
            self.arme = self.t - self.det.w - self.det.l;
        }
    }

    // ───────────── 3. mesure ─────────────

    /// TFD d'une période à partir de `u`, aux rangs `rangs`, après mélange
    /// par e^{-jΦ(t)} (Φ(t) = phi0 + ω·(t - u)). Normalisée : une
    /// harmonique Re{c·e^{jωt}} donne c (à la rotation de phase près).
    fn tfd(&self, u: usize, w: usize, rangs: &[usize], osc: &[Cpx], phi0: f64, omega: f64) -> Vec<Cpx> {
        let mut z = Vec::with_capacity(w);
        let rot0 = Cpx::from_polar(1.0, -(phi0.rem_euclid(2.0 * std::f64::consts::PI)) as f32);
        let pas = Cpx::from_polar(1.0, -omega as f32);
        let mut rot = rot0;
        for n in 0..w {
            z.push(rot * self.r(u + n));
            rot = rot * pas;
        }
        let g = 2.0 / w as f32;
        rangs
            .iter()
            .map(|&h| {
                let mut acc = Cpx::ZERO;
                let mut k = 0usize;
                for zn in &z {
                    acc += *zn * osc[k].conj();
                    k += h;
                    if k >= w {
                        k -= w;
                    }
                }
                acc * g
            })
            .collect()
    }

    fn mesurer(&mut self) {
        let s = self.synchro.clone().unwrap();
        let fin = s.debut_motif + duree_motif();
        if self.t < fin + GABARIT {
            return;
        }
        // Vérifie les autres changements de note
        let verifies = (1..MOTIF.len() - 1)
            .filter(|&i| (0..5).map(|d| self.correler_passage((s.debut_motif + d).saturating_sub(2), i)).fold(0.0f32, f32::max) > SEUIL_CALAGE)
            .count();
        if 2 * verifies < MOTIF.len() - 2 {
            let d = s.debut_motif + CROCHE;
            self.recommencer(d);
            return;
        }
        // Fenêtres d'une période dans la moitié tenue de chaque note seule
        let fenetres = |i: usize, d: Degre| -> Vec<usize> {
            let c = MOTIF[i].1;
            let w = periode(self.voix, d);
            let debut = s.debut_motif + debut_motif(i) + ARTICULATION + GARDE;
            let fin = s.debut_motif + debut_motif(i) + c * CROCHE / 2;
            (0..).map(|p| debut + p * w).take_while(|u| u + w <= fin).collect()
        };
        // Décalage de fréquence : rotation d'une période à la suivante
        let (mut num, mut den) = (0.0f64, 0.0f64);
        let notes: Vec<(usize, Degre)> = (0..MOTIF.len()).filter_map(|i| note_du_motif(i).map(|d| (i, d))).collect();
        for &(i, d) in &notes {
            let w = periode(self.voix, d);
            let rangs = harmoniques(self.voix, self.duplex, d);
            let o = &self.osc[d.index()];
            let x: Vec<Vec<Cpx>> = fenetres(i, d).iter().map(|&u| self.tfd(u, w, &rangs, o, 0.0, 0.0)).collect();
            let mut rot = Cpx::ZERO;
            for p in 1..x.len() {
                for (a, b) in x[p].iter().zip(&x[p - 1]) {
                    rot += *a * b.conj();
                }
            }
            let df = rot.arg() as f64 * FE as f64 / (2.0 * std::f64::consts::PI * w as f64);
            num += df * rot.abs() as f64;
            den += rot.abs() as f64;
        }
        let df = if den > 0.0 { num / den } else { 0.0 };
        let omega = 2.0 * std::f64::consts::PI * df / FE as f64;
        // TFD d'une fenêtre, phases ramenées à l'origine de la note
        let tfd_note = |u: usize, d: Degre, t_note: usize| -> Vec<Cpx> {
            let w = periode(self.voix, d);
            let rangs = harmoniques(self.voix, self.duplex, d);
            let o = &self.osc[d.index()];
            let x = self.tfd(u, w, &rangs, o, omega * u as f64, omega);
            let decal = (u - t_note) % w;
            x.iter().zip(&rangs).map(|(v, &h)| *v * o[(h * decal) % w].conj()).collect()
        };
        // Canal et bruit de chaque harmonique, sur la moitié tenue
        let n = DEGRES.len();
        let mut m = Mesure { canal: vec![Vec::new(); n], snr_db: vec![Vec::new(); n], sinr_db: vec![Vec::new(); n], snr_lisse_db: vec![Vec::new(); n] };
        for &(i, d) in &notes {
            let ref_c = reference(self.voix, self.duplex, d);
            let t_note = s.debut_motif + debut_motif(i);
            let y: Vec<Vec<Cpx>> = fenetres(i, d).iter().map(|&u| tfd_note(u, d, t_note)).collect();
            let p = y.len();
            let mut canal = Vec::new();
            let mut snr = Vec::new();
            for k in 0..ref_c.len() {
                let mut moy = Cpx::ZERO;
                for v in &y {
                    moy += v[k];
                }
                moy *= 1.0 / p as f32;
                // variance robuste (médiane) : les clics n'y pèsent pas
                let mut e: Vec<f32> = y.iter().map(|v| (v[k] - moy).norm_sqr()).collect();
                e.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let var = (e[e.len() / 2] / std::f32::consts::LN_2).max(1e-20) * p as f32 / (p as f32 - 1.0).max(1.0);
                canal.push(moy * ref_c[k].conj() * (1.0 / ref_c[k].norm_sqr()));
                snr.push(10.0 * (moy.norm_sqr() / var).log10());
            }
            m.canal[d.index()] = canal;
            m.snr_db[d.index()] = snr;
        }
        // Signal / (bruit + interférence) sur le tremblant : des symboles
        // connus, découpés exactement comme les données (garde, transition).
        // Par note et par harmonique : (reçu, émis) de chaque symbole, phase
        // commune retirée.
        let mut obs: Vec<Vec<Vec<(Cpx, Cpx)>>> = vec![Vec::new(); n];
        for sc in &self.journal {
            let d = sc.degre;
            let y = tfd_note(s.debut_motif + sc.fenetre - RECUL, d, s.debut_motif + sc.debut_note);
            let att: Vec<Cpx> = m.canal[d.index()].iter().zip(&sc.c).map(|(h, c)| *h * *c).collect();
            let mut g = Cpx::ZERO;
            for (v, a) in y.iter().zip(&att) {
                g += *v * a.conj();
            }
            let r = Cpx::from_polar(1.0, -g.arg());
            let o = &mut obs[d.index()];
            if o.is_empty() {
                *o = vec![Vec::new(); att.len()];
            }
            for k in 0..att.len() {
                o[k].push((y[k] * r, sc.c[k]));
            }
        }
        // Le récepteur de données suit le canal *effectif* (la part de
        // l'interférence corrélée au symbole courant s'y fond) : on ajuste
        // de même un gain par harmonique, puis on mesure le reste.
        let err: Vec<Vec<Vec<f32>>> = obs
            .iter()
            .map(|o| {
                o.iter()
                    .map(|v| {
                        let (mut num, mut den) = (Cpx::ZERO, 0.0f32);
                        for (y, c) in v {
                            num += *y * c.conj();
                            den += c.norm_sqr();
                        }
                        let h = num * (1.0 / den.max(1e-30));
                        let ajuste = (v.len() as f32 / (v.len() as f32 - 1.0).max(1.0)).max(1.0);
                        v.iter().map(|(y, c)| (*y - h * *c).norm_sqr() / (h * *c).norm_sqr().max(1e-30) * ajuste).collect()
                    })
                    .collect()
            })
            .collect();
        // (fréquence, SINR par unité de puissance émise et par échantillon, dB)
        let mut points: Vec<(f32, f32)> = Vec::new();
        for &(_, d) in &notes {
            let w = periode(self.voix, d);
            let rangs = harmoniques(self.voix, self.duplex, d);
            let ref_c = reference(self.voix, self.duplex, d);
            m.sinr_db[d.index()] = err[d.index()]
                .iter()
                .enumerate()
                .map(|(k, e)| {
                    // médiane : un clic sur un symbole n'y pèse pas
                    let mut e = e.clone();
                    e.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let v = (e[e.len() / 2] / std::f32::consts::LN_2).max(1e-12);
                    let sinr = -10.0 * v.log10();
                    // le bruit d'une case de TFD vaut 4σ²/W : on s'en affranchit
                    points.push((rangs[k] as f32 * FE / w as f32, sinr - 10.0 * (ref_c[k].norm_sqr() * 4.0 / w as f32).log10()));
                    sinr
                })
                .collect();
        }
        // En pleine bande, l'écho résiduel de notre annuleur (appris sur un
        // motif court, dans le bruit de ligne) est de l'ordre du bruit : le
        // motif d'en face ne le voit pas (on se taisait).
        let plancher = match self.duplex {
            Duplex::Duo => PLANCHER_DB,
            Duplex::PleineBande => PLANCHER_DB - 3.0,
        };
        let penalite = match self.duplex {
            Duplex::Duo => 0.0,
            Duplex::PleineBande => PENALITE_ECHO_DB,
        };
        // Bruit blanc, ligne lisse : on met en commun les mesures voisines
        // (médiane à ±80 Hz, toutes notes confondues)
        for d in DEGRES {
            let w = periode(self.voix, d);
            let ref_c = reference(self.voix, self.duplex, d);
            m.snr_lisse_db[d.index()] = harmoniques(self.voix, self.duplex, d)
                .iter()
                .enumerate()
                .map(|(k, &h)| {
                    let f = h as f32 * FE / w as f32;
                    let mut v: Vec<f32> = points.iter().filter(|(g, _)| (g - f).abs() <= 80.0).map(|(_, s)| *s).collect();
                    if v.is_empty() {
                        return -100.0;
                    }
                    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let sinr = v[v.len() / 2] + 10.0 * (ref_c[k].norm_sqr() * 4.0 / w as f32).log10() - penalite;
                    -10.0 * (10f32.powf(-sinr / 10.0) + 10f32.powf(-plancher / 10.0)).log10()
                })
                .collect();
        }
        let ch = Chargement::depuis_snr(self.voix, self.duplex, self.mode, &m.snr_lisse_db, self.marge_db);
        self.bases = DEGRES.iter().map(|&d| base_donnees(&ch, d)).collect();
        self.chargement = Some(ch);
        self.canal = m.canal.clone();
        self.mesure = Some(m);
        self.omega = omega;
        self.phi = 0.0;
        self.t_phi = 0;
        self.synchro = Some(Synchro { verifies, decalage_hz: df as f32, ..s.clone() });
        self.t_donnees = s.debut_motif + duree_motif() + decalage_donnees(self.voix);
        self.prochain = self.ordo.next();
        self.etat = Etat::Attente;
    }

    /// Remplace le chargement (s'il a été modifié avant d'être envoyé).
    pub fn imposer_chargement(&mut self, ch: Chargement) {
        self.bases = DEGRES.iter().map(|&d| base_donnees(&ch, d)).collect();
        self.chargement = Some(ch);
    }

    // ───────────── 4. données ─────────────

    fn demoduler(&mut self) {
        while let Some(s) = self.prochain {
            let u = self.t_donnees + s.fenetre - RECUL;
            if self.t < u + s.periode {
                return;
            }
            self.symbole(&s, u);
            self.prochain = self.ordo.next();
        }
    }

    fn symbole(&mut self, s: &Symbole, u: usize) {
        let d = s.degre;
        let w = s.periode;
        let rangs = harmoniques(self.voix, self.duplex, d);
        let phi_u = self.phi + self.omega * (u - self.t_phi) as f64;
        self.phi = phi_u;
        self.t_phi = u;
        let o = self.osc[d.index()].clone();
        let x = self.tfd(u, w, &rangs, &o, phi_u, self.omega);
        let decal = (u - (self.t_donnees + s.debut_note)) % w;
        let y: Vec<Cpx> = x.iter().zip(&rangs).map(|(v, &h)| *v * o[(h * decal) % w].conj()).collect();
        let base = self.bases[d.index()].clone();
        let bits = self.chargement.as_ref().unwrap().bits[d.index()].clone();
        let canal = &mut self.canal[d.index()];
        let attendu: Vec<Cpx> = canal.iter().zip(&base).map(|(h, b)| *h * *b).collect();
        let p_att: f32 = attendu.iter().map(|a| a.norm_sqr()).sum();
        let p_recu: f32 = y.iter().zip(&base).filter(|(_, b)| b.norm_sqr() > 0.0).map(|(v, _)| v.norm_sqr()).sum();
        let efface = p_recu < 0.1 * p_att || p_recu > 10.0 * p_att;
        if efface {
            self.stats.effacements += 1;
        }
        if s.reference {
            let mut g = Cpx::ZERO;
            for (v, a) in y.iter().zip(&attendu) {
                g += *v * a.conj();
            }
            if !efface {
                let th = g.arg();
                self.phi += th as f64;
                let r = Cpx::from_polar(1.0, -th);
                for i in 0..rangs.len() {
                    if base[i].norm_sqr() > 0.0 {
                        let est = y[i] * r * (1.0 / base[i].norm_sqr()) * base[i].conj();
                        canal[i] = canal[i] + (est - canal[i]) * 0.5;
                    }
                }
            }
            self.idx = vec![0; rangs.len()];
            return;
        }
        // Données : décisions, phase commune, décisions finales
        let actif: Vec<bool> = base.iter().map(|b| b.norm_sqr() > 0.0).collect();
        let z: Vec<Cpx> = (0..rangs.len())
            .map(|i| {
                if actif[i] {
                    let a = attendu[i];
                    y[i] * a.conj() * (1.0 / a.norm_sqr().max(1e-30))
                } else {
                    Cpx::ZERO
                }
            })
            .collect();
        let mode = self.mode;
        let decider = |i: usize, zi: Cpx| -> (Cpx, u32) {
            let b = bits[i];
            if b == 0 {
                return (Cpx::new(1.0, 0.0), 0);
            }
            match mode {
                Mode::Joli => {
                    let k = mdp_decider(b, zi);
                    (mdp(b, k), k)
                }
                Mode::Rapide => {
                    let (v, p) = maq_decider(b, zi);
                    (p, v)
                }
            }
        };
        // Phase commune : décisions provisoires, écart moyen pondéré par la
        // puissance attendue (les pilotes et les harmoniques sans données y
        // comptent comme des décisions sûres)
        let mut g = Cpx::ZERO;
        for i in 0..rangs.len() {
            if actif[i] {
                let (p, _) = decider(i, z[i]);
                g += z[i] * p.conj() * attendu[i].norm_sqr();
            }
        }
        let th = g.arg();
        let r = Cpx::from_polar(1.0, -th);
        if !efface {
            self.phi += 0.5 * th as f64;
            self.omega += 0.02 * th as f64 / (w + GARDE) as f64;
        }
        let mut err = 0.0f64;
        let mut sig = 0.0f64;
        for i in 0..rangs.len() {
            if !actif[i] {
                continue;
            }
            let zi = z[i] * r;
            let (p, v) = decider(i, zi);
            let b = bits[i];
            if b > 0 {
                let val = match mode {
                    Mode::Joli => {
                        let m = 1u32 << b;
                        let delta = (v + m - self.idx[i]) % m;
                        self.idx[i] = v;
                        gray(delta)
                    }
                    Mode::Rapide => v,
                };
                for j in (0..b).rev() {
                    self.bits.push(((val >> j) & 1) as u8);
                }
            }
            let e = zi - p;
            let a2 = attendu[i].norm_sqr() as f64;
            err += e.norm_sqr() as f64 * a2;
            sig += p.norm_sqr() as f64 * a2;
            if !efface {
                let ph = &mut self.stats.par_harmonique;
                if ph.len() < DEGRES.len() {
                    ph.resize(DEGRES.len(), Vec::new());
                }
                let v = &mut ph[d.index()];
                if v.len() < rangs.len() {
                    v.resize(rangs.len(), (0.0, 0.0));
                }
                v[i].0 += e.norm_sqr() as f64;
                v[i].1 += p.norm_sqr() as f64;
                // LMS normalisé du canal de cette harmonique
                let bn = base[i].norm_sqr();
                let corr = (y[i] * r - attendu[i] * p) * p.conj() * base[i].conj() * (1.0 / bn);
                canal[i] = canal[i] + corr * 0.05;
            }
        }
        if !efface {
            self.stats.err += err;
            self.stats.sig += sig;
        }
        self.stats.symboles += 1;
    }
}

/// Plancher de l'implémentation (dB) : suivi de phase et de canal.
const PLANCHER_DB: f32 = 42.0;
/// Pleine bande : pénalité pour l'écho résiduel, que le motif d'en face
/// ne voit pas (dB).
const PENALITE_ECHO_DB: f32 = 2.0;

fn osc(w: usize) -> Vec<Cpx> {
    (0..w).map(|n| Cpx::from_polar(1.0, 2.0 * PI * n as f32 / w as f32)).collect()
}
