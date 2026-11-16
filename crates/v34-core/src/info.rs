//! Séquences INFO de la phase 2 (V.34 §10.1.2.3) : codage binaire, CRC,
//! et modem DPSK 600 bit/s qui les transporte.

use crate::api::{CrcMode, NOMINAL_RMS};
use crate::dsp::osc::Osc;
use crate::dsp::shaping::{Downconverter, PassbandMod};
use crate::dsp::Cpx;

/// Synchro de trame INFO : 01110010 (bit de gauche émis en premier).
const INFO_SYNC: [bool; 8] = [false, true, true, true, false, false, true, false];

/// CRC V.34 : x^16 + x^12 + x^5 + 1, registre initialisé à 1.
pub fn crc16(bits: &[bool]) -> u16 {
    let mut reg: u16 = 0xFFFF;
    for &b in bits {
        let fb = ((reg >> 15) & 1) as u8 ^ b as u8;
        reg <<= 1;
        if fb == 1 {
            reg ^= 0x1021;
        }
    }
    reg
}

/// Ajoute le CRC à la trame : bit de poids FORT en premier (contrairement
/// aux champs numériques, émis poids faible en premier). Vérifié sur un
/// vrai modem (U.S. Robotics 56K, 1998) : son INFO0 n'a un CRC juste que
/// lu ainsi. Vaut pour toutes les séquences qui utilisent ce CRC (INFO,
/// MP de V.34, Ja/Jd/CP/MP de V.90). Voir [`CrcMode`] pour l'ancien ordre.
pub fn push_crc(bits: &mut Vec<bool>, crc: u16) {
    for i in (0..16).rev() {
        bits.push((crc >> i) & 1 == 1);
    }
}

/// Relit un CRC émis par [`push_crc`].
pub fn read_crc(bits: &[bool], start: usize) -> u16 {
    bits[start..start + 16].iter().fold(0, |a, &b| a << 1 | b as u16)
}

/// Passe le CRC (16 bits à partir de `pos`) d'une trame à l'ancien ordre
/// (poids faible en premier), ou l'inverse : il suffit de le retourner.
pub fn swap_crc(bits: &mut [bool], pos: usize) {
    bits[pos..pos + 16].reverse();
}

/// Ordre dans lequel le CRC d'une trame reçue était juste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrcSeen {
    Standard,
    Legacy,
    /// Les deux (CRC palindrome) : on ne peut rien en conclure
    Either,
}

/// Vérifie le CRC d'une trame (`crc` = CRC calculé sur la charge utile,
/// lu à partir de `pos`) dans les ordres que `mode` accepte.
pub fn check_crc(crc: u16, frame: &[bool], pos: usize, mode: CrcMode) -> Option<CrcSeen> {
    let std = mode != CrcMode::Legacy && read_crc(frame, pos) == crc;
    let leg = mode != CrcMode::Standard && read_crc(frame, pos) == crc.reverse_bits();
    match (std, leg) {
        (true, true) => Some(CrcSeen::Either),
        (true, false) => Some(CrcSeen::Standard),
        (false, true) => Some(CrcSeen::Legacy),
        (false, false) => None,
    }
}

/// Position du CRC dans une trame INFO complète (suivi de 4 bits de remplissage).
pub fn info_crc_pos(frame_len: usize) -> usize {
    frame_len - 20
}

fn push_field(bits: &mut Vec<bool>, value: u32, n: usize) {
    for i in 0..n {
        bits.push((value >> i) & 1 == 1);
    }
}

fn field(bits: &[bool], start: usize, n: usize) -> u32 {
    (0..n).fold(0, |acc, i| acc | ((bits[start + i] as u32) << i))
}

/// Encadre des bits d'information : remplissage, synchro, info, CRC, remplissage.
fn frame(info: &[bool]) -> Vec<bool> {
    let mut b = vec![true; 4];
    b.extend_from_slice(&INFO_SYNC);
    b.extend_from_slice(info);
    push_crc(&mut b, crc16(info));
    b.extend_from_slice(&[true; 4]);
    b
}

