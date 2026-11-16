//! Tampon de gigue adaptatif.
//!
//! Les paquets UDP arrivent avec des délais variables, parfois dans le
//! désordre, parfois jamais. Le tampon les remet en ordre et les restitue
//! à cadence régulière (une trame de 20 ms à la fois), avec une profondeur
//! cible (~60 ms) qui s'adapte : elle augmente après un sous-débit et
//! redescend lentement quand le réseau est stable.
//!
//! Pour un modem, toute perte ou tout saut de délai est une vraie
//! perturbation de la « ligne ». Le pire est le **glissement** : quand le
//! tampon se re-remplit (silence inséré) ou jette des trames, tout le signal
//! qui suit est décalé de 20 ms, et l'égaliseur comme le treillis du modem
//! déraillent jusqu'au prochain réapprentissage. D'où le mode « modem »
//! ([`JitterBuffer::for_modem`]) : profondeur fixe et généreuse, et un paquet
//! en retard devient un simple trou de 20 ms (silence) *à sa place*, sans
//! décaler la suite. C'est alors au récepteur de suivre l'horloge de
//! l'émetteur (voir [`JitterBuffer::level`]).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::packet::FRAME_SAMPLES;

const FRAME_MS: f32 = 20.0;
/// Trames stables (sans sous-débit) avant de réduire la cible (10 s).
const STABLE_FRAMES_BEFORE_SHRINK: u32 = 500;
/// Au-delà de cet écart de séquence, on considère un nouveau flux.
const RESYNC_GAP: i64 = 250;
/// Mode modem : paquets en retard consécutifs avant de se recaler (l'autre
/// côté s'est vraiment arrêté un moment ; le glissement est inévitable).
const LATE_RESYNC: u32 = 5;

/// Que jouer quand une trame manque.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Concealment {
    /// Silence
    Silence,
    /// Répète la dernière trame en l'atténuant (comme la plupart des VoIP)
    #[default]
    Repeat,
    /// Pour un modem : silence, profondeur fixe et **jamais de glissement**
    /// (voir [`JitterBuffer::for_modem`])
    Modem,
}

/// Nature d'une trame restituée.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// Vrai audio reçu
    Audio,
    /// Trame inventée pour masquer une perte
    Concealed,
    /// Remplissage initial / après sous-débit
    Buffering,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct JitterStats {
    pub received: u64,
    pub lost: u64,
    pub late: u64,
    pub duplicates: u64,
    pub reordered: u64,
    pub underruns: u64,
    pub dropped: u64,
    pub concealed: u64,
    /// Recalages forcés (mode modem : glissement inévitable)
    pub resyncs: u64,
    /// Trames perdues rattrapées grâce à la copie du paquet suivant
    pub recovered: u64,
    /// Gigue estimée (RFC 3550), ms
    pub jitter_ms: f32,
    /// Profondeur actuelle du tampon, ms
    pub depth_ms: f32,
    /// Profondeur cible actuelle, ms
    pub target_ms: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Buffering,
    Playing,
}

pub struct JitterBuffer {
    frames: BTreeMap<i64, Vec<f32>>,
    /// Numéro de séquence « déplié » de la prochaine trame à jouer
    next: Option<i64>,
    /// Dernier numéro brut (u32) correspondant à `next`, pour le dépliage
    next_raw: u32,
    highest: Option<i64>,
    state: State,
    target: usize,
    min_target: usize,
    max_target: usize,
    stable: u32,
    concealment: Concealment,
    last_frame: Vec<f32>,
    conceal_run: u32,
    jitter: f64,
    last_transit: Option<f64>,
    stats: JitterStats,
    /// Mode modem : jamais de re-remplissage ni de trames jetées (sauf excès)
    slip_free: bool,
    /// Mode modem : profondeur au-delà de laquelle on jette quand même
    hard_max: usize,
    late_run: u32,
    /// Mode modem : re-remplissage après un recalage (on y jettera la rafale)
    after_resync: bool,
}

impl JitterBuffer {
    /// `target_ms` : profondeur cible initiale (arrondie à 20 ms).
    pub fn new(target_ms: u32, concealment: Concealment) -> Self {
        let target = ((target_ms as f32 / FRAME_MS).round() as usize).clamp(1, 25);
        Self {
            frames: BTreeMap::new(),
            next: None,
            next_raw: 0,
            highest: None,
            state: State::Buffering,
            target,
            min_target: if concealment == Concealment::Modem { target } else { target.min(2).max(1) },
            max_target: if concealment == Concealment::Modem { target } else { 10 },
            stable: 0,
            concealment,
            last_frame: vec![0.0; FRAME_SAMPLES],
            conceal_run: 0,
            jitter: 0.0,
            last_transit: None,
            stats: JitterStats::default(),
            slip_free: concealment == Concealment::Modem,
            hard_max: target * 3 + 25,
            late_run: 0,
            after_resync: false,
        }
    }

