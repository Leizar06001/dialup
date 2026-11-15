//! Modulation et démodulation V.23, caractères asynchrones 7 bits + parité
//! paire.
//!
//! Émission : FSK à phase continue, avec des changements de fréquence placés
//! à la fraction d'échantillon près (à 1200 bit/s, un bit ne dure que 6,67
//! échantillons : arrondir à l'échantillon ferait trembler les fronts de 15 %).
//!
//! Réception : filtre passe-bande (qui retire aussi l'écho de notre propre
//! émission, dans l'autre voie), puis discriminateur de fréquence : le signal
//! est ramené autour de zéro par un mélangeur au milieu des deux fréquences,
//! et l'on mesure de combien sa phase tourne en quelques échantillons. Le
//! résultat vaut à peu près −1 pour un 1 (« mark », la fréquence basse) et +1
//! pour un 0 (« space »), quel que soit le niveau reçu : la pente des aigus
//! d'une ligne longue ne déséquilibre pas les deux fréquences. Le récepteur
//! de caractères attend le front du bit de départ, puis moyenne le milieu de
//! chaque bit.

use std::collections::VecDeque;
use std::f64::consts::{PI, TAU};

use crate::api::{NOMINAL_RMS, SAMPLE_RATE};
use crate::dsp::Cpx;

const FS: f64 = SAMPLE_RATE as f64;

/// Amplitude crête d'émission (niveau nominal).
pub const AMPLITUDE: f32 = NOMINAL_RMS * std::f32::consts::SQRT_2;

/// Une des deux voies du V.23.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Voie {
    /// 1200 bit/s, du serveur vers le Minitel
    Aller,
    /// 75 bit/s, du clavier du Minitel vers le serveur
    Retour,
}

impl Voie {
    pub fn bauds(self) -> f64 {
        match self {
            Voie::Aller => 1200.0,
            Voie::Retour => 75.0,
        }
    }

    /// (fréquence du 1 « mark », fréquence du 0 « space »)
    pub fn frequences(self) -> (f64, f64) {
        match self {
            Voie::Aller => (1300.0, 2100.0),
            Voie::Retour => (390.0, 450.0),
        }
    }

    /// Bit/s utiles en caractères par seconde (10 bits par caractère).
    pub fn caracteres_par_s(self) -> f64 {
        self.bauds() / 10.0
    }
}

// ───────────────────────── filtres ─────────────────────────

/// Filtre à réponse impulsionnelle finie (coefficients réels).
#[derive(Clone, Debug)]
pub struct Fir {
    h: Vec<f32>,
    /// Tampon doublé : la fenêtre courante est toujours contiguë
    buf: Vec<f32>,
    pos: usize,
}

impl Fir {
    /// Passe-bande `f1`..`f2` Hz (`f1` = 0 : passe-bas), `n` coefficients,
    /// sinus cardinal fenêtré par Blackman (≈ 74 dB de réjection).
    pub fn passe_bande(f1: f64, f2: f64, n: usize) -> Fir {
        let m = (n - 1) as f64 / 2.0;
        let sinc = |fc: f64, t: f64| if t == 0.0 { 2.0 * fc / FS } else { (TAU * fc / FS * t).sin() / (PI * t) };
        let h: Vec<f32> = (0..n)
            .map(|k| {
                let t = k as f64 - m;
                let w = 0.42 - 0.5 * (TAU * k as f64 / (n - 1) as f64).cos() + 0.08 * (2.0 * TAU * k as f64 / (n - 1) as f64).cos();
                (w * (sinc(f2, t) - if f1 > 0.0 { sinc(f1, t) } else { 0.0 })) as f32
            })
            .collect();
        Fir { buf: vec![0.0; 2 * n], h, pos: 0 }
    }

    pub fn passe_bas(fc: f64, n: usize) -> Fir {
        Fir::passe_bande(0.0, fc, n)
    }

