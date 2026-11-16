//! Couche liaison fiable inspirée de V.42 / LAPM (procédure HDLC).
//!
//! Le modem V.34 fournit un « tuyau de bits » synchrone et bidirectionnel,
//! mais non fiable (erreurs isolées, salves). Ce module le transforme en flux
//! d'octets fiable et ordonné, dans les deux sens à la fois :
//!
//! * **Tramage HDLC** : fanion 0x7E, transparence par insertion d'un 0 après
//!   cinq 1 consécutifs, octets émis bit de poids faible en premier, FCS
//!   16 bits CRC-CCITT (x^16 + x^12 + x^5 + 1, init 0xFFFF, complément final,
//!   émis LSB en premier). Au repos, des fanions sont émis en continu.
//!   Abandons (≥ 7 uns), trames trop courtes / trop longues / non alignées
//!   sur l'octet et FCS faux sont rejetés silencieusement (et comptés).
//! * **Procédure LAPM** : adresse 1 octet, contrôle étendu modulo 128.
//!   Trames I, S (RR, RNR, REJ ; un SREJ reçu est traité comme un REJ) et
//!   U (SABME, UA, DISC, DM). N401 = 128 octets, fenêtre k = 15.
//! * **ARQ go-back-N** : REJ sur trou de séquence ; temporisateur T401 compté
//!   en bits émis. À expiration : interrogation RR P=1, puis retransmission à
//!   partir du N(R) porté par la réponse F=1.
//!
//! Tout est piloté bit par bit : [`Lapm::next_tx_bit`] à chaque bit émis par
//! le modem, [`Lapm::rx_bit`] à chaque bit reçu. Aucune horloge système :
//! l'unique base de temps est le nombre de bits émis.
//!
//! ## Conventions retenues
//!
//! * Adresse : EA = 1, DLCI = 0, bit C/R : l'appelant (`originator`) émet ses
//!   commandes avec C/R = 0 et ses réponses avec C/R = 1, l'appelé l'inverse.
//!   (L'interopérabilité avec un vrai modem V.42 n'est pas un objectif : pas
//!   de phase de détection ODP/ADP ni de négociation XID.)
//! * Établissement : l'appelant répète SABME (P=1) toutes les T401 jusqu'à
//!   recevoir UA. Après UA, il émet immédiatement une trame (I ou RR).
//!   L'appelé, une fois UA envoyé, n'émet pas de trame I tant qu'il n'a pas
//!   reçu au moins une trame I ou S de l'appelant (« confirmation ») : cela
//!   garantit qu'un SABME répété (UA perdu, ou T401 plus court que l'aller-
//!   retour) ne provoque ni perte ni doublon. Faute de confirmation, l'appelé
//!   interroge (RR P=1) toutes les T401.
//! * T401 effectif = max(2048, valeur réglée par [`Lapm::set_t401_bits`]).
//!   Il est armé à la FIN de l'émission de la trame concernée et redémarré
//!   à chaque acquittement qui fait progresser V(A). Son expiration est
//!   reportée tant qu'une trame est en cours de réception (elle peut porter
//!   l'acquittement). Pour éviter des interrogations superflues, on conseille
//!   T401 > 2 × retard aller + durée d'une trame I pleine (~1100 bits).
//! * Pendant la reprise sur temporisation, les trames I neuves continuent de
//!   partir (comme Q.921) ; la réponse F=1 ne provoque un retour arrière que
//!   si une trame émise avant l'interrogation n'est pas acquittée.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// Taille maximale par défaut du champ d'information (N401), en octets.
pub const N401_DEFAULT: usize = 128;
/// Borne supérieure acceptée par [`Lapm::set_n401`].
pub const N401_MAX: usize = 2048;
/// Fenêtre d'émission k : nombre maximal de trames I non acquittées.
pub const K_WINDOW: usize = 15;
/// Valeur plancher (et par défaut) de T401, en bits émis.
pub const T401_MIN_BITS: u64 = 2048;
/// Nombre d'expirations de T401 tolérées pendant la déconnexion (N400).
const N400_DISC: u32 = 10;

/// Fanion HDLC.
const FLAG: u8 = 0x7E;
/// Résidu du FCS calculé sur « trame + FCS » reçus (sans complément final).
const FCS_RESIDUE: u16 = 0xF0B8;
/// Plus petite trame valide : adresse + contrôle (1 octet) + FCS (2 octets).
const MIN_FRAME_BYTES: usize = 4;

// Trames S : premier octet de contrôle (le second porte N(R) et P/F).
const S_RR: u8 = 0x01;
const S_RNR: u8 = 0x05;
const S_REJ: u8 = 0x09;
const S_SREJ: u8 = 0x0D;
// Trames U : octet de contrôle unique, bit P/F = 0x10.
const U_SABME: u8 = 0x6F;
const U_UA: u8 = 0x63;
const U_DISC: u8 = 0x43;
const U_DM: u8 = 0x0F;
const U_PF: u8 = 0x10;
/// XID (échange d'identification, V.42 §8.2.4.13), bit P/F à 0
const U_XID: u8 = 0xAF;

// ---------------------------------------------------------------------------
// FCS 16 bits (CRC-CCITT version HDLC / X.25, traitement LSB en premier)
// ---------------------------------------------------------------------------

/// Table du CRC réfléchi (polynôme 0x1021 retourné = 0x8408).
const fn build_fcs_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u16;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0x8408 } else { c >> 1 };
            j += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static FCS_TABLE: [u16; 256] = build_fcs_table();

#[inline]
fn fcs_update(fcs: u16, byte: u8) -> u16 {
    (fcs >> 8) ^ FCS_TABLE[((fcs ^ byte as u16) & 0xFF) as usize]
}

/// FCS HDLC d'une trame (adresse + contrôle + information), déjà complémenté.
/// À émettre octet de poids faible en premier.
pub fn fcs16(data: &[u8]) -> u16 {
    !data.iter().fold(0xFFFF, |f, &b| fcs_update(f, b))
}

/// Vérifie une trame reçue FCS compris (méthode du résidu).
fn fcs_ok(frame_with_fcs: &[u8]) -> bool {
    frame_with_fcs.iter().fold(0xFFFF, |f, &b| fcs_update(f, b)) == FCS_RESIDUE
}

// ---------------------------------------------------------------------------
// Couche bits : émission (FCS + bourrage + fanions)
// ---------------------------------------------------------------------------

/// Sérialiseur HDLC : contient les bits de la trame (ou du fanion de repos)
/// en cours d'émission.
#[derive(Clone, Debug, Default)]
struct HdlcTx {
    bits: Vec<bool>,
    pos: usize,
    /// Au moins un fanion a déjà été émis (sinon il faut un fanion d'ouverture).
    started: bool,
}

impl HdlcTx {
    /// Tous les bits chargés ont été émis.
    fn done(&self) -> bool {
        self.pos >= self.bits.len()
    }

    fn next(&mut self) -> bool {
        let b = self.bits[self.pos];
        self.pos += 1;
        b
    }

    fn push_flag(&mut self) {
        for i in 0..8 {
            self.bits.push((FLAG >> i) & 1 == 1);
        }
    }

    /// Charge un fanion de repos.
    fn load_idle(&mut self) {
        self.bits.clear();
        self.pos = 0;
        self.push_flag();
        self.started = true;
    }

