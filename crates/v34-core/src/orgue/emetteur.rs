//! L'émetteur : bits → échantillons. Il joue le motif d'appel, puis la
//! mélodie en boucle, chaque symbole étant une période de la note (plus sa
//! garde, qui n'est que la note qui continue).

use std::collections::VecDeque;
use std::f32::consts::PI;

use super::chargement::{base_donnees, Chargement};
use super::constellation::{gray_inverse, maq, mdp};
use super::partition::{
    debut_motif, duree_motif, harmoniques, periode, reference, transition, Degre, Ordonnanceur, Symbole, ALLUMAGE, ARTICULATION, CROCHE, DEGRES, EXTINCTION, GARDE, MOTIF,
};
use super::{Duplex, Mode, Voix};
use crate::dsp::Cpx;

/// Rampe en cosinus surélevé de 0 à 1 pour `x` dans [0, 1].
#[inline]
pub(crate) fn rampe(x: f32) -> f32 {
    0.5 - 0.5 * (PI * x.clamp(0.0, 1.0)).cos()
}

/// Une note tenue avec des amplitudes complexes fixes : sa forme d'onde
/// (analytique) est périodique, précalculée sur une période.
#[derive(Clone, Debug)]
struct Son {
    w: usize,
    /// Origine des phases (temps absolu)
    t0: usize,
    rangs: Vec<usize>,
    c: Vec<Cpx>,
    /// e^{j2πn/w}
    osc: Vec<Cpx>,
    table: Vec<Cpx>,
    /// Changement d'amplitudes en cours : ancienne forme d'onde, début et
    /// durée du fondu
    fondu: Option<(Vec<Cpx>, usize, usize)>,
    /// Notes « agitées » du motif (phases tirées au hasard à chaque symbole)
    agitation: Option<Agitation>,
}

#[derive(Clone, Debug)]
struct Agitation {
    prochain: usize,
    fin: usize,
    fondu: usize,
    base: Vec<Cpx>,
    /// Note seule (tremblant) : ses symboles sont consignés au journal
    degre: Option<Degre>,
}

/// Un symbole connu du motif (tremblant d'une note seule).
#[derive(Clone, Debug, PartialEq)]
pub struct SymboleConnu {
    /// Début de la fenêtre utile (échantillons depuis le début du motif)
    pub fenetre: usize,
    pub degre: Degre,
    /// Origine des phases de la note
    pub debut_note: usize,
    pub c: Vec<Cpx>,
}

/// Durée du fondu d'un accord agité.
const FONDU_ACCORD: usize = 16;

impl Son {
    fn new(w: usize, t0: usize, rangs: Vec<usize>, c: Vec<Cpx>, osc: Option<Vec<Cpx>>) -> Son {
        let osc = osc.unwrap_or_else(|| (0..w).map(|n| Cpx::from_polar(1.0, 2.0 * PI * n as f32 / w as f32)).collect());
        let mut table = vec![Cpx::ZERO; w];
        for (k, ck) in rangs.iter().zip(&c) {
            if ck.norm_sqr() == 0.0 {
                continue;
            }
            for (n, v) in table.iter_mut().enumerate() {
                *v += *ck * osc[(k * n) % w];
            }
        }
        Son { w, t0, rangs, c, osc, table, fondu: None, agitation: None }
    }

    /// Nouvelles amplitudes, en fondu de `duree` à partir de `t`.
    fn changer(&mut self, c: Vec<Cpx>, t: usize, duree: usize) {
        let neuf = Son::new(self.w, self.t0, std::mem::take(&mut self.rangs), c, Some(std::mem::take(&mut self.osc)));
        let ancien = std::mem::replace(&mut self.table, neuf.table);
        self.rangs = neuf.rangs;
        self.osc = neuf.osc;
        self.c = neuf.c;
        self.fondu = Some((ancien, t, duree));
    }

    #[inline]
    fn at(&self, t: usize) -> Cpx {
        let i = (t - self.t0) % self.w;
        let v = self.table[i];
        match &self.fondu {
            Some((ancien, d, n)) if t < d + n => {
                let r = rampe((t - d) as f32 / *n as f32);
                ancien[i] * (1.0 - r) + v * r
            }
            _ => v,
        }
    }
}

#[derive(Clone, Debug)]
enum Passage {
    Aucun,
    /// Joli : chaque harmonique glisse de son ancienne phase à la nouvelle
    /// à amplitude constante (le timbre ne bouge pas)
    Glissement { debut: usize, duree: usize, de: Vec<Cpx>, delta: Vec<f32> },
    /// Rapide : fondu des amplitudes complexes
    Fondu { debut: usize, duree: usize, ancien: Son },
    /// Changement de note (ou d'accord) : l'ancienne s'éteint, la
    /// nouvelle s'allume
    Articulation { debut: usize, ancien: Vec<Son> },
}

