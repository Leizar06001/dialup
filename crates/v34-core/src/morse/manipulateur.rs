//! Le manipulateur : le texte à émettre devient un ton pur, baissé et levé
//! au rythme du code. Les fronts sont adoucis (cosinus surélevé de 5 ms) :
//! un tout-ou-rien franc claquerait dans le haut-parleur (les « clics de
//! manipulation » des radioamateurs) et déborderait jusque sur la fréquence
//! d'écoute d'en face.

use std::collections::VecDeque;

use super::code::{Lecteur, Signe};
use super::Cadence;
use crate::api::NOMINAL_RMS;
use crate::dsp::osc::Osc;

/// Durée d'un front (échantillons) : 5 ms. La mi-hauteur tombe au milieu,
/// donc la durée d'un point ou d'un trait est respectée.
const FRONT: usize = 40;
/// Une procédure entamée (« <K ») sans suite pendant 1 s part telle quelle.
const PATIENCE: u32 = 8000;
/// Silence au début de la liaison (250 ms), le temps que la ligne se calme
/// (la fin du V.8) et que l'écoute d'en face soit prête.
pub const BLANC_INITIAL: u32 = 2000;

/// Une étape du programme : touche baissée ou levée, pour tant d'échantillons ;
/// `fait` : éléments du signe déjà partis pendant cette étape.
#[derive(Clone, Copy, Debug)]
struct Etape {
    baisse: bool,
    duree: u32,
    fait: u8,
}

pub struct Manipulateur {
    osc: Osc,
    amp: f32,
    cadence: Cadence,
    lecteur: Lecteur,
    /// Signes en attente
    file: VecDeque<Signe>,
    /// Signe en cours et ce qu'il reste à jouer
    courant: Option<Signe>,
    programme: VecDeque<Etape>,
    restant: u32,
    baisse: bool,
    fait: u8,
    /// Position dans le front (0 : levé, FRONT : baissé)
    rampe: usize,
    enveloppe: [f32; FRONT + 1],
    /// Signes qui viennent de commencer (pour les événements)
    partis: Vec<Signe>,
    oisif: u32,
}

impl Manipulateur {
    pub fn new(freq: f64, cadence: Cadence) -> Self {
        let mut enveloppe = [0.0f32; FRONT + 1];
        for (k, e) in enveloppe.iter_mut().enumerate() {
            *e = (0.5 - 0.5 * (std::f64::consts::PI * k as f64 / FRONT as f64).cos()) as f32;
        }
        Self {
            osc: Osc::new(freq),
            // ton au niveau nominal quand la touche est baissée
            amp: NOMINAL_RMS * std::f32::consts::SQRT_2,
            cadence,
            lecteur: Lecteur::default(),
            file: VecDeque::new(),
            courant: None,
            programme: VecDeque::new(),
            restant: 0,
            baisse: false,
            fait: 0,
            rampe: 0,
            enveloppe,
            partis: Vec::new(),
            oisif: 0,
        }
    }

    /// Commence par un silence de `n` échantillons.
    pub fn avec_blanc(mut self, n: u32) -> Self {
        self.programme.push_back(Etape { baisse: false, duree: n, fait: 0 });
        self
    }

    /// Du texte à émettre (UTF-8, procédures entre chevrons).
    pub fn envoyer(&mut self, octets: &[u8]) {
        let s = self.lecteur.pousser(octets);
        self.file.extend(s);
    }

    /// Oublie tout ce qui attend (le signe en cours se termine).
    pub fn couper(&mut self) {
        self.file.clear();
        self.lecteur = Lecteur::default();
    }

    /// Plus rien à émettre, touche levée et front retombé.
    pub fn oisif(&self) -> bool {
        self.file.is_empty() && self.programme.is_empty() && self.restant == 0 && self.rampe == 0
    }

    pub fn cle(&self) -> bool {
        self.baisse
    }

    /// Signe en cours d'émission et éléments déjà partis.
    pub fn courant(&self) -> Option<(Signe, u8)> {
        self.courant.map(|s| (s, self.fait))
    }

    /// Signes en attente (sans compter celui en cours).
    pub fn attente(&self) -> usize {
        self.file.iter().filter(|s| !s.est_espace()).count()
    }

    /// Les signes qui ont commencé à partir depuis le dernier appel.
    pub fn partis(&mut self) -> Vec<Signe> {
        std::mem::take(&mut self.partis)
    }