/// INFO0 (Table 14) — capacités du modem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info0 {
    pub sr2743: bool,
    pub sr2800: bool,
    pub sr3429: bool,
    pub low3000: bool,
    pub high3000: bool,
    pub low3200: bool,
    pub high3200: bool,
    pub allow3429: bool,
    pub power_reduction: bool,
    /// Écart max de vitesse de symbole entre les deux sens (0..5)
    pub max_asym: u8,
    pub cme: bool,
    pub c1664: bool,
    pub tx_clock: u8,
    /// Acquittement d'un INFO0 reçu (reprise sur erreur)
    pub ack: bool,
    /// Champs propres à l'INFO0d du modem numérique V.90 (tableau 7/V.90)
    pub v90d: Option<Info0d>,
}

/// Bits 29:41 de l'INFO0d (V.90) : puissance et loi du modem numérique.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info0d {
    /// Puissance nominale en phase 2, par pas de −1 dBm0 (0 = −6 dBm0)
    pub nominal_power: u8,
    /// Puissance maximale, par pas de −0,5 dBm0 (0 = −0,5 dBm0)
    pub max_power: u8,
    /// Puissance mesurée en sortie du codec (sinon aux bornes du modem)
    pub power_at_codec: bool,
    /// Loi A (sinon µ)
    pub alaw: bool,
    /// V.90 possible avec 3429 bauds en montée
    pub up3429: bool,
}

impl Info0 {
    pub fn ours(cfg: &crate::api::ModemConfig) -> Self {
        Info0 {
            sr2743: cfg.allow_2743,
            sr2800: cfg.allow_2800,
            sr3429: cfg.allow_3429,
            low3000: true,
            high3000: true,
            low3200: true,
            high3200: true,
            allow3429: cfg.allow_3429,
            power_reduction: false,
            max_asym: 5,
            cme: false,
            c1664: true,
            tx_clock: 0,
            ack: false,
            v90d: None,
        }
    }

    pub const INFO_BITS: usize = 17; // bits 12..28
    /// INFO0d du modem numérique V.90 : bits 12..41
    pub const INFO0D_BITS: usize = 30;

    pub fn to_bits(&self) -> Vec<bool> {
        let mut i = Vec::new();
        for b in [self.sr2743, self.sr2800, self.sr3429, self.low3000, self.high3000, self.low3200, self.high3200, self.allow3429, self.power_reduction] {
            i.push(b);
        }
        push_field(&mut i, self.max_asym as u32, 3);
        i.push(self.cme);
        i.push(self.c1664);
        push_field(&mut i, self.tx_clock as u32, 2);
        i.push(self.ack);
        if let Some(d) = &self.v90d {
            push_field(&mut i, d.nominal_power as u32, 4);
            push_field(&mut i, d.max_power as u32, 5);
            i.push(d.power_at_codec);
            i.push(d.alaw);
            i.push(d.up3429);
            i.push(false); // réservé (bit 41)
        }
        frame(&i)
    }

    pub fn from_info(i: &[bool]) -> Self {
        Info0 {
            sr2743: i[0],
            sr2800: i[1],
            sr3429: i[2],
            low3000: i[3],
            high3000: i[4],
            low3200: i[5],
            high3200: i[6],
            allow3429: i[7],
            power_reduction: i[8],
            max_asym: field(i, 9, 3) as u8,
            cme: i[12],
            c1664: i[13],
            tx_clock: field(i, 14, 2) as u8,
            ack: i[16],
            v90d: (i.len() >= Self::INFO0D_BITS).then(|| Info0d {
                nominal_power: field(i, 17, 4) as u8,
                max_power: field(i, 21, 5) as u8,
                power_at_codec: i[26],
                alaw: i[27],
                up3429: i[28],
            }),
        }
    }
}

/// Résultat de sondage pour une vitesse de symbole (champ de 9 bits d'INFO1c).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeResult {
    pub high_carrier: bool,
    /// Index de pré-accentuation 0..10
    pub preemph: u8,
    /// Débit max projeté en multiples de 2400 (0 = vitesse inutilisable)
    pub max_rate: u8,
}