struct Donnees {
    t0: usize,
    ordo: Ordonnanceur,
    prochain: Symbole,
}

/// Émetteur d'une voix.
pub struct Emetteur {
    pub voix: Voix,
    pub duplex: Duplex,
    pub mode: Mode,
    /// Temps absolu (échantillons déjà produits)
    pub t: usize,
    cur: Option<Son>,
    /// Les autres notes d'un accord (motif seulement)
    accord: Vec<Son>,
    passage: Passage,
    motif: Option<(usize, usize)>,
    donnees: Option<Donnees>,
    chargement: Chargement,
    bases: Vec<Vec<Cpx>>,
    /// Indices MDP courants (joli, codage différentiel)
    idx: Vec<u32>,
    bits: VecDeque<u8>,
    /// Bits de remplissage (file vide) et bits de la file émis
    pub bourrage: usize,
    pub bits_emis: usize,
    /// Symboles de données émis
    pub symboles: usize,
    /// Hasard (déterministe) des notes agitées du motif
    hasard: u32,
    /// Symboles connus émis dans le motif (pour le récepteur d'en face)
    pub journal: Vec<SymboleConnu>,
    t_motif: usize,
}

impl Emetteur {
    pub fn new(voix: Voix, duplex: Duplex, mode: Mode) -> Self {
        let chargement = Chargement::uniforme(voix, duplex, mode, 2);
        let bases = DEGRES.iter().map(|&d| base_donnees(&chargement, d)).collect();
        Self {
            voix,
            duplex,
            mode,
            t: 0,
            cur: None,
            accord: Vec::new(),
            passage: Passage::Aucun,
            motif: None,
            donnees: None,
            chargement,
            bases,
            idx: Vec::new(),
            bits: VecDeque::new(),
            bourrage: 0,
            bits_emis: 0,
            symboles: 0,
            hasard: match voix {
                Voix::Appelant => 0x1234_5679,
                Voix::Appele => 0x8765_4321,
            },
            journal: Vec::new(),
            t_motif: 0,
        }
    }

    /// Chargement imposé par le récepteur d'en face.
    pub fn charger(&mut self, c: Chargement) {
        assert_eq!((c.voix, c.duplex, c.mode), (self.voix, self.duplex, self.mode), "chargement d'une autre voix");
        self.bases = DEGRES.iter().map(|&d| base_donnees(&c, d)).collect();
        self.chargement = c;
    }

    pub fn chargement(&self) -> &Chargement {
        &self.chargement
    }

    /// Bits à émettre (0 ou 1), dans l'ordre.
    pub fn pousser(&mut self, bits: impl IntoIterator<Item = u8>) {
        self.bits.extend(bits);
    }

    pub fn en_attente(&self) -> usize {
        self.bits.len()
    }

    /// Le motif d'appel commencera à l'instant absolu `t0`.
    pub fn programmer_motif(&mut self, t0: usize) {
        assert!(t0 >= self.t);
        self.motif = Some((t0, 0));
        self.t_motif = t0;
    }

    /// La mélodie (données) commencera à l'instant absolu `t0`.
    pub fn programmer_donnees(&mut self, t0: usize) {
        assert!(t0 >= self.t);
        let mut ordo = Ordonnanceur::new(self.voix);
        let prochain = ordo.next().unwrap();
        self.donnees = Some(Donnees { t0, ordo, prochain });
    }

    /// Le motif est-il fini (extinction comprise) ?
    pub fn motif_fini(&self) -> bool {
        self.motif.is_none()
    }

    pub fn en_donnees(&self) -> bool {
        self.donnees.as_ref().is_some_and(|d| self.t >= d.t0)
    }

    fn prendre(&mut self, b: u8) -> u32 {
        let mut v = 0u32;
        for _ in 0..b {
            let bit = match self.bits.pop_front() {
                Some(x) => {
                    self.bits_emis += 1;
                    x & 1
                }
                None => {
                    self.bourrage += 1;
                    0
                }
            };
            v = (v << 1) | bit as u32;
        }
        v
    }

    /// Fait sonner de nouvelles notes (vide : silence) après une articulation.
    fn jouer(&mut self, notes: &[(Degre, Vec<Cpx>)], t0: usize) {
        let mut ancien: Vec<Son> = self.cur.take().into_iter().collect();
        ancien.append(&mut self.accord);
        self.passage = Passage::Articulation { debut: self.t, ancien };
        for (d, c) in notes {
            let son = Son::new(periode(self.voix, *d), t0, harmoniques(self.voix, self.duplex, *d), c.clone(), None);
            if self.cur.is_none() {
                self.cur = Some(son);
            } else {
                self.accord.push(son);
            }
        }
    }