    #[inline]
    pub fn push(&mut self, x: f32) -> f32 {
        let n = self.h.len();
        self.buf[self.pos] = x;
        self.buf[self.pos + n] = x;
        self.pos = (self.pos + 1) % n;
        // buf[pos..pos+n] : du plus ancien au plus récent
        let w = &self.buf[self.pos..self.pos + n];
        self.h.iter().rev().zip(w).map(|(h, x)| h * x).sum()
    }

    /// Gain (module) à la fréquence `f` Hz.
    pub fn gain(&self, f: f64) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (k, &h) in self.h.iter().enumerate() {
            let a = TAU * f / FS * k as f64;
            re += h as f64 * a.cos();
            im -= h as f64 * a.sin();
        }
        (re * re + im * im).sqrt() as f32
    }
}

// ───────────────────────── émission ─────────────────────────

/// Les dix bits d'un caractère, dans l'ordre d'émission : départ, b0..b6,
/// parité paire, arrêt.
pub fn trame(c: u8) -> [bool; 10] {
    let c = c & 0x7F;
    let mut b = [false; 10];
    for i in 0..7 {
        b[1 + i] = (c >> i) & 1 == 1;
    }
    b[8] = c.count_ones() % 2 == 1; // le total des uns devient pair
    b[9] = true;
    b
}

/// Modulateur d'une voie, avec sa file de caractères. Sans rien à dire, la
/// porteuse reste sur le 1 (repos).
#[derive(Clone, Debug)]
pub struct Emetteur {
    voie: Voie,
    phase: f64,
    /// Position dans le bit courant (fraction de bit)
    frac: f64,
    pas: f64,
    bit: bool,
    bits: VecDeque<bool>,
    file: VecDeque<u8>,
    pub porteuse: bool,
    amplitude: f32,
}

impl Emetteur {
    pub fn new(voie: Voie) -> Emetteur {
        Emetteur { voie, phase: 0.0, frac: 0.0, pas: voie.bauds() / FS, bit: true, bits: VecDeque::new(), file: VecDeque::new(), porteuse: false, amplitude: AMPLITUDE }
    }

    /// Caractères à envoyer (7 bits ; le 8e est ignoré).
    pub fn envoyer(&mut self, octets: &[u8]) {
        self.file.extend(octets.iter().map(|o| o & 0x7F));
    }

    /// Caractères en attente (y compris celui en cours d'émission).
    pub fn en_attente(&self) -> usize {
        self.file.len() + usize::from(!self.bits.is_empty())
    }

    pub fn vide(&self) -> bool {
        self.file.is_empty() && self.bits.is_empty()
    }

    /// Un caractère part en ce moment.
    pub fn emet(&self) -> bool {
        !self.bits.is_empty()
    }

    fn bit_suivant(&mut self) -> bool {
        if self.bits.is_empty() {
            if let Some(c) = self.file.pop_front() {
                self.bits.extend(trame(c));
            }
        }
        self.bits.pop_front().unwrap_or(true)
    }

    fn frequence(&self, bit: bool) -> f64 {
        let (m, s) = self.voie.frequences();
        if bit {
            m
        } else {
            s
        }
    }

    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        if !self.porteuse {
            // la porteuse repart sur un bit entier
            self.frac = 0.0;
            self.bit = true;
            return 0.0;
        }
        let y = self.amplitude * self.phase.cos() as f32;
        // Phase pendant l'intervalle [n, n+1) : le front de bit tombe à la
        // fraction `a` de l'intervalle, les deux fréquences s'y partagent.
        let f0 = self.frequence(self.bit);
        let fin = self.frac + self.pas;
        let dphi = if fin >= 1.0 {
            let a = (1.0 - self.frac) / self.pas;
            self.bit = self.bit_suivant();
            self.frac = fin - 1.0;
            TAU / FS * (f0 * a + self.frequence(self.bit) * (1.0 - a))
        } else {
            self.frac = fin;
            TAU / FS * f0
        };
        self.phase = (self.phase + dphi) % TAU;
        y
    }
}

/// Générateur de l'ANS (V.25) : 2100 Hz pur, sans modulation ni inversion.
#[derive(Clone, Debug, Default)]
pub struct Ans {
    phase: f64,
}

