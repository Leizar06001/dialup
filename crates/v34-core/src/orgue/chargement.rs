//! Chargement en bits : combien de bits porte chaque harmonique de chaque
//! note, choisi par le récepteur d'après le bruit qu'il a mesuré pendant
//! le motif d'appel (et rendu à l'émetteur d'en face).

use super::constellation::{bits_max, snr_requis_db};
use super::partition::{f0, harmoniques, reference, registration, Ordonnanceur, DEGRES, DUO_BAS, DUO_HAUT, MELODIE, TEMPS};
use super::{Duplex, Mode, Voix};
use crate::api::NOMINAL_RMS;
use crate::dsp::Cpx;

/// Mode rapide : une harmonique sur six est un pilote (phase et amplitude
/// fixes, une tenue d'orgue), pour suivre la phase commune sans ambiguïté.
pub fn pilote(mode: Mode, i: usize) -> bool {
    mode == Mode::Rapide && i % 6 == 3
}

/// Marge supplémentaire (dB) près de la coupure du duo : notre propre
/// écho (l'autre bande), mal annulé et pas orthogonal aux harmoniques
/// d'en face, déborde sur les plus proches — et le motif ne le voit pas
/// (on s'y tait).
pub const MARGE_VOISINAGE_DB: f32 = 4.0;

fn marge_voisinage(voix: Voix, duplex: Duplex, f: f32) -> f32 {
    let pres = match (duplex, voix) {
        (Duplex::PleineBande, _) => false,
        (Duplex::Duo, Voix::Appelant) => f > DUO_BAS - 250.0,
        (Duplex::Duo, Voix::Appele) => f < DUO_HAUT + 250.0,
    };
    if pres {
        MARGE_VOISINAGE_DB
    } else {
        0.0
    }
}

/// Bits par harmonique, pour chaque degré (indexé par `Degre::index`).
#[derive(Clone, Debug, PartialEq)]
pub struct Chargement {
    pub mode: Mode,
    pub duplex: Duplex,
    /// La voix qui émet avec ce chargement
    pub voix: Voix,
    pub bits: Vec<Vec<u8>>,
}

impl Chargement {
    /// Même nombre de bits partout (avant d'avoir mesuré la ligne).
    pub fn uniforme(voix: Voix, duplex: Duplex, mode: Mode, b: u8) -> Self {
        let bits = DEGRES
            .iter()
            .map(|&d| (0..harmoniques(voix, duplex, d).len()).map(|i| if pilote(mode, i) { 0 } else { b }).collect())
            .collect();
        Self { mode, duplex, voix, bits }
    }

    /// Choisit les bits d'après le SNR mesuré (dB, par degré et par
    /// harmonique, avec les amplitudes du motif), avec une marge (dB).
    pub fn depuis_snr(voix: Voix, duplex: Duplex, mode: Mode, snr_motif_db: &[Vec<f32>], marge_db: f32) -> Self {
        let mut bits = Vec::new();
        for &d in DEGRES.iter() {
            let reg = registration(voix, duplex, d);
            let n = reg.len();
            // En mode rapide toutes les harmoniques ont la même amplitude
            // (au pire toutes actives : l'estimation est prudente).
            let a_plat = NOMINAL_RMS * (2.0 / n as f32).sqrt();
            let snr = &snr_motif_db[d.index()];
            let rangs = harmoniques(voix, duplex, d);
            let v: Vec<u8> = (0..n)
                .map(|i| {
                    if pilote(mode, i) {
                        return 0;
                    }
                    let gain = match mode {
                        Mode::Joli => 0.0,
                        Mode::Rapide => 20.0 * (a_plat / reg[i]).log10(),
                    };
                    let f = rangs[i] as f32 * f0(voix, d);
                    let s = snr.get(i).copied().unwrap_or(-100.0) + gain - marge_db - marge_voisinage(voix, duplex, f);
                    (1..=bits_max(mode)).rev().find(|&b| snr_requis_db(mode, b) <= s).unwrap_or(0)
                })
                .collect();
            bits.push(v);
        }
        Self { mode, duplex, voix, bits }
    }

    /// Bits portés par un symbole de données de ce degré.
    pub fn bits_par_symbole(&self, d: super::partition::Degre) -> usize {
        self.bits[d.index()].iter().map(|&b| b as usize).sum()
    }

    /// Débit net (bit/s) moyen sur une boucle de la mélodie : symboles de
    /// référence, gardes et articulations déduits.
    pub fn debit_net(&self) -> f32 {
        let mut total = 0usize;
        for (i, (d, _)) in MELODIE.iter().enumerate() {
            total += Ordonnanceur::symboles_de_note(self.voix, i) * self.bits_par_symbole(*d);
        }
        let duree: usize = MELODIE.iter().map(|(_, t)| t * TEMPS).sum();
        total as f32 * super::partition::FE / duree as f32
    }

    /// Bits moyens par harmonique active (données), sur les notes de la mélodie.
    pub fn bits_moyens(&self) -> f32 {
        let (mut s, mut n) = (0usize, 0usize);
        for (d, _) in MELODIE {
            for &b in &self.bits[d.index()] {
                s += b as usize;
                n += 1;
            }
        }
        s as f32 / n.max(1) as f32
    }
}

/// Amplitudes complexes de base des données (avant modulation) pour un
/// degré : la registration en mode joli ; en mode rapide, une amplitude
/// égale pour les harmoniques actives (chargées ou pilotes), zéro sinon.
pub fn base_donnees(c: &Chargement, d: super::partition::Degre) -> Vec<Cpx> {
    let r = reference(c.voix, c.duplex, d);
    match c.mode {
        Mode::Joli => r,
        Mode::Rapide => {
            let bits = &c.bits[d.index()];
            let actif: Vec<bool> = (0..r.len()).map(|i| bits[i] > 0 || pilote(c.mode, i)).collect();
            let n = actif.iter().filter(|a| **a).count().max(1);
            let a = NOMINAL_RMS * (2.0 / n as f32).sqrt();
            r.iter().zip(&actif).map(|(z, &on)| if on { *z * (a / z.abs()) } else { Cpx::ZERO }).collect()
        }
    }
}
