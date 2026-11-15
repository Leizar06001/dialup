//! La partition : l'accord (des périodes entières d'échantillons), la
//! mélodie, le motif d'appel, les registres, la registration (timbre) et
//! le découpage du temps en symboles.
//!
//! Tout ici est déterministe et connu des deux bouts : c'est ce qui permet
//! au récepteur de savoir, à l'échantillon près, quelle note joue l'autre
//! et où commence chaque symbole.

use std::f32::consts::PI;

use super::{Duplex, Mode, Voix};
use crate::api::NOMINAL_RMS;
use crate::dsp::Cpx;

/// Fréquence d'échantillonnage (Hz).
pub const FE: f32 = 8000.0;
/// Un temps (noire) de la mélodie : 0,6 s, soit 100 à la noire.
pub const TEMPS: usize = 4800;
/// Unité du motif d'appel : la croche (0,3 s).
pub const CROCHE: usize = TEMPS / 2;
/// Changement de note : l'ancienne s'éteint sur `[0, EXTINCTION)`, la
/// nouvelle s'allume sur `[ALLUMAGE, ARTICULATION)` (cosinus surélevés).
/// Le petit creux au milieu détache les notes répétées (« do do do »).
pub const EXTINCTION: usize = 64;
pub const ALLUMAGE: usize = 32;
pub const ARTICULATION: usize = 96;
/// Préfixe cyclique (« la note qui continue ») : 4 ms.
pub const GARDE: usize = 32;
/// Transition entre deux symboles d'une même note, au début de la garde.
/// Le reste de la garde (`GARDE - transition`) absorbe la dispersion de
/// la ligne.
pub fn transition(mode: Mode) -> usize {
    match mode {
        Mode::Joli => 16,
        Mode::Rapide => 8,
    }
}

/// Bande téléphonique utilisée (Hz).
pub const F_BAS: f32 = 320.0;
pub const F_HAUT: f32 = 3350.0;
/// Partage du duo : l'appelant (grave) en dessous, l'appelé (aigu) au-dessus.
pub const DUO_BAS: f32 = 1650.0;
pub const DUO_HAUT: f32 = 1950.0;

/// Les degrés joués (« do mobile »). `DoGrave` est l'octave du dessous
/// (le « fa grave » des Rencontres du troisième type) ; `Sol`, `La`, `Si`
/// sont sous la tonique.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Degre {
    DoGrave,
    Sol,
    La,
    Si,
    Do,
    Re,
    Mi,
}

pub const DEGRES: [Degre; 7] = [Degre::DoGrave, Degre::Sol, Degre::La, Degre::Si, Degre::Do, Degre::Re, Degre::Mi];

impl Degre {
    /// Période (échantillons) pour la voix grave. Choisies par recherche
    /// exhaustive : avec une tonique de 108 échantillons (74,07 Hz), chaque
    /// intervalle de la mélodie et du motif est à moins de 10 cents du
    /// tempérament égal, ET toutes les périodes sont paires, si bien que
    /// l'appelé (périodes / 2) joue des octaves exactes.
    pub fn periode_grave(self) -> usize {
        match self {
            Degre::DoGrave => 216,
            Degre::Sol => 144,
            Degre::La => 128,
            Degre::Si => 114,
            Degre::Do => 108,
            Degre::Re => 96,
            Degre::Mi => 86,
        }
    }

    /// Demi-tons au-dessus de la tonique (tempérament égal visé).
    pub fn demi_tons(self) -> i32 {
        match self {
            Degre::DoGrave => -12,
            Degre::Sol => -5,
            Degre::La => -3,
            Degre::Si => -1,
            Degre::Do => 0,
            Degre::Re => 2,
            Degre::Mi => 4,
        }
    }