/// INFO1c (Table 15) — résultats du sondage du sens appelé→appelant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info1c {
    pub min_power_red: u8,
    pub add_power_red: u8,
    pub md_len: u8,
    /// Index 0..5 = 2400, 2743, 2800, 3000, 3200, 3429
    pub results: [ProbeResult; 6],
    /// Décalage de fréquence mesuré, en 0,02 Hz (−512 = inconnu)
    pub freq_offset: i16,
}

impl Info1c {
    pub const INFO_BITS: usize = 77; // bits 12..88

    pub fn to_bits(&self) -> Vec<bool> {
        let mut i = Vec::new();
        push_field(&mut i, self.min_power_red as u32, 3);
        push_field(&mut i, self.add_power_red as u32, 3);
        push_field(&mut i, self.md_len as u32, 7);
        for r in &self.results {
            i.push(r.high_carrier);
            push_field(&mut i, r.preemph as u32, 4);
            push_field(&mut i, r.max_rate as u32, 4);
        }
        push_field(&mut i, (self.freq_offset as i32 & 0x3FF) as u32, 10);
        frame(&i)
    }

    pub fn from_info(i: &[bool]) -> Self {
        let mut results = [ProbeResult::default(); 6];
        for (k, r) in results.iter_mut().enumerate() {
            let o = 13 + 9 * k;
            *r = ProbeResult { high_carrier: i[o], preemph: field(i, o + 1, 4) as u8, max_rate: field(i, o + 5, 4) as u8 };
        }
        let fo = field(i, 67, 10) as i32;
        let fo = if fo & 0x200 != 0 { fo - 1024 } else { fo };
        Info1c {
            min_power_red: field(i, 0, 3) as u8,
            add_power_red: field(i, 3, 3) as u8,
            md_len: field(i, 6, 7) as u8,
            results,
            freq_offset: fo as i16,
        }
    }
}

/// INFO1a (Table 16) — décision finale de l'appelé.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info1a {
    pub min_power_red: u8,
    pub add_power_red: u8,
    pub md_len: u8,
    /// Porteuse haute pour le sens appelant→appelé
    pub high_carrier: bool,
    pub preemph: u8,
    /// Débit max projeté appelant→appelé (multiples de 2400)
    pub max_rate: u8,
    /// Vitesse de symbole appelé→appelant (index 0..5)
    pub sr_a2c: u8,
    /// Vitesse de symbole appelant→appelé (index 0..5 ; 6 = V.90, 8000
    /// symboles PCM par seconde)
    pub sr_c2a: u8,
    pub freq_offset: i16,
    /// V.90 (tableau 10/V.90) : Ucode du point de l'apprentissage à deux
    /// niveaux (UINFO) ; présent si et seulement si `sr_c2a == 6`
    pub uinfo: Option<u8>,
}

impl Info1a {
    pub const INFO_BITS: usize = 38; // bits 12..49

    pub fn to_bits(&self) -> Vec<bool> {
        let mut i = Vec::new();
        if let Some(u) = self.uinfo {
            // INFO1a du V.90 : réservés, MD, UINFO, réservés, montée, 6
            push_field(&mut i, 0, 6);
            push_field(&mut i, self.md_len as u32, 7);
            push_field(&mut i, u as u32, 7);
            push_field(&mut i, 0, 2);
            push_field(&mut i, self.sr_a2c as u32, 3);
            push_field(&mut i, 6, 3);
            push_field(&mut i, (self.freq_offset as i32 & 0x3FF) as u32, 10);
            return frame(&i);
        }
        push_field(&mut i, self.min_power_red as u32, 3);
        push_field(&mut i, self.add_power_red as u32, 3);
        push_field(&mut i, self.md_len as u32, 7);
        i.push(self.high_carrier);
        push_field(&mut i, self.preemph as u32, 4);
        push_field(&mut i, self.max_rate as u32, 4);
        push_field(&mut i, self.sr_a2c as u32, 3);
        push_field(&mut i, self.sr_c2a as u32, 3);
        push_field(&mut i, (self.freq_offset as i32 & 0x3FF) as u32, 10);
        frame(&i)
    }