    /// Tampon pour un modem : profondeur fixe `target_ms` (ne rétrécit pas),
    /// trous de silence à la place des paquets manquants ou en retard, sans
    /// jamais décaler le flux.
    pub fn for_modem(target_ms: u32) -> Self {
        Self::new(target_ms, Concealment::Modem)
    }

    /// Oublie tout (nouveau flux).
    pub fn reset(&mut self) {
        let (t, c) = (self.target, self.concealment);
        let stats = std::mem::take(&mut self.stats);
        *self = Self::new(t as u32 * FRAME_MS as u32, c);
        self.stats = stats;
    }

    /// Niveau du tampon en trames (présentes, cible), une fois la lecture
    /// commencée. Le récepteur s'en sert pour caler sa cadence sur celle de
    /// l'émetteur : trop plein → il traite un bloc de plus ; trop vide → un
    /// de moins. Aucun échantillon n'est ni inventé ni jeté.
    pub fn level(&self) -> Option<(usize, usize)> {
        (self.state == State::Playing).then(|| (self.frames.len(), self.target))
    }

    fn unwrap_seq(&self, seq: u32) -> Option<i64> {
        self.next.map(|n| n + seq.wrapping_sub(self.next_raw) as i32 as i64)
    }

    /// Ajoute un paquet reçu. `arrival_s` : instant d'arrivée en secondes
    /// (horloge locale quelconque), `timestamp` : horodatage du paquet en
    /// échantillons à 8 kHz.
    pub fn push(&mut self, seq: u32, timestamp: u32, samples: Vec<f32>, arrival_s: f64) {
        self.stats.received += 1;

        // Gigue RFC 3550 : variation du temps de transit.
        let transit = arrival_s * 8000.0 - timestamp as f64;
        if let Some(prev) = self.last_transit {
            let d = (transit - prev).abs();
            if d < 8000.0 * 5.0 {
                self.jitter += (d - self.jitter) / 16.0;
            }
        }
        self.last_transit = Some(transit);
        self.stats.jitter_ms = (self.jitter / 8.0) as f32;

        let key = match self.unwrap_seq(seq) {
            None => {
                self.next = Some(0);
                self.next_raw = seq;
                0
            }
            Some(k) => {
                let n = self.next.unwrap();
                if (k - n).abs() > RESYNC_GAP {
                    // Nouveau flux (l'autre côté a redémarré) : on repart de zéro.
                    self.reset();
                    self.next = Some(0);
                    self.next_raw = seq;
                    0
                } else if k < n {
                    if self.state == State::Buffering && self.frames.is_empty() {
                        // Pendant le remplissage initial, on peut reculer le départ.
                        self.next = Some(k);
                        self.next_raw = seq;
                        k
                    } else if self.slip_free && self.late_run + 1 >= LATE_RESYNC {
                        // Tous les paquets arrivent « en retard » : l'autre
                        // côté a pris du retard pour de bon. On se recale sur
                        // lui (glissement inévitable) et on re-remplit.
                        self.stats.late += 1;
                        self.stats.resyncs += 1;
                        self.late_run = 0;
                        self.frames.clear();
                        self.highest = None;
                        self.next = Some(k);
                        self.next_raw = seq;
                        self.state = State::Buffering;
                        self.after_resync = true;
                        k
                    } else {
                        self.stats.late += 1;
                        self.late_run += 1;
                        return;
                    }
                } else {
                    k
                }
            }
        };

        if self.frames.contains_key(&key) {
            self.stats.duplicates += 1;
            return;
        }
        self.late_run = 0;
        if let Some(h) = self.highest {
            if key < h {
                self.stats.reordered += 1;
            }
        }
        self.highest = Some(self.highest.map_or(key, |h| h.max(key)));
        let mut s = samples;
        s.resize(FRAME_SAMPLES, 0.0);
        self.frames.insert(key, s);
        self.update_depth();
    }

