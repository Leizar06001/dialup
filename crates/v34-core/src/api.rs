//! Types publics partagés entre le moteur, l'appli et le réseau.
//!
//! Convention audio : 8000 Hz, mono, f32, pleine échelle = ±1.0.
//! Le niveau nominal d'émission (V.2 : -9/-10 dBm0) est fixé à NOMINAL_RMS.

use serde::{Deserialize, Serialize};

/// Fréquence d'échantillonnage du « réseau téléphonique » (G.711).
pub const SAMPLE_RATE: u32 = 8000;
/// Valeur efficace du signal émis au niveau nominal (≈ -12 dBFS).
pub const NOMINAL_RMS: f32 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// Modem appelant (ATD)
    Call,
    /// Modem appelé (ATA)
    Answer,
}

/// Grandes étapes de la connexion, pour l'affichage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Idle,
    /// V.8 : ANSam, CM/JM/CJ
    Phase1,
    /// Sondage / mesure du délai : INFO0, tons A/B, L1/L2, INFO1
    Phase2,
    /// Entraînement égaliseur : S, PP, TRN, J
    Phase3,
    /// Entraînement final : TRN, MP, E, B1
    Phase4,
    /// Mode données
    Data,
    /// Réentraînement ou renégociation de débit en cours
    Retrain,
    /// Raccrochage
    Cleardown,
    /// Échec de la négociation
    Failed,
    /// Télécopie (T.30 / V.29) en cours
    Fax,
    /// Minitel (V.23 1200/75) : le sifflement, puis l'écran Vidéotex
    Minitel,
    /// Morse : mode propriétaire négocié en V.8 (services non normalisés)
    Morse,
}

/// Les LEDs d'un modem externe des années 90.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leds {
    /// High Speed : connecté à ≥ 14400 bit/s
    pub hs: bool,
    /// Auto Answer : répondra automatiquement
    pub aa: bool,
    /// Carrier Detect : porteuse distante détectée (mode données)
    pub cd: bool,
    /// Off Hook : décroché
    pub oh: bool,
    /// Receive Data : octets reçus récemment
    pub rd: bool,
    /// Send Data : octets envoyés récemment
    pub sd: bool,
    /// Terminal Ready
    pub tr: bool,
    /// Modem Ready
    pub mr: bool,
}

/// Paramètres de liaison négociés / mesurés. `None` = pas encore connu.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LinkParams {
    /// Modes communs annoncés dans JM (ex. "V.34", "V.32bis")
    pub v8_modes: Vec<String>,
    pub symbol_rate_tx: Option<u32>,
    pub symbol_rate_rx: Option<u32>,
    pub carrier_tx_hz: Option<f32>,
    pub carrier_rx_hz: Option<f32>,
    pub preemphasis_tx: Option<u8>,
    pub data_rate_tx: Option<u32>,
    pub data_rate_rx: Option<u32>,
    /// Délai aller-retour mesuré en phase 2 (ms)
    pub rtd_ms: Option<f32>,
    /// SNR mesuré par le sondage L1/L2 ou en données (dB)
    pub snr_db: Option<f32>,
    /// Débit max projeté par symbole (index 0=2400 … 5=3429), bit/s
    pub projected_rates: Vec<(u32, u32)>,
    pub trellis_states: Option<u16>,
    /// Réponse en fréquence mesurée par L1/L2 : (Hz, dB)
    pub line_response: Vec<(f32, f32)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LogLevel {
    Info,
    Detail,
    Warn,
}

/// Événements produits par le moteur (à vider avec `Modem::drain_events`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ModemEvent {
    PhaseChanged(Phase),
    /// Nom du signal actuellement émis (ex. "ANSam", "CM", "INFO0c", "L1", "TRN")
    TxSignal(String),
    /// Nom du dernier signal reconnu en réception
    RxSignal(String),
    Log { level: LogLevel, msg: String },
    ParamsUpdated(LinkParams),
    Connected { tx_rate: u32, rx_rate: u32 },
    Disconnected { reason: String },
    /// Octets reçus de l'autre modem (mode données)
    Data(Vec<u8>),
    /// Télécopie : étapes, lignes de la page reçue, fin
    Fax(crate::fax::session::FaxEvenement),
    /// Morse : un signe part sur la ligne, ou vient d'être reconnu
    Morse(crate::morse::MorseEvenement),
}

/// Commandes envoyées au moteur.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ModemCommand {
    /// Décrocher et démarrer en appelant (après numérotation)
    Dial,
    /// Décrocher et démarrer en appelé
    Answer,
    /// Raccrocher (cleardown propre si en données, sinon immédiat)
    Hangup,
    /// Octets à envoyer en mode données
    Send(Vec<u8>),
    /// Forcer un réentraînement complet
    Retrain,
    /// Forcer une renégociation de débit
    Renegotiate,
    /// Prépare une télécopie : le prochain `Dial` envoie cette page (lignes
    /// de 1728 points, vrai = noir ; `fine` : 7,7 lignes/mm) au lieu de
    /// négocier une liaison de données
    PreparerFax { lignes: Vec<Vec<bool>>, fine: bool },
}