    /// Charge une trame : [fanion d'ouverture si besoin] corps bourré + FCS,
    /// fanion de fermeture (qui sert aussi d'ouverture à la trame suivante).
    fn load_frame(&mut self, bytes: &[u8]) {
        self.bits.clear();
        self.pos = 0;
        if !self.started {
            self.push_flag();
            self.started = true;
        }
        let fcs = fcs16(bytes);
        let tail = [fcs as u8, (fcs >> 8) as u8];
        let mut ones = 0;
        for &byte in bytes.iter().chain(tail.iter()) {
            for i in 0..8 {
                let b = (byte >> i) & 1 == 1;
                self.bits.push(b);
                if b {
                    ones += 1;
                    if ones == 5 {
                        // Transparence : un 0 après cinq 1.
                        self.bits.push(false);
                        ones = 0;
                    }
                } else {
                    ones = 0;
                }
            }
        }
        self.push_flag();
    }
}

// ---------------------------------------------------------------------------
// Couche bits : réception (fanions, débourrage, abandon, FCS)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RxOutcome {
    /// Rien de notable (bit absorbé, fanion de repos).
    Nothing,
    /// Trame valide (FCS correct) disponible dans `HdlcRx::frame`.
    Frame,
    /// Séquence non vide rejetée (FCS faux, abandon, longueur invalide).
    Rejected,
}

#[derive(Clone, Debug)]
struct HdlcRx {
    /// Nombre de 1 consécutifs reçus.
    ones: u32,
    /// Bits débourrés depuis le dernier fanion. À la détection du fanion
    /// suivant, ils se terminent par « 0 11111 » (début du fanion), à retirer.
    bits: Vec<bool>,
    /// En attente d'un fanion (démarrage, après abandon ou débordement).
    hunting: bool,
    /// Longueur maximale d'une trame, en bits débourrés (+ 6 bits de fanion).
    max_bits: usize,
    /// Dernière trame valide (FCS compris).
    frame: Vec<u8>,
}

impl HdlcRx {
    fn new(n401: usize) -> Self {
        HdlcRx {
            ones: 0,
            bits: Vec::with_capacity(Self::max_bits_for(n401)),
            hunting: true,
            max_bits: Self::max_bits_for(n401),
            frame: Vec::with_capacity(n401 + 5),
        }
    }

    /// Une trame est en cours de réception (des bits de données au-delà
    /// d'un simple début de fanion ont été reçus depuis le dernier fanion).
    fn receiving(&self) -> bool {
        !self.hunting && self.bits.len() > 6
    }

    /// Adresse (1) + contrôle (2) + information (N401) + FCS (2), en bits,
    /// plus les 6 premiers bits du fanion de fermeture.
    fn max_bits_for(n401: usize) -> usize {
        (n401 + 5) * 8 + 6
    }

    fn push_data_bit(&mut self, bit: bool) -> RxOutcome {
        if self.hunting {
            return RxOutcome::Nothing;
        }
        self.bits.push(bit);
        if self.bits.len() > self.max_bits {
            // Trame trop longue : on l'abandonne jusqu'au prochain fanion.
            self.hunting = true;
            self.bits.clear();
            return RxOutcome::Rejected;
        }
        RxOutcome::Nothing
    }

    fn push(&mut self, bit: bool) -> RxOutcome {
        if bit {
            self.ones += 1;
            match self.ones {
                1..=5 => self.push_data_bit(true),
                // Sixième 1 : fanion ou abandon, décidé au bit suivant.
                6 => RxOutcome::Nothing,
                7 => {
                    // Abandon : ne compte que s'il y avait une trame en cours
                    // (au-delà des 6 bits « 0 11111 » d'un fanion corrompu).
                    let had_frame = !self.hunting && self.bits.len() > 6;
                    self.hunting = true;
                    self.bits.clear();
                    if had_frame {
                        RxOutcome::Rejected
                    } else {
                        RxOutcome::Nothing
                    }
                }
                // Ligne au repos « marque » : on reste en chasse.
                _ => RxOutcome::Nothing,
            }
        } else {
            let ones = std::mem::replace(&mut self.ones, 0);
            match ones {
                6 => {
                    // Fanion 01111110 : clôt la trame en cours.
                    let out = if self.hunting { RxOutcome::Nothing } else { self.finish() };
                    self.hunting = false;
                    self.bits.clear();
                    out
                }
                // 0 de transparence : supprimé.
                5 => RxOutcome::Nothing,
                _ => self.push_data_bit(false),
            }
        }
    }

    /// Évalue la séquence comprise entre deux fanions.
    fn finish(&mut self) -> RxOutcome {
        let n = self.bits.len().saturating_sub(6);
        if n == 0 {
            // Fanions consécutifs : repos.
            return RxOutcome::Nothing;
        }
        if !n.is_multiple_of(8) || n / 8 < MIN_FRAME_BYTES {
            return RxOutcome::Rejected;
        }
        self.frame.clear();
        for chunk in self.bits[..n].chunks_exact(8) {
            let byte = chunk
                .iter()
                .enumerate()
                .fold(0u8, |acc, (i, &b)| acc | ((b as u8) << i));
            self.frame.push(byte);
        }
        if fcs_ok(&self.frame) {
            RxOutcome::Frame
        } else {
            RxOutcome::Rejected
        }
    }
}

// ---------------------------------------------------------------------------
// Machine d'états LAPM
// ---------------------------------------------------------------------------

/// État de la liaison logique.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LapmState {
    /// Pas de liaison (l'appelé attend un SABME).
    Disconnected,
    /// L'appelant émet SABME et attend UA.
    Establishing,
    /// Transfert d'information.
    Connected,
    /// DISC émis, attente de UA / DM.
    Disconnecting,
}

/// Nature de la trame en cours d'émission (pour armer T401 à sa fin).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SentKind {
    Info,
    Poll,
    Sabme,
    Disc,
    Ua,
    Other,
}

/// Trame I numérotée, en attente d'acquittement.
#[derive(Clone, Debug)]
struct TxEntry {
    data: Vec<u8>,
    /// Déjà émise au moins une fois (sa prochaine émission est une retransmission).
    sent: bool,
}

/// Statistiques de la couche liaison.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LapmStats {
    /// Liaison établie (transfert d'information possible).
    pub established: bool,
    /// Trames émises (hors fanions de repos).
    pub frames_tx: u64,
    /// Trames reçues avec un FCS correct.
    pub frames_rx: u64,
    /// Trames I retransmises.
    pub retransmissions: u64,
    /// Séquences reçues rejetées : FCS faux, abandon, longueur invalide.
    pub fcs_errors: u64,
    /// Octets livrés à l'utilisateur (dans l'ordre, sans doublon).
    pub bytes_delivered: u64,
    /// Octets émis et acquittés par le correspondant.
    pub bytes_acked: u64,
    /// Octets en attente : pas encore émis, ou émis mais non acquittés.
    pub tx_queue: usize,
}

/// Couche liaison fiable LAPM (un côté de la liaison).
#[derive(Clone, Debug)]
pub struct Lapm {
    originator: bool,
    state: LapmState,
    n401: usize,
    /// T401 demandé par l'appelant (0 = défaut).
    t401_user: u64,
    /// Horloge : nombre de bits émis.
    clock: u64,
    /// Échéance de T401 (en bits émis), si armé.
    t401_deadline: Option<u64>,