    /// Copie d'une trame précédente portée par un paquet (redondance) : prise
    /// seulement si la trame manque encore et n'a pas déjà été jouée.
    /// À appeler après [`push`](Self::push) du paquet qui la porte.
    pub fn push_backup(&mut self, seq: u32, samples: &[f32]) {
        let (Some(key), Some(next)) = (self.unwrap_seq(seq), self.next) else {
            return;
        };
        if key < next || key - next > RESYNC_GAP || self.frames.contains_key(&key) {
            return;
        }
        let mut s = samples.to_vec();
        s.resize(FRAME_SAMPLES, 0.0);
        self.frames.insert(key, s);
        self.stats.recovered += 1;
        self.highest = Some(self.highest.map_or(key, |h| h.max(key)));
        self.update_depth();
    }

    /// Ajoute un paquet et les copies des trames précédentes qu'il porte.
    pub fn push_with_backup(&mut self, seq: u32, timestamp: u32, samples: Vec<f32>, backup: &[Vec<f32>], arrival_s: f64) {
        self.push(seq, timestamp, samples, arrival_s);
        for (i, b) in backup.iter().enumerate() {
            self.push_backup(seq.wrapping_sub(i as u32 + 1), b);
        }
    }

    fn update_depth(&mut self) {
        self.stats.depth_ms = self.frames.len() as f32 * FRAME_MS;
        self.stats.target_ms = self.target as f32 * FRAME_MS;
    }

    fn advance(&mut self) {
        if let Some(n) = self.next.as_mut() {
            *n += 1;
            self.next_raw = self.next_raw.wrapping_add(1);
        }
    }

    fn conceal(&mut self) -> Vec<f32> {
        self.stats.concealed += 1;
        self.conceal_run += 1;
        match self.concealment {
            Concealment::Repeat if self.conceal_run <= 3 => {
                let g = 0.5f32.powi(self.conceal_run as i32);
                self.last_frame.iter().map(|v| v * g).collect()
            }
            _ => vec![0.0; FRAME_SAMPLES],
        }
    }

    /// Restitue la trame suivante (FRAME_SAMPLES échantillons).
    pub fn pop_frame(&mut self) -> (Vec<f32>, FrameKind) {
        let out = self.pop_inner();
        self.update_depth();
        out
    }

    fn pop_inner(&mut self) -> (Vec<f32>, FrameKind) {
        if self.state == State::Buffering {
            if self.frames.len() >= self.target {
                self.state = State::Playing;
                // Mode modem, après un recalage : la rafale des paquets
                // bloqués arrive d'un coup. Tout garder ajouterait sa durée
                // au délai (+500 ms après une coupure d'une demi-seconde),
                // et il ne se résorberait qu'en secondes, pendant le
                // réentraînement qui mesure l'aller-retour. Il y a déjà une
                // discontinuité : on jette l'excédent tout de suite.
                if self.slip_free && std::mem::take(&mut self.after_resync) {
                    while self.frames.len() > self.target {
                        let (&first, _) = self.frames.iter().next().unwrap();
                        self.frames.remove(&first);
                        self.stats.dropped += 1;
                    }
                }
                // Commence à la plus ancienne trame disponible.
                if let Some((&first, _)) = self.frames.iter().next() {
                    if let Some(n) = self.next {
                        if first > n {
                            let skip = first - n;
                            self.next = Some(first);
                            self.next_raw = self.next_raw.wrapping_add(skip as u32);
                        }
                    }
                }
            } else {
                return (vec![0.0; FRAME_SAMPLES], FrameKind::Buffering);
            }
        }

        // Trop de retard accumulé : on jette les plus anciennes trames.
        let limit = if self.slip_free { self.hard_max } else { self.target + 3 };
        while self.frames.len() > limit {
            let (&first, _) = self.frames.iter().next().unwrap();
            self.frames.remove(&first);
            self.stats.dropped += 1;
            let n = self.next.unwrap_or(first);
            if first >= n {
                let skip = first + 1 - n;
                self.next = Some(first + 1);
                self.next_raw = self.next_raw.wrapping_add(skip as u32);
            }
        }

        let n = match self.next {
            Some(n) => n,
            None => return (vec![0.0; FRAME_SAMPLES], FrameKind::Buffering),
        };

        if let Some(frame) = self.frames.remove(&n) {
            self.advance();
            self.last_frame.clone_from(&frame);
            self.conceal_run = 0;
            self.stable += 1;
            if self.stable >= STABLE_FRAMES_BEFORE_SHRINK && self.target > self.min_target {
                self.target -= 1;
                self.stable = 0;
            }
            return (frame, FrameKind::Audio);
        }

        if self.frames.is_empty() {
            self.stats.underruns += 1;
            self.stable = 0;
            if self.slip_free {
                // Mode modem : le temps de la ligne continue. Ce paquet-là
                // est un trou de 20 ms ; s'il arrive plus tard, il sera jeté.
                self.advance();
                return (self.conceal(), FrameKind::Concealed);
            }
            // Sous-débit : le paquet est peut-être juste en retard. On attend
            // (re-remplissage) et on augmente la cible.
            self.target = (self.target + 1).min(self.max_target);
            self.state = State::Buffering;
            return (self.conceal(), FrameKind::Concealed);
        }

        // Trame manquante alors que les suivantes sont là : perdue.
        self.stats.lost += 1;
        self.advance();
        (self.conceal(), FrameKind::Concealed)
    }