    pub fn nom(self) -> &'static str {
        match self {
            Degre::DoGrave => "do grave",
            Degre::Sol => "sol",
            Degre::La => "la",
            Degre::Si => "si",
            Degre::Do => "do",
            Degre::Re => "ré",
            Degre::Mi => "mi",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Période (échantillons) d'un degré pour une voix : l'appelé joue une
/// octave au-dessus.
pub fn periode(voix: Voix, d: Degre) -> usize {
    match voix {
        Voix::Appelant => d.periode_grave(),
        Voix::Appele => d.periode_grave() / 2,
    }
}

/// Fréquence fondamentale (Hz).
pub fn f0(voix: Voix, d: Degre) -> f32 {
    FE / periode(voix, d) as f32
}

/// « Au clair de la lune » (degré, durée en temps) : 4 phrases, 64 temps,
/// 38,4 s, jouée en boucle.
pub const MELODIE: [(Degre, usize); 44] = {
    use Degre::*;
    [
        // Au clair de la lune, mon ami Pierrot
        (Do, 1), (Do, 1), (Do, 1), (Re, 1), (Mi, 2), (Re, 2), (Do, 1), (Mi, 1), (Re, 1), (Re, 1), (Do, 4),
        // Prête-moi ta plume pour écrire un mot
        (Do, 1), (Do, 1), (Do, 1), (Re, 1), (Mi, 2), (Re, 2), (Do, 1), (Mi, 1), (Re, 1), (Re, 1), (Do, 4),
        // Ma chandelle est morte, je n'ai plus de feu
        (Re, 1), (Re, 1), (Re, 1), (Re, 1), (La, 2), (La, 2), (Re, 1), (Do, 1), (Si, 1), (La, 1), (Sol, 4),
        // Ouvre-moi ta porte pour l'amour de Dieu
        (Do, 1), (Do, 1), (Do, 1), (Re, 1), (Mi, 2), (Re, 2), (Do, 1), (Mi, 1), (Re, 1), (Re, 1), (Do, 4),
    ]
};

/// Durée de la boucle mélodique (échantillons).
pub fn duree_melodie() -> usize {
    MELODIE.iter().map(|(_, t)| t * TEMPS).sum()
}

/// Motif d'appel (notes jouées ensemble, durée en croches) : les cinq
/// notes des *Rencontres du troisième type* (ré mi do do↓ sol↓), puis
/// « la si ». Chaque note est tenue sa première moitié, phases connues :
/// le récepteur y cale le rythme, la fréquence, le canal et le bruit de
/// chaque harmonique de chaque note de la mélodie (toutes y figurent) ;
/// elle tremble sa seconde moitié (symboles connus découpés comme les
/// données) : il y mesure l'interférence que verront les données. Puis la
/// mineur et sol majeur, deux fois (la dominante appelle le do de la
/// mélodie), en accords agités : à eux deux ils font sonner *ensemble*
/// toutes les notes de la mélodie, étalées comme des données — ce qu'il
/// faut à notre annuleur d'écho pour apprendre l'écho sur toute la bande.
pub const MOTIF: [(&[Degre], usize); 11] = {
    use Degre::*;
    [
        (&[Re], 1),
        (&[Mi], 1),
        (&[Do], 1),
        (&[DoGrave], 1),
        (&[Sol], 2),
        (&[La], 1),
        (&[Si], 1),
        (&[La, Do, Mi], 1),
        (&[Sol, Si, Re], 1),
        (&[La, Do, Mi], 1),
        (&[Sol, Si, Re], 2),
    ]
};

/// La note seule jouée au pas `i` du motif (`None` pour un accord).
pub fn note_du_motif(i: usize) -> Option<Degre> {
    match MOTIF[i].0 {
        [d] => Some(*d),
        _ => None,
    }
}

/// Durée du motif (échantillons), sans l'extinction de la dernière note.
pub fn duree_motif() -> usize {
    MOTIF.iter().map(|(_, c)| c * CROCHE).sum()
}

/// Début du pas `i` du motif (échantillons depuis le début du motif).
pub fn debut_motif(i: usize) -> usize {
    MOTIF[..i].iter().map(|(_, c)| c * CROCHE).sum()
}

/// Bande (Hz) des harmoniques d'une voix.
pub fn bande(voix: Voix, duplex: Duplex) -> (f32, f32) {
    match (duplex, voix) {
        (Duplex::PleineBande, _) => (F_BAS, F_HAUT),
        (Duplex::Duo, Voix::Appelant) => (F_BAS, DUO_BAS),
        (Duplex::Duo, Voix::Appele) => (DUO_HAUT, F_HAUT),
    }
}

/// Rangs des harmoniques jouées (= cases de la TFD sur une période).
pub fn harmoniques(voix: Voix, duplex: Duplex, d: Degre) -> Vec<usize> {
    let w = periode(voix, d) as f32;
    let (lo, hi) = bande(voix, duplex);
    let a = (lo * w / FE).ceil() as usize;
    let b = (hi * w / FE).floor() as usize;
    (a.max(1)..=b).collect()
}

/// Phases de référence (Newman : facteur de crête faible, son « lisse »).
pub fn phases_reference(k: usize) -> Vec<f32> {
    (0..k).map(|i| PI * (i * i) as f32 / k as f32).collect()
}

/// Registration (timbre « principal ») : amplitude ∝ f^-1/2, soit -3 dB
/// par octave ; puissance totale = puissance nominale.
pub fn registration(voix: Voix, duplex: Duplex, d: Degre) -> Vec<f32> {
    let w = periode(voix, d) as f32;
    let h = harmoniques(voix, duplex, d);
    let brut: Vec<f32> = h.iter().map(|&k| (k as f32 * FE / w / F_BAS).powf(-0.5)).collect();
    let p: f32 = brut.iter().map(|a| a * a / 2.0).sum();
    let g = NOMINAL_RMS / p.sqrt();
    brut.iter().map(|a| a * g).collect()
}

/// Amplitudes complexes de référence (registration × phases de Newman) :
/// ce que jouent le motif et le premier symbole de chaque note.
pub fn reference(voix: Voix, duplex: Duplex, d: Degre) -> Vec<Cpx> {
    let a = registration(voix, duplex, d);
    let ph = phases_reference(a.len());
    a.iter().zip(&ph).map(|(&a, &p)| Cpx::from_polar(a, p)).collect()
}

/// Un symbole de la partie de données.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symbole {
    /// Rang de la note dans la mélodie
    pub note: usize,
    pub degre: Degre,
    pub periode: usize,
    /// Début de la note : origine des phases de ses harmoniques
    /// (échantillons depuis le début des données)
    pub debut_note: usize,
    /// Début du symbole (garde comprise)
    pub debut: usize,
    /// Début de la fenêtre utile (une période)
    pub fenetre: usize,
    /// Premier symbole de la note : phases connues (« l'attaque »)
    pub reference: bool,
}

/// Découpe la mélodie (en boucle) en symboles. Chaque note commence pile
/// sur son temps : symbole de référence (articulation + garde + le reste
/// de la division), puis autant de symboles de données que possible.
#[derive(Clone, Debug)]
pub struct Ordonnanceur {
    voix: Voix,
    note: usize,
    debut_note: usize,
    /// Symbole suivant dans la note courante (0 = référence)
    rang: usize,
    n_donnees: usize,
    reste: usize,
}

impl Ordonnanceur {
    pub fn new(voix: Voix) -> Self {
        let mut o = Self { voix, note: 0, debut_note: 0, rang: 0, n_donnees: 0, reste: 0 };
        o.preparer();
        o
    }