    htx: HdlcTx,
    hrx: HdlcRx,
    /// Trame en cours d'émission.
    sending: Option<SentKind>,
    /// Tampon de construction des trames.
    scratch: Vec<u8>,

    /// Octets utilisateur pas encore placés dans une trame I.
    tx_queue: VecDeque<u8>,
    /// Trames I numérotées non acquittées : N(S) = V(A), V(A)+1, …
    window: VecDeque<TxEntry>,
    /// Fenêtre k : trames I émises sans acquittement au plus
    k: usize,
    /// Indice dans `window` de la prochaine trame à (ré)émettre :
    /// V(S) = V(A) + next_idx (modulo 128).
    next_idx: usize,
    /// V(A) : plus ancienne trame non acquittée.
    va: u8,
    /// V(R) : prochain N(S) attendu.
    vr: u8,
    /// Octets reçus, en attente de `take_received`.
    rx_out: Vec<u8>,

    /// Le correspondant a prouvé qu'il est en transfert d'information.
    confirmed: bool,
    /// Un acquittement est à transmettre (porté par I, sinon RR).
    ack_pending: bool,
    /// Une réponse F=1 est due (commande P=1 reçue).
    f_pending: bool,
    /// Un REJ est à émettre.
    rej_due: bool,
    /// Condition « REJ émis » : pas d'autre REJ avant comblement du trou.
    rej_sent: bool,
    /// T401 a expiré : une interrogation RR P=1 est à émettre.
    poll_due: bool,
    /// Interrogation émise, réponse F=1 attendue (reprise sur temporisation).
    poll_outstanding: bool,
    /// Trames I émises avant l'interrogation et pas encore acquittées : si
    /// la réponse F=1 ne les acquitte pas toutes, il y a eu perte.
    poll_backlog: usize,
    /// Le correspondant a signalé RNR.
    peer_busy: bool,
    sabme_due: bool,
    disc_due: bool,
    disc_retries: u32,
    /// Réponse U à émettre (code, bit F).
    u_resp: Option<(u8, bool)>,
    /// V.42 strict, face à un vrai modem : bit C/R de la Table 6/V.42
    /// (l'inverse de notre convention d'origine) et réponse aux XID
    standard: bool,
    /// Champ d'information de la réponse XID à émettre
    xid_resp: Option<Vec<u8>>,

    /// Compteurs (les champs `established` et `tx_queue` sont calculés).
    counters: LapmStats,
}

impl Lapm {
    /// Crée un côté de liaison. L'appelant (`originator = true`) lance
    /// l'établissement (SABME) dès les premiers bits émis.
    pub fn new(originator: bool) -> Self {
        Lapm {
            originator,
            state: if originator { LapmState::Establishing } else { LapmState::Disconnected },
            n401: N401_DEFAULT,
            t401_user: 0,
            clock: 0,
            t401_deadline: None,
            htx: HdlcTx::default(),
            hrx: HdlcRx::new(N401_DEFAULT),
            sending: None,
            scratch: Vec::with_capacity(N401_DEFAULT + 3),
            tx_queue: VecDeque::new(),
            window: VecDeque::with_capacity(K_WINDOW),
            k: K_WINDOW,
            next_idx: 0,
            va: 0,
            vr: 0,
            rx_out: Vec::new(),
            confirmed: false,
            ack_pending: false,
            f_pending: false,
            rej_due: false,
            rej_sent: false,
            poll_due: false,
            poll_outstanding: false,
            poll_backlog: 0,
            peer_busy: false,
            sabme_due: originator,
            disc_due: false,
            disc_retries: 0,
            u_resp: None,
            standard: false,
            xid_resp: None,
            counters: LapmStats::default(),
        }
    }

    /// Règle T401 (en bits émis). La valeur effective ne descend pas sous
    /// [`T401_MIN_BITS`].
    pub fn set_t401_bits(&mut self, bits: u64) {
        self.t401_user = bits;
    }

    /// T401 effectif, en bits émis.
    pub fn t401_bits(&self) -> u64 {
        self.t401_user.max(T401_MIN_BITS)
    }

    /// Règle N401 (taille max du champ d'information, 1..=N401_MAX).
    /// Les deux côtés doivent utiliser la même valeur (pas de négociation XID).
    pub fn set_n401(&mut self, n: usize) {
        self.n401 = n.clamp(1, N401_MAX);
        self.hrx.max_bits = HdlcRx::max_bits_for(self.n401);
    }

    /// Règle la fenêtre k (1..=127, numérotation modulo 128). Les deux côtés
    /// doivent l'accepter : on ne l'élargit qu'entre modems qui le savent
    /// (les deux bouts d'une liaison 56k sont forcément récents).
    pub fn set_window(&mut self, k: usize) {
        self.k = k.clamp(1, 127);
    }

    /// Met en file des octets utilisateur (possible avant l'établissement).
    pub fn send(&mut self, data: &[u8]) {
        self.tx_queue.extend(data.iter().copied());
    }

    /// Prochain bit à émettre ; à appeler une fois par bit émis par le modem.
    pub fn next_tx_bit(&mut self) -> bool {
        self.clock += 1;
        if let Some(deadline) = self.t401_deadline {
            // T401 n'expire pas pendant la réception d'une trame : elle porte
            // peut-être l'acquittement attendu (report d'au plus une trame).
            if self.clock >= deadline && !self.hrx.receiving() {
                self.t401_deadline = None;
                self.on_t401_expiry();
            }
        }
        if self.htx.done() {
            if let Some(kind) = self.sending.take() {
                self.on_frame_sent(kind);
            }
            let mut out = std::mem::take(&mut self.scratch);
            match self.select_frame(&mut out) {
                Some(kind) => {
                    self.htx.load_frame(&out);
                    self.sending = Some(kind);
                    self.counters.frames_tx += 1;
                }
                None => self.htx.load_idle(),
            }
            self.scratch = out;
        }
        self.htx.next()
    }

    /// Traite un bit reçu ; à appeler une fois par bit reçu par le modem.
    pub fn rx_bit(&mut self, bit: bool) {
        match self.hrx.push(bit) {
            RxOutcome::Nothing => {}
            RxOutcome::Rejected => self.counters.fcs_errors += 1,
            RxOutcome::Frame => {
                self.counters.frames_rx += 1;
                let frame = std::mem::take(&mut self.hrx.frame);
                // On retire le FCS (2 octets) avant l'analyse.
                self.handle_frame(&frame[..frame.len() - 2]);
                self.hrx.frame = frame;
            }
        }
    }