impl Ans {
    #[inline]
    pub fn next_sample(&mut self) -> f32 {
        let y = AMPLITUDE * self.phase.cos() as f32;
        self.phase = (self.phase + TAU * 2100.0 / FS) % TAU;
        y
    }
}

// ───────────────────────── réception ─────────────────────────

/// Discriminateur de fréquence d'une voie : passe-bande, mélangeur au milieu
/// des deux fréquences, passe-bas, rotation de phase sur `retard`
/// échantillons.
#[derive(Clone, Debug)]
pub struct Discriminateur {
    bp: Fir,
    phase: f64,
    pas: f64,
    lp_i: Fir,
    lp_q: Fir,
    retard: VecDeque<Cpx>,
    echelle: f32,
    /// Puissance du signal dans la voie (|z|² lissé ; ton d'amplitude A : A²/4)
    pub puissance: f32,
    lissage: f32,
    /// La même, lissée sur à peu près un bit : tombe tout de suite quand la
    /// porteuse s'interrompt
    pub rapide: f32,
    lissage_rapide: f32,
    /// |z|² de l'échantillon courant
    pub instant: f32,
}

impl Discriminateur {
    pub fn new(voie: Voie) -> Discriminateur {
        let (m, s) = voie.frequences();
        let centre = (m + s) / 2.0;
        let excursion = (s - m) / 2.0;
        // (bandes et retards choisis pour chaque voie, voir l'en-tête)
        let (bp, lp, retard, tau_ms, tau_rapide_ms) = match voie {
            Voie::Aller => (Fir::passe_bande(820.0, 2800.0, 95), Fir::passe_bas(1100.0, 31), 2usize, 8.0, 1.0),
            Voie::Retour => (Fir::passe_bande(200.0, 650.0, 127), Fir::passe_bas(150.0, 63), 16usize, 20.0, 8.0),
        };
        Discriminateur {
            bp,
            phase: 0.0,
            pas: TAU * centre / FS,
            lp_i: lp.clone(),
            lp_q: lp,
            retard: VecDeque::from(vec![Cpx::ZERO; retard]),
            echelle: (FS / (TAU * excursion * retard as f64)) as f32,
            puissance: 0.0,
            lissage: (1.0 / (tau_ms * FS / 1000.0)) as f32,
            rapide: 0.0,
            instant: 0.0,
            lissage_rapide: (1.0 / (tau_rapide_ms * FS / 1000.0)) as f32,
        }
    }

    /// Un échantillon reçu → écart de fréquence normalisé (≈ −1 : 1, ≈ +1 : 0).
    #[inline]
    pub fn push(&mut self, x: f32) -> f32 {
        let v = self.bp.push(x);
        let (s, c) = self.phase.sin_cos();
        self.phase = (self.phase + self.pas) % TAU;
        let z = Cpx::new(self.lp_i.push(v * c as f32), self.lp_q.push(-v * s as f32));
        self.puissance += self.lissage * (z.norm_sqr() - self.puissance);
        self.instant = z.norm_sqr();
        self.rapide += self.lissage_rapide * (self.instant - self.rapide);
        let ancien = self.retard.pop_front().unwrap_or(Cpx::ZERO);
        self.retard.push_back(z);
        let rot = z * ancien.conj();
        if rot.norm_sqr() < 1e-14 {
            return 0.0;
        }
        rot.im.atan2(rot.re) * self.echelle
    }
}

/// Ce que livre le récepteur de caractères.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recu {
    Caractere(u8),
    /// Caractère dont la parité est fausse
    Parite(u8),
    /// Pas de bit d'arrêt (parasite, ou désynchronisation)
    Cadrage,
}

#[derive(Clone, Copy, Debug)]
enum EtatRx {
    /// Après un trou : on attend au moins un bit de repos (1)
    AttenteRepos,
    /// Repos : on guette le front du bit de départ
    Repos,
    /// Un caractère commence à l'instant `t0`
    Caractere { t0: f64 },
    /// Après une erreur de cadrage, dans un flot continu : on cherche, après
    /// `depuis`, un front suivi de trois caractères valides
    Recalage { depuis: f64 },
}

