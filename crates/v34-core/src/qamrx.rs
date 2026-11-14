//! Récepteur QAM des phases 3/4 et du mode données.
//!
//! Chaîne : mélange + filtre adapté RRC (évalué aux instants exacts) →
//! échantillonnage à T/2 → égaliseur adaptatif fractionnaire (LMS) →
//! correction de phase de porteuse (boucle du 2e ordre).
//!
//! Le rythme symbole est estimé en boucle ouverte pendant le signal S,
//! puis figé : les deux modems partagent la même horloge à 8 kHz (le
//! réseau est numérique), l'égaliseur absorbe le reste.

use std::collections::VecDeque;

use crate::dsp::shaping::{Downconverter, TimingEstimator};
use crate::dsp::Cpx;
use crate::train::{QAM_ALPHA, QAM_SPAN};

/// Nombre de coefficients de l'égaliseur (espacés de T/2) : 16 symboles.
const NT: usize = 32;
/// Coefficient central (échantillon à l'instant symbole)
const CENTER: usize = 15;
/// Retard de l'égaliseur en symboles : (NT − 1 − CENTER) / 2
pub const EQ_DELAY: i64 = 8;

/// Un symbole en sortie du récepteur.
#[derive(Clone, Copy, Debug)]
pub struct RxSym {
    /// Indice du symbole (0 = premier instant symbole après création)
    pub k: i64,
    /// Sortie de l'égaliseur, phase corrigée
    pub z: Cpx,
    /// Échantillon brut (filtre adapté + CAG) à l'instant symbole
    pub raw: Cpx,
}

pub struct QamRx {
    d: Downconverter,
    sps: f64,
    te: TimingEstimator,
    /// Prochain instant d'échantillonnage T/2 (en échantillons d'entrée)
    t_next: f64,
    timing_frozen: bool,
    /// Nombre d'échantillons T/2 produits
    h: u64,
    /// Décalage de la grille : le symbole k est échantillonné à
    /// k·sps + grid_offset (échantillons d'entrée)
    grid_offset: f64,
    buf: VecDeque<Cpx>,
    eq: Vec<Cpx>,
    agc: f32,
    phase: f32,
    freq: f32,
    /// Niveau moyen |y|² (bande de réception)
    level: f32,
    /// Erreur quadratique moyenne de l'égaliseur (après entraînement)
    mse: f32,
    trained_syms: u64,
    pub mu: f32,
}

impl QamRx {
    pub fn new(symbol_rate: f64, carrier_hz: f64) -> Self {
        let d = Downconverter::new(symbol_rate, carrier_hz, QAM_ALPHA, QAM_SPAN);
        let sps = d.samples_per_symbol();
        let mut eq = vec![Cpx::ZERO; NT];
        eq[CENTER] = Cpx::new(1.0, 0.0);
        Self {
            d,
            sps,
            te: TimingEstimator::new(symbol_rate, 24.0),
            t_next: 0.0,
            // Grille d'échantillonnage fixe : les deux modems partagent la
            // même horloge (le réseau transporte des échantillons), il n'y a
            // donc aucune dérive à suivre ; l'égaliseur fractionnaire T/2 est
            // insensible à la phase d'échantillonnage. (L'estimateur en
            // boucle ouverte reste disponible via `unfreeze_timing`.)
            timing_frozen: true,
            h: 0,
            grid_offset: 0.0,
            buf: VecDeque::from(vec![Cpx::ZERO; NT]),
            eq,
            agc: 1.0,
            phase: 0.0,
            freq: 0.0,
            level: 0.0,
            mse: 1.0,
            trained_syms: 0,
            mu: 0.25,
        }
    }

    /// Niveau reçu moyen (avant CAG).
    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn mse(&self) -> f32 {
        self.mse
    }

    /// SNR estimé à partir de l'erreur de l'égaliseur (dB), pour une
    /// constellation de puissance moyenne 1.
    pub fn snr_db(&self) -> f32 {
        -10.0 * self.mse.max(1e-6).log10()
    }

    /// Fige le rythme symbole.
    pub fn freeze_timing(&mut self) {
        self.timing_frozen = true;
    }