    pub fn stats(&self) -> &JitterStats {
        &self.stats
    }

    pub fn target_ms(&self) -> f32 {
        self.target as f32 * FRAME_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(v: f32) -> Vec<f32> {
        vec![v; FRAME_SAMPLES]
    }

    fn push(jb: &mut JitterBuffer, seq: u32) {
        jb.push(seq, seq * 160, frame(seq as f32), seq as f64 * 0.02);
    }

    #[test]
    fn reorders_packets() {
        let mut jb = JitterBuffer::new(60, Concealment::Silence);
        for s in [0, 2, 1, 4, 3] {
            push(&mut jb, s);
        }
        let mut got = Vec::new();
        for _ in 0..5 {
            let (f, k) = jb.pop_frame();
            assert_eq!(k, FrameKind::Audio);
            got.push(f[0] as u32);
        }
        assert_eq!(got, vec![0, 1, 2, 3, 4]);
        assert_eq!(jb.stats().lost, 0);
        assert_eq!(jb.stats().reordered, 2);
    }

    #[test]
    fn counts_losses_and_conceals() {
        let mut jb = JitterBuffer::new(60, Concealment::Repeat);
        for s in [0, 1, 3, 4, 5] {
            push(&mut jb, s);
        }
        let kinds: Vec<(FrameKind, f32)> = (0..5).map(|_| {
            let (f, k) = jb.pop_frame();
            (k, f[0])
        }).collect();
        assert_eq!(kinds[0], (FrameKind::Audio, 0.0));
        assert_eq!(kinds[1], (FrameKind::Audio, 1.0));
        assert_eq!(kinds[2], (FrameKind::Concealed, 0.5)); // répétition atténuée de la trame 1
        assert_eq!(kinds[3], (FrameKind::Audio, 3.0));
        assert_eq!(jb.stats().lost, 1);
    }

    #[test]
    fn late_and_duplicate_packets() {
        let mut jb = JitterBuffer::new(40, Concealment::Silence);
        for s in 0..4 {
            push(&mut jb, s);
        }
        push(&mut jb, 3);
        assert_eq!(jb.stats().duplicates, 1);
        jb.pop_frame();
        jb.pop_frame();
        push(&mut jb, 1);
        assert_eq!(jb.stats().late, 1);
    }

    #[test]
    fn underrun_rebuffers_and_grows_target() {
        let mut jb = JitterBuffer::new(40, Concealment::Silence);
        push(&mut jb, 0);
        assert_eq!(jb.pop_frame().1, FrameKind::Buffering);
        push(&mut jb, 1);
        assert_eq!(jb.pop_frame().1, FrameKind::Audio);
        assert_eq!(jb.pop_frame().1, FrameKind::Audio);
        let before = jb.target_ms();
        assert_eq!(jb.pop_frame().1, FrameKind::Concealed);
        assert_eq!(jb.stats().underruns, 1);
        assert!(jb.target_ms() > before);
        // Le paquet 2 arrive en retard mais pas perdu : il sera joué après re-remplissage.
        for s in 2..5 {
            push(&mut jb, s);
        }
        let (f, k) = jb.pop_frame();
        assert_eq!((k, f[0]), (FrameKind::Audio, 2.0));
    }

    /// Modem : coupure d'une demi-seconde puis rafale des paquets bloqués →
    /// recalage, et le délai revient aussitôt à la cible (pas +500 ms).
    #[test]
    fn modem_recalage_sans_garder_la_rafale() {
        let mut jb = JitterBuffer::for_modem(140);
        let mut seq = 0u32;
        for _ in 0..20 {
            push(&mut jb, seq);
            seq += 1;
            jb.pop_frame();
        }
        // 27 trames sans rien (joués en trous), puis tout arrive d'un coup
        for _ in 0..27 {
            jb.pop_frame();
        }
        for _ in 0..33 {
            push(&mut jb, seq);
            seq += 1;
        }
        jb.pop_frame();
        assert!(jb.stats().resyncs >= 1);
        assert!(jb.stats().depth_ms <= 140.0, "délai après recalage : {} ms", jb.stats().depth_ms);
    }

    #[test]
    fn excess_depth_is_trimmed() {
        let mut jb = JitterBuffer::new(60, Concealment::Silence);
        for s in 0..20 {
            push(&mut jb, s);
        }
        let (f, k) = jb.pop_frame();
        assert_eq!(k, FrameKind::Audio);
        assert!(f[0] >= 14.0, "{}", f[0]);
        assert!(jb.stats().dropped > 0);
    }

    #[test]
    fn stream_restart_resyncs() {
        let mut jb = JitterBuffer::new(40, Concealment::Silence);
        for s in 1000..1004 {
            push(&mut jb, s);
        }
        jb.pop_frame();
        for s in 0..3 {
            push(&mut jb, s);
        }
        let (f, k) = jb.pop_frame();
        assert_eq!((k, f[0]), (FrameKind::Audio, 0.0));
    }

    #[test]
    fn lost_packet_recovered_from_next_one() {
        let mut jb = JitterBuffer::for_modem(60);
        let frame = |v: f32| vec![v; FRAME_SAMPLES];
        let mut t = 0.0;
        for seq in 0..10u32 {
            if seq == 4 {
                t += 0.02;
                continue; // paquet 4 perdu…
            }
            let backup = if seq == 0 { vec![] } else { vec![frame(seq as f32 - 1.0)] };
            jb.push_with_backup(seq, seq * FRAME_SAMPLES as u32, frame(seq as f32), &backup, t);
            t += 0.02;
        }
        // … rattrapé grâce au paquet 5
        let mut got = Vec::new();
        for _ in 0..10 {
            let (f, kind) = jb.pop_frame();
            if kind == FrameKind::Audio {
                got.push(f[0]);
            }
        }
        assert!(got.contains(&4.0), "{got:?}");
        assert_eq!(jb.stats().recovered, 1);
        assert_eq!(jb.stats().lost, 0);
    }

    #[test]
    fn modem_mode_never_slips() {
        // Cible 3 trames : lecture dès que 0, 1, 2 sont là.
        let mut jb = JitterBuffer::for_modem(60);
        for q in 0..3 {
            push(&mut jb, q);
        }
        for q in 0..3 {
            assert_eq!(jb.pop_frame(), (frame(q as f32), FrameKind::Audio));
        }
        // Le paquet 3 est en retard : trou de silence à SA place…
        assert_eq!(jb.pop_frame(), (vec![0.0; FRAME_SAMPLES], FrameKind::Concealed));
        assert_eq!(jb.stats().underruns, 1);
        // … il arrive enfin : trop tard, jeté. Le 4 est joué juste après,
        // sans re-remplissage ni décalage.
        push(&mut jb, 3);
        push(&mut jb, 4);
        assert_eq!(jb.stats().late, 1);
        assert_eq!(jb.pop_frame(), (frame(4.0), FrameKind::Audio));
        assert_eq!(jb.level(), Some((0, 3)));
    }

    #[test]
    fn modem_mode_keeps_excess_and_resyncs_when_peer_stalls() {
        let mut jb = JitterBuffer::for_modem(60);
        for q in 0..20 {
            push(&mut jb, q);
        }
        // 20 trames d'avance : rien n'est jeté (c'est au récepteur d'accélérer).
        assert_eq!(jb.pop_frame().0, frame(0.0));
        assert_eq!(jb.stats().dropped, 0);
        assert_eq!(jb.level(), Some((19, 3)));
        for _ in 1..20 {
            jb.pop_frame();
        }
        // L'autre côté s'arrête 10 trames puis reprend là où il en était :
        // tout arrive en retard, on finit par se recaler sur lui.
        for _ in 0..10 {
            jb.pop_frame();
        }
        for q in 20..30 {
            push(&mut jb, q);
        }
        assert_eq!(jb.stats().resyncs, 1);
        // recalé sur le flux, au délai cible : les 3 dernières trames
        let (f, k) = jb.pop_frame();
        assert_eq!(k, FrameKind::Audio);
        assert_eq!(f[0], 27.0, "recalé sur le flux : {}", f[0]);
    }
}