/// Un caractère lu dans l'historique.
#[derive(Clone, Copy, Debug)]
struct Lu {
    c: u8,
    depart: bool,
    parite: bool,
    arret: bool,
}

/// Récepteur de caractères asynchrones (départ, 7 bits, parité paire, arrêt).
///
/// Il travaille sur un historique de la sortie du discriminateur : après une
/// erreur de cadrage au milieu d'un flot de caractères collés les uns aux
/// autres (le cas des pages Vidéotex), un récepteur ordinaire se recale
/// au hasard sur le premier front venu et peut lire des dizaines de
/// caractères de travers. Celui-ci cherche le front suivi de trois
/// caractères valides d'affilée (départ, parité, arrêt) avant de reprendre.
#[derive(Clone, Debug)]
pub struct Uart {
    t_bit: f64,
    /// (sortie du discriminateur, échantillon douteux : un clic passe)
    hist: VecDeque<(f32, bool)>,
    /// Instant (échantillon) de `hist[0]`
    base: u64,
    /// Instant du prochain échantillon
    n: u64,
    /// Où en est la lecture (instant)
    pos: f64,
    etat: EtatRx,
    sortie: VecDeque<Recu>,
    pub recalages: u32,
    /// Parités fausses d'affilée
    erreurs: u32,
}

impl Uart {
    pub fn new(voie: Voie) -> Uart {
        Uart { t_bit: FS / voie.bauds(), hist: VecDeque::new(), base: 0, n: 0, pos: 0.0, etat: EtatRx::AttenteRepos, sortie: VecDeque::new(), recalages: 0, erreurs: 0 }
    }

    /// Plus de porteuse : on oublie le caractère en cours.
    pub fn reinitialiser(&mut self) {
        self.n += 1;
        self.hist.clear();
        self.base = self.n;
        self.pos = self.n as f64;
        self.etat = EtatRx::AttenteRepos;
    }

    /// `d` : sortie du discriminateur (négatif = 1, positif = 0) ;
    /// `douteux` : un clic passe, l'échantillon ne compte pas.
    pub fn push(&mut self, d: f32, douteux: bool) -> Option<Recu> {
        self.hist.push_back((d, douteux));
        self.n += 1;
        while self.avancer() {}
        // historique borné (50 bits), sans jeter ce qui reste à lire
        let garder = (50.0 * self.t_bit) as u64;
        let min_utile = (self.pos.floor() as u64).saturating_sub(2);
        while self.hist.len() as u64 > garder && self.base < min_utile {
            self.hist.pop_front();
            self.base += 1;
        }
        self.sortie.pop_front()
    }

    fn d(&self, i: u64) -> f32 {
        if i < self.base {
            return 0.0;
        }
        self.hist.get((i - self.base) as usize).map_or(0.0, |h| h.0)
    }

    fn douteux(&self, i: u64) -> bool {
        i >= self.base && self.hist.get((i - self.base) as usize).is_some_and(|h| h.1)
    }

    /// Premier front 1 → 0 entre `de` et `a` (instants), daté entre deux
    /// échantillons.
    fn front(&self, de: f64, a: f64) -> Option<f64> {
        let mut i = (de.floor() as u64).max(self.base + 1);
        let fin = (a.floor() as u64).min(self.n.saturating_sub(1));
        while i <= fin {
            let (p, d) = (self.d(i - 1), self.d(i));
            // (un clic sur la porteuse au repos ne fait pas un bit de départ)
            if p < 0.0 && d >= 0.0 && !self.douteux(i) && !self.douteux(i - 1) {
                let t = (i - 1) as f64 + (-p / (d - p).max(1e-6)) as f64;
                if t >= de {
                    return Some(t);
                }
            }
            i += 1;
        }
        None
    }

