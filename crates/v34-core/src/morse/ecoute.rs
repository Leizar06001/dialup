//! L'écoute : retrouver les points et les traits dans ce qui arrive de la
//! ligne, puis les lettres.
//!
//! 1. Filtre étroit : le signal est ramené en bande de base (mélange avec
//!    la fréquence d'écoute) puis passe dans deux moyennes glissantes de
//!    10 ms (réponse triangulaire de 20 ms, ±50 Hz). Leurs zéros tombent tous
//!    les 100 Hz, donc pile sur notre propre fréquence, à 800 Hz de là : notre
//!    écho, aussi fort que le signal d'en face sur une longue ligne, s'éteint.
//! 2. Seuil adaptatif : un niveau « haut » (crête des derniers tons) et le
//!    niveau moyen du bruit, mesuré par un filtre jumeau à mi-chemin entre
//!    les deux tons (1200 Hz : un zéro pile sur chacun, il n'entend que le
//!    bruit) ; seuil à mi-hauteur, avec hystérésis, jamais à moins de 12 dB
//!    au-dessus du bruit. Un changement
//!    d'état ne compte qu'après un demi-point (anti-rebond : les clics
//!    d'orage et les micro-coupures ne font ni faux points ni faux silences).
//! 3. Rythme : un ton de moins de deux points est un point, sinon un trait ;
//!    la durée du point s'ajuste en douceur autour de la vitesse convenue.
//!    Le silence décide : plus de deux points, la lettre est finie ; plus de
//!    cinq, le mot. Chaque lettre sort dès que son silence est assez long.
//!
//! AR (`.-.-.`) est aussi le « + » : la fin de message n'est annoncée que si
//! un long silence (ou un KA) suit ; sinon, c'était un « + ».
//!
//! Au début, l'écoute attend 50 ms de calme avant de lire quoi que ce soit :
//! la fin du JM de l'appelé (V.21 à 1650 Hz, à 50 Hz de notre 1600 Hz)
//! arrive encore chez l'appelant quand le morse commence, et ferait un
//! faux point.

use super::code;
use super::Cadence;
use crate::api::NOMINAL_RMS;
use crate::dsp::osc::Osc;

/// Longueur de chacune des deux moyennes glissantes : 10 ms.
const MOYENNE: usize = 80;
/// Plus faible ton reconnu : 45 dB sous le niveau nominal.
const NIVEAU_MIN: f32 = NOMINAL_RMS * std::f32::consts::SQRT_2 * 0.0056;
/// Oubli du niveau haut pendant les silences (constante de temps : 10 s).
const OUBLI: f32 = 1.0 / 80_000.0;
/// Calme à entendre avant de commencer à lire (50 ms), au plus 2 s d'attente.
const CALME: u32 = 400;
const CALME_MAX: u64 = 16_000;

/// Filtre étroit : mélange à `freq`, puis deux moyennes glissantes.
struct Filtre {
    osc: Osc,
    m1: Moyenne,
    m2: Moyenne,
}

impl Filtre {
    fn new(freq: f64) -> Self {
        Self { osc: Osc::new(freq), m1: Moyenne::new(), m2: Moyenne::new() }
    }

    /// Amplitude du ton à `freq` (un ton d'amplitude A donne A).
    #[inline]
    fn pousser(&mut self, x: f32) -> f32 {
        let (c, s) = self.osc.next_iq();
        let (x, c, s) = (x as f64, c as f64, s as f64);
        let (r1, i1) = self.m1.pousser(x * c, -x * s);
        let (r2, i2) = self.m2.pousser(r1, i1);
        2.0 * (r2 * r2 + i2 * i2).sqrt() as f32
    }
}

/// Moyenne glissante complexe (sommes en f64 : pas de dérive sur des heures).
struct Moyenne {
    buf: Vec<(f64, f64)>,
    i: usize,
    somme: (f64, f64),
}

impl Moyenne {
    fn new() -> Self {
        Self { buf: vec![(0.0, 0.0); MOYENNE], i: 0, somme: (0.0, 0.0) }
    }

    #[inline]
    fn pousser(&mut self, re: f64, im: f64) -> (f64, f64) {
        let (vr, vi) = self.buf[self.i];
        self.buf[self.i] = (re, im);
        self.i = (self.i + 1) % MOYENNE;
        self.somme.0 += re - vr;
        self.somme.1 += im - vi;
        (self.somme.0 / MOYENNE as f64, self.somme.1 / MOYENNE as f64)
    }
}