    /// Recalage fin du rythme par corrélation avec une séquence connue
    /// (ex. la première période de PP) : cherche le décalage fractionnaire
    /// τ ∈ [−T/2, T/2] qui maximise |Σ y(t_k + τ)·conj(ref_k)|, puis décale
    /// la grille d'échantillonnage. L'égaliseur repart de zéro (les
    /// échantillons en mémoire sont sur l'ancienne grille).
    /// `refs` : (indice symbole, symbole attendu). Renvoie τ en symboles.
    pub fn fine_timing(&mut self, refs: &[(i64, Cpx)]) -> f64 {
        let mut best = (0.0f64, -1.0f32);
        let mut best_acc = Cpx::ZERO;
        for step in -8..=8 {
            let tau = step as f64 / 16.0;
            let mut acc = Cpx::ZERO;
            let mut ok = 0;
            for &(k, r) in refs {
                let t = k as f64 * self.sps + self.grid_offset + tau * self.sps;
                if let Some(y) = self.d.at(t) {
                    acc += y * r.conj();
                    ok += 1;
                }
            }
            if ok > refs.len() / 2 && acc.norm_sqr() > best.1 {
                best = (tau, acc.norm_sqr());
                best_acc = acc;
            }
        }
        let tau = best.0;
        let shift = tau * self.sps;
        self.grid_offset += shift;
        self.t_next += shift;
        self.reset_equalizer();
        // La même corrélation donne la phase de porteuse et le gain du
        // canal (signal avant CAG) : y ≈ g·r·e^{jφ}, g = |Σ y·r*| / Σ|r|².
        self.phase = best_acc.arg();
        let er: f32 = refs.iter().map(|(_, r)| r.norm_sqr()).sum();
        let g = best_acc.abs() / er.max(1e-9);
        if g > 1e-6 {
            self.agc = 1.0 / g;
        }
        tau
    }

    /// Réactive l'estimation de rythme en boucle ouverte (utile si un jour
    /// les horloges des deux modems diffèrent, ex. liaison acoustique).
    pub fn unfreeze_timing(&mut self) {
        self.timing_frozen = false;
    }

    /// Règle la CAG pour que le signal reçu ait une puissance unité.
    pub fn set_agc_from_level(&mut self) {
        if self.level > 1e-9 {
            self.agc = 1.0 / self.level.sqrt();
        }
    }

    /// Change la fréquence porteuse attendue (ex. après INFO1).
    pub fn reset_equalizer(&mut self) {
        self.eq = vec![Cpx::ZERO; NT];
        self.eq[CENTER] = Cpx::new(1.0, 0.0);
        self.phase = 0.0;
        self.freq = 0.0;
        self.trained_syms = 0;
    }

    /// Traite un échantillon ; renvoie un symbole quand il y en a un.
    pub fn push(&mut self, x: f32) -> Option<RxSym> {
        let y0 = self.d.push(x);
        let t_now = self.d.latest();
        if !self.timing_frozen {
            self.te.push(t_now, y0);
        }
        self.level = 0.998 * self.level + 0.002 * y0.norm_sqr();
        let mut out = None;
        while self.t_next <= t_now {
            let t = self.t_next;
            self.t_next += self.sps / 2.0;
            let y = self.d.at(t).unwrap_or(Cpx::ZERO) * self.agc;
            self.buf.pop_front();
            self.buf.push_back(y);
            let is_symbol_instant = self.h % 2 == 0;
            self.h += 1;
            if !is_symbol_instant {
                continue;
            }
            // Recalage du rythme (avant gel) sur l'estimation en boucle ouverte
            if !self.timing_frozen {
                let before = self.t_next;
                let (frac, _) = self.te.phase();
                let pos = (t / self.sps).fract();
                let mut err = frac - pos;
                if err > 0.5 {
                    err -= 1.0;
                } else if err < -0.5 {
                    err += 1.0;
                }
                self.t_next += 0.2 * err * self.sps;
                self.grid_offset += self.t_next - before;
            }
            let mut z = Cpx::ZERO;
            for (c, b) in self.eq.iter().zip(self.buf.iter()) {
                z += *c * *b;
            }
            self.phase += self.freq;
            let rot = Cpx::from_polar(1.0, -self.phase);
            let k = (self.h as i64 - 1) / 2 - EQ_DELAY;
            out = Some(RxSym { k, z: z * rot, raw: self.buf[CENTER] });
        }
        out
    }

    /// Adapte l'égaliseur et la boucle de phase vers la référence `r`
    /// (symbole connu en entraînement, ou décision). `z` = sortie reçue.
    /// Y a-t-il du signal dans la mémoire de l'égaliseur ? (Faux pendant un
    /// trou : paquet perdu, coupure de ligne.)
    pub fn signal_ok(&self) -> bool {
        let pw: f32 = self.buf.iter().map(|b| b.norm_sqr()).sum::<f32>();
        let pw_ref = NT as f32 * self.level * self.agc * self.agc;
        pw_ref <= 0.0 || pw >= 0.15 * pw_ref
    }