    /// Moyenne du milieu du bit `k` d'un caractère commencé à `t0`.
    fn bit(&self, t0: f64, k: usize) -> f32 {
        let a = (t0 + (k as f64 + 0.25) * self.t_bit).ceil() as u64;
        let b = (t0 + (k as f64 + 0.75) * self.t_bit).floor() as u64;
        let surs: Vec<f32> = (a..=b.max(a)).filter(|&i| !self.douteux(i)).map(|i| self.d(i)).collect();
        if surs.is_empty() {
            (a..=b.max(a)).map(|i| self.d(i)).sum()
        } else {
            surs.iter().sum()
        }
    }

    /// Un clic autour du bit de départ : ce n'est sans doute pas un caractère.
    fn depart_douteux(&self, t0: f64) -> bool {
        if self.t_bit > 20.0 {
            // à 75 bit/s, un clic ne dure pas un bit
            return false;
        }
        let a = (t0 - 2.0 * self.t_bit).floor().max(0.0) as u64;
        let b = (t0 + 1.5 * self.t_bit).ceil() as u64;
        (a..=b).any(|i| self.douteux(i))
    }

    fn lire(&self, t0: f64) -> Lu {
        let mut c = 0u8;
        for i in 0..7 {
            c |= u8::from(self.bit(t0, 1 + i) < 0.0) << i;
        }
        let p = self.bit(t0, 8) < 0.0;
        Lu { c, depart: self.bit(t0, 0) > 0.0 && !self.depart_douteux(t0), parite: (c.count_ones() + u32::from(p)).is_multiple_of(2), arret: self.bit(t0, 9) < 0.0 }
    }

    fn livrer(&mut self, l: Lu) {
        self.sortie.push_back(if l.parite { Recu::Caractere(l.c) } else { Recu::Parite(l.c) });
    }

    /// Fait avancer la lecture ; vrai s'il faut recommencer tout de suite.
    fn avancer(&mut self) -> bool {
        let t = self.t_bit;
        let maintenant = self.n as f64 - 1.0;
        match self.etat {
            EtatRx::AttenteRepos => {
                // un bit entier de repos (1) d'affilée
                let mut i = (self.pos.ceil() as u64).max(self.base);
                let mut run = 0u32;
                while i < self.n {
                    if self.d(i) < 0.0 {
                        run += 1;
                        if run as f64 >= t {
                            self.pos = i as f64;
                            self.etat = EtatRx::Repos;
                            return true;
                        }
                    } else {
                        run = 0;
                    }
                    i += 1;
                }
                self.pos = (self.n as f64 - run as f64).max(self.base as f64);
                false
            }
            EtatRx::Repos => match self.front(self.pos, maintenant) {
                Some(t0) => {
                    self.etat = EtatRx::Caractere { t0 };
                    true
                }
                None => {
                    self.pos = maintenant;
                    false
                }
            },
            EtatRx::Caractere { t0 } => {
                // faux départ (un parasite) : on cherche le front suivant
                if maintenant >= t0 + 0.75 * t && self.bit(t0, 0) <= 0.0 {
                    self.pos = t0 + 0.5;
                    self.etat = EtatRx::Repos;
                    return true;
                }
                if maintenant < t0 + 9.75 * t {
                    return false;
                }
                let l = self.lire(t0);
                if !l.depart {
                    // un clic : pas un caractère (au repos, on retrouve vite
                    // le repos ; au milieu d'une page, le bon cadrage)
                    self.etat = EtatRx::Recalage { depuis: t0 + 0.5 };
                } else if l.arret {
                    self.livrer(l);
                    // deux parités fausses de suite : on lit sans doute de travers
                    self.erreurs = if l.parite { 0 } else { self.erreurs + 1 };
                    if self.erreurs >= 2 {
                        self.erreurs = 0;
                        self.etat = EtatRx::Recalage { depuis: t0 + 0.5 };
                    } else {
                        self.pos = t0 + 9.5 * t;
                        self.etat = EtatRx::Repos;
                    }
                } else {
                    self.sortie.push_back(Recu::Cadrage);
                    self.etat = EtatRx::Recalage { depuis: t0 + 0.5 };
                }
                true
            }
            EtatRx::Recalage { depuis } => {
                // un long repos (le clavier entre deux touches) : le front
                // suivant sera un vrai départ
                if let Some(fin) = self.repos(depuis, maintenant, 12.0 * t) {
                    self.pos = fin;
                    self.etat = EtatRx::Repos;
                    return true;
                }
                // sinon, il faut de quoi lire trois caractères après chaque front candidat
                if maintenant < depuis + 42.0 * t {
                    return false;
                }
                let mut f = depuis;
                while let Some(t0) = self.front(f, depuis + 10.0 * t) {
                    if let Some(trois) = self.valides(t0) {
                        self.recalages += 1;
                        for l in trois.0 {
                            self.livrer(l);
                        }
                        self.pos = trois.1;
                        self.etat = EtatRx::Repos;
                        return true;
                    }
                    f = t0 + 0.5;
                }
                // rien de bon dans ce caractère-là : on glisse d'un caractère
                self.etat = EtatRx::Recalage { depuis: depuis + 10.0 * t };
                true
            }
        }
    }