pub struct Ecoute {
    /// Le filtre de l'écoute, et son jumeau qui n'entend que le bruit
    filtre: Filtre,
    bruit: Filtre,
    n: u64,
    /// Niveau du ton dans le filtre (amplitude)
    niveau: f32,
    /// Niveau des tons reçus (crêtes) et niveau moyen du bruit
    haut: f32,
    bas: f32,
    haut_connu: bool,
    /// Échantillons de bruit déjà moyennés (pour démarrer vite)
    n_bas: u32,
    /// Prête à lire (après un moment de calme) ; durée du calme en cours
    armee: bool,
    calme: u32,
    /// Crête du ton en cours
    pic: f32,
    /// État reconnu (ton présent ?), depuis quand (franchissement du seuil)
    ton: bool,
    depuis: u64,
    /// Franchissement en sens inverse, en attente de confirmation
    candidat: Option<u64>,
    cadence: Cadence,
    /// Durée estimée du point (échantillons)
    point: f32,
    /// Éléments du signe en cours
    code: String,
    /// Un signe est sorti depuis le dernier espace ; l'espace de ce silence est fait
    a_ecrit: bool,
    mot_fait: bool,
    /// AR reconnu, en attente : fin de message ou « + » ? (vrai : un espace a suivi)
    ar: Option<bool>,
    /// Silence de mot entendu : l'espace sortira devant le signe suivant
    espace_attente: bool,
    sortie: Vec<(String, String)>,
}

impl Ecoute {
    /// Écoute de `freq` ; `f_bruit` : une fréquence où personne n'émet.
    pub fn new(freq: f64, f_bruit: f64, cadence: Cadence) -> Self {
        Self {
            filtre: Filtre::new(freq),
            bruit: Filtre::new(f_bruit),
            n: 0,
            niveau: 0.0,
            haut: 0.0,
            bas: 0.0,
            haut_connu: false,
            n_bas: 0,
            armee: false,
            calme: 0,
            pic: 0.0,
            ton: false,
            depuis: 0,
            candidat: None,
            cadence,
            point: cadence.point as f32,
            code: String::new(),
            a_ecrit: false,
            mot_fait: false,
            ar: None,
            espace_attente: false,
            sortie: Vec::new(),
        }
    }

    /// Le ton d'en face est-il entendu en ce moment ?
    pub fn ton(&self) -> bool {
        self.ton
    }

    /// Points et traits du signe en cours.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Écart entre le niveau des tons et le bruit dans le filtre (dB).
    pub fn snr_db(&self) -> Option<f32> {
        (self.haut_connu && self.haut > 0.0).then(|| 20.0 * (self.haut / self.bas.max(1e-6)).log10())
    }

    /// Durée estimée du point (ms).
    pub fn point_ms(&self) -> f32 {
        self.point / 8.0
    }