    fn evenements(&mut self) {
        let t = self.t;
        if let Some((t0, i)) = self.motif {
            if i < MOTIF.len() && t == t0 + debut_motif(i) {
                // un accord de n notes garde la puissance nominale
                let g = 1.0 / (MOTIF[i].0.len() as f32).sqrt();
                let notes: Vec<(Degre, Vec<Cpx>)> =
                    MOTIF[i].0.iter().map(|&d| (d, reference(self.voix, self.duplex, d).into_iter().map(|c| c * g).collect())).collect();
                self.jouer(&notes, t);
                if notes.len() > 1 {
                    // Accord agité : chaque note change de phases à chaque
                    // symbole (de sa propre durée), comme des données
                    for s in self.cur.iter_mut().chain(self.accord.iter_mut()) {
                        s.agitation = Some(Agitation { prochain: t + s.w + GARDE, fin: usize::MAX, fondu: FONDU_ACCORD, base: s.c.clone(), degre: None });
                    }
                } else {
                    // Note seule : tenue la première moitié, puis tremblant
                    // (symboles connus, découpés comme les données)
                    let l = MOTIF[i].1 * CROCHE;
                    let s = self.cur.as_mut().unwrap();
                    let p = s.w + GARDE;
                    let n = (l / 2) / p;
                    s.agitation = Some(Agitation { prochain: t + l / 2, fin: t + l / 2 + n * p, fondu: transition(self.mode), base: s.c.clone(), degre: Some(notes[0].0) });
                }
                self.motif = Some((t0, i + 1));
            } else if i == MOTIF.len() && t == t0 + duree_motif() {
                self.jouer(&[], t);
                self.motif = None;
            }
        }
        self.agiter();
        let s = match &self.donnees {
            Some(dn) if t == dn.t0 + dn.prochain.debut => (dn.prochain, dn.t0),
            _ => return,
        };
        let (s, t_d) = s;
        let dn = self.donnees.as_mut().unwrap();
        dn.prochain = dn.ordo.next().unwrap();
        let d = s.degre;
        let base = self.bases[d.index()].clone();
        if s.reference {
            self.idx = vec![0; base.len()];
            self.jouer(&[(d, base)], t_d + s.debut_note);
            return;
        }
        self.symboles += 1;
        let bits = self.chargement.bits[d.index()].clone();
        let mut c = base.clone();
        for (i, &b) in bits.iter().enumerate() {
            if b == 0 {
                continue;
            }
            let v = self.prendre(b);
            c[i] = match self.mode {
                Mode::Joli => {
                    let m = 1u32 << b;
                    self.idx[i] = (self.idx[i] + gray_inverse(v)) % m;
                    base[i] * mdp(b, self.idx[i])
                }
                Mode::Rapide => base[i] * maq(b, v),
            };
        }
        let ancien = self.cur.take().expect("symbole de données sans note");
        let duree = transition(self.mode);
        self.passage = match self.mode {
            Mode::Joli => {
                let delta = ancien.c.iter().zip(&c).map(|(a, b)| (*b * a.conj()).arg()).collect();
                Passage::Glissement { debut: t, duree, de: ancien.c.clone(), delta }
            }
            Mode::Rapide => Passage::Fondu { debut: t, duree, ancien: ancien.clone() },
        };
        self.cur = Some(Son::new(ancien.w, ancien.t0, ancien.rangs.clone(), c, Some(ancien.osc.clone())));
    }

    fn agiter(&mut self) {
        let t = self.t;
        let mut h = self.hasard;
        for s in self.cur.iter_mut().chain(self.accord.iter_mut()) {
            let a = match &mut s.agitation {
                Some(a) if a.prochain == t && t < a.fin => a,
                _ => continue,
            };
            let c: Vec<Cpx> = a
                .base
                .iter()
                .map(|z| {
                    h ^= h << 13;
                    h ^= h >> 17;
                    h ^= h << 5;
                    *z * mdp(3, h >> 29)
                })
                .collect();
            a.prochain += s.w + GARDE;
            let (fondu, degre) = (a.fondu, a.degre);
            if let Some(d) = degre {
                self.journal.push(SymboleConnu { fenetre: t + GARDE - self.t_motif, degre: d, debut_note: s.t0 - self.t_motif, c: c.clone() });
            }
            s.changer(c, t, fondu);
        }
        self.hasard = h;
    }