    pub fn adapt(&mut self, z: Cpx, r: Cpx) {
        // Puissance du signal dans la mémoire de l'égaliseur, comparée à
        // celle attendue (niveau moyen lent, après CAG). Pendant un trou de
        // silence (paquet perdu sur Internet, coupure de ligne), elle tombe
        // presque à zéro : le pas normalisé exploserait et l'égaliseur avec.
        // On n'adapte alors rien, et le pas est plafonné juste après.
        let pw: f32 = self.buf.iter().map(|b| b.norm_sqr()).sum::<f32>();
        let pw_ref = NT as f32 * self.level * self.agc * self.agc;
        if pw_ref > 0.0 && pw < 0.15 * pw_ref {
            return;
        }
        let e = r - z;
        self.mse = 0.99 * self.mse + 0.01 * e.norm_sqr();
        // Erreur de phase (sin de l'écart), normalisée
        let rn = r.norm_sqr().max(1e-6);
        let pe = (z.im * r.re - z.re * r.im) / rn;
        // Boucle de phase du 2e ordre (suit un décalage de fréquence de la
        // ligne sans erreur de phase résiduelle). La phase initiale vient de
        // la corrélation sur PP, ce qui évite le combat avec l'égaliseur.
        // ωn = √k2 rad/symbole, amortissement ζ = k1 / (2√k2) ≈ 0,7–1.
        let (k1, k2) = if self.trained_syms < 64 { (0.05, 0.0006) } else { (0.02, 0.0002) };
        self.phase += k1 * pe;
        self.freq += k2 * pe;
        // LMS normalisé dans le domaine non tourné
        let eu = e * Cpx::from_polar(1.0, self.phase);
        let mu = self.mu / pw.max(0.5 * pw_ref).max(1e-6);
        for (c, b) in self.eq.iter_mut().zip(self.buf.iter()) {
            *c += eu * b.conj() * mu;
        }
        self.trained_syms += 1;
    }

    /// Phase de rythme estimée (pour l'affichage / le débogage).
    pub fn timing_phase(&self) -> f64 {
        self.te.phase().0
    }
}

/// Pas d'adaptation de l'égaliseur en fonction du nombre de symboles
/// d'entraînement déjà vus : rapide au début, fin ensuite.
pub fn mu_schedule(trained: usize) -> f32 {
    match trained {
        0..=399 => 0.25,
        400..=999 => 0.12,
        1000..=1999 => 0.05,
        _ => 0.02,
    }
}

/// Détecteur du signal S et de la transition S → S̄ (§10.1.3.7).
///
/// S alterne deux points à 90° : son spectre a une raie à la porteuse et
/// deux raies aux bords de bande (fc ± S/2), que la ligne atténue souvent
/// fortement. On s'appuie donc sur la raie à la porteuse : dans S,
/// y(k)·conj(y(k−2)) > 0 ; S̄ = −S fait basculer ce produit en négatif.
#[derive(Default)]
pub struct SDetector {
    hist: VecDeque<Cpx>,
    /// Références moyennées des deux points de S (parité du symbole)
    refs: [Cpx; 2],
    /// Symboles consécutifs ressemblant à S
    pub s_count: u32,
    neg_count: u32,
    first_neg: i64,
}

pub enum SEvent {
    /// k = indice (approché à ±2) du premier symbole de S̄
    Transition(i64),
}

impl SDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, s: &RxSym) -> Option<SEvent> {
        self.hist.push_back(s.raw);
        if self.hist.len() > 3 {
            self.hist.pop_front();
        }
        if self.hist.len() < 3 {
            return None;
        }
        let par = (s.k & 1) as usize;
        let (a, c) = (self.hist[0], self.hist[2]);
        let p = (c.norm_sqr() * a.norm_sqr()).sqrt().max(1e-9);
        let same = (c * a.conj()).re / p;
        if self.s_count >= 16 {
            // Projection sur la référence de même parité : S̄ = −S
            let r = self.refs[par];
            let proj = (c * r.conj()).re / r.norm_sqr().max(1e-12);
            if proj < 0.0 {
                if self.neg_count == 0 {
                    self.first_neg = s.k;
                }
                self.neg_count += 1;
                if self.neg_count >= 3 && proj < -0.5 {
                    self.s_count = 0;
                    self.neg_count = 0;
                    return Some(SEvent::Transition(self.first_neg));
                }
                return None;
            }
            self.neg_count = 0;
        }
        if same > 0.6 {
            self.s_count += 1;
            self.refs[par] = if self.s_count <= 2 { c } else { self.refs[par] * 0.85 + c * 0.15 };
        } else if self.s_count < 16 {
            self.s_count = 0;
        }
        None
    }
}

