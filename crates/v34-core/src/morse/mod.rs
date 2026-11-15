//! Le morse : un mode « propriétaire », négocié en V.8 par les services
//! non normalisés (§6.6/V.8, voir [`crate::v8::NonNormalise`]).
//!
//! L'appelant (DIP 4 « MORSE ») glisse dans son CM un bloc « France,
//! fournisseur TV (TINYVERSE), M + vitesse » ; l'appelé qui sait faire le
//! recopie dans son JM. Après le CJ, au lieu de la phase 2 du V.34, chacun manipule un ton pur en
//! tout-ou-rien, à sa fréquence :
//!
//! - l'appelant émet à [`F_APPELANT`] (800 Hz, la note du télégraphiste) ;
//! - l'appelé à [`F_APPELE`] (1600 Hz, une octave au-dessus).
//!
//! Le duplex est intégral : chacun écoute la fréquence de l'autre, assez loin
//! de la sienne pour que son propre écho (−20 dB sur une longue ligne, aussi
//! fort que le signal d'en face) disparaisse dans le filtre de l'écoute.
//!
//! Le code est le vrai code morse international (UIT-R M.1677), avec les
//! lettres accentuées françaises (É, È, À, Ç…) et les procédures : KA
//! (début de message), AR (fin de message), SK (fin de contact, on
//! raccroche). Cadence PARIS : à 20 mots par minute, le point dure 60 ms.
//!
//! - [`code`] : la table, le texte en signes et retour ;
//! - [`manipulateur`] : l'émission (le ton, ses fronts adoucis) ;
//! - [`ecoute`] : la réception (filtre étroit, seuil adaptatif, rythme) ;
//! - [`session`] : les deux réunis, branchés sur le modem.

pub mod code;
pub mod ecoute;
pub mod manipulateur;
pub mod session;

use serde::{Deserialize, Serialize};

use crate::api::SAMPLE_RATE;

/// Vitesse proposée : 20 mots par minute (cadence PARIS).
pub const MPM: u8 = 20;
/// Vitesses acceptées en négociation (mots par minute).
pub const MPM_MIN: u8 = 5;
pub const MPM_MAX: u8 = 40;
/// Fréquence du manipulateur de l'appelant (Hz).
pub const F_APPELANT: f64 = 800.0;
/// Fréquence du manipulateur de l'appelé (Hz).
pub const F_APPELE: f64 = 1600.0;

/// Durées du code, en échantillons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    /// Le point, unité de tout le reste (un trait = 3 points)
    pub point: u32,
    /// Silence entre deux lettres (3 points, plus en Farnsworth)
    pub lettre: u32,
    /// Silence entre deux mots (7 points, plus en Farnsworth)
    pub mot: u32,
}

impl Cadence {
    /// Cadence PARIS : le mot « PARIS » et son espace font 50 points, donc
    /// le point dure 1,2 s / mpm.
    pub fn paris(mpm: u8) -> Self {
        let point = SAMPLE_RATE * 6 / 5 / mpm.max(1) as u32;
        Cadence { point, lettre: 3 * point, mot: 7 * point }
    }

    /// Farnsworth : les lettres à `mpm_lettres`, les silences étirés pour
    /// tomber à `mpm` mots par minute en moyenne (méthode de l'ARRL).
    pub fn farnsworth(mpm_lettres: u8, mpm: u8) -> Self {
        let (c, s) = (mpm_lettres.max(mpm).max(1) as f64, mpm.max(1) as f64);
        let mut cad = Cadence::paris(c as u8);
        if s < c {
            // retard total réparti sur les 19 unités d'espacement du mot PARIS
            let ta = (60.0 * c - 37.2 * s) / (s * c);
            cad.lettre = (3.0 * ta / 19.0 * SAMPLE_RATE as f64).round() as u32;
            cad.mot = (7.0 * ta / 19.0 * SAMPLE_RATE as f64).round() as u32;
        }
        cad
    }

    /// Point en millisecondes.
    pub fn point_ms(&self) -> f32 {
        self.point as f32 * 1000.0 / SAMPLE_RATE as f32
    }
}

/// Débit « texte » d'une liaison morse (bit/s) : PARIS fait six caractères
/// de huit bits par mot, soit 0,8 bit/s par mot/minute (16 bit/s à 20).
pub fn debit_equivalent(mpm: u8) -> u32 {
    mpm as u32 * 4 / 5
}

/// Événements du morse (dans `ModemEvent::Morse`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MorseEvenement {
    /// Un signe commence à partir sur la ligne : son texte (« A », « É »,
    /// « <AR> »…) et son code (« .- »)
    Emis { texte: String, code: String },
    /// Un signe vient d'être reconnu à l'écoute (« * » : code inconnu ;
    /// « » : espace entre deux mots)
    Recu { texte: String, code: String },
}

/// Le morse en direct, pour l'afficheur et le karaoké (`ModemStatus::morse`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MorseVue {
    /// Vitesse convenue (mots par minute)
    pub mpm: u8,
    /// Fréquences d'émission et d'écoute (Hz)
    pub f_tx: f32,
    pub f_rx: f32,
    /// Notre manipulateur : touche baissée en ce moment
    pub tx_cle: bool,
    /// Signe en cours d'émission, son code, et ses éléments déjà partis
    pub tx_texte: String,
    pub tx_code: String,
    pub tx_fait: u8,
    /// Signes encore en attente d'émission
    pub tx_attente: u32,
    /// L'écoute : ton entendu en ce moment
    pub rx_cle: bool,
    /// Points et traits du signe en cours de réception
    pub rx_code: String,
    /// Dernier signe reconnu et son code
    pub rx_texte: String,
    pub rx_dernier_code: String,
    /// Écart entre le ton reçu et le bruit, dans le filtre de l'écoute (dB)
    pub snr_db: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_paris_et_farnsworth() {
        let c = Cadence::paris(20);
        assert_eq!(c.point, 480, "60 ms à 8 kHz");
        assert_eq!((c.lettre, c.mot), (1440, 3360));
        assert!((c.point_ms() - 60.0).abs() < 1e-3);
        // Farnsworth 18/5 : lettres rapides, grands silences
        let f = Cadence::farnsworth(18, 5);
        assert_eq!(f.point, Cadence::paris(18).point);
        assert!(f.lettre > 3 * f.point && f.mot > f.lettre * 2, "{f:?}");
        // sans ralentissement, c'est du PARIS
        assert_eq!(Cadence::farnsworth(20, 20), Cadence::paris(20));
        assert_eq!(debit_equivalent(20), 16);
    }
}