    /// Signes reconnus depuis le dernier appel : (texte, code).
    pub fn prendre(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.sortie)
    }

    /// Seuils de montée et de descente. Pendant un ton, sa crête compte
    /// aussi (le tout premier, ou le premier après un long silence).
    fn seuils(&self) -> (f32, f32) {
        let plancher = (4.0 * self.bas).max(NIVEAU_MIN);
        let mut h = if self.haut_connu { self.haut } else { 0.0 };
        if self.ton {
            h = h.max(self.pic);
        }
        if h > 1.5 * plancher {
            let ecart = h - self.bas;
            ((self.bas + 0.55 * ecart).max(plancher), (self.bas + 0.4 * ecart).max(0.8 * plancher))
        } else {
            (plancher, 0.8 * plancher)
        }
    }

    /// Un échantillon reçu de la ligne.
    pub fn pousser(&mut self, x: f32) {
        let e = self.filtre.pousser(x);
        self.niveau = e;
        self.n += 1;
        // Bruit : moyenne sur 0,25 s (plus vite au début)
        let b = self.bruit.pousser(x);
        self.n_bas = self.n_bas.saturating_add(1);
        self.bas += (b - self.bas) * (1.0 / self.n_bas as f32).max(1.0 / 2000.0);

        let (monte, descend) = self.seuils();
        if !self.armee {
            // la fin de la négociation V.8 traîne peut-être encore sur la ligne
            self.calme = if e < monte { self.calme + 1 } else { 0 };
            self.armee = self.calme >= CALME || self.n >= CALME_MAX;
            self.depuis = self.n;
            return;
        }
        let brut = if self.ton { e > descend } else { e > monte };
        if brut != self.ton {
            let t0 = *self.candidat.get_or_insert(self.n);
            let anti_rebond = self.cadence.point as u64 / 2;
            if self.n - t0 >= anti_rebond {
                self.basculer(t0);
            }
        } else {
            self.candidat = None;
        }

        if self.ton {
            self.pic = self.pic.max(e);
        } else {
            if self.haut_connu {
                self.haut += (self.bas - self.haut) * OUBLI;
            }
            self.silence();
        }
    }

    /// Changement d'état confirmé, daté du franchissement `t0`.
    fn basculer(&mut self, t0: u64) {
        let duree = (t0 - self.depuis) as f32;
        if self.ton {
            self.fin_de_ton(duree);
        } else {
            self.pic = self.niveau;
            self.mot_fait = false;
        }
        self.ton = !self.ton;
        self.depuis = t0;
        self.candidat = None;
    }

    fn fin_de_ton(&mut self, d: f32) {
        // Le niveau haut : fixé par le premier ton, puis suivi
        if !self.haut_connu || self.pic > self.haut {
            self.haut = self.pic;
            self.haut_connu = true;
        } else {
            self.haut += 0.25 * (self.pic - self.haut);
        }
        let p = self.point;
        if d > 8.0 * p {
            // bien trop long pour du morse (une porteuse ?) : on oublie
            self.code.clear();
            return;
        }
        if d < 2.0 * p {
            self.code.push('.');
            self.point += 0.15 * (d - p);
        } else {
            self.code.push('-');
            self.point += 0.15 * (d / 3.0 - p);
        }
        let nominal = self.cadence.point as f32;
        self.point = self.point.clamp(0.6 * nominal, 1.6 * nominal);
    }

    /// Pendant un silence : la lettre, le mot, la fin de message.
    fn silence(&mut self) {
        let fin = self.candidat.unwrap_or(self.n);
        let g = (fin - self.depuis) as f32;
        let c = self.cadence;
        let k = self.point / c.point as f32;
        let lettre = k * (c.point + c.lettre) as f32 / 2.0;
        let mot = k * (c.lettre + c.mot) as f32 / 2.0;
        let fin_message = k * (c.mot + 3 * c.point) as f32;
        if !self.code.is_empty() && g >= lettre {
            let code = std::mem::take(&mut self.code);
            self.signe(code);
        }
        if self.a_ecrit && !self.mot_fait && g >= mot {
            self.mot_fait = true;
            self.espace();
        }
        if self.ar.is_some() && g >= fin_message {
            self.ar = None;
            self.a_ecrit = false;
            self.espace_attente = false;
            self.sortir("<AR>", ".-.-.");
        }
    }

    fn signe(&mut self, code: String) {
        let texte = code::lire(&code).unwrap_or("*");
        if let Some(espace) = self.ar.take() {
            if texte == "<KA>" {
                self.espace_attente = false;
                self.sortir("<AR>", ".-.-.");
            } else {
                self.sortir_espace();
                self.sortir("+", ".-.-.");
                self.espace_attente = espace;
            }
        }
        // (pas d'espace après SK : c'est fini)
        self.a_ecrit = texte != "<SK>";
        if texte == "<AR>" {
            self.ar = Some(false);
            return;
        }
        // l'espace d'avant ne sort que devant du texte (pas devant KA ou SK)
        if texte == "<KA>" || texte == "<SK>" {
            self.espace_attente = false;
        } else {
            self.sortir_espace();
        }
        self.sortir(texte, &code);
    }

    /// Silence de mot : l'espace attend le signe suivant (un AR qui arrive
    /// en retard, après une pause, ne laisse pas d'espace avant lui).
    fn espace(&mut self) {
        if let Some(e) = self.ar.as_mut() {
            *e = true;
            return;
        }
        self.a_ecrit = false;
        self.espace_attente = true;
    }

    fn sortir_espace(&mut self) {
        if std::mem::take(&mut self.espace_attente) {
            self.sortir(" ", "");
        }
    }

    fn sortir(&mut self, texte: &str, code: &str) {
        self.sortie.push((texte.to_string(), code.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::super::manipulateur::Manipulateur;
    use super::*;
    use crate::line::{EchoPath, LineParams, LineSim};

    /// Émet `texte` à `f_tx` et l'écoute à travers une ligne, avec l'écho
    /// d'une autre émission simultanée à `f_echo` ; renvoie le texte lu.
    fn ecouter(texte: &str, ligne: &str, f_echo: f64, graine: u64) -> String {
        let p = LineParams::preset(ligne).unwrap();
        let cad = Cadence::paris(20);
        let mut m = Manipulateur::new(800.0, cad).avec_blanc(super::super::manipulateur::BLANC_INITIAL);
        let mut moi = Manipulateur::new(f_echo, cad);
        let mut e = Ecoute::new(800.0, 1200.0, cad);
        let mut sim = LineSim::with_seed(p.clone(), graine);
        let mut echo = EchoPath::new(p);
        m.envoyer(texte.as_bytes());
        moi.envoyer("VVV LIGNE DE TEST, ON PARLE EN MEME TEMPS 1234567890 ".repeat(4).as_bytes());
        let (mut a, mut b, mut r) = ([0.0f32; 160], [0.0f32; 160], [0.0f32; 160]);
        let mut lu = String::new();
        for _ in 0..(120 * 50) {
            for i in 0..160 {
                a[i] = m.echantillon();
                b[i] = moi.echantillon();
            }
            sim.process(&a, &mut r);
            echo.process(&b, &mut r);
            for &x in r.iter() {
                e.pousser(x);
            }
            for (t, _) in e.prendre() {
                lu.push_str(&t);
            }
            if m.oisif() && lu.ends_with("<AR>") {
                break;
            }
        }
        lu
    }

    #[test]
    fn lit_une_ligne_parfaite() {
        let t = "<KA>CQ CQ DE F5TINY, ÇA VA ? 73<AR>";
        assert_eq!(ecouter(t, "Parfaite", 1600.0, 1), t);
    }

    #[test]
    fn lit_malgre_l_echo_d_une_longue_ligne() {
        // l'écho de notre propre émission (−20 dB) est aussi fort que le
        // signal d'en face (−18 dB, pente dans les aigus) : il ne doit rien écrire
        let t = "<KA>LE PETIT CHAT EST MORT. 1 + 1 = 2<AR>";
        assert_eq!(ecouter(t, "Longue distance", 1600.0, 2), t);
        assert_eq!(ecouter(t, "Urbaine", 1600.0, 3), t);
    }

    #[test]
    fn plus_ou_fin_de_message() {
        // « + » au milieu d'un message, AR à la fin, KA juste derrière
        let t = "<KA>C++ ET 2+2<AR><KA>OK<AR>";
        assert_eq!(ecouter(t, "Parfaite", 1600.0, 4), t);
    }

    #[test]
    fn pas_d_espace_devant_un_ar_en_retard() {
        // la ligne de test renvoie AR après une pause (elle attend de savoir
        // si c'était un « + ») : pas d'espace parasite devant
        assert_eq!(ecouter("<KA>SOS DE TEST <AR>", "Parfaite", 1600.0, 6), "<KA>SOS DE TEST<AR>");
        assert_eq!(ecouter("<KA>A B <KA>C<AR>", "Parfaite", 1600.0, 7), "<KA>A B<KA>C<AR>");
    }

    #[test]
    fn rien_dans_le_bruit_ni_dans_les_clics() {
        let p = LineParams { snr_db: 10.0, impulses_per_s: 20.0, ..LineParams::preset("Orage").unwrap() };
        let mut sim = LineSim::with_seed(p, 5);
        let mut e = Ecoute::new(800.0, 1200.0, Cadence::paris(20));
        let zero = [0.0f32; 160];
        let mut r = [0.0f32; 160];
        for _ in 0..(20 * 50) {
            sim.process(&zero, &mut r);
            for &x in r.iter() {
                e.pousser(x);
            }
        }
        assert_eq!(e.prendre(), Vec::<(String, String)>::new(), "20 s de bruit et 400 clics : rien");
    }
}