/// Recalage fin du début de PP par corrélation (PP est périodique de 48 et
/// a une excellente autocorrélation) : corrige l'incertitude de ±quelques
/// symboles de la détection S → S̄.
pub struct PpAligner {
    /// Début de la séquence supposé (indice symbole)
    pub guess: i64,
    raw: Vec<(i64, Cpx)>,
    /// Séquence de référence (PP par défaut, ou TRN en renégociation)
    reference: Vec<Cpx>,
    /// Corrélation normalisée (0..1) du meilleur recalage
    score: f32,
}

pub const PP_ALIGN_SPAN: i64 = 4;

impl PpAligner {
    pub fn new(guess: i64) -> Self {
        Self { guess, raw: Vec::with_capacity(64), reference: crate::train::seq_pp()[..48].to_vec(), score: 0.0 }
    }

    /// Recalage sur une autre séquence connue (48 premiers symboles).
    pub fn with_reference(guess: i64, reference: &[Cpx]) -> Self {
        Self { guess, raw: Vec::with_capacity(64), reference: reference[..48].to_vec(), score: 0.0 }
    }

    /// Ajoute un symbole brut ; renvoie le début de PP corrigé quand la
    /// première période a été vue.
    pub fn push(&mut self, s: &RxSym) -> Option<i64> {
        if s.k >= self.guess - PP_ALIGN_SPAN {
            self.raw.push((s.k, s.raw));
        }
        if s.k < self.guess + 48 + PP_ALIGN_SPAN {
            return None;
        }
        let pp = &self.reference;
        let mut best = (0i64, -1.0f32);
        for d in -PP_ALIGN_SPAN..=PP_ALIGN_SPAN {
            let start = self.guess + d;
            let mut acc = Cpx::ZERO;
            let mut e = 0.0f32;
            for &(k, r) in &self.raw {
                let i = k - start;
                if (0..48).contains(&i) {
                    acc += r * pp[i as usize].conj();
                    e += r.norm_sqr();
                }
            }
            let score = acc.norm_sqr() / e.max(1e-9);
            if score > best.1 {
                best = (d, score);
            }
        }
        let eref: f32 = pp.iter().map(|r| r.norm_sqr()).sum();
        self.score = best.1 / eref.max(1e-9);
        Some(self.guess + best.0)
    }