    pub fn from_info(i: &[bool]) -> Self {
        let fo = field(i, 28, 10) as i32;
        let fo = if fo & 0x200 != 0 { fo - 1024 } else { fo };
        if field(i, 25, 3) == 6 {
            return Info1a {
                min_power_red: 0,
                add_power_red: 0,
                md_len: field(i, 6, 7) as u8,
                high_carrier: false,
                preemph: 0,
                max_rate: 0,
                sr_a2c: field(i, 22, 3) as u8,
                sr_c2a: 6,
                freq_offset: fo as i16,
                uinfo: Some(field(i, 13, 7) as u8),
            };
        }
        Info1a {
            min_power_red: field(i, 0, 3) as u8,
            add_power_red: field(i, 3, 3) as u8,
            md_len: field(i, 6, 7) as u8,
            high_carrier: i[13],
            preemph: field(i, 14, 4) as u8,
            max_rate: field(i, 18, 4) as u8,
            sr_a2c: field(i, 22, 3) as u8,
            sr_c2a: field(i, 25, 3) as u8,
            freq_offset: fo as i16,
            uinfo: None,
        }
    }
}

/// Recherche de trames INFO dans un flux binaire.
///
/// On garde une fenêtre glissante de « 11 + synchro + info + CRC » et on
/// teste chaque position : une fausse synchro (fréquente dans du bruit)
/// ne peut donc jamais masquer une vraie trame qui commence juste après.
#[derive(Clone, Debug)]
pub struct InfoFramer {
    win: std::collections::VecDeque<bool>,
    /// Deux bits de remplissage supplémentaires (pour juger « 1111 »)
    pre: u8,
    /// Nombre de bits d'information attendus (17, 77 ou 38)
    pub info_bits: usize,
    /// Ordres de CRC acceptés
    pub crc_mode: CrcMode,
    /// Ordre du CRC de la dernière trame acceptée
    pub seen: Option<CrcSeen>,
}

const SYNC_HDR: [bool; 10] = [true, true, false, true, true, true, false, false, true, false];

impl InfoFramer {
    pub fn new(info_bits: usize) -> Self {
        Self { win: Default::default(), pre: 0, info_bits, crc_mode: CrcMode::Auto, seen: None }
    }

    /// Renvoie les bits d'information d'une trame dont le CRC est bon, ou
    /// `Err(())` si une trame bien synchronisée (« 1111 » + synchro) a un
    /// CRC faux.
    pub fn push(&mut self, bit: bool) -> Option<Result<Vec<bool>, ()>> {
        let len = 10 + self.info_bits + 16;
        self.win.push_back(bit);
        if self.win.len() > len {
            let old = self.win.pop_front().unwrap();
            self.pre = (self.pre << 1 | old as u8) & 3;
        }
        if self.win.len() < len || !self.win.iter().take(10).eq(SYNC_HDR.iter()) {
            return None;
        }
        let bits: Vec<bool> = self.win.iter().copied().collect();
        let info = &bits[10..10 + self.info_bits];
        if let Some(seen) = check_crc(crc16(info), &bits, 10 + self.info_bits, self.crc_mode) {
            self.seen = Some(seen);
            let out = info.to_vec();
            self.win.clear();
            self.pre = 0;
            Some(Ok(out))
        } else if self.pre == 3 {
            Some(Err(()))
        } else {
            None
        }
    }

    pub fn reset(&mut self) {
        self.win.clear();
        self.pre = 0;
    }
}

/// Émetteur DPSK des séquences INFO (600 bit/s ; 1 = rotation de 180°).
/// Appelant : porteuse 1200 Hz au niveau nominal.
/// Appelé : porteuse 2400 Hz à −1 dB + ton de garde 1800 Hz à −7 dB.
pub struct InfoTx {
    m: PassbandMod,
    guard: Option<Osc>,
    guard_amp: f32,
    point: Cpx,
    bits: std::collections::VecDeque<bool>,
    /// Symboles « de queue » émis après la fin pour vider le filtre
    tail: usize,
    need_ref: bool,
}

pub const INFO_BAUD: f64 = 600.0;
const INFO_ALPHA: f64 = 0.5;
const INFO_SPAN: usize = 6;