    fn preparer(&mut self) {
        let (d, t) = MELODIE[self.note % MELODIE.len()];
        let w = periode(self.voix, d);
        let l = t * TEMPS;
        let fixe = ARTICULATION + GARDE + w;
        self.n_donnees = (l - fixe) / (w + GARDE);
        self.reste = l - fixe - self.n_donnees * (w + GARDE);
        self.rang = 0;
    }

    /// Symboles de données par note (hors référence), pour la note `i`.
    pub fn symboles_de_note(voix: Voix, i: usize) -> usize {
        let (d, t) = MELODIE[i % MELODIE.len()];
        let w = periode(voix, d);
        (t * TEMPS - ARTICULATION - GARDE - w) / (w + GARDE)
    }
}

impl Iterator for Ordonnanceur {
    type Item = Symbole;

    fn next(&mut self) -> Option<Symbole> {
        if self.rang > self.n_donnees {
            let (_, t) = MELODIE[self.note % MELODIE.len()];
            self.debut_note += t * TEMPS;
            self.note += 1;
            self.preparer();
        }
        let (d, _) = MELODIE[self.note % MELODIE.len()];
        let w = periode(self.voix, d);
        let s = if self.rang == 0 {
            Symbole {
                note: self.note,
                degre: d,
                periode: w,
                debut_note: self.debut_note,
                debut: self.debut_note,
                fenetre: self.debut_note + ARTICULATION + GARDE + self.reste,
                reference: true,
            }
        } else {
            let debut = self.debut_note + ARTICULATION + GARDE + self.reste + w + (self.rang - 1) * (w + GARDE);
            Symbole { note: self.note, degre: d, periode: w, debut_note: self.debut_note, debut, fenetre: debut + GARDE, reference: false }
        };
        self.rang += 1;
        Some(s)
    }
}

/// Écart (cents) entre deux fréquences.
pub fn cents(f: f32, reference: f32) -> f32 {
    1200.0 * (f / reference).log2()
}

/// Justesse de l'accord : (pire écart d'un intervalle mélodique au
/// tempérament égal, pire écart absolu au la 440, octave voix/voix) en
/// cents. Les intervalles parcourus sont ceux de la mélodie et du motif.
pub fn justesse() -> (f32, f32, f32) {
    let mut suite: Vec<Degre> = MELODIE.iter().map(|(d, _)| *d).collect();
    suite.extend((0..MOTIF.len()).filter_map(note_du_motif));
    let mut pire_intervalle = 0.0f32;
    let mut pire_absolu = 0.0f32;
    let mut pire_octave = 0.0f32;
    for voix in [Voix::Appelant, Voix::Appele] {
        for w in suite.windows(2) {
            let (a, b) = (w[0], w[1]);
            let e = cents(f0(voix, b), f0(voix, a)) - 100.0 * (b.demi_tons() - a.demi_tons()) as f32;
            pire_intervalle = pire_intervalle.max(e.abs());
        }
        for d in DEGRES {
            let midi = 69.0 + 12.0 * (f0(voix, d) / 440.0).log2();
            pire_absolu = pire_absolu.max(100.0 * (midi - midi.round()).abs());
        }
    }
    for d in DEGRES {
        let e = cents(f0(Voix::Appele, d), f0(Voix::Appelant, d)) - 1200.0;
        pire_octave = pire_octave.max(e.abs());
    }
    (pire_intervalle, pire_absolu, pire_octave)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accord_juste_a_dix_cents_pres() {
        let (intervalle, absolu, octave) = justesse();
        assert!(intervalle < 10.0, "intervalle {intervalle:.1} cents");
        assert!(absolu < 25.0, "absolu {absolu:.1} cents");
        assert_eq!(octave, 0.0, "l'appelé joue des octaves exactes");
        for d in DEGRES {
            assert_eq!(d.periode_grave() % 2, 0);
        }
    }

    #[test]
    fn melodie_de_64_temps() {
        assert_eq!(MELODIE.iter().map(|(_, t)| t).sum::<usize>(), 64);
        // toutes les notes de la mélodie sont présentées dans le motif
        for (d, _) in MELODIE {
            assert!((0..MOTIF.len()).any(|i| note_du_motif(i) == Some(d)), "{d:?} absent du motif");
        }
    }

    #[test]
    fn symboles_contigus_et_notes_sur_le_temps() {
        for voix in [Voix::Appelant, Voix::Appele] {
            let mut o = Ordonnanceur::new(voix);
            let mut fin_prec = 0usize;
            let mut debut_note = 0usize;
            let mut i_note = 0usize;
            for _ in 0..5000 {
                let s = o.next().unwrap();
                if s.reference {
                    assert_eq!(s.debut, fin_prec, "{voix:?} : la note commence où finit la précédente");
                    assert_eq!(s.debut_note, debut_note);
                    assert!(s.fenetre - s.debut >= ARTICULATION + GARDE);
                    debut_note += MELODIE[i_note % MELODIE.len()].1 * TEMPS;
                    i_note += 1;
                } else {
                    assert_eq!(s.debut, fin_prec);
                    assert_eq!(s.fenetre, s.debut + GARDE);
                }
                fin_prec = s.fenetre + s.periode;
                // la fenêtre ne déborde jamais sur la note suivante
                assert!(fin_prec <= debut_note, "{voix:?} : {s:?}");
            }
        }
    }

    #[test]
    fn harmoniques_dans_la_bande() {
        for duplex in [Duplex::Duo, Duplex::PleineBande] {
            for voix in [Voix::Appelant, Voix::Appele] {
                for d in DEGRES {
                    let (lo, hi) = bande(voix, duplex);
                    let h = harmoniques(voix, duplex, d);
                    assert!(!h.is_empty());
                    for k in h {
                        let f = k as f32 * f0(voix, d);
                        assert!(f >= lo - 0.01 && f <= hi + 0.01);
                    }
                    let p: f32 = registration(voix, duplex, d).iter().map(|a| a * a / 2.0).sum();
                    assert!((p.sqrt() - NOMINAL_RMS).abs() < 1e-4);
                }
            }
        }
    }
}