    /// Qualité du recalage : ≈ 1 si la séquence reçue est bien la référence,
    /// ≈ 0 si c'est autre chose (ex. des MP au lieu du TRN attendu).
    pub fn score(&self) -> f32 {
        self.score
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::{LineParams, LineSim};
    use crate::train::{seq_pp, seq_s, QamTx, TrainMapper};

    /// Phase 3 « à blanc » : S, S̄, PP, TRN à travers une ligne, détection
    /// de la transition, entraînement sur PP + TRN, puis vérification que
    /// les décisions sur la suite de TRN sont toutes justes.
    fn train_over(preset: &str, sr: f64, fc: f64) -> f32 {
        let params = LineParams::preset(preset).expect("préréglage");
        let mut line = LineSim::new(params);
        let mut tx = QamTx::new(sr, fc);
        let mut rx = QamRx::new(sr, fc);
        let mut sd = SDetector::new();
        let mut mapper = TrainMapper::new(false);
        // Séquence émise : silence, S 128, S̄ 16, PP 288, TRN 2000
        let mut sent = Vec::new();
        sent.extend(seq_s(128, false));
        sent.extend(seq_s(16, true));
        sent.extend(seq_pp().iter().map(|p| *p * std::f32::consts::SQRT_2));
        mapper.scr.reset();
        let trn: Vec<Cpx> = (0..4000).map(|_| mapper.trn_symbol()).collect();
        sent.extend(trn.iter().copied());
        // référence normalisée (puissance 1)
        let norm = 1.0 / 2f32.sqrt();
        let mut start_pp: Option<i64> = None;
        let mut aligner: Option<PpAligner> = None;
        let mut errors = 0;
        let mut checked = 0;
        let mut pushed = false;
        let mut y = [0.0f32];
        for n in 0..(8000 * 3) {
            if n == 300 && !pushed {
                tx.push(&sent);
                pushed = true;
            }
            let x = if n >= 300 { tx.next_sample() } else { 0.0 };
            line.process(&[x], &mut y);
            if let Some(s) = rx.push(y[0]) {
                if start_pp.is_none() {
                    if let Some(al) = aligner.as_mut() {
                        if let Some(st) = al.push(&s) {
                            let pp = seq_pp();
                            let refs: Vec<(i64, Cpx)> = (0..48).map(|i| (st + i as i64, pp[i])).collect();
                            rx.fine_timing(&refs);
                            start_pp = Some(st);
                        }
                        continue;
                    }
                    if sd.s_count == 40 {
                        rx.set_agc_from_level();
                        rx.freeze_timing();
                    }
                    if let Some(SEvent::Transition(k0)) = sd.push(&s) {
                        aligner = Some(PpAligner::new(k0 + 16));
                    }
                    continue;
                }
                let i = s.k - start_pp.unwrap();
                // Après le recalage, la mémoire de l'égaliseur se remplit
                // (16 symboles) : on entraîne à partir de la 2e période de PP.
                if i < 64 {
                    continue;
                }
                let i = i as usize;
                let r = if i < 288 { sent[144 + i] * norm } else if i - 288 < trn.len() { trn[i - 288] * norm } else { continue };
                if i < 288 + 512 {
                    rx.adapt(s.z, r);
                    rx.mu = mu_schedule((i + 240).saturating_sub(288));
                } else {
                    // décision 4 points
                    let dcs = Cpx::new(s.z.re.signum() * norm, s.z.im.signum() * norm);
                    if (dcs - r).abs() > 1e-3 {
                        errors += 1;
                    }
                    rx.mu = mu_schedule((i + 240).saturating_sub(288));
                    rx.adapt(s.z, dcs);
                    checked += 1;
                }
            }
        }
        assert!(start_pp.is_some(), "{preset} : transition S→S̄ non détectée");
        assert!(checked > 500, "{preset} : trop peu de symboles vérifiés ({checked})");
        assert_eq!(errors, 0, "{preset} : {errors} erreurs sur {checked} (SNR eq {:.1} dB)", rx.snr_db());
        eprintln!("{preset}: SNR égaliseur {:.1} dB", rx.snr_db());
        rx.snr_db()
    }

    #[test]
    fn phase3_training_perfect_3429() {
        let snr = train_over("Parfaite", 2400.0 * 10.0 / 7.0, 2400.0 * 10.0 / 7.0 * 4.0 / 7.0);
        assert!(snr > 30.0, "SNR {snr}");
    }

    #[test]
    fn phase3_training_campagne_3200() {
        train_over("Campagne", 3200.0, 3200.0 * 4.0 / 7.0);
    }

    #[test]
    fn phase3_training_longue_distance_3200() {
        train_over("Longue distance", 3200.0, 3200.0 * 4.0 / 7.0);
    }
}

#[cfg(test)]
mod eq_tests {
    use super::*;
    use crate::train::{QamTx, TrainMapper};

    /// Quel que soit le décalage de rythme initial, le recalage fin par
    /// corrélation + l'égaliseur doivent atteindre > 40 dB sur ligne parfaite.
    #[test]
    fn equalizer_reaches_40db_any_phase() {
        let (sr, fc) = (2400.0 * 10.0 / 7.0, 2400.0 * 10.0 / 7.0 * 4.0 / 7.0);
        for start in [0usize, 1, 300] {
            let mut tx = QamTx::new(sr, fc);
            let mut rx = QamRx::new(sr, fc);
            let mut m = TrainMapper::new(false);
            let trn: Vec<Cpx> = (0..4000).map(|_| m.trn_symbol()).collect();
            tx.push(&trn);
            rx.agc = 1.0 / (crate::api::NOMINAL_RMS * 2f32.sqrt());
            let norm = 1.0 / 2f32.sqrt();
            let sps = 8000.0 / sr;
            let shift = ((start as f64) / sps).round() as i64;
            let mut cnt = 0;
            let mut refs = Vec::new();
            let mut tuned = false;
            for n in 0..(start + 11000) {
                let x = if n >= start { tx.next_sample() } else { 0.0 };
                if let Some(s) = rx.push(x) {
                    let j = s.k - 16 - shift;
                    if (20..68).contains(&j) {
                        refs.push((s.k, trn[j as usize] * norm));
                    }
                    if j == 72 && !tuned {
                        rx.fine_timing(&refs);
                        tuned = true;
                    }
                    if j < 100 || j as usize >= trn.len() {
                        continue;
                    }
                    rx.mu = mu_schedule(cnt);
                    rx.adapt(s.z, trn[j as usize] * norm);
                    cnt += 1;
                }
            }
            assert!(rx.snr_db() > 40.0, "départ {start} : SNR {:.1} dB", rx.snr_db());
        }
    }
}