    fn charger(&mut self, s: Signe) {
        let c = self.cadence;
        if s.est_espace() {
            // le silence de la lettre précédente est déjà joué : on complète
            self.courant = None;
            self.partis.push(s);
            self.programme.push_back(Etape { baisse: false, duree: c.mot.saturating_sub(c.lettre), fait: 0 });
            return;
        }
        self.courant = Some(s);
        self.partis.push(s);
        let n = s.code.len();
        for (i, e) in s.code.bytes().enumerate() {
            let duree = if e == b'-' { 3 * c.point } else { c.point };
            self.programme.push_back(Etape { baisse: true, duree, fait: i as u8 });
            let silence = if i + 1 == n { c.lettre } else { c.point };
            self.programme.push_back(Etape { baisse: false, duree: silence, fait: i as u8 + 1 });
        }
    }

    /// Échantillon suivant de l'émission.
    pub fn echantillon(&mut self) -> f32 {
        if self.restant == 0 {
            if self.programme.is_empty() {
                match self.file.pop_front() {
                    Some(s) => self.charger(s),
                    None => {
                        self.courant = None;
                        self.oisif = self.oisif.saturating_add(1);
                        if self.oisif > PATIENCE && self.lecteur.en_attente() {
                            let s = self.lecteur.vider();
                            self.file.extend(s);
                        }
                    }
                }
            }
            if let Some(e) = self.programme.pop_front() {
                self.oisif = 0;
                self.baisse = e.baisse;
                self.restant = e.duree;
                self.fait = e.fait;
            } else {
                self.baisse = false;
            }
        }
        self.restant = self.restant.saturating_sub(1);
        if self.baisse {
            self.rampe = (self.rampe + 1).min(FRONT);
        } else {
            self.rampe = self.rampe.saturating_sub(1);
        }
        // l'oscillateur tourne toujours : la phase reste continue
        let x = self.osc.next_cos();
        self.amp * self.enveloppe[self.rampe] * x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Durées (ms) des tons et silences d'une émission, mesurées à mi-hauteur.
    fn rythme(texte: &str) -> Vec<(bool, u32)> {
        let mut m = Manipulateur::new(800.0, Cadence::paris(20));
        m.envoyer(texte.as_bytes());
        let mut env = Vec::new();
        let mut pic = 0.0f32;
        // enveloppe crête sur une période de 10 ms (8 périodes de 800 Hz)
        for _ in 0..8000 * 10 {
            let x = m.echantillon().abs();
            pic = pic.max(x);
            env.push(x);
            if m.oisif() && env.len() > 100 {
                break;
            }
        }
        let haut: Vec<bool> = env.windows(10).map(|w| w.iter().cloned().fold(0.0, f32::max) > 0.5 * pic).collect();
        let mut v: Vec<(bool, u32)> = Vec::new();
        for h in haut {
            match v.last_mut() {
                Some((b, n)) if *b == h => *n += 1,
                _ => v.push((h, 1)),
            }
        }
        v.iter().map(|&(b, n)| (b, (n as f32 / 8.0).round() as u32)).collect()
    }

    #[test]
    fn paris_a_20_mots_minute() {
        // A = point, silence, trait : 60, 60, 180 ms (±2 ms de mesure)
        let r = rythme("A");
        let tons: Vec<u32> = r.iter().filter(|x| x.0).map(|x| x.1).collect();
        assert_eq!(tons.len(), 2, "{r:?}");
        assert!((tons[0] as i32 - 60).abs() <= 2 && (tons[1] as i32 - 180).abs() <= 2, "{r:?}");
        let silences: Vec<u32> = r.iter().skip(1).filter(|x| !x.0).map(|x| x.1).collect();
        assert!((silences[0] as i32 - 60).abs() <= 2, "{r:?}");
        // E E : silence de lettre (180 ms) ; E E en deux mots : 420 ms
        let r = rythme("EE");
        assert!((r[2].1 as i32 - 180).abs() <= 2, "{r:?}");
        let r = rythme("E E");
        assert!((r[2].1 as i32 - 420).abs() <= 2, "{r:?}");
    }

    #[test]
    fn fronts_doux_et_niveau_nominal() {
        let mut m = Manipulateur::new(800.0, Cadence::paris(20));
        m.envoyer(b"T");
        let x: Vec<f32> = (0..2400).map(|_| m.echantillon()).collect();
        // pas de saut brutal à la montée : le ton naît en 5 ms
        assert!(x[..4].iter().all(|v| v.abs() < 0.01), "{:?}", &x[..4]);
        let crete = |r: std::ops::Range<usize>| x[r].iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(crete(10..20) < 0.5 * crete(400..410), "montée trop rapide");
        // au milieu du trait : le niveau nominal
        let rms = (x[400..1200].iter().map(|v| v * v).sum::<f32>() / 800.0).sqrt();
        assert!((rms - NOMINAL_RMS).abs() < 0.005, "{rms}");
        assert!(m.oisif() || !m.cle());
        // les événements : un signe parti
        assert_eq!(m.partis().iter().map(|s| s.texte).collect::<Vec<_>>(), ["T"]);
    }
}