impl InfoTx {
    pub fn new(answer: bool) -> Self {
        let fc = if answer { 2400.0 } else { 1200.0 };
        let mut m = PassbandMod::new(INFO_BAUD, fc, INFO_ALPHA, INFO_SPAN);
        // Puissance de sortie = |a|²/2 → amplitude pour le niveau voulu
        let level_db = if answer { -1.0 } else { 0.0 };
        m.gain = NOMINAL_RMS * 2f32.sqrt() * 10f32.powf(level_db / 20.0);
        let (guard, guard_amp) = if answer {
            (Some(Osc::new(1800.0)), NOMINAL_RMS * 2f32.sqrt() * 10f32.powf(-7.0 / 20.0))
        } else {
            (None, 0.0)
        };
        Self { m, guard, guard_amp, point: Cpx::new(1.0, 0.0), bits: Default::default(), tail: 0, need_ref: false }
    }

    /// Ajoute des bits à émettre (une ou plusieurs séquences INFO).
    /// Un groupe de séquences est précédé d'un point de référence à phase
    /// arbitraire (§10.1.2.3.1) : c'est lui qui sert de référence au premier
    /// bit différentiel.
    pub fn queue(&mut self, bits: &[bool]) {
        if self.bits.is_empty() {
            self.need_ref = true;
        }
        self.bits.extend(bits.iter().copied());
        self.tail = 0;
    }

    /// Tous les bits ont été émis (en tenant compte du retard du filtre).
    pub fn done(&self) -> bool {
        self.bits.is_empty() && self.tail > 2 * INFO_SPAN
    }

    /// Bits restant à moduler.
    pub fn pending_bits(&self) -> usize {
        self.bits.len()
    }

    pub fn next_sample(&mut self) -> f32 {
        while self.m.needs_symbol() {
            if self.need_ref {
                self.need_ref = false;
                self.m.push_symbol(self.point);
                continue;
            }
            match self.bits.pop_front() {
                Some(b) => {
                    if b {
                        self.point = -self.point;
                    }
                    self.m.push_symbol(self.point);
                }
                None => {
                    self.tail += 1;
                    self.m.push_symbol(Cpx::ZERO);
                }
            }
        }
        let mut x = self.m.next_sample();
        if let Some(g) = self.guard.as_mut() {
            x += self.guard_amp * g.next_cos();
        }
        x
    }
}

/// Récepteur DPSK des séquences INFO, avec cadrage intégré.
///
/// Plutôt que de récupérer le rythme symbole (difficile quand une INFO
/// suit directement un autre signal, ex. L2 → INFO1c), on démodule en
/// parallèle sur 4 phases d'échantillonnage décalées de T/4 : l'une d'elles
/// est toujours à moins de T/8 de l'optimum, ce qui suffit largement en
/// DPSK. La première trame dont le CRC est bon l'emporte.
pub struct InfoRx {
    d: Downconverter,
    sps: f64,
    phases: Vec<InfoPhase>,
    /// Après une trame valide, on ignore les doublons des autres phases
    holdoff_until: f64,
    info_bits: usize,
    pending_err: Option<f64>,
    crc_mode: CrcMode,
    /// Ordre du CRC de la dernière trame rendue
    seen: Option<CrcSeen>,
}

struct InfoPhase {
    t_next: f64,
    prev: Cpx,
    framer: InfoFramer,
}

impl InfoRx {
    /// `answer_signal` : true pour recevoir l'INFO de l'appelé (2400 Hz).
    /// `info_bits` : nombre de bits d'information attendus (17, 77 ou 38).
    pub fn new(answer_signal: bool, info_bits: usize) -> Self {
        let fc = if answer_signal { 2400.0 } else { 1200.0 };
        let sps = 8000.0 / INFO_BAUD;
        let phases = (0..4).map(|k| InfoPhase { t_next: k as f64 * sps / 4.0, prev: Cpx::ZERO, framer: InfoFramer::new(info_bits) }).collect();
        Self {
            d: Downconverter::new(INFO_BAUD, fc, INFO_ALPHA, INFO_SPAN),
            sps,
            phases,
            holdoff_until: -1.0,
            info_bits,
            pending_err: None,
            crc_mode: CrcMode::Auto,
            seen: None,
        }
    }

    /// Ordres de CRC acceptés (voir [`CrcMode`]).
    pub fn with_crc_mode(mut self, mode: CrcMode) -> Self {
        self.crc_mode = mode;
        for p in &mut self.phases {
            p.framer.crc_mode = mode;
        }
        self
    }