/// Configuration du modem.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModemConfig {
    /// Débit max autorisé (bit/s), ≤ 33600
    pub max_rate: u32,
    /// Vitesses de symbole optionnelles supportées
    pub allow_2743: bool,
    pub allow_2800: bool,
    pub allow_3429: bool,
    /// Durée du silence initial de l'appelé (ms, ≥ 200)
    pub answer_silence_ms: u32,
    /// 56k (V.90) : proposer le mode PCM pendant la négociation V.8. Côté
    /// appelant, le modem joue le modem « analogique » ; côté répondant, le
    /// modem « numérique », ce qui demande aussi `digital`.
    pub v90: bool,
    /// Le modem est raccordé en numérique au réseau (le FAI du central) : il
    /// peut jouer le modem numérique du V.90.
    pub digital: bool,
    /// Débit descendant max en V.90 (bit/s, de 28 000 à 56 000)
    pub max_rate_pcm: u32,
    /// 56k « express » : DIL court, aucune pause. Sinon (par défaut), les
    /// lenteurs d'origine : long DIL avec symboles de référence écouté deux
    /// fois, temps de réflexion après Jd, SCR pendant le calcul des niveaux.
    pub express: bool,
    /// Identité du télécopieur (TSI/CSI) : le numéro de l'abonné
    #[serde(default)]
    pub ident: String,
    /// Appelé : répondre en serveur Minitel (ANS pur puis V.23 1200/75) au
    /// lieu de négocier en V.8 (3615 ORACLE, numéros cachés)
    #[serde(default)]
    pub minitel: bool,
    /// Morse (DIP 4) : l'appelant le propose en V.8 (service non normalisé
    /// TINYVERSE), l'appelé l'accepte ; sinon l'appel continue en V.34/V.90
    #[serde(default)]
    pub morse: bool,
    /// Ordre des bits du CRC des séquences de la poignée de main (INFO, MP,
    /// séquences du V.90). Laisser `Auto` : les autres valeurs servent aux
    /// essais (imiter un de nos modems d'avant, ou un vrai modem).
    #[serde(default)]
    pub crc_mode: CrcMode,
}

/// Ordre des bits du CRC des séquences de la poignée de main.
///
/// La norme (V.34 §10.1.2.3) émet le CRC poids fort en premier, comme les
/// vrais modems (vérifié sur un U.S. Robotics 56K de 1998). Nos modems
/// d'avant (appli 0.1.6 et plus anciennes, centraux de 2026 jusqu'en
/// novembre) l'émettaient poids faible en premier : deux de nos
/// modems s'entendaient, mais pas avec du vrai matériel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrcMode {
    /// Accepte les deux ordres ; émet dans l'ancien tant que l'autre n'a pas
    /// montré qu'il suit la norme (un modem d'avant reste compris), puis
    /// comme lui.
    #[default]
    Auto,
    /// Comme nos modems d'avant : l'ancien ordre seulement
    Legacy,
    /// Comme un vrai modem : l'ordre de la norme seulement
    Standard,
}

impl Default for ModemConfig {
    fn default() -> Self {
        Self {
            max_rate: 33600,
            allow_2743: true,
            allow_2800: true,
            allow_3429: true,
            answer_silence_ms: 400,
            v90: true,
            digital: false,
            max_rate_pcm: 56000,
            express: false,
            ident: String::new(),
            minitel: false,
            morse: false,
            crc_mode: CrcMode::Auto,
        }
    }
}

/// Réception 56k vue comme une échelle : chaque symbole est un niveau de la
/// loi A (ou µ) choisi dans le jeu de sa position dans la trame de 6.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PcmView {
    /// Niveaux attendus (amplitudes ≥ 0, normalisées à ±1) pour chacune des
    /// 6 positions de la trame ; le signe s'y ajoute
    pub levels: Vec<Vec<f32>>,
    /// Derniers symboles reçus après l'égaliseur : (position 0..5, valeur)
    pub points: Vec<(u8, f32)>,
    /// Jeu des données (vrai) ou d'apprentissage de la phase 4 (faux)
    pub data: bool,
}

/// Instantané de l'état, à interroger à ~30 Hz par l'interface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModemStatus {
    pub role: Role,
    pub phase: Phase,
    pub leds: Leds,
    pub tx_signal: String,
    pub rx_signal: String,
    pub params: LinkParams,
    /// Derniers points de constellation reçus (après égaliseur), normalisés
    pub constellation: Vec<(f32, f32)>,
    /// 56k (modem analogique) : pas de constellation 2D, des niveaux PCM
    #[serde(default)]
    pub pcm: Option<PcmView>,
    /// Octets envoyés / reçus depuis la connexion
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    /// Statistiques de la couche de correction d'erreurs (si une liaison existe)
    pub link: Option<crate::lapm::LapmStats>,
    /// Morse : manipulateur et écoute, en direct
    #[serde(default)]
    pub morse: Option<crate::morse::MorseVue>,
}