    /// Trois caractères valides d'affilée à partir de `t0` (ou moins, s'ils
    /// sont suivis d'un long repos) ? Rend les caractères et l'instant où
    /// reprendre la lecture.
    fn valides(&self, t0: f64) -> Option<(Vec<Lu>, f64)> {
        let t = self.t_bit;
        let mut lus = Vec::with_capacity(3);
        let mut debut = t0;
        while lus.len() < 3 {
            if !lus.is_empty() {
                // le caractère suivant commence au premier front après l'arrêt
                let arret = debut + 9.5 * t;
                match self.front(arret, arret + 3.0 * t) {
                    Some(f) => debut = f,
                    None if self.repos(arret, arret + 13.0 * t, 12.0 * t).is_some() => break,
                    None => return None,
                }
            }
            let l = self.lire(debut);
            if !(l.depart && l.parite && l.arret) {
                return None;
            }
            lus.push(l);
        }
        Some((lus, debut + 9.5 * t))
    }

    /// Première période de repos (1) d'au moins `duree` entre `de` et `a` :
    /// l'instant où elle atteint cette durée.
    fn repos(&self, de: f64, a: f64, duree: f64) -> Option<f64> {
        let mut run = 0u32;
        let mut i = (de.ceil() as u64).max(self.base);
        let fin = (a.floor() as u64).min(self.n.saturating_sub(1));
        while i <= fin {
            if self.d(i) < 0.0 {
                run += 1;
                if run as f64 >= duree {
                    return Some(i as f64);
                }
            } else {
                run = 0;
            }
            i += 1;
        }
        None
    }
}

/// Seuil de présence d'une porteuse : −43 dBm, soit 33 dB sous le niveau
/// nominal (même seuil que les tons du V.34), en puissance |z|² = A²/4.
pub const SEUIL_PORTEUSE: f32 = {
    let a = AMPLITUDE * 0.0224;
    a * a / 4.0
};