    /// Ordre du CRC de la dernière trame rendue.
    pub fn crc_seen(&self) -> Option<CrcSeen> {
        self.seen
    }

    /// Change la longueur de trame attendue (ex. INFO0 → INFO1).
    pub fn expect(&mut self, info_bits: usize) {
        self.info_bits = info_bits;
        for p in &mut self.phases {
            p.framer = InfoFramer::new(info_bits);
            p.framer.crc_mode = self.crc_mode;
        }
    }

    /// Traite un échantillon ; renvoie les bits d'information d'une trame
    /// valide, ou `Err(())` si une trame a été vue sur toutes les phases
    /// actives mais avec un CRC faux.
    pub fn push(&mut self, x: f32) -> Option<Result<Vec<bool>, ()>> {
        self.d.push(x);
        let t_now = self.d.latest();
        let mut out = None;
        let mut errs = 0;
        for k in 0..self.phases.len() {
            if t_now < self.phases[k].t_next {
                continue;
            }
            let t = self.phases[k].t_next;
            self.phases[k].t_next += self.sps;
            let y = match self.d.at(t) {
                Some(y) => y,
                None => continue,
            };
            let ph = &mut self.phases[k];
            let p = y.norm_sqr();
            let valid = p > 1e-6 && ph.prev.norm_sqr() > 1e-6;
            let dot = y.re * ph.prev.re + y.im * ph.prev.im;
            ph.prev = y;
            if !valid {
                continue;
            }
            match ph.framer.push(dot < 0.0) {
                Some(Ok(bits)) => {
                    if t > self.holdoff_until && out.is_none() {
                        self.seen = ph.framer.seen;
                        out = Some(Ok(bits));
                        self.holdoff_until = t + 8.0 * self.sps;
                    }
                }
                Some(Err(())) => errs += 1,
                None => {}
            }
        }
        if out.is_none() && errs > 0 && t_now > self.holdoff_until {
            // On ne signale l'erreur que si aucune autre phase ne réussit
            // dans la foulée : on attend la fin des 4 phases (≤ 1 symbole).
            self.pending_err = Some(t_now + self.sps);
        }
        if let Some(t) = self.pending_err {
            if out.is_some() {
                self.pending_err = None;
            } else if t_now >= t {
                self.pending_err = None;
                if t_now > self.holdoff_until {
                    return Some(Err(()));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// INFO0 émis par un vrai modem (U.S. Robotics 56K Voice, firmware
    /// 12.1.0 de 1998), capté à travers un adaptateur téléphonique : il
    /// fixe l'ordre des bits du CRC (poids fort en premier).
    #[test]
    fn info0_d_un_vrai_usr_56k() {
        let s = "1111".to_owned() + "01110010" + "11111111100001000" + "1000111111011001" + "1111";
        let bits: Vec<bool> = s.chars().map(|c| c == '1').collect();
        let mut f = InfoFramer::new(Info0::INFO_BITS);
        let got = bits.iter().find_map(|&b| f.push(b)).expect("trame repérée");
        let i0 = Info0::from_info(&got.expect("CRC juste"));
        assert!(i0.sr2743 && i0.sr2800 && i0.sr3429 && i0.low3000 && i0.high3000 && i0.low3200 && i0.high3200 && i0.allow3429);
        assert!(i0.power_reduction && i0.c1664 && !i0.cme && !i0.ack);
        assert_eq!((i0.max_asym, i0.tx_clock), (0, 0));
        // notre encodage des mêmes capacités redonne exactement la trame
        assert_eq!(i0.to_bits(), bits);
    }

    #[test]
    fn info0d_et_info1a_du_v90() {
        let mut i = Info0::ours(&crate::api::ModemConfig::default());
        i.v90d = Some(Info0d { nominal_power: 3, max_power: 1, power_at_codec: true, alaw: true, up3429: false });
        let bits = i.to_bits();
        assert_eq!(bits.len(), 62, "tableau 7/V.90 : bits 0..61");
        let mut f = InfoFramer::new(Info0::INFO0D_BITS);
        let got = bits.iter().find_map(|&b| f.push(b)).unwrap().unwrap();
        assert_eq!(Info0::from_info(&got), i);

        let a = Info1a { min_power_red: 0, add_power_red: 0, md_len: 4, high_carrier: false, preemph: 0, max_rate: 0, sr_a2c: 4, sr_c2a: 6, freq_offset: -3, uinfo: Some(90) };
        let bits = a.to_bits();
        assert_eq!(bits.len(), 70);
        let mut f = InfoFramer::new(Info1a::INFO_BITS);
        let got = bits.iter().find_map(|&b| f.push(b)).unwrap().unwrap();
        assert_eq!(Info1a::from_info(&got), a);
        // bits 25:31 = UINFO, 37:39 = 6 (tableau 10/V.90)
        assert_eq!(field(&bits, 25, 7), 90);
        assert_eq!(field(&bits, 37, 3), 6);
    }

    #[test]
    fn info0_roundtrip() {
        let i = Info0::ours(&crate::api::ModemConfig::default());
        let bits = i.to_bits();
        assert_eq!(bits.len(), 49);
        let mut f = InfoFramer::new(Info0::INFO_BITS);
        let mut got = None;
        for &b in &bits {
            if let Some(Ok(info)) = f.push(b) {
                got = Some(Info0::from_info(&info));
            }
        }
        assert_eq!(got, Some(i));
    }

    #[test]
    fn info1_lengths() {
        let c = Info1c { min_power_red: 0, add_power_red: 0, md_len: 0, results: [ProbeResult { high_carrier: true, preemph: 3, max_rate: 12 }; 6], freq_offset: -7 };
        assert_eq!(c.to_bits().len(), 109);
        let info: Vec<bool> = c.to_bits()[12..89].to_vec();
        assert_eq!(Info1c::from_info(&info), c);
        let a = Info1a { min_power_red: 1, add_power_red: 2, md_len: 0, high_carrier: true, preemph: 5, max_rate: 13, sr_a2c: 5, sr_c2a: 4, freq_offset: 12, uinfo: None };
        assert_eq!(a.to_bits().len(), 70);
        let info: Vec<bool> = a.to_bits()[12..50].to_vec();
        assert_eq!(Info1a::from_info(&info), a);
    }

    #[test]
    fn dpsk_over_the_air() {
        for answer in [false, true] {
            let mut tx = InfoTx::new(answer);
            let mut rx = InfoRx::new(answer, Info0::INFO_BITS);
            let info = Info0::ours(&crate::api::ModemConfig::default());
            // silence avant, puis 2 INFO0 à la suite
            let mut got = 0;
            for n in 0..12000 {
                if n == 333 {
                    tx.queue(&info.to_bits());
                    tx.queue(&info.to_bits());
                }
                let x = if n >= 333 { tx.next_sample() } else { 0.0 };
                if let Some(Ok(bits)) = rx.push(x) {
                    assert_eq!(Info0::from_info(&bits), info);
                    got += 1;
                }
            }
            assert_eq!(got, 2, "INFO0 reçus (appelé={answer})");
        }
    }
}

#[cfg(test)]
mod burst_tests {
    use super::*;

    /// La toute première trame d'une salve doit être reçue, quel que soit
    /// le décalage de rythme entre émetteur et récepteur.
    #[test]
    fn first_frame_of_burst_is_received() {
        for answer in [false, true] {
            for off in 0..14usize {
                let mut tx = InfoTx::new(answer);
                let mut rx = InfoRx::new(answer, Info0::INFO_BITS);
                let info = Info0::ours(&crate::api::ModemConfig::default());
                // Précédé d'un autre signal (pas de silence) : cas L2 → INFO1c
                let mut probe = crate::probe::ProbeGen::new(0.0);
                let mut ok = false;
                for n in 0..2500 {
                    if n == 600 + off {
                        tx.queue(&info.to_bits());
                    }
                    let x = if n >= 600 + off { tx.next_sample() } else { probe.next_sample() };
                    if let Some(Ok(bits)) = rx.push(x) {
                        ok = Info0::from_info(&bits) == info;
                    }
                }
                assert!(ok, "trame perdue (appelé={answer}, décalage={off})");
            }
        }
    }
}