    /// Échantillon suivant, analytique (la partie réelle part sur la ligne).
    pub fn suivant(&mut self) -> Cpx {
        self.evenements();
        let t = self.t;
        self.t += 1;
        let mut nouveau = self.cur.as_ref().map(|s| s.at(t)).unwrap_or(Cpx::ZERO);
        for s in &self.accord {
            nouveau += s.at(t);
        }
        match &self.passage {
            Passage::Aucun => nouveau,
            Passage::Glissement { debut, duree, de, delta } => {
                let u = t - debut;
                if u >= *duree {
                    self.passage = Passage::Aucun;
                    return nouveau;
                }
                let r = rampe((u as f32 + 0.5) / *duree as f32);
                let s = self.cur.as_ref().unwrap();
                let tau = t - s.t0;
                let mut z = Cpx::ZERO;
                for ((k, c), dl) in s.rangs.iter().zip(de).zip(delta) {
                    z += *c * Cpx::from_polar(1.0, r * dl) * s.osc[(k * tau) % s.w];
                }
                z
            }
            Passage::Fondu { debut, duree, ancien } => {
                let u = t - debut;
                if u >= *duree {
                    self.passage = Passage::Aucun;
                    return nouveau;
                }
                let r = rampe((u as f32 + 0.5) / *duree as f32);
                ancien.at(t) * (1.0 - r) + nouveau * r
            }
            Passage::Articulation { debut, ancien } => {
                let u = t - debut;
                if u >= ARTICULATION {
                    self.passage = Passage::Aucun;
                    return nouveau;
                }
                let mut z = Cpx::ZERO;
                if u < EXTINCTION {
                    let g = 1.0 - rampe((u as f32 + 0.5) / EXTINCTION as f32);
                    for a in ancien {
                        z += a.at(t) * g;
                    }
                }
                if u >= ALLUMAGE {
                    z += nouveau * rampe((u - ALLUMAGE) as f32 / (ARTICULATION - ALLUMAGE) as f32);
                }
                z
            }
        }
    }

    /// Échantillon suivant pour la ligne.
    pub fn echantillon(&mut self) -> f32 {
        self.suivant().re
    }

    pub fn remplir(&mut self, out: &mut [f32]) {
        for o in out {
            *o = self.echantillon();
        }
    }
}

/// Le motif d'appel d'une voix, en signal analytique (pour le filtre
/// adapté du récepteur), extinction comprise, et ses symboles connus.
pub fn motif_analytique(voix: Voix, duplex: Duplex, mode: Mode) -> (Vec<Cpx>, Vec<SymboleConnu>) {
    let mut e = Emetteur::new(voix, duplex, mode);
    e.programmer_motif(0);
    let x = (0..duree_motif() + EXTINCTION).map(|_| e.suivant()).collect();
    (x, e.journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NOMINAL_RMS;
    use crate::orgue::partition::f0;

    fn puissance(x: &[f32]) -> f32 {
        x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32
    }

    #[test]
    fn le_motif_est_une_note_tenue_au_niveau_nominal() {
        for voix in [Voix::Appelant, Voix::Appele] {
            let (m, journal) = motif_analytique(voix, Duplex::PleineBande, Mode::Joli);
            let re: Vec<f32> = m.iter().map(|z| z.re).collect();
            // première note (ré), loin des articulations : périodique et au
            // niveau sur sa première moitié (la seconde tremble)
            let w = periode(voix, Degre::Re);
            let seg = &re[200..CROCHE / 2];
            for i in 0..seg.len() - w {
                assert!((seg[i] - seg[i + w]).abs() < 1e-4);
            }
            let p = puissance(seg).sqrt();
            assert!((p - NOMINAL_RMS).abs() < 0.01, "{voix:?} : {p}");
            assert!((f0(voix, Degre::Re) - 8000.0 / w as f32).abs() < 1e-3);
            // chaque note seule du motif tremble sur sa seconde moitié
            assert!(journal.len() > 7 * 5, "{}", journal.len());
            assert!(journal.iter().all(|s| s.fenetre >= s.debut_note + CROCHE / 2 + GARDE));
        }
    }

    #[test]
    fn mode_joli_sans_donnees_une_tenue_parfaite() {
        // des zéros en mode joli : phases inchangées → la note est périodique
        let mut e = Emetteur::new(Voix::Appelant, Duplex::PleineBande, Mode::Joli);
        e.programmer_donnees(0);
        let x: Vec<f32> = (0..4800).map(|_| e.echantillon()).collect();
        let w = periode(Voix::Appelant, Degre::Do);
        for i in ARTICULATION..4800 - w {
            assert!((x[i] - x[i + w]).abs() < 1e-4, "{i}");
        }
    }
}