/// Récepteur complet d'une voie : discriminateur, porteuse et caractères.
///
/// La porteuse d'un modem FSK a une enveloppe constante : on retient son
/// niveau tant qu'elle est là, et l'on ferme le récepteur de caractères dès
/// que le signal retombe nettement en dessous (coupure, fin de l'appel), ce
/// qui évite de lire des caractères dans le bruit ou dans la traîne des
/// filtres. Un clic se voit à un surcroît d'énergie dans la voie : les
/// échantillons qu'il touche sont douteux (ils ne comptent pas dans les
/// bits, et un « bit de départ » qu'il fabrique sur la porteuse au repos
/// n'est pas pris pour un caractère).
///
/// (Effacer les clics à l'entrée, comme on le fait parfois, abîmait plus
/// qu'il ne réparait : les trous découpés dans la porteuse par un clic hors
/// de la voie, que le filtre aurait ignoré, fabriquaient des caractères.)
#[derive(Clone, Debug)]
pub struct Recepteur {
    pub disc: Discriminateur,
    uart: Uart,
    /// Échantillons consécutifs avec / sans porteuse
    present: u32,
    absent: u32,
    porteuse: bool,
    /// Durées de confirmation (échantillons)
    t_on: u32,
    t_off: u32,
    /// Niveau de la porteuse (|z|²), retenu tant qu'elle est là
    niveau_porteuse: f32,
    /// Échantillons depuis que le récepteur est fermé alors qu'un signal est là
    ferme_avec_signal: u32,
    /// Échantillons depuis la création (le temps que les filtres se remplissent)
    age: u32,
    /// Échantillons d'ouverture continue, et ce qu'il en faut avant de lire
    /// (un clic isolé, après la fin de la porteuse, ne fait pas un caractère)
    ouvert_depuis: u32,
    reprise: u32,
    /// Clics vus
    pub clics: u64,
    /// Échantillons encore douteux après un clic, et la garde
    douteux: u32,
    garde: u32,
}

impl Recepteur {
    /// `on_ms` : porteuse confirmée après ce temps ; `off_ms` : perdue après
    /// ce temps d'absence (les coupures plus courtes sont pardonnées).
    pub fn new(voie: Voie, on_ms: u32, off_ms: u32) -> Recepteur {
        Recepteur {
            disc: Discriminateur::new(voie),
            uart: Uart::new(voie),
            present: 0,
            absent: 0,
            porteuse: false,
            t_on: on_ms * 8,
            t_off: off_ms * 8,
            niveau_porteuse: 0.0,
            ferme_avec_signal: 0,
            age: 0,
            ouvert_depuis: 0,
            reprise: if voie == Voie::Aller { 12 * 8 } else { 30 * 8 },
            clics: 0,
            douteux: 0,
            garde: if voie == Voie::Aller { 24 } else { 80 },
        }
    }

    /// Porteuse confirmée.
    pub fn porteuse(&self) -> bool {
        self.porteuse
    }

    /// Durée de présence continue du signal (ms).
    pub fn present_ms(&self) -> u32 {
        self.present / 8
    }

    /// Durée d'absence continue (ms).
    pub fn absent_ms(&self) -> u32 {
        self.absent / 8
    }