    /// Octets livrés (dans l'ordre) depuis le dernier appel.
    pub fn take_received(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.rx_out)
    }

    /// Instantané des statistiques.
    pub fn stats(&self) -> LapmStats {
        LapmStats {
            established: self.is_established(),
            tx_queue: self.pending_bytes(),
            ..self.counters.clone()
        }
    }

    /// État courant de la liaison.
    pub fn state(&self) -> LapmState {
        self.state
    }

    /// Liaison établie (transfert d'information).
    pub fn is_established(&self) -> bool {
        self.state == LapmState::Connected
    }

    /// Demande la libération : DISC est émis (répété jusqu'à UA/DM, au plus
    /// N400 fois). Les données non encore acquittées sont abandonnées ; pour
    /// une fermeture propre, attendre `idle()` avant d'appeler cette méthode.
    pub fn disconnect(&mut self) {
        match self.state {
            LapmState::Connected | LapmState::Establishing => {
                self.reset_link();
                self.tx_queue.clear();
                self.state = LapmState::Disconnecting;
                self.sabme_due = false;
                self.disc_due = true;
                self.disc_retries = 0;
            }
            LapmState::Disconnected | LapmState::Disconnecting => {}
        }
    }

    /// Rien en attente d'envoi ni d'acquittement (et aucune trame en cours
    /// d'émission ou de contrôle à émettre).
    pub fn idle(&self) -> bool {
        let frame_in_progress = self.sending.is_some() && !self.htx.done();
        matches!(self.state, LapmState::Connected | LapmState::Disconnected)
            && self.tx_queue.is_empty()
            && self.window.is_empty()
            && !frame_in_progress
            && self.u_resp.is_none()
            && !self.ack_pending
            && !self.f_pending
            && !self.rej_due
            && !self.poll_due
            && !self.poll_outstanding
    }

    // --- outils internes ---------------------------------------------------

    fn pending_bytes(&self) -> usize {
        self.tx_queue.len() + self.window.iter().map(|e| e.data.len()).sum::<usize>()
    }

    /// V.42 strict (face à un vrai modem) : bit C/R de la Table 6/V.42 et
    /// réponse aux XID. Sinon, notre convention d'origine (C/R inversé), que
    /// nos modems d'avant attendent.
    pub fn set_standard(&mut self, standard: bool) {
        self.standard = standard;
    }

    /// Octet d'adresse d'une trame émise (commande ou réponse). V.42 : les
    /// commandes de l'appelant portent C/R = 1, ses réponses C/R = 0 (et
    /// l'inverse pour l'appelé) ; notre convention d'origine est l'inverse.
    fn addr(&self, command: bool) -> u8 {
        let cr = (command != self.originator) != self.standard;
        0x01 | ((cr as u8) << 1)
    }

    /// Réponse à une XID de négociation (V.42 §8.10, §12.2) : fonctions
    /// optionnelles HDLC sans SREJ ni FCS 32 bits, longueur de trame et
    /// fenêtre demandées ramenées à nos limites, pas de V.42 bis (aucun
    /// sous-champ privé : pas de compression).
    fn xid_response(&self, cmd: &[u8]) -> Vec<u8> {
        // paramètres demandés : PI → valeur
        let mut asked: Vec<(u8, u32)> = Vec::new();
        if cmd.first() == Some(&0x82) {
            let mut i = 1;
            while i + 3 <= cmd.len() {
                let (gi, gl) = (cmd[i], u16::from_be_bytes([cmd[i + 1], cmd[i + 2]]) as usize);
                let body = &cmd[i + 3..(i + 3 + gl).min(cmd.len())];
                if gi == 0x80 {
                    let mut j = 0;
                    while j + 2 <= body.len() {
                        let (pi, pl) = (body[j], body[j + 1] as usize);
                        let pv = &body[(j + 2).min(body.len())..(j + 2 + pl).min(body.len())];
                        asked.push((pi, pv.iter().fold(0u32, |a, &b| a << 8 | b as u32)));
                        j += 2 + pl;
                    }
                }
                i += 3 + gl;
            }
        }
        let mut params: Vec<u8> = vec![3, 4, 0x8A, 0x89, 0x00, 0x00];
        let n401_bits = (self.n401 * 8) as u32;
        for (pi, v) in asked {
            let ours = match pi {
                5 | 6 => v.min(n401_bits),
                7 | 8 => v.min(self.k as u32),
                _ => continue,
            };
            let bytes: Vec<u8> = ours.to_be_bytes().into_iter().skip_while(|&b| b == 0).collect();
            let bytes = if bytes.is_empty() { vec![0] } else { bytes };
            params.push(pi);
            params.push(bytes.len() as u8);
            params.extend(bytes);
        }
        let mut info = vec![0x82, 0x80];
        info.extend((params.len() as u16).to_be_bytes());
        info.extend(params);
        info
    }

    fn arm_t401(&mut self) {
        self.t401_deadline = Some(self.clock + self.t401_bits());
    }

    /// Remise à zéro des variables d'état (établissement / réinitialisation).
    /// Les trames non acquittées repartent en tête de file, dans l'ordre.
    fn reset_link(&mut self) {
        while let Some(e) = self.window.pop_back() {
            for &b in e.data.iter().rev() {
                self.tx_queue.push_front(b);
            }
        }
        self.next_idx = 0;
        self.va = 0;
        self.vr = 0;
        self.ack_pending = false;
        self.f_pending = false;
        self.rej_due = false;
        self.rej_sent = false;
        self.poll_due = false;
        self.poll_outstanding = false;
        self.poll_backlog = 0;
        self.peer_busy = false;
        self.t401_deadline = None;
    }

    /// Arrête T401 lorsqu'il n'a plus de raison de tourner en transfert
    /// d'information : ni trame I en vol, ni interrogation en cours, ni
    /// correspondant occupé, ni confirmation attendue.
    fn enforce_timer(&mut self) {
        if self.state == LapmState::Connected
            && self.confirmed
            && !self.poll_outstanding
            && !self.poll_due
            && !self.peer_busy
            && self.next_idx == 0
        {
            self.t401_deadline = None;
        }
    }

    // --- temporisation -----------------------------------------------------

    fn on_t401_expiry(&mut self) {
        match self.state {
            LapmState::Establishing => self.sabme_due = true,
            LapmState::Disconnecting => {
                self.disc_retries += 1;
                if self.disc_retries >= N400_DISC {
                    self.state = LapmState::Disconnected;
                } else {
                    self.disc_due = true;
                }
            }
            LapmState::Connected => self.poll_due = true,
            LapmState::Disconnected => {}
        }
    }

    /// Fin d'émission d'une trame : armement éventuel de T401.
    fn on_frame_sent(&mut self, kind: SentKind) {
        let connected = self.state == LapmState::Connected;
        match kind {
            SentKind::Info => {
                if connected
                    && self.t401_deadline.is_none()
                    && !self.poll_outstanding
                    && !self.poll_due
                    && self.next_idx > 0
                {
                    self.arm_t401();
                }
            }
            SentKind::Poll => {
                if connected && self.poll_outstanding {
                    self.arm_t401();
                }
            }
            SentKind::Sabme => {
                if self.state == LapmState::Establishing && !self.sabme_due {
                    self.arm_t401();
                }
            }
            SentKind::Disc => {
                if self.state == LapmState::Disconnecting && !self.disc_due {
                    self.arm_t401();
                }
            }
            SentKind::Ua => {
                // Appelé : on attend la confirmation de l'appelant.
                if connected
                    && !self.confirmed
                    && self.t401_deadline.is_none()
                    && !self.poll_outstanding
                    && !self.poll_due
                {
                    self.arm_t401();
                }
            }
            SentKind::Other => {}
        }
    }

    // --- émission : choix de la prochaine trame ----------------------------

    /// Construit dans `out` la prochaine trame à émettre (sans FCS), par ordre
    /// de priorité. `None` : rien à émettre, on envoie un fanion.
    fn select_frame(&mut self, out: &mut Vec<u8>) -> Option<SentKind> {
        out.clear();
        if let Some((code, f)) = self.u_resp.take() {
            out.push(self.addr(false));
            out.push(code | if f { U_PF } else { 0 });
            return Some(if code == U_UA { SentKind::Ua } else { SentKind::Other });
        }
        if let Some(info) = self.xid_resp.take() {
            out.push(self.addr(false));
            out.push(U_XID);
            out.extend(info);
            return Some(SentKind::Other);
        }
        match self.state {
            LapmState::Establishing if self.sabme_due => {
                self.sabme_due = false;
                out.push(self.addr(true));
                out.push(U_SABME | U_PF);
                Some(SentKind::Sabme)
            }
            LapmState::Disconnecting if self.disc_due => {
                self.disc_due = false;
                out.push(self.addr(true));
                out.push(U_DISC | U_PF);
                Some(SentKind::Disc)
            }
            LapmState::Connected => self.select_connected(out),
            _ => None,
        }
    }

    fn push_s(&mut self, out: &mut Vec<u8>, kind: u8, command: bool, pf: bool) {
        out.push(self.addr(command));
        out.push(kind);
        out.push((self.vr << 1) | pf as u8);
        self.ack_pending = false;
    }

    /// Une trame I peut partir. Pendant la reprise sur temporisation
    /// (interrogation en cours), on continue d'émettre des trames NEUVES
    /// comme le permet Q.921 : la réponse F=1 dira s'il faut revenir en arrière.
    fn can_send_i(&self) -> bool {
        self.confirmed
            && !self.poll_due
            && !self.peer_busy
            && (self.next_idx < self.window.len()
                || (self.window.len() < self.k && !self.tx_queue.is_empty()))
    }

    fn select_connected(&mut self, out: &mut Vec<u8>) -> Option<SentKind> {
        // 1. Réponse F=1 à une interrogation du correspondant.
        if self.f_pending {
            self.f_pending = false;
            let kind = if self.rej_due {
                self.rej_due = false;
                S_REJ
            } else {
                S_RR
            };
            self.push_s(out, kind, false, true);
            return Some(SentKind::Other);
        }
        // 2. Notre interrogation (T401 expiré).
        if self.poll_due {
            self.poll_due = false;
            self.poll_outstanding = true;
            self.poll_backlog = self.next_idx;
            self.push_s(out, S_RR, true, true);
            return Some(SentKind::Poll);
        }
        // 3. REJ sur trou de séquence.
        if self.rej_due {
            self.rej_due = false;
            self.push_s(out, S_REJ, false, false);
            return Some(SentKind::Other);
        }
        // 4. Trame I (nouvelle ou retransmise), qui porte aussi l'acquittement.
        if self.can_send_i() {
            if self.next_idx == self.window.len() {
                let n = self.tx_queue.len().min(self.n401);
                let data: Vec<u8> = self.tx_queue.drain(..n).collect();
                self.window.push_back(TxEntry { data, sent: false });
            }
            let ns = ((self.va as usize + self.next_idx) & 0x7F) as u8;
            out.push(self.addr(true));
            out.push(ns << 1);
            out.push(self.vr << 1);
            let entry = &mut self.window[self.next_idx];
            if entry.sent {
                self.counters.retransmissions += 1;
            }
            entry.sent = true;
            out.extend_from_slice(&entry.data);
            self.next_idx += 1;
            self.ack_pending = false;
            return Some(SentKind::Info);
        }
        // 5. RR seul, uniquement si un acquittement est dû.
        if self.ack_pending {
            self.push_s(out, S_RR, false, false);
            return Some(SentKind::Other);
        }
        None
    }

    // --- réception : analyse des trames ------------------------------------

    /// Trame valide, FCS retiré.
    fn handle_frame(&mut self, f: &[u8]) {
        if f.len() < 2 {
            return;
        }
        let addr = f[0];
        // EA = 1 et DLCI = 0 ; seul le bit C/R peut varier.
        if addr & !0x02 != 0x01 {
            return;
        }
        let cr = addr & 0x02 != 0;
        let is_cmd = (cr == self.originator) != self.standard;
        let c0 = f[1];
        if c0 & 0x01 == 0 {
            // Trame I : toujours une commande.
            if f.len() < 3 || f.len() - 3 > self.n401 || !is_cmd {
                return;
            }
            self.on_i_frame(c0 >> 1, f[2] >> 1, f[2] & 1 != 0, &f[3..]);
        } else if c0 & 0x03 == 0x01 {
            // Trame S : exactement 3 octets, bits 5 à 8 réservés à 0.
            if f.len() != 3 || c0 & 0xF0 != 0 {
                return;
            }
            self.on_s_frame(c0, f[2] >> 1, f[2] & 1 != 0, is_cmd);
        } else if c0 & !U_PF == U_XID {
            // XID de négociation d'un vrai modem (V.42) : on répond
            if is_cmd && self.standard {
                self.xid_resp = Some(self.xid_response(&f[2..]));
            }
        } else {
            // Trame U : sans champ d'information pour celles qu'on traite.
            if f.len() != 2 {
                return;
            }
            self.on_u_frame(c0 & !U_PF, c0 & U_PF != 0, is_cmd);
        }
    }

    /// N(R) acceptable : V(A) ≤ N(R) ≤ V(A) + trames numérotées (modulo 128).
    fn nr_valid(&self, nr: u8) -> bool {
        (nr.wrapping_sub(self.va) & 0x7F) as usize <= self.window.len()
    }

    /// Prend en compte l'acquittement N(R) (supposé valide).
    fn apply_nr(&mut self, nr: u8) {
        let acked = (nr.wrapping_sub(self.va) & 0x7F) as usize;
        if acked == 0 {
            return;
        }
        for e in self.window.drain(..acked) {
            self.counters.bytes_acked += e.data.len() as u64;
        }
        self.va = nr;
        self.next_idx = self.next_idx.saturating_sub(acked);
        self.poll_backlog = self.poll_backlog.saturating_sub(acked);
        // Hors reprise sur temporisation : T401 redémarre s'il reste des
        // trames en vol, sinon il s'arrête.
        if !self.poll_outstanding && !self.poll_due {
            if self.next_idx > 0 || self.peer_busy {
                self.arm_t401();
            } else {
                self.t401_deadline = None;
            }
        }
    }

    fn on_i_frame(&mut self, ns: u8, nr: u8, p: bool, info: &[u8]) {
        match self.state {
            LapmState::Connected => {}
            LapmState::Disconnected => {
                if p {
                    self.u_resp = Some((U_DM, true));
                }
                return;
            }
            _ => return,
        }
        if !self.nr_valid(nr) {
            return;
        }
        self.confirmed = true;
        self.apply_nr(nr);
        let diff = (ns.wrapping_sub(self.vr) & 0x7F) as usize;
        if diff == 0 {
            // Dans l'ordre : livraison.
            self.rx_out.extend_from_slice(info);
            self.counters.bytes_delivered += info.len() as u64;
            self.vr = (self.vr + 1) & 0x7F;
            self.rej_sent = false;
            self.rej_due = false;
            self.ack_pending = true;
        } else if diff <= self.k {
            // Trou de séquence : un seul REJ jusqu'à ce qu'il soit comblé.
            if !self.rej_sent {
                self.rej_sent = true;
                self.rej_due = true;
            }
        } else {
            // Doublon (déjà reçu) : on rappelle V(R) à l'émetteur.
            self.ack_pending = true;
        }
        if p {
            self.f_pending = true;
        }
        self.enforce_timer();
    }

    fn on_s_frame(&mut self, kind: u8, nr: u8, pf: bool, is_cmd: bool) {
        match self.state {
            LapmState::Connected => {}
            LapmState::Disconnected => {
                if is_cmd && pf {
                    self.u_resp = Some((U_DM, true));
                }
                return;
            }
            _ => return,
        }
        if !self.nr_valid(nr) {
            return;
        }
        self.confirmed = true;
        if is_cmd && pf {
            self.f_pending = true;
        }
        self.peer_busy = kind == S_RNR;
        self.apply_nr(nr);
        if !is_cmd && pf && self.poll_outstanding {
            // Réponse à notre interrogation : tout ce qui a été émis avant
            // elle est arrivé (canal FIFO), N(R) est donc exact. Si une trame
            // antérieure à l'interrogation manque, elle et toutes les
            // suivantes ont été rejetées : go-back-N depuis N(R).
            self.poll_outstanding = false;
            if self.poll_backlog > 0 {
                self.next_idx = 0;
            }
            self.poll_backlog = 0;
            if self.next_idx > 0 || self.peer_busy {
                self.arm_t401();
            } else {
                self.t401_deadline = None;
            }
        } else if (kind == S_REJ || kind == S_SREJ) && !self.poll_outstanding {
            // Go-back-N : retour au premier non acquitté.
            self.next_idx = 0;
        }
        if self.peer_busy && self.t401_deadline.is_none() && !self.poll_outstanding && !self.poll_due {
            self.arm_t401();
        }
        self.enforce_timer();
    }

    fn on_u_frame(&mut self, code: u8, pf: bool, is_cmd: bool) {
        match (code, is_cmd) {
            (U_SABME, true) => {
                if self.state == LapmState::Disconnecting {
                    self.u_resp = Some((U_DM, pf));
                } else {
                    // (Ré)initialisation en tant qu'appelé : on attendra la
                    // confirmation avant d'émettre des trames I.
                    self.reset_link();
                    self.state = LapmState::Connected;
                    self.confirmed = false;
                    self.sabme_due = false;
                    self.u_resp = Some((U_UA, pf));
                }
            }
            (U_DISC, true) => match self.state {
                LapmState::Connected | LapmState::Disconnecting => {
                    self.reset_link();
                    self.state = LapmState::Disconnected;
                    self.disc_due = false;
                    self.u_resp = Some((U_UA, pf));
                }
                LapmState::Establishing | LapmState::Disconnected => {
                    self.u_resp = Some((U_DM, pf));
                }
            },
            (U_UA, false) => match self.state {
                LapmState::Establishing => {
                    self.reset_link();
                    self.state = LapmState::Connected;
                    self.confirmed = true;
                    // Émettre tout de suite une trame (I ou RR) pour
                    // confirmer l'établissement à l'appelé.
                    self.ack_pending = true;
                }
                LapmState::Disconnecting => {
                    self.state = LapmState::Disconnected;
                    self.disc_due = false;
                    self.t401_deadline = None;
                }
                _ => {}
            },
            (U_DM, false) => match self.state {
                LapmState::Disconnecting => {
                    self.state = LapmState::Disconnected;
                    self.disc_due = false;
                    self.t401_deadline = None;
                }
                LapmState::Connected => {
                    // Le correspondant n'est plus en liaison.
                    self.reset_link();
                    if self.originator {
                        self.state = LapmState::Establishing;
                        self.sabme_due = true;
                    } else {
                        self.state = LapmState::Disconnected;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// V.42 strict (face à un vrai modem) : C/R de la Table 6/V.42 — le
    /// SABME de l'appelant porte C/R = 1 — et la liaison s'établit et
    /// transporte les données comme avec notre convention d'origine.
    #[test]
    fn v42_strict_bit_cr_et_donnees() {
        let a = Lapm::new(true);
        let mut s = Lapm::new(true);
        s.set_standard(true);
        assert_eq!(a.addr(true), 0x01, "convention d'origine : commande de l'appelant C/R = 0");
        assert_eq!(s.addr(true), 0x03, "V.42 : commande de l'appelant C/R = 1");
        assert_eq!(s.addr(false), 0x01, "V.42 : réponse de l'appelant C/R = 0");
        let mut pair = Pair::new(40, 3);
        pair.a.set_standard(true);
        pair.b.set_standard(true);
        let da: Vec<u8> = (0..900u32).map(|i| (i * 7) as u8).collect();
        let db: Vec<u8> = (0..700u32).map(|i| (i * 13) as u8).collect();
        transfer(&mut pair, &da, &db, 400_000);
    }

    /// XID de négociation typique d'un modem V.42 bis (N401 = 256 octets,
    /// k = 15, compression demandée) : réponse dans nos limites, sans
    /// sous-champ privé (pas de V.42 bis), avec les bits obligatoires.
    #[test]
    fn reponse_xid_v42() {
        let mut b = Lapm::new(false);
        b.set_standard(true);
        let params: Vec<u8> = vec![3, 4, 0x8A, 0x89, 0, 0, 5, 2, 0x08, 0x00, 6, 2, 0x08, 0x00, 7, 1, 15, 8, 1, 15];
        let mut info = vec![0x82, 0x80];
        info.extend((params.len() as u16).to_be_bytes());
        info.extend(&params);
        // sous-champ privé V.42 bis : « V42 », P0 = 3, P1 = 2048, P2 = 32
        info.extend([0xF0, 0, 14, 0, 3, b'V', b'4', b'2', 1, 1, 3, 2, 2, 0x08, 0x00, 3, 1, 32]);
        let mut frame = vec![0x03, U_XID]; // commande de l'appelant (C/R = 1)
        frame.extend(&info);
        b.handle_frame(&frame);
        let resp = b.xid_resp.clone().expect("réponse XID");
        assert_eq!(&resp[..2], &[0x82, 0x80]);
        let gl = u16::from_be_bytes([resp[2], resp[3]]) as usize;
        assert_eq!(resp.len(), 4 + gl, "pas de sous-champ privé (pas de compression)");
        let body = &resp[4..];
        assert_eq!(&body[..6], &[3, 4, 0x8A, 0x89, 0, 0]);
        // N401 ramené à 128 octets = 1024 bits, k = 15
        assert!(body.windows(4).any(|w| w == [5, 2, 0x04, 0x00]) && body.windows(4).any(|w| w == [6, 2, 0x04, 0x00]));
        assert!(body.windows(3).any(|w| w == [7, 1, 15]) && body.windows(3).any(|w| w == [8, 1, 15]));
        let mut out = Vec::new();
        b.select_frame(&mut out);
        assert_eq!(&out[..2], &[0x03, U_XID], "réponse de l'appelé : C/R = 1");
        // notre convention d'origine n'y répond pas (nos modems n'en envoient pas)
        let mut legacy = Lapm::new(false);
        legacy.handle_frame(&frame);
        assert!(legacy.xid_resp.is_none());
    }

    /// Générateur pseudo-aléatoire xorshift64* (déterministe, sans dépendance).
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Self {
            Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }

        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        /// Réel uniforme dans [0, 1).
        fn unit(&mut self) -> f64 {
            (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
        }

        fn bytes(&mut self, n: usize) -> Vec<u8> {
            (0..n).map(|_| (self.next_u64() >> 56) as u8).collect()
        }
    }

    /// Canal binaire simulé : retard fixe, erreurs aléatoires, salves.
    struct Channel {
        line: VecDeque<bool>,
        /// Taux d'erreur binaire hors salves.
        ber: f64,
        /// Période des salves en bits (0 = pas de salves).
        burst_period: u64,
        /// Longueur d'une salve (bits rendus aléatoires).
        burst_len: u64,
        /// Intervalle [début, fin) de bits rendus aléatoires.
        garble: (u64, u64),
        t: u64,
        rng: Rng,
    }

    impl Channel {
        fn new(delay: usize, seed: u64) -> Self {
            Channel {
                line: (0..delay).map(|_| true).collect(),
                ber: 0.0,
                burst_period: 0,
                burst_len: 0,
                garble: (0, 0),
                t: 0,
                rng: Rng::new(seed),
            }
        }

        fn pass(&mut self, bit: bool) -> bool {
            let t = self.t;
            self.t += 1;
            let in_burst =
                self.burst_period > 0 && t % self.burst_period >= self.burst_period - self.burst_len;
            let garbled = t >= self.garble.0 && t < self.garble.1;
            let b = if in_burst || garbled {
                bit ^ (self.rng.next_u64() & 1 == 1)
            } else if self.ber > 0.0 && self.rng.unit() < self.ber {
                !bit
            } else {
                bit
            };
            self.line.push_back(b);
            self.line.pop_front().unwrap_or(true)
        }
    }

    /// Deux modems reliés par deux canaux (A → B et B → A).
    struct Pair {
        a: Lapm,
        b: Lapm,
        ab: Channel,
        ba: Channel,
        got_a: Vec<u8>,
        got_b: Vec<u8>,
        bits: u64,
    }

    impl Pair {
        fn new(delay: usize, seed: u64) -> Self {
            Pair {
                a: Lapm::new(true),
                b: Lapm::new(false),
                ab: Channel::new(delay, seed ^ 0xA5A5),
                ba: Channel::new(delay, seed ^ 0x5A5A),
                got_a: Vec::new(),
                got_b: Vec::new(),
                bits: 0,
            }
        }

        fn step(&mut self) {
            let x = self.ab.pass(self.a.next_tx_bit());
            let y = self.ba.pass(self.b.next_tx_bit());
            self.b.rx_bit(x);
            self.a.rx_bit(y);
            self.bits += 1;
            if self.bits.is_multiple_of(256) {
                self.collect();
            }
        }

        fn collect(&mut self) {
            self.got_a.extend(self.a.take_received());
            self.got_b.extend(self.b.take_received());
        }

        /// Avance jusqu'à ce que `done` soit vrai (ou `max` bits écoulés).
        fn run_until(&mut self, max: u64, done: impl Fn(&Pair) -> bool) -> bool {
            let end = self.bits + max;
            while self.bits < end {
                if done(self) {
                    return true;
                }
                self.step();
            }
            self.collect();
            done(self)
        }
    }

    /// Vérifie l'égalité de deux flux en signalant le premier écart.
    fn assert_same(label: &str, got: &[u8], want: &[u8]) {
        if got != want {
            let first = got.iter().zip(want).position(|(x, y)| x != y);
            panic!(
                "{label} : flux différents (reçu {} octets, attendu {}, premier écart : {:?})",
                got.len(),
                want.len(),
                first
            );
        }
    }

    /// Transfert bidirectionnel complet (mise en file puis vérifications).
    fn transfer(pair: &mut Pair, data_a: &[u8], data_b: &[u8], max_bits: u64) {
        pair.a.send(data_a);
        pair.b.send(data_b);
        complete(pair, data_a, data_b, max_bits);
    }

    /// Fait tourner la paire jusqu'à réception complète des données (déjà en
    /// file), vérifie leur intégrité puis le retour au repos.
    fn complete(pair: &mut Pair, data_a: &[u8], data_b: &[u8], max_bits: u64) {
        let (na, nb) = (data_a.len(), data_b.len());
        let ok = pair.run_until(max_bits, |p| {
            p.got_b.len() + p.b.rx_out.len() >= na && p.got_a.len() + p.a.rx_out.len() >= nb
        });
        pair.collect();
        assert!(
            ok,
            "transfert incomplet après {} bits : A→B {}/{} ; B→A {}/{} ; stats A {:?} ; stats B {:?}",
            pair.bits,
            pair.got_b.len(),
            na,
            pair.got_a.len(),
            nb,
            pair.a.stats(),
            pair.b.stats()
        );
        assert_same("A → B", &pair.got_b, data_a);
        assert_same("B → A", &pair.got_a, data_b);
        // Les derniers acquittements doivent aboutir : les deux côtés au repos.
        let idle = pair.run_until(200_000, |p| p.a.idle() && p.b.idle());
        assert!(idle, "pas de retour au repos : A {:?} ; B {:?}", pair.a.stats(), pair.b.stats());
        pair.collect();
        assert_eq!(pair.got_b.len(), na, "doublon A → B après la fin");
        assert_eq!(pair.got_a.len(), nb, "doublon B → A après la fin");
        let (sa, sb) = (pair.a.stats(), pair.b.stats());
        assert_eq!(sa.bytes_acked, na as u64);
        assert_eq!(sb.bytes_acked, nb as u64);
        assert_eq!(sb.bytes_delivered, na as u64);
        assert_eq!(sa.bytes_delivered, nb as u64);
        assert_eq!(sa.tx_queue, 0);
        assert_eq!(sb.tx_queue, 0);
    }

    #[test]
    fn fcs_vecteur_de_reference() {
        // CRC-16/X-25 de "123456789" = 0x906E.
        assert_eq!(fcs16(b"123456789"), 0x906E);
        // Trame + FCS (LSB en premier) : résidu 0xF0B8.
        let mut v = b"123456789".to_vec();
        let f = fcs16(&v);
        v.push(f as u8);
        v.push((f >> 8) as u8);
        assert!(fcs_ok(&v));
        v[3] ^= 0x10;
        assert!(!fcs_ok(&v));
    }

    #[test]
    fn t401_plancher() {
        let mut l = Lapm::new(true);
        assert_eq!(l.t401_bits(), T401_MIN_BITS);
        l.set_t401_bits(100);
        assert_eq!(l.t401_bits(), T401_MIN_BITS);
        l.set_t401_bits(5000);
        assert_eq!(l.t401_bits(), 5000);
    }

    /// (d) Bourrage au niveau bits : pas de 6 uns hors fanions, aller-retour exact.
    #[test]
    fn bourrage_trame_isolee() {
        let mut body = vec![0x01u8, 0x00, 0x00];
        for i in 0..N401_DEFAULT {
            body.push([0x7E, 0xFF, 0x7D, 0xFE, 0x3F, 0xFC][i % 6]);
        }
        let mut tx = HdlcTx::default();
        tx.load_frame(&body);
        let bits = tx.bits.clone();
        // Hors fanions d'ouverture et de fermeture, jamais six 1 de suite.
        let inner = &bits[8..bits.len() - 8];
        let mut run = 0;
        for &b in inner {
            run = if b { run + 1 } else { 0 };
            assert!(run <= 5, "six 1 consécutifs dans le corps bourré");
        }
        // Précédée de bruit et de fanions, la trame est retrouvée à l'identique.
        let mut rx = HdlcRx::new(N401_DEFAULT);
        let mut frames = Vec::new();
        let noise = [true, false, true, true, true, true, true, true, true, false];
        for &b in noise.iter().chain(bits.iter()) {
            if rx.push(b) == RxOutcome::Frame {
                frames.push(rx.frame.clone());
            }
        }
        assert_eq!(frames.len(), 1);
        assert_eq!(&frames[0][..frames[0].len() - 2], &body[..]);
    }

    /// (a) Sans erreurs : 50 Ko aléatoires dans chaque sens.
    #[test]
    fn transfert_sans_erreur() {
        let mut rng = Rng::new(1);
        let data_a = rng.bytes(50_000);
        let data_b = rng.bytes(50_000);
        let mut pair = Pair::new(300, 11);
        transfer(&mut pair, &data_a, &data_b, 2_000_000);
        let (sa, sb) = (pair.a.stats(), pair.b.stats());
        assert!(sa.established && sb.established);
        assert_eq!(sa.retransmissions + sb.retransmissions, 0, "{sa:?} {sb:?}");
        assert_eq!(sa.fcs_errors + sb.fcs_errors, 0, "{sa:?} {sb:?}");
        // Efficacité : moins de 15 % de surcoût sur le canal.
        // Ni interrogation parasite ni RR superflu : 391 trames I + quelques
        // trames de service (SABME/UA, derniers RR).
        let i_frames = 50_000u64.div_ceil(N401_DEFAULT as u64);
        assert!(sa.frames_tx <= i_frames + 6 && sb.frames_tx <= i_frames + 6, "{sa:?} {sb:?}");
        // Efficacité : moins de 10 % de surcoût sur le canal.
        assert!(pair.bits < 50_000 * 8 * 110 / 100, "trop de bits : {}", pair.bits);
    }

    /// (b) BER 1e-4 + salves de 50 bits, 50 Ko dans chaque sens.
    #[test]
    fn transfert_avec_erreurs() {
        let mut rng = Rng::new(2);
        let data_a = rng.bytes(50_000);
        let data_b = rng.bytes(50_000);
        let mut pair = Pair::new(300, 22);
        for ch in [&mut pair.ab, &mut pair.ba] {
            ch.ber = 1e-4;
            ch.burst_period = 47_351;
            ch.burst_len = 50;
        }
        transfer(&mut pair, &data_a, &data_b, 4_000_000);
        let (sa, sb) = (pair.a.stats(), pair.b.stats());
        assert!(sa.retransmissions > 0 && sb.retransmissions > 0, "{sa:?} {sb:?}");
        assert!(sa.fcs_errors > 0 && sb.fcs_errors > 0, "{sa:?} {sb:?}");
    }

    /// (c) Données mises en file avant l'établissement.
    #[test]
    fn envoi_avant_etablissement() {
        let mut rng = Rng::new(3);
        let data_a = rng.bytes(3_000);
        let data_b = rng.bytes(2_000);
        let mut pair = Pair::new(300, 33);
        pair.a.send(&data_a[..1000]);
        pair.b.send(&data_b);
        assert!(!pair.a.is_established() && !pair.b.is_established());
        assert_eq!(pair.a.stats().tx_queue, 1000);
        assert_eq!(pair.b.state(), LapmState::Disconnected);
        // Quelques bits : SABME parti, pas encore d'UA.
        pair.run_until(200, |_| false);
        assert!(!pair.a.is_established());
        assert_eq!(pair.a.state(), LapmState::Establishing);
        assert!(pair.got_a.is_empty() && pair.got_b.is_empty());
        // Le reste est ajouté pendant l'établissement.
        pair.a.send(&data_a[1000..]);
        complete(&mut pair, &data_a, &data_b, 1_000_000);
        assert!(pair.a.is_established() && pair.b.is_established());
    }

    /// (c bis) UA perdu : SABME répété, l'appelé se réinitialise sans doublon.
    #[test]
    fn etablissement_ua_perdu() {
        let mut rng = Rng::new(4);
        let data_a = rng.bytes(5_000);
        let data_b = rng.bytes(5_000);
        let mut pair = Pair::new(300, 44);
        // Tout ce que B émet pendant les 1500 premiers bits est détruit.
        pair.ba.garble = (0, 1500);
        transfer(&mut pair, &data_a, &data_b, 1_000_000);
    }

    /// (c ter) Aller-retour plus long que T401 : SABME répété avant l'arrivée
    /// de l'UA ; aucun doublon ni perte.
    #[test]
    fn etablissement_rtt_superieur_a_t401() {
        let mut rng = Rng::new(5);
        let data_a = rng.bytes(8_000);
        let data_b = rng.bytes(8_000);
        let mut pair = Pair::new(1500, 55);
        transfer(&mut pair, &data_a, &data_b, 2_000_000);
    }

    /// (d) Données pleines de 0x7E / 0xFF à travers la liaison.
    #[test]
    fn bourrage_transfert() {
        let pattern = [0x7E, 0x7E, 0xFF, 0xFF, 0x7D, 0xFE, 0x3F, 0x7E, 0xFF, 0x1F];
        let data_a: Vec<u8> = (0..4000).map(|i| pattern[i % pattern.len()]).collect();
        let data_b: Vec<u8> = vec![0xFF; 3000];
        let mut pair = Pair::new(300, 66);
        transfer(&mut pair, &data_a, &data_b, 1_000_000);
        let (sa, sb) = (pair.a.stats(), pair.b.stats());
        assert_eq!(sa.fcs_errors + sb.fcs_errors, 0);
    }

    /// Flux dans un seul sens : un RR au plus par trame I reçue.
    #[test]
    fn pas_de_rafale_de_rr() {
        let mut rng = Rng::new(7);
        let data_a = rng.bytes(20_000);
        let mut pair = Pair::new(300, 77);
        transfer(&mut pair, &data_a, &[], 1_000_000);
        // Laisser tourner longtemps au repos : aucune trame supplémentaire.
        let before = (pair.a.stats().frames_tx, pair.b.stats().frames_tx);
        pair.run_until(50_000, |_| false);
        let after = (pair.a.stats().frames_tx, pair.b.stats().frames_tx);
        assert_eq!(before, after, "trames émises au repos");
        let (sa, sb) = (pair.a.stats(), pair.b.stats());
        assert!(sb.frames_tx <= sa.frames_tx + 2, "trop de RR : {sa:?} {sb:?}");
    }

    /// Libération : DISC → UA, les deux côtés repassent hors liaison.
    #[test]
    fn deconnexion() {
        let mut pair = Pair::new(300, 88);
        transfer(&mut pair, b"bonjour", b"au revoir", 200_000);
        pair.a.disconnect();
        assert_eq!(pair.a.state(), LapmState::Disconnecting);
        let ok = pair.run_until(20_000, |p| {
            p.a.state() == LapmState::Disconnected && p.b.state() == LapmState::Disconnected
        });
        assert!(ok, "A {:?} B {:?}", pair.a.state(), pair.b.state());
        assert!(!pair.a.is_established() && !pair.b.is_established());
    }
}