    pub fn push(&mut self, x: f32) -> Option<Recu> {
        self.age = self.age.saturating_add(1);
        let d = self.disc.push(x);
        let (p, rapide) = (self.disc.puissance, self.disc.rapide);
        // Ouvert : assez de signal, et pas nettement moins que la porteuse
        // retenue. Un niveau qui change pour de bon (autre ligne) est
        // réappris au bout de 200 ms.
        let ouvert = p > SEUIL_PORTEUSE * 0.5 && rapide > SEUIL_PORTEUSE * 0.5 && rapide > 0.25 * self.niveau_porteuse;
        if ouvert {
            self.ferme_avec_signal = 0;
            let a = if self.niveau_porteuse == 0.0 { 1.0 } else { 0.002 };
            self.niveau_porteuse += a * (rapide - self.niveau_porteuse);
        } else if rapide > SEUIL_PORTEUSE {
            self.ferme_avec_signal += 1;
            if self.ferme_avec_signal > 200 * 8 {
                self.niveau_porteuse = rapide;
            }
        }
        if ouvert {
            self.present = self.present.saturating_add(1);
            self.absent = 0;
            if self.present >= self.t_on {
                self.porteuse = true;
            }
        } else {
            self.absent = self.absent.saturating_add(1);
            self.present = 0;
            if self.absent >= self.t_off {
                self.porteuse = false;
                self.niveau_porteuse = 0.0;
            }
        }
        // Clic : un surcroît d'énergie dans la voie (une porteuse FSK a une
        // enveloppe constante). Les échantillons voisins sont douteux.
        if self.niveau_porteuse > 0.0 && self.disc.instant > 3.0 * self.niveau_porteuse {
            if self.douteux == 0 {
                self.clics += 1;
            }
            self.douteux = self.garde;
        }
        let douteux = self.douteux > 0;
        self.douteux = self.douteux.saturating_sub(1);
        // rien à lire sans porteuse, ni pendant que les filtres se remplissent
        self.ouvert_depuis = if ouvert { self.ouvert_depuis.saturating_add(1) } else { 0 };
        if self.ouvert_depuis < self.reprise || self.age < 40 * 8 {
            self.uart.reinitialiser();
            return None;
        }
        self.uart.push(d, douteux)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trame_7_bits_parite_paire() {
        // 'A' = 0x41 : deux uns, parité 0
        assert_eq!(trame(b'A'), [false, true, false, false, false, false, false, true, false, true]);
        // 'C' = 0x43 : trois uns, parité 1
        assert!(trame(b'C')[8]);
    }

    #[test]
    fn filtres_separent_les_deux_voies() {
        let d = Discriminateur::new(Voie::Aller);
        assert!(d.bp.gain(1300.0) > 0.9 && d.bp.gain(2100.0) > 0.9);
        assert!(d.bp.gain(450.0) < 0.001 && d.bp.gain(390.0) < 0.001, "écho de la voie retour");
        let r = Discriminateur::new(Voie::Retour);
        assert!(r.bp.gain(390.0) > 0.9 && r.bp.gain(450.0) > 0.9);
        assert!(r.bp.gain(1300.0) < 0.001 && r.bp.gain(2100.0) < 0.001, "écho de la voie aller");
    }

    fn boucle(voie: Voie, texte: &[u8], bruit: f32) -> Vec<Recu> {
        let mut e = Emetteur::new(voie);
        let mut r = Recepteur::new(voie, 20, 100);
        e.porteuse = true;
        let mut out = Vec::new();
        let mut graine = 12345u32;
        let total = (FS * 0.1 + FS / voie.bauds() * 10.0 * (texte.len() + 6) as f64) as usize;
        for n in 0..total {
            if n == (FS * 0.1) as usize {
                e.envoyer(texte);
            }
            graine = graine.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let b = ((graine >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * bruit;
            if let Some(c) = r.push(0.3 * e.next_sample() + b) {
                out.push(c);
            }
        }
        out
    }

    /// Un clic sur la porteuse au repos ne doit pas faire un caractère.
    #[test]
    fn un_clic_n_est_pas_un_caractere() {
        let mut fantomes = Vec::new();
        for (k, f) in [500.0f64, 900.0, 1300.0, 1700.0, 2000.0, 2100.0, 2400.0, 2800.0].iter().enumerate() {
            for amp in [2.0f64, 3.0, 4.5, 6.0] {
                for tau in [0.0005f64, 0.001, 0.002] {
                    let mut e = Emetteur::new(Voie::Aller);
                    e.porteuse = true;
                    e.amplitude = 0.0887;
                    let mut r = Recepteur::new(Voie::Aller, 20, 100);
                    let rx_rms = 0.0627;
                    for n in 0..4000usize {
                        let mut x = e.next_sample();
                        if n >= 2000 {
                            let t = (n - 2000) as f64 / FS;
                            x += (amp * rx_rms * (-t / tau).exp() * (TAU * f * t).sin()) as f32;
                        }
                        if let Some(c) = r.push(x) {
                            fantomes.push((k, *f, amp, tau, c, r.clics));
                        }
                    }
                }
            }
        }
        assert!(fantomes.is_empty(), "{} fantômes : {fantomes:?}", fantomes.len());
    }

    #[test]
    fn caracteres_dans_les_deux_voies() {
        let texte: Vec<u8> = b"3615 ORACLE \x13\x41 {|}~\x7f\x00\x1b".to_vec();
        for voie in [Voie::Aller, Voie::Retour] {
            let recu = boucle(voie, &texte, 0.05);
            let attendu: Vec<Recu> = texte.iter().map(|&c| Recu::Caractere(c)).collect();
            assert_eq!(recu, attendu, "{voie:?}");
        }
    }
}
