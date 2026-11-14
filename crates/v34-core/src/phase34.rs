//! Phases 3 et 4 (§11.3, §11.4), puis passage en mode données.
//!
//! Phase 3 : chaque modem envoie S, S̄, PP, TRN puis J ; l'autre détecte
//! la transition S→S̄, recale son rythme et sa phase sur PP, entraîne son
//! égaliseur sur PP + TRN, puis lit J (taille de constellation demandée).
//! Phase 4 : TRN final (4 ou 16 points), échange des paramètres MP/MP′,
//! séquence E, puis B1 et données.

use std::collections::VecDeque;

use crate::api::*;
use crate::datamode::{max_rate_for_snr, DataDecoder, DataEncoder, DataParams};
use crate::info::{Info1a, Info1c};
use crate::lapm::Lapm;
use crate::modem::{Modem, St, MS};
use crate::tx::{ToneTx, TxMode};
use crate::probe::{carrier_hz, MAX_RATE_FOR_SR, SYMBOL_RATES};
use crate::qamrx::{mu_schedule, PpAligner, QamRx, RxSym, SDetector, SEvent};
use crate::dsp::Cpx;
use crate::train::{j_pattern, j_prime_pattern, j_symbols, quarter_point, rate_mask_upto, seq_pp, seq_s, Mp, QamTx, Scrambler, TrainMapper};

/// Phases 3 et 4 du V.90 (56k) : voir `phase34/v90.rs`.
pub(crate) mod v90;

/// Actions demandées par les phases 3/4 à la machine principale.
pub(crate) enum P34Action {
    Phase(Phase),
    Tx(String),
    Rx(String),
    Log(LogLevel, String),
    Params(Box<dyn FnOnce(&mut LinkParams) + Send>),
    Connected { tx_rate: u32, rx_rate: u32 },
    Data(Vec<u8>),
    Fail(String),
    /// Raccrocher (fin de cleardown)
    Hangup(String),
    /// Demande un réentraînement complet (retour en phase 2)
    Retrain(String),
    /// Le 56k est impossible sur cette ligne : réentraînement en V.34
    NoV90(String),
}

/// Écart à la capacité toléré en mode données (dB).
const DATA_GAP_DB: f32 = 9.0;
/// Longueur de TRN émise en phase 3 (symboles) — ≥ 512T exigés.
const TRN3_LEN: usize = 1536;
/// Longueur de TRN émise en phase 4 (symboles).
const TRN4_LEN: usize = 768;
/// Longueur de TRN émise en renégociation de débit (symboles, 4 points).
const TRN_RN_LEN: usize = 720;
/// Symboles de TRN utilisés avec référence connue en renégociation.
const TRN_RN_TRAIN: usize = 480;
/// Période de surveillance de la qualité en mode données (échantillons).
const MON_PERIOD: u64 = 4000;

#[derive(Clone, Debug, PartialEq)]
enum TxSt {
    Silence,
    /// Suite finie de symboles (S, S̄, PP, J′…) puis état suivant
    Burst,
    Trn { left: usize },
    J { sixteen: bool },
    Mp,
    E,
    Data,
    /// V.90, modem analogique : S répété jusqu'à nouvel ordre
    SForever,
    /// V.90, modem analogique : séquence de bits répétée (Ja, CPt, CP)
    V90Seq,
    /// V.90, modem analogique : SCR (uns brouillés) pendant `left` symboles
    V90Scr { left: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RxSt {
    /// Attente de S puis de la transition S→S̄
    WaitS,
    /// Recalage sur la première période de PP
    Align,
    /// Entraînement sur PP + TRN (références connues)
    Train,
    /// Lecture de bits (J, J′, MP, E) avec décisions
    Bits,
    /// Renégociation : recalage puis entraînement sur le TRN connu de l'autre
    RnAlign,
    RnTrain,
    Data,
}

/// Étapes de la négociation, vues de ce modem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    // appelé
    A3Send,
    A3WaitCallS,
    A3RxJ,
    // appelant
    C3WaitS,
    C3Send,
    C3WaitAnsS,
    // commun phase 4
    P4Trn,
    P4Mp,
    Connected,
    /// Renégociation / raccrochage propre : S, S̄ émis, attente de ceux de l'autre
    RnWaitS,
}

pub(crate) struct P34 {
    answer: bool,
    actions: Vec<P34Action>,
    tx: QamTx,
    mapper: TrainMapper,
    burst: VecDeque<Cpx>,
    after_burst: Option<TxSt>,
    tx_st: TxSt,
    /// TRN de l'autre modem en phase 3 (référence d'entraînement)
    remote_trn: Vec<Cpx>,
    /// Indice de symbole à partir duquel l'autre émet en 16 points
    rx16_from: Option<i64>,
    rx: QamRx,
    rx_st: RxSt,
    sdet: SDetector,
    aligner: Option<PpAligner>,
    pp_start: i64,
    trained: usize,
    /// Taille de constellation de l'autre en phase 4 (= notre demande J)
    rx16: bool,
    /// Notre taille en phase 4 (= demande J de l'autre)
    tx16: bool,
    // bits reçus
    descr: Scrambler,
    prev_z: u32,
    bits: VecDeque<bool>,
    ones_run: u32,
    step: Step,
    t_step: u64,
    n: u64,
    // MP
    our_mp: Mp,
    mp_bits: Vec<bool>,
    mp_pos: usize,
    mp_rx: Option<Mp>,
    mp_ack_rx: bool,
    sent_mp_ack: bool,
    e_rx: bool,
    sym_rate_tx: f64,
    /// Points de constellation récents (affichage)
    points: VecDeque<(f32, f32)>,
    rx_trn_count: usize,
    last_k: i64,
    /// Attente avant d'entraîner l'annuleur d'écho en phase 3 (échantillons)
    ec_wait: u64,
    /// Demande d'afficher l'atténuation d'écho obtenue
    pub ec_report: bool,
    pub tx_rate: u32,
    pub rx_rate: u32,
    sr_tx_idx: usize,
    sr_rx_idx: usize,
    /// Codeur / décodeur du mode données (créés après E)
    enc: Option<DataEncoder>,
    dec: Option<DataDecoder>,
    /// Couche liaison fiable (inspirée de V.42 / LAPM) sur le tuyau de bits
    link: Lapm,
    mp_refined: bool,
    // Renégociation, raccrochage propre, surveillance de la qualité
    renegotiating: bool,
    cleardown: bool,
    last_reneg: u64,
    recent_renegs: u32,
    /// Montée de débit tentée depuis ce débit (pour savoir si elle a servi)
    up_from: Option<u32>,
    /// Montées successives qui n'ont rien donné : délai doublé à chaque fois
    up_fails: u32,
    rn_cap: Option<u32>,
    mon_t: u64,
    mon_err: u64,
    mon_4d: u64,
    mon_fcs: u64,
    mon_frames: u64,
    bad_windows: u32,
    good_windows: u32,
    /// Erreur de décision moyenne en mode données, sur les symboles où il
    /// y a du signal (les trous ne comptent pas) : sert à distinguer une
    /// ligne devenue bruyante (renégocier) de trous isolés (LAPM suffit).
    dec_mse: f32,
    /// Instant de fin du raccrochage propre
    hangup_at: Option<u64>,
    /// Mode V.90 (56k) : émetteur ou récepteur PCM et leur pilotage
    pub(crate) v90: Option<Box<v90::V90>>,
    /// Longueur (symboles) du dernier S avant une transition S→S̄
    s_run: u32,
}

impl P34 {
    fn new(answer: bool, sr_tx: usize, tx_high: bool, sr_rx: usize, rx_high: bool, our_mp: Mp) -> Self {
        let st = SYMBOL_RATES[sr_tx];
        let srr = SYMBOL_RATES[sr_rx];
        Self {
            answer,
            actions: Vec::new(),
            tx: QamTx::new(st, carrier_hz(sr_tx, tx_high)),
            // Brouilleur d'émission : polynôme de l'appelant si on appelle
            mapper: TrainMapper::new(!answer),
            burst: VecDeque::new(),
            after_burst: None,
            tx_st: TxSt::Silence,
            remote_trn: {
                // L'autre brouille avec SON polynôme (appelant si on est l'appelé)
                let mut m = TrainMapper::new(answer);
                (0..TRN3_LEN + 64).map(|_| m.trn_symbol()).collect()
            },
            rx16_from: None,
            rx: QamRx::new(srr, carrier_hz(sr_rx, rx_high)),
            rx_st: RxSt::WaitS,
            sdet: SDetector::new(),
            aligner: None,
            pp_start: 0,
            trained: 0,
            rx16: false,
            tx16: false,
            // On désembrouille ce que l'autre a brouillé avec SON polynôme
            descr: Scrambler::new(answer),
            prev_z: 0,
            bits: VecDeque::new(),
            ones_run: 0,
            step: if answer { Step::A3Send } else { Step::C3WaitS },
            t_step: 0,
            n: 0,
            our_mp,
            mp_bits: Vec::new(),
            mp_pos: 0,
            mp_rx: None,
            mp_ack_rx: false,
            sent_mp_ack: false,
            e_rx: false,
            sym_rate_tx: st,
            points: VecDeque::new(),
            rx_trn_count: 0,
            last_k: 0,
            ec_wait: 0,
            ec_report: false,
            tx_rate: 0,
            rx_rate: 0,
            sr_tx_idx: sr_tx,
            sr_rx_idx: sr_rx,
            enc: None,
            dec: None,
            link: Lapm::new(!answer),
            mp_refined: false,
            renegotiating: false,
            cleardown: false,
            last_reneg: 0,
            recent_renegs: 0,
            up_from: None,
            up_fails: 0,
            rn_cap: None,
            mon_t: 0,
            mon_err: 0,
            mon_4d: 0,
            mon_fcs: 0,
            mon_frames: 0,
            bad_windows: 0,
            good_windows: 0,
            dec_mse: 0.0,
            hangup_at: None,
            v90: None,
            s_run: 0,
        }
    }

    fn act(&mut self, a: P34Action) {
        self.actions.push(a);
    }

    fn log(&mut self, msg: impl Into<String>) {
        self.act(P34Action::Log(LogLevel::Info, msg.into()));
    }

    fn detail(&mut self, msg: impl Into<String>) {
        self.act(P34Action::Log(LogLevel::Detail, msg.into()));
    }

    fn goto(&mut self, s: Step) {
        self.step = s;
        self.t_step = self.n;
    }

    fn ms_since_step(&self) -> u64 {
        (self.n - self.t_step) / 8
    }

    // ------------------------------------------------------------------
    // Émission
    // ------------------------------------------------------------------

    fn send_burst(&mut self, syms: Vec<Cpx>, then: TxSt, name: &str) {
        self.burst.extend(syms);
        self.after_burst = Some(then);
        self.tx_st = TxSt::Burst;
        self.act(P34Action::Tx(name.to_string()));
    }

    fn silence_syms(&self, ms: f64) -> Vec<Cpx> {
        vec![Cpx::ZERO; (ms / 1000.0 * self.sym_rate_tx).round() as usize]
    }

    /// Envoie S (128T), S̄ (16T) puis `then`.
    fn send_s_sbar(&mut self, prefix: Vec<Cpx>, then: TxSt) {
        let mut v = prefix;
        v.extend(seq_s(128, false));
        v.extend(seq_s(16, true));
        self.send_burst(v, then, "S S̄");
    }

    fn start_trn(&mut self, len: usize, sixteen: bool, name: &str) {
        self.mapper.scr.reset();
        self.mapper.sixteen = sixteen;
        self.tx.set_constellation_power(if sixteen { 10.0 } else { 2.0 });
        self.tx_st = TxSt::Trn { left: len };
        self.act(P34Action::Tx(name.to_string()));
    }

    fn start_mp(&mut self) {
        if !self.mp_refined {
            // Notre capacité de RÉCEPTION d'après le SNR réellement mesuré par
            // l'égaliseur (plus fiable que la projection du sondage).
            self.mp_refined = true;
            let snr = self.rx.snr_db();
            let cap = max_rate_for_snr(self.sr_rx_idx, snr, 1.5).max(if self.sr_rx_idx == 0 { 2400 } else { 4800 });
            let cap = cap.min(self.rn_cap.unwrap_or(u32::MAX));
            let n = (cap / 2400) as u8;
            let field = if self.answer { &mut self.our_mp.rate_c2a } else { &mut self.our_mp.rate_a2c };
            if n < *field {
                *field = n;
            }
            let f = *field as u32 * 2400;
            self.detail(format!("Débit max en réception d'après le SNR mesuré ({snr:.1} dB) : {f} bit/s"));
        }
        self.mp_bits = self.our_mp.to_bits();
        self.mp_pos = 0;
        self.tx_st = TxSt::Mp;
        self.act(P34Action::Tx(if self.our_mp.ack { "MP′".into() } else { "MP".into() }));
    }

    fn fill_tx(&mut self) {
        while self.tx.queued() < 4 {
            match self.tx_st.clone() {
                TxSt::Silence => self.tx.push(&[Cpx::ZERO]),
                TxSt::Burst => match self.burst.pop_front() {
                    Some(p) => self.tx.push(&[p]),
                    None => {
                        let next = self.after_burst.take().unwrap_or(TxSt::Silence);
                        match next {
                            TxSt::Trn { left } => {
                                let six = self.mapper.sixteen;
                                self.start_trn(left, six, "TRN");
                            }
                            TxSt::Mp => self.start_mp(),
                            other => self.tx_st = other,
                        }
                    }
                },
                TxSt::Trn { left } => {
                    let p = self.mapper.trn_symbol();
                    self.tx.push(&[p]);
                    if left <= 1 {
                        self.on_trn_sent();
                    } else {
                        self.tx_st = TxSt::Trn { left: left - 1 };
                    }
                }
                TxSt::J { sixteen } => {
                    // J est émis par motifs complets de 16 bits (8 symboles)
                    let syms = j_symbols(&mut self.mapper, &j_pattern(sixteen));
                    self.tx.push(&syms);
                }
                TxSt::Mp => {
                    let bps = self.mapper.bits_per_symbol();
                    let mut chunk = Vec::with_capacity(bps);
                    for _ in 0..bps {
                        chunk.push(self.mp_bits[self.mp_pos]);
                        self.mp_pos += 1;
                        if self.mp_pos == self.mp_bits.len() {
                            self.mp_pos = 0;
                            self.on_mp_sent();
                            if self.tx_st != TxSt::Mp {
                                // On complète le symbole avec les premiers bits de E
                                break;
                            }
                        }
                    }
                    while chunk.len() < bps {
                        chunk.push(true);
                    }
                    let p = self.mapper.diff_symbol(&chunk);
                    self.tx.push(&[p]);
                }
                TxSt::SForever => {
                    let v = seq_s(2, false);
                    self.tx.push(&v);
                }
                TxSt::V90Seq => self.v90_fill_seq(),
                TxSt::V90Scr { left } => {
                    let p = self.mapper.diff_symbol(&[true, true]);
                    self.tx.push(&[p]);
                    if left <= 1 {
                        self.v90_scr_done();
                    } else {
                        self.tx_st = TxSt::V90Scr { left: left - 1 };
                    }
                }
                TxSt::E => {
                    // 20 uns
                    let bps = self.mapper.bits_per_symbol();
                    let n = 20usize.div_ceil(bps);
                    let ones = vec![true; bps];
                    for _ in 0..n {
                        let p = self.mapper.diff_symbol(&ones);
                        self.tx.push(&[p]);
                    }
                    // B1 puis données, avec les paramètres négociés
                    let (tx_rate, _) = match &self.v90 {
                        Some(v) => (v.up_rate, 0),
                        None => self.negotiated_rates(),
                    };
                    let prm = DataParams::new(self.sr_tx_idx, tx_rate, false);
                    let enc = DataEncoder::new(prm.clone(), !self.answer);
                    self.tx.set_constellation_power(enc.power);
                    self.link.set_t401_bits((tx_rate as u64 * 6 / 10).max(2048));
                    self.detail(format!("Émission en mode données : {tx_rate} bit/s, constellation {} points, {} bits/trame", prm.l, prm.b));
                    self.enc = Some(enc);
                    self.tx_st = TxSt::Data;
                    self.act(P34Action::Tx("B1 / données".into()));
                }
                TxSt::Data => {
                    // Une trame de mise en correspondance (8 symboles) : les
                    // bits viennent de la couche liaison (fanions au repos).
                    let link = &mut self.link;
                    let enc = self.enc.as_mut().expect("codeur de données");
                    let pts = enc.frame(&mut || link.next_tx_bit());
                    let syms: Vec<Cpx> = pts.iter().map(|&(x, y)| Cpx::new(x as f32, y as f32)).collect();
                    self.tx.push(&syms);
                }
            }
        }
    }

    fn on_trn_sent(&mut self) {
        if matches!(self.step, Step::A3Send | Step::C3Send) {
            self.ec_report = true;
        }
        if self.v90.is_some() {
            self.v90_on_trn_sent();
            return;
        }
        match self.step {
            Step::A3Send => {
                let want16 = self.rx16;
                self.tx_st = TxSt::J { sixteen: want16 };
                self.act(P34Action::Tx(format!("J ({} points)", if want16 { 16 } else { 4 })));
                self.goto(Step::A3WaitCallS);
                self.rx_st = RxSt::WaitS;
                self.sdet = SDetector::new();
            }
            Step::C3Send => {
                let want16 = self.rx16;
                self.tx_st = TxSt::J { sixteen: want16 };
                self.act(P34Action::Tx(format!("J ({} points)", if want16 { 16 } else { 4 })));
                self.goto(Step::C3WaitAnsS);
                self.rx_st = RxSt::WaitS;
                self.sdet = SDetector::new();
            }
            Step::P4Trn | Step::P4Mp => {
                self.start_mp();
                if self.step == Step::P4Trn {
                    self.goto(Step::P4Mp);
                }
            }
            // Renégociation : l'autre n'a pas encore répondu, on prolonge TRN
            // (sans remettre le brouilleur à zéro).
            Step::RnWaitS => self.tx_st = TxSt::Trn { left: 128 },
            _ => self.tx_st = TxSt::Silence,
        }
    }

    fn on_mp_sent(&mut self) {
        // Fin d'une séquence MP : passer à MP′ si on a reçu la MP distante,
        // puis à E quand les deux acquittements sont faits.
        if self.our_mp.ack {
            self.sent_mp_ack = true;
        }
        if self.cleardown && self.sent_mp_ack && self.mp_ack_rx {
            // §11.7 : MP′ émis et reçus avec des débits nuls → on coupe, après
            // avoir laissé partir un MP′ de plus (sinon les derniers symboles,
            // encore dans le filtre d'émission, n'arriveraient jamais).
            if self.hangup_at.is_none() {
                let syms = (self.mp_bits.len() / self.mapper.bits_per_symbol() + 24) as f64;
                self.hangup_at = Some(self.n + (syms / self.sym_rate_tx * 8000.0) as u64);
            }
            return;
        }
        if self.mp_rx.is_some() && !self.our_mp.ack {
            self.our_mp.ack = true;
            self.mp_bits = self.our_mp.to_bits();
            self.act(P34Action::Tx("MP′".into()));
            return;
        }
        if self.sent_mp_ack && (self.mp_ack_rx || self.e_rx) && !self.cleardown {
            self.tx_st = TxSt::E;
            self.act(P34Action::Tx("E".into()));
        }
    }

    // ------------------------------------------------------------------
    // Réception
    // ------------------------------------------------------------------

    fn on_rx_symbol(&mut self, s: RxSym) {
        self.last_k = s.k;
        if matches!(self.rx_st, RxSt::Train | RxSt::Bits | RxSt::Data) {
            self.points.push_back((s.z.re, s.z.im));
            if self.points.len() > 256 {
                self.points.pop_front();
            }
        }
        match self.rx_st {
            RxSt::WaitS => {
                if self.sdet.s_count == 24 {
                    self.act(P34Action::Rx("S".into()));
                    if self.v90.is_some() {
                        self.v90_on_s_present();
                    }
                }
                let run = self.sdet.s_count;
                if let Some(SEvent::Transition(k0)) = self.sdet.push(&s) {
                    self.s_run = run;
                    self.act(P34Action::Rx("S̄".into()));
                    self.on_s_transition(k0);
                }
            }
            RxSt::Align => {
                if let Some(al) = self.aligner.as_mut() {
                    if let Some(st) = al.push(&s) {
                        let pp = seq_pp();
                        let refs: Vec<(i64, Cpx)> = (0..48).map(|i| (st + i as i64, pp[i])).collect();
                        let tau = self.rx.fine_timing(&refs);
                        self.pp_start = st;
                        self.trained = 0;
                        self.rx_st = RxSt::Train;
                        self.rx.start_ls();
                        self.act(P34Action::Rx("PP".into()));
                        self.detail(format!("PP recalé (rythme {tau:+.2} T) — entraînement de l'égaliseur"));
                    }
                }
            }
            RxSt::Train => {
                let i = s.k - self.pp_start;
                if i < 64 {
                    return;
                }
                let i = i as usize;
                let r = if i < 288 {
                    seq_pp()[i]
                } else {
                    // TRN de l'autre : uns brouillés par SON polynôme, 4 points
                    let j = i - 288;
                    if j == 0 {
                        self.act(P34Action::Rx("TRN".into()));
                    }
                    let p = self.remote_trn_symbol(j);
                    p * (1.0 / 2f32.sqrt())
                };
                self.rx.mu = mu_schedule(self.trained);
                self.rx.adapt(s.z, r);
                self.trained += 1;
                if i >= 288 + 512 {
                    // Coefficients optimaux sur tout l'apprentissage (le LMS
                    // n'a fait que dégrossir) ; on saute les 160 premiers
                    // symboles, le temps que la boucle de phase s'accroche.
                    self.rx.finish_ls(160);
                    // Fin de l'entraînement supervisé : on lit les bits (J)
                    self.prev_z = quadrant_of(r);
                    self.rx_st = RxSt::Bits;
                    self.descr = Scrambler::new(self.answer);
                    self.bits.clear();
                    let snr = self.rx.snr_db();
                    self.detail(format!("Égaliseur entraîné : SNR {snr:.1} dB"));
                    if !self.answer {
                        // L'appelant émet J après son entraînement : il peut
                        // choisir la taille d'après le SNR réellement mesuré
                        // (16 points demandent ≈ 20 dB ; on garde 4 dB de marge).
                        self.rx16 = snr >= 24.0;
                    }
                    self.act(P34Action::Params(Box::new(move |p: &mut LinkParams| p.snr_db = Some(snr))));
                }
            }
            RxSt::Bits => self.rx_bits(s),
            RxSt::RnAlign => {
                if let Some(al) = self.aligner.as_mut() {
                    if let Some(st) = al.push(&s) {
                        if al.score() < 0.3 {
                            // Pas de TRN après S̄ (raccrochage propre : MP
                            // directement) : on garde l'égaliseur et on lit les MP.
                            self.prev_z = decide(s.z, false).rot;
                            self.rx_st = RxSt::Bits;
                            self.descr = Scrambler::new(self.answer);
                            self.bits.clear();
                            return;
                        }
                        // Recalage fin + réinitialisation de l'égaliseur sur le
                        // TRN connu : sauve un égaliseur qui aurait divergé.
                        let n = 48.min(self.remote_trn.len());
                        let refs: Vec<(i64, Cpx)> = (0..n).map(|i| (st + i as i64, self.remote_trn[i] * (1.0 / 2f32.sqrt()))).collect();
                        self.rx.fine_timing(&refs);
                        self.pp_start = st;
                        self.trained = 0;
                        self.rx_st = RxSt::RnTrain;
                        self.act(P34Action::Rx("TRN".into()));
                    }
                }
            }
            RxSt::RnTrain => {
                let j = s.k - self.pp_start;
                if j < 64 {
                    return;
                }
                let j = j as usize;
                let r = self.remote_trn_symbol(j) * (1.0 / 2f32.sqrt());
                self.rx.mu = mu_schedule(self.trained);
                self.rx.adapt(s.z, r);
                self.trained += 1;
                if j >= TRN_RN_TRAIN {
                    self.prev_z = quadrant_of(r);
                    self.rx_st = RxSt::Bits;
                    self.descr = Scrambler::new(self.answer);
                    self.bits.clear();
                    let snr = self.rx.snr_db();
                    self.detail(format!("Égaliseur réentraîné sur TRN : SNR {snr:.1} dB"));
                    self.act(P34Action::Params(Box::new(move |p: &mut LinkParams| p.snr_db = Some(snr))));
                }
            }
            RxSt::Data => {
                // Un S de l'autre pendant les données = renégociation ou
                // raccrochage propre demandé par lui (§11.6.1.2, §11.7.2).
                if self.v90.is_some() && self.sdet.s_count == 24 {
                    // V.90 : la renégociation n'est pas encore gérée
                    self.act(P34Action::Retrain("l'abonné renégocie (V.90) : réentraînement".into()));
                    return;
                }
                if let Some(SEvent::Transition(k0)) = self.sdet.push(&s) {
                    if !self.renegotiating {
                        self.start_reneg(false, None, false);
                    }
                    self.on_s_transition(k0);
                    return;
                }
                if self.sdet.s_count == 24 && !self.renegotiating {
                    self.act(P34Action::Rx("S".into()));
                    self.log("S reçu en mode données : l'autre modem renégocie");
                    self.start_reneg(false, None, false);
                    return;
                }
                let Some(dec) = self.dec.as_mut() else { return };
                let (d, bits) = dec.push(s.z);
                if self.rx.signal_ok() {
                    // Erreur plafonnée à la frontière de décision : un
                    // parasite isolé (choc, clic) ne fait pas croire à une
                    // ligne bruyante.
                    let e2 = (s.z - d).norm_sqr().min(dec.scale * dec.scale);
                    self.dec_mse = 0.998 * self.dec_mse + 0.002 * e2;
                }
                // On n'adapte l'égaliseur que sur des décisions vraisemblables :
                // sous un bruit très fort, adapter sur des décisions fausses le
                // ferait dériver (et il resterait faux une fois le bruit parti).
                let tol = 0.7 * dec.scale;
                if (s.z - d).norm_sqr() < tol * tol {
                    self.rx.mu = 0.01;
                    self.rx.adapt(s.z, d);
                }
                if let Some(bits) = bits {
                    for b in bits {
                        self.link.rx_bit(b);
                    }
                    let got = self.link.take_received();
                    if !got.is_empty() {
                        self.act(P34Action::Data(got));
                    }
                }
            }
        }
    }

    /// Symbole TRN de l'autre modem en phase 3 (4 points, brouilleur remis
    /// à zéro au début de TRN).
    fn remote_trn_symbol(&mut self, j: usize) -> Cpx {
        self.remote_trn[j.min(self.remote_trn.len() - 1)]
    }

    fn on_s_transition(&mut self, k0: i64) {
        if self.v90.is_some() && self.v90_on_s_transition(k0) {
            return;
        }
        match self.step {
            Step::C3WaitS | Step::A3WaitCallS => {
                // Phase 3 : S̄ (16T) puis PP
                self.aligner = Some(PpAligner::new(k0 + 16));
                self.rx_st = RxSt::Align;
                if self.step == Step::A3WaitCallS {
                    // §11.3.1.2.4 : l'appelé se tait dès la transition détectée
                    self.tx_st = TxSt::Silence;
                    self.act(P34Action::Tx("silence".into()));
                    self.goto(Step::A3RxJ);
                }
            }
            Step::RnWaitS => {
                self.act(P34Action::Rx("S̄".into()));
                if self.cleardown {
                    // Raccrochage propre : MP directement après S̄
                    self.rx_st = RxSt::Bits;
                    self.descr = Scrambler::new(self.answer);
                    self.bits.clear();
                } else {
                    // TRN de l'autre après S̄ (16T) : recalage puis entraînement
                    self.aligner = Some(PpAligner::with_reference(k0 + 16, &self.remote_trn.iter().map(|p| *p * (1.0 / 2f32.sqrt())).collect::<Vec<_>>()));
                    self.rx_st = RxSt::RnAlign;
                }
                self.goto(Step::P4Trn);
            }
            Step::C3WaitAnsS => {
                // Début de la phase 4 côté appelant : J′ puis TRN
                self.act(P34Action::Phase(Phase::Phase4));
                self.log("Phase 4 — S/S̄ de l'appelé reçus : envoi J′ puis TRN final");
                let syms = j_symbols(&mut self.mapper, &j_prime_pattern());
                let six = self.tx16;
                self.mapper.sixteen = six;
                self.send_burst(syms, TxSt::Trn { left: TRN4_LEN }, "J′");
                self.goto(Step::P4Trn);
                // On reçoit maintenant le TRN de l'appelé (après S̄, 16T),
                // dans la taille que nous avons demandée par J.
                if self.rx16 {
                    self.rx16_from = Some(k0 + 16);
                }
                self.rx_st = RxSt::Bits;
                self.rx_trn_count = 0;
                self.bits.clear();
            }
            _ => {}
        }
    }

    fn rx_bits(&mut self, s: RxSym) {
        let sixteen = matches!(self.rx16_from, Some(k) if s.k >= k);
        let d = decide(s.z, sixteen);
        self.rx.mu = 0.03;
        self.rx.adapt(s.z, d.point);
        let i = (d.rot + 4 - self.prev_z) & 3;
        self.prev_z = d.rot;
        let mut out = vec![self.descr.descramble(i & 1 == 1), self.descr.descramble(i & 2 == 2)];
        if sixteen {
            out.push(self.descr.descramble(d.label & 1 == 1));
            out.push(self.descr.descramble(d.label & 2 == 2));
        }
        for b in out {
            self.on_rx_bit(b);
        }
    }

    fn on_rx_bit(&mut self, b: bool) {
        if self.v90.is_some() {
            self.v90_on_bit(b);
            return;
        }
        self.bits.push_back(b);
        if self.bits.len() > 200 {
            self.bits.pop_front();
        }
        self.ones_run = if b { self.ones_run + 1 } else { 0 };
        match self.step {
            Step::A3RxJ | Step::C3WaitS | Step::A3WaitCallS => {
                if let Some(sixteen) = detect_j(&self.bits) {
                    self.on_j(sixteen);
                }
            }
            Step::P4Trn | Step::P4Mp => {
                if self.answer && self.rx16_from.is_none() && self.rx16 && self.bits.len() >= 16 {
                    let n = self.bits.len();
                    if self.bits.range(n - 16..).copied().eq(j_prime_pattern().iter().copied()) {
                        // J′ reçu : le TRN de l'appelant commence au symbole suivant
                        self.act(P34Action::Rx("J′".into()));
                        self.rx16_from = Some(self.last_k + 1);
                    }
                }
                if self.mp_rx.is_none() || !self.mp_ack_rx {
                    if let Some(mp) = detect_mp(&self.bits) {
                        self.on_mp(mp);
                    }
                }
                if self.mp_rx.is_some() && self.ones_run >= 20 && !self.e_rx && !self.cleardown {
                    self.e_rx = true;
                    self.act(P34Action::Rx("E".into()));
                    self.check_connected();
                }
            }
            _ => {}
        }
    }

    fn on_j(&mut self, sixteen: bool) {
        self.act(P34Action::Rx(format!("J ({} points)", if sixteen { 16 } else { 4 })));
        self.tx16 = sixteen;
        match self.step {
            Step::C3WaitS => {
                // L'appelant a fini d'entraîner son récepteur : à lui d'émettre
                self.log(format!("J reçu : l'appelé demande {} points en phase 4 — envoi S, S̄, PP, TRN", if sixteen { 16 } else { 4 }));
                let mut v = Vec::new();
                v.extend(seq_s(128, false));
                v.extend(seq_s(16, true));
                v.extend(seq_pp().iter().map(|p| *p * std::f32::consts::SQRT_2));
                self.mapper.sixteen = false;
                self.tx.set_constellation_power(2.0);
                // Notre annuleur d'écho n'apprend qu'une fois le J de l'appelé
                // éteint (S + S̄ + un aller-retour + 60 ms, `ec_wait`). Avec
                // un long aller-retour (Internet, ~0,5 s), un TRN de durée fixe
                // finissait presque aussitôt : 30 ms d'apprentissage, 2 dB
                // d'atténuation, et notre propre écho brouillait la phase 4.
                // Le TRN dure donc au moins jusqu'à `ec_wait` + 400 ms (l'autre
                // attend J pour en voir la fin : V.34 le permet).
                let deja = v.len() as f64;
                let besoin = ((self.ec_wait as f64 / 8000.0) + 0.4) * self.sym_rate_tx - deja;
                let trn = TRN3_LEN.max(besoin.ceil().max(0.0) as usize);
                self.send_burst(v, TxSt::Trn { left: trn }, "S S̄ PP");
                self.goto(Step::C3Send);
                self.rx_st = RxSt::WaitS;
            }
            Step::A3RxJ => {
                // Phase 4 côté appelé : S 128T, S̄ 16T, TRN
                self.act(P34Action::Phase(Phase::Phase4));
                self.log(format!("Phase 4 — J reçu (l'appelant demande {} points) : envoi S, S̄, TRN final", if sixteen { 16 } else { 4 }));
                self.mapper.sixteen = sixteen;
                self.send_s_sbar(Vec::new(), TxSt::Trn { left: TRN4_LEN });
                self.goto(Step::P4Trn);
                self.bits.clear();
            }
            _ => {}
        }
    }

    fn on_mp(&mut self, mp: Mp) {
        if mp.rate_c2a == 0 && mp.rate_a2c == 0 && !self.cleardown {
            // §11.7.2 : l'autre demande des débits nuls = raccrochage propre
            self.cleardown = true;
            self.log("L'autre modem raccroche (MP à débit nul) : raccrochage propre");
        }
        if self.mp_rx.is_none() {
            self.act(P34Action::Rx(if mp.ack { "MP′".into() } else { "MP".into() }));
            self.detail(format!(
                "MP reçue : max appelant→appelé {} bit/s, appelé→appelant {} bit/s, treillis {} états",
                mp.rate_c2a as u32 * 2400,
                mp.rate_a2c as u32 * 2400,
                [16, 32, 64, 0][mp.trellis as usize & 3]
            ));
        }
        if mp.ack && !self.mp_ack_rx {
            self.mp_ack_rx = true;
            if self.mp_rx.is_some() {
                self.act(P34Action::Rx("MP′".into()));
            }
        }
        self.mp_rx = Some(mp);
    }

    /// Débits finaux (§11.4.1.1.3) : max activé chez les deux ≤ valeurs des
    /// deux MP, ramené dans la plage valide pour la vitesse de symbole.
    fn negotiated_rates(&self) -> (u32, u32) {
        let Some(remote) = self.mp_rx.clone() else { return (0, 0) };
        let mask = self.our_mp.rate_mask & remote.rate_mask;
        let best = |limit: u8| -> u32 {
            let mut r = 0;
            for n in 1..=limit.min(14) {
                if mask & (1 << (n - 1)) != 0 {
                    r = n as u32 * 2400;
                }
            }
            r
        };
        let asym = self.our_mp.asym && remote.asym;
        let c2a = remote.rate_c2a.min(self.our_mp.rate_c2a);
        let a2c = remote.rate_a2c.min(self.our_mp.rate_a2c);
        let (c2a, a2c) = if asym { (best(c2a), best(a2c)) } else {
            let r = best(c2a.min(a2c));
            (r, r)
        };
        let (tx, rx) = if self.answer { (a2c, c2a) } else { (c2a, a2c) };
        let fix = |sr: usize, r: u32| -> u32 {
            let min = if sr == 0 { 2400 } else { 4800 };
            r.clamp(min, crate::probe::MAX_RATE_FOR_SR[sr])
        };
        (fix(self.sr_tx_idx, tx), fix(self.sr_rx_idx, rx))
    }

    fn check_connected(&mut self) {
        if self.step == Step::Connected || self.mp_rx.is_none() || !self.e_rx {
            return;
        }
        let (tx, rx) = self.negotiated_rates();
        self.tx_rate = tx;
        self.rx_rate = rx;
        // Le symbole qui suit E est le premier de B1 : décodeur prêt.
        let prm = DataParams::new(self.sr_rx_idx, rx, false);
        self.dec = Some(DataDecoder::new(prm, self.answer));
        self.goto(Step::Connected);
        self.rx_st = RxSt::Data;
        self.sdet = SDetector::new();
        self.act(P34Action::Phase(Phase::Data));
        if self.renegotiating {
            self.renegotiating = false;
            self.log(format!("Renégociation terminée — émission {tx} bit/s, réception {rx} bit/s"));
        } else {
            self.log(format!("CONNECT — émission {tx} bit/s, réception {rx} bit/s"));
        }
        self.reset_monitor();
        self.act(P34Action::Connected { tx_rate: tx, rx_rate: rx });
    }

    // ------------------------------------------------------------------
    // Interface avec la machine principale
    // ------------------------------------------------------------------

    pub fn step(&mut self, x: f32, n: u64) -> f32 {
        if self.v90.is_some() {
            return self.v90_step(x, n);
        }
        self.n = n;
        if let Some(s) = self.rx.push(x) {
            self.on_rx_symbol(s);
        }
        // Garde-fous
        let limit = match self.step {
            Step::Connected => u64::MAX,
            Step::C3WaitS | Step::A3WaitCallS | Step::C3WaitAnsS => 6000,
            Step::RnWaitS => 2500,
            _ if self.renegotiating || self.cleardown => 4000,
            _ => 8000,
        };
        if self.ms_since_step() > limit {
            if self.cleardown {
                self.cleardown = false;
                self.act(P34Action::Hangup("raccrochage (l'autre n'a pas confirmé)".into()));
            } else if self.renegotiating {
                self.act(P34Action::Retrain("la renégociation n'aboutit pas".into()));
            } else {
                let why = format!("délai dépassé en phase 3/4 ({:?})", self.step);
                self.act(P34Action::Fail(why));
            }
            self.t_step = self.n;
        }
        if self.step == Step::Connected {
            self.monitor();
        }
        if let Some(t) = self.hangup_at {
            if self.n >= t {
                self.hangup_at = None;
                self.tx_st = TxSt::Silence;
                self.log("Raccrochage propre terminé (cleardown V.34)");
                self.act(P34Action::Hangup("raccrochage propre (cleardown V.34)".into()));
            }
        }
        self.fill_tx();
        self.tx.next_sample()
    }

    /// Pas d'adaptation demandé à l'annuleur d'écho : il s'entraîne sur
    /// notre S/PP/TRN (large bande, pseudo-aléatoire) quand l'autre se tait,
    /// avec un pas fort au début de la fenêtre puis de plus en plus fin.
    pub fn ec_mu(&self) -> f32 {
        if let Some(v) = &self.v90 {
            if let Some(i) = v.trn1d_index() {
                // V.90, modem numérique : l'écho de notre PCM (hybride du
                // central) s'apprend sur TRN1d et le premier Jd, quand
                // l'abonné se tait sûrement : il a cessé Ja (un aller-retour
                // + 100 ms après le début de TRN1d) et ne peut pas encore
                // répondre au Jd (un aller-retour après sa fin).
                let fin = self.ec_wait as usize + crate::v90::pcmtx::TRN1D_LEN + crate::v90::seq::Jd::LEN - 100 * 8;
                if i >= fin {
                    return 0.0;
                }
                return match i.checked_sub(self.ec_wait as usize).map(|j| j / 8) {
                    None => 0.0,
                    Some(0..=99) => 1.0,
                    Some(100..=199) => 0.5,
                    Some(200..=399) => 0.2,
                    Some(_) => 0.05,
                };
            }
            if v.digital || v.ec_done {
                return 0.0;
            }
        }
        let since = self.n - self.t_step;
        let open_at = match self.step {
            // L'appelant se tait pendant notre S/PP/TRN : écho seul
            Step::A3Send => 70 * 8,
            // L'appelé émet encore J jusqu'à ce qu'il détecte notre S→S̄ :
            // on attend S + S̄ (144T) + délai aller-retour + 60 ms de marge.
            Step::C3Send => self.ec_wait,
            _ => return 0.0,
        };
        if since < open_at || !matches!(self.tx_st, TxSt::Burst | TxSt::Trn { .. }) {
            return 0.0;
        }
        // NLMS : μ = 1 donne la convergence la plus rapide (constante de
        // temps ≈ 256 éch. = 32 ms) ; on décroît ensuite pour réduire le
        // bruit résiduel dû au bruit de ligne.
        match (since - open_at) / 8 {
            0..=99 => 1.0,
            100..=199 => 0.5,
            200..=399 => 0.2,
            _ => 0.05,
        }
    }

    pub fn take_actions(&mut self) -> Vec<P34Action> {
        std::mem::take(&mut self.actions)
    }

    pub fn constellation(&self) -> Vec<(f32, f32)> {
        self.points.iter().copied().collect()
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.link.send(bytes);
    }

    pub fn link_stats(&self) -> crate::lapm::LapmStats {
        self.link.stats()
    }

    /// Renvoie true si un raccrochage propre (cleardown) est engagé.
    pub fn request_cleardown(&mut self) -> bool {
        if self.step != Step::Connected || self.v90.is_some() {
            return false;
        }
        self.start_reneg(true, None, true);
        true
    }

    pub fn request_renegotiation(&mut self) {
        if self.v90.is_some() {
            self.log("Renégociation 56k pas encore disponible");
            return;
        }
        if self.step == Step::Connected {
            self.log("Renégociation de débit demandée");
            self.start_reneg(true, None, false);
        }
    }

    /// Le modem doit-il surveiller un ton A/B de réentraînement venant de
    /// l'autre (§11.4.2, §11.5) ?
    pub fn retrain_watch(&self) -> bool {
        // Pendant tout P3/P4/données : un ton A/B pur qui domine = l'autre
        // a lancé un réentraînement (§11.3.2, §11.4.2, §11.5).
        true
    }

    /// Récupère la couche liaison (pour la conserver pendant un réentraînement).
    pub fn take_link(&mut self) -> Lapm {
        std::mem::replace(&mut self.link, Lapm::new(!self.answer))
    }

    /// Démarre une renégociation de débit (§11.6) ou un raccrochage propre
    /// (§11.7). `cap` : débit max imposé en réception (renégociation vers le bas).
    fn start_reneg(&mut self, initiator: bool, cap: Option<u32>, cleardown: bool) {
        self.renegotiating = !cleardown;
        self.cleardown = cleardown;
        self.act(P34Action::Phase(if cleardown { Phase::Cleardown } else { Phase::Retrain }));
        self.mp_rx = None;
        self.mp_ack_rx = false;
        self.sent_mp_ack = false;
        self.e_rx = false;
        self.our_mp.ack = false;
        self.rx16 = false;
        self.tx16 = false;
        self.rx16_from = None;
        self.mapper.sixteen = false;
        self.enc = None;
        self.rn_cap = cap;
        if cleardown {
            self.our_mp.rate_c2a = 0;
            self.our_mp.rate_a2c = 0;
            self.mp_refined = true;
        } else {
            // Notre émission : on propose le maximum, l'autre limitera selon
            // ce qu'il reçoit. Notre réception : décidée après réentraînement.
            let max_tx = (MAX_RATE_FOR_SR[self.sr_tx_idx] / 2400) as u8;
            let max_rx = (MAX_RATE_FOR_SR[self.sr_rx_idx] / 2400) as u8;
            let mask_max = (16 - self.our_mp.rate_mask.leading_zeros()) as u8;
            let (tx, rx) = (max_tx.min(mask_max), max_rx.min(mask_max));
            if self.answer {
                self.our_mp.rate_a2c = tx;
                self.our_mp.rate_c2a = rx;
            } else {
                self.our_mp.rate_c2a = tx;
                self.our_mp.rate_a2c = rx;
            }
            self.mp_refined = false;
        }
        self.tx.set_constellation_power(2.0);
        let next = if cleardown { TxSt::Mp } else { TxSt::Trn { left: TRN_RN_LEN } };
        self.send_s_sbar(Vec::new(), next);
        self.rx_st = RxSt::WaitS;
        if initiator {
            self.sdet = SDetector::new();
        }
        self.goto(Step::RnWaitS);
        self.last_reneg = self.n;
        if !cleardown {
            self.recent_renegs += 1;
        }
    }

    fn reset_monitor(&mut self) {
        self.mon_t = self.n;
        let st = self.link.stats();
        self.mon_fcs = st.fcs_errors;
        self.mon_frames = st.frames_rx;
        if let Some(d) = self.dec.as_ref() {
            self.mon_err = d.errors;
            self.mon_4d = d.symbols4d();
        }
        self.bad_windows = 0;
        self.good_windows = 0;
        self.dec_mse = 0.0;
    }

    /// Surveillance de la qualité en mode données : renégocie vers le bas si
    /// les erreurs augmentent, vers le haut si la ligne s'améliore, et
    /// demande un réentraînement complet si rien n'y fait.
    fn monitor(&mut self) {
        if self.n - self.mon_t < MON_PERIOD || self.v90.is_some() {
            return;
        }
        let Some(dec) = self.dec.as_ref() else { return };
        if dec.in_b1() {
            return;
        }
        let st = self.link.stats();
        let derr = dec.errors - self.mon_err;
        let d4 = (dec.symbols4d() - self.mon_4d).max(1);
        let dfcs = st.fcs_errors - self.mon_fcs;
        let dfr = st.frames_rx - self.mon_frames;
        self.mon_t = self.n;
        self.mon_err = dec.errors;
        self.mon_4d = dec.symbols4d();
        self.mon_fcs = st.fcs_errors;
        self.mon_frames = st.frames_rx;
        let err_rate = derr as f32 / d4 as f32;
        // La ligne supporte-t-elle encore ce débit ? (SNR mesuré hors trous)
        let snr_vu = -10.0 * self.dec_mse.max(1e-6).log10();
        let ligne_ok = max_rate_for_snr(self.sr_rx_idx, snr_vu, 1.0) >= self.rx_rate;
        // Erreurs dues au bruit : renégocier vers le bas aide. Erreurs dues à
        // des trous (paquets perdus) avec une ligne saine : LAPM retransmet,
        // renégocier ne servirait à rien. Au-delà de 20 %, filet de sécurité.
        let bad = ((err_rate > 0.01 || (dfcs >= 2 && dfcs > dfr)) && !ligne_ok) || err_rate > 0.2;
        if bad {
            self.bad_windows += 1;
            self.good_windows = 0;
        } else {
            self.good_windows += 1;
            self.bad_windows = 0;
        }
        let since = (self.n - self.last_reneg) / 8; // ms
        if since > 20_000 {
            self.recent_renegs = 0;
        }
        // Erreurs DURABLES seulement (3 fenêtres d'affilée, 1,5 s) : un trou
        // isolé (paquet perdu, coupure brève) abîme une fenêtre, et LAPM
        // retransmet ; renégocier pour si peu coûterait bien plus cher.
        if self.bad_windows >= 3 && since > 1500 {
            if self.recent_renegs >= 2 {
                self.log(format!("Toujours trop d'erreurs ({:.1} % des symboles) : réentraînement complet", err_rate * 100.0));
                self.act(P34Action::Retrain("trop d'erreurs malgré les renégociations".into()));
                self.recent_renegs = 0;
                self.reset_monitor();
                return;
            }
            let min = if self.sr_rx_idx == 0 { 2400 } else { 4800 };
            let cap = self.rx_rate.saturating_sub(2400).max(min);
            self.log(format!("Trop d'erreurs ({:.1} % des symboles, {dfcs} trames rejetées) : renégociation vers le bas", err_rate * 100.0));
            self.start_reneg(true, Some(cap), false);
            return;
        }
        // Bilan de la dernière montée tentée : si le débit n'a pas monté, on
        // attendra deux fois plus longtemps avant de réessayer (sinon :
        // renégociation inutile toutes les 10 s).
        if let Some(from) = self.up_from.take() {
            self.up_fails = if self.rx_rate > from { 0 } else { (self.up_fails + 1).min(6) };
        }
        let attente = 10_000u64 << self.up_fails;
        if self.good_windows >= 8 && since > attente {
            // SNR le plus prudent des deux mesures (égaliseur, décisions)
            let snr = self.rx.snr_db().min(snr_vu);
            let cap = max_rate_for_snr(self.sr_rx_idx, snr, 3.0);
            if cap >= self.rx_rate + 4800 {
                self.log(format!("La ligne s'est améliorée (SNR {snr:.1} dB) : renégociation vers le haut"));
                self.up_from = Some(self.rx_rate);
                self.start_reneg(true, None, false);
            }
            self.good_windows = 0;
        }
    }
}

/// Décision : point le plus proche (constellation normalisée à puissance 1),
/// avec son étiquette dans le quart (0..3) et sa rotation Z (horaire).
struct Decision {
    point: Cpx,
    label: u32,
    rot: u32,
}

fn decide(z: Cpx, sixteen: bool) -> Decision {
    let (n_lbl, scale) = if sixteen { (4, 1.0 / 10f32.sqrt()) } else { (1, 1.0 / 2f32.sqrt()) };
    let mut best = Decision { point: Cpx::ZERO, label: 0, rot: 0 };
    let mut bd = f32::MAX;
    for label in 0..n_lbl {
        let q = quarter_point(label as usize) * scale;
        for rot in 0..4 {
            let p = q.rot_cw(rot);
            let d = (z - p).norm_sqr();
            if d < bd {
                bd = d;
                best = Decision { point: p, label, rot };
            }
        }
    }
    best
}

/// Quadrant (rotation horaire de point0) d'un symbole 4 points.
fn quadrant_of(p: Cpx) -> u32 {
    decide(p, false).rot
}

/// Détection de J : les 32 derniers bits sont deux répétitions d'une
/// rotation circulaire du motif 4 ou 16 points. Renvoie la taille demandée.
fn detect_j(bits: &VecDeque<bool>) -> Option<bool> {
    if bits.len() < 32 {
        return None;
    }
    let n = bits.len();
    let last: Vec<bool> = bits.range(n - 32..).copied().collect();
    if last[..16] != last[16..] {
        return None;
    }
    for sixteen in [false, true] {
        let pat = j_pattern(sixteen);
        for r in 0..16 {
            if (0..16).all(|i| pat[(i + r) % 16] == last[16 + i]) {
                return Some(sixteen);
            }
        }
    }
    None
}

/// Recherche d'une séquence MP valide dans les derniers bits reçus.
fn detect_mp(bits: &VecDeque<bool>) -> Option<Mp> {
    let n = bits.len();
    for len in [88usize, 188] {
        if n < len {
            continue;
        }
        let w: Vec<bool> = bits.range(n - len..).copied().collect();
        if w[..17].iter().all(|&b| b) && !w[17] && w[18] == (len == 188) {
            if let Some(mp) = Mp::from_bits(&w) {
                return Some(mp);
            }
        }
    }
    None
}

impl Modem {
    /// Capacité de réception (multiples de 2400) pour une vitesse de symbole.
    fn rx_capability(&self, sr_idx: usize) -> u8 {
        let probe = self.probe_report.as_ref().map(|r| r.results[sr_idx].max_rate).unwrap_or(0);
        let cap = (MAX_RATE_FOR_SR[sr_idx].min(self.cfg.max_rate) / 2400) as u8;
        probe.min(cap)
    }

    /// Phase 3 du V.90 : le modem analogique émet S, S̄, PP, TRN, Ja ;
    /// le numérique les reçoit puis répond en PCM (voir `phase34/v90.rs`).
    fn start_phase3_v90(&mut self) {
        self.set_phase(Phase::Phase3);
        let answer = self.p2_answer();
        let i1a = self.info1a.clone().unwrap();
        let i1c = self.info1c.clone().unwrap();
        let up = (i1a.sr_a2c as usize).min(5);
        let high = i1c.results[up].high_carrier;
        let mp = Mp {
            type1: false,
            rate_c2a: 1,
            rate_a2c: 1,
            aux: false,
            trellis: 0,
            nonlinear: false,
            shaping_expanded: false,
            ack: false,
            rate_mask: rate_mask_upto(self.cfg.max_rate),
            asym: true,
            precoder: [(0, 0); 3],
        };
        let mut p = P34::new(answer, up, high, up, high, mp);
        p.ec_wait = 0;
        p.n = self.n;
        p.t_step = self.n;
        if answer {
            self.log(LogLevel::Info, &format!("Phase 3 (56k) — émission V.34 {:.0} bauds / porteuse {:.0} Hz, réception PCM 8000 symboles/s", SYMBOL_RATES[up], carrier_hz(up, high)));
            let i0d = self.info0_remote.as_ref().and_then(|i| i.v90d);
            p.start_v90_analog(i0d, self.cfg.max_rate_pcm, self.cfg.max_rate, self.cfg.express);
        } else {
            self.log(LogLevel::Info, &format!("Phase 3 (56k) — émission PCM 8000 symboles/s, réception V.34 {:.0} bauds / porteuse {:.0} Hz", SYMBOL_RATES[up], carrier_hz(up, high)));
            p.start_v90_digital(self.cfg.max_rate_pcm, self.cfg.max_rate);
        }
        if let Some(link) = self.saved_link.take() {
            p.link = link;
        }
        // Liaison 56k : les deux bouts sont récents, on peut élargir la
        // couche de correction d'erreurs pour encaisser l'aller-retour du
        // réseau (trames de 256 octets, fenêtre de 31, T401 d'une seconde).
        p.link.set_n401(256);
        p.link.set_window(31);
        p.link.set_t401_bits(if answer { 33600 } else { 56000 });
        if !answer {
            p.ec_wait = self.rtd.unwrap_or(0) + 100 * MS;
        }
        self.retrain_det = Some(crate::dsp::tone::ToneDetector::new(if answer { 1200.0 } else { 2400.0 }, 160));
        self.retrain_on = 0;
        let actions = p.take_actions();
        self.p34 = Some(Box::new(p));
        self.goto(St::P3);
        self.apply_p34_actions(actions);
    }

    pub(crate) fn start_phase3(&mut self) {
        if self.info1a.as_ref().is_some_and(|i| i.uinfo.is_some()) {
            self.start_phase3_v90();
            return;
        }
        self.set_phase(Phase::Phase3);
        let answer = self.p2_answer();
        let i1a: Info1a = self.info1a.clone().unwrap();
        let i1c: Info1c = self.info1c.clone().unwrap();
        let (sr_tx, tx_high, sr_rx, rx_high) = if answer {
            (i1a.sr_a2c as usize, i1c.results[i1a.sr_a2c as usize].high_carrier, i1a.sr_c2a as usize, i1a.high_carrier)
        } else {
            (i1a.sr_c2a as usize, i1a.high_carrier, i1a.sr_a2c as usize, i1c.results[i1a.sr_a2c as usize].high_carrier)
        };
        // Capacités : notre réception (sondage), l'émission (projection de l'autre)
        let rx_cap = self.rx_capability(sr_rx);
        let tx_cap = if answer { i1c.results[sr_tx].max_rate } else { i1a.max_rate };
        let tx_cap = tx_cap.min((MAX_RATE_FOR_SR[sr_tx].min(self.cfg.max_rate) / 2400) as u8);
        let (c2a, a2c) = if answer { (rx_cap, tx_cap) } else { (tx_cap, rx_cap) };
        let mp = Mp {
            type1: false,
            rate_c2a: c2a.max(1),
            rate_a2c: a2c.max(1),
            aux: false,
            trellis: 0,
            nonlinear: false,
            shaping_expanded: false,
            ack: false,
            rate_mask: rate_mask_upto(self.cfg.max_rate),
            asym: true,
            precoder: [(0, 0); 3],
        };
        let mut p = P34::new(answer, sr_tx, tx_high, sr_rx, rx_high, mp);
        // Taille demandée à l'autre pour la phase 4 : l'appelé décide sur le
        // SNR du sondage (il n'a pas encore entraîné son égaliseur) avec
        // une forte marge ; l'appelant re-décidera sur le SNR mesuré.
        let snr = self.status.params.snr_db.unwrap_or(0.0);
        p.rx16 = snr >= 30.0;
        p.ec_wait = (144.0 / SYMBOL_RATES[sr_tx] * 8000.0) as u64 + self.rtd.unwrap_or(0) + 60 * 8;
        p.n = self.n;
        p.t_step = self.n;
        self.log(LogLevel::Info, &format!(
            "Phase 3 — émission {:.0} bauds / porteuse {:.0} Hz, réception {:.0} bauds / porteuse {:.0} Hz",
            SYMBOL_RATES[sr_tx], carrier_hz(sr_tx, tx_high), SYMBOL_RATES[sr_rx], carrier_hz(sr_rx, rx_high)
        ));
        if answer {
            // §11.3.1.2.1 : silence 70 ms, S 128T, S̄ 16T, PP, TRN, puis J
            let pre = p.silence_syms(70.0);
            let mut v = pre;
            v.extend(seq_s(128, false));
            v.extend(seq_s(16, true));
            v.extend(seq_pp().iter().map(|x| *x * std::f32::consts::SQRT_2));
            p.send_burst(v, TxSt::Trn { left: TRN3_LEN }, "S S̄ PP");
            p.rx_st = RxSt::WaitS;
        } else {
            p.act(P34Action::Tx("silence".into()));
        }
        let _ = DATA_GAP_DB;
        if let Some(link) = self.saved_link.take() {
            // Réentraînement : on garde la couche liaison (données en attente,
            // numéros de séquence) ; les trames perdues seront retransmises.
            p.link = link;
        }
        self.retrain_det = Some(crate::dsp::tone::ToneDetector::new(if answer { 1200.0 } else { 2400.0 }, 160));
        self.retrain_on = 0;
        self.p34 = Some(Box::new(p));
        self.goto(St::P3);
    }

    /// Réentraînement (§11.5) : on quitte le mode données, on garde la
    /// couche liaison, silence 70 ms, puis ton B (appelant) ou A (appelé) et
    /// on reprend la phase 2 à la mesure du délai (les INFO0 restent valables).
    pub(crate) fn initiate_retrain(&mut self, why: &str, initiator: bool) {
        if self.info0_remote.is_none() {
            // Sans INFO0 de l'autre, un réentraînement ne suffit pas
            self.p34 = None;
            self.start_info0_repeat(why);
            return;
        }
        if let Some(mut p) = self.p34.take() {
            self.saved_link = Some(p.take_link());
        }
        self.retrain_det = None;
        self.status.leds.cd = false;
        self.status.leds.hs = false;
        self.set_phase(Phase::Retrain);
        self.log(LogLevel::Info, &format!("Réentraînement complet ({why})"));
        self.set_tx(TxMode::Silence, "silence");
        self.rt_initiator = initiator;
        self.t_mark = self.n + 70 * MS;
        self.goto(St::RtSilence);
    }

    pub(crate) fn retrain_step(&mut self, f: &crate::modem::RxFlags) {
        let answer = self.p2_answer();
        match self.st {
            St::RtSilence => {
                if self.n >= self.t_mark {
                    // Récepteurs de phase 2 : ton de l'autre et ses inversions
                    let f = if answer { 1200.0 } else { 2400.0 };
                    self.tone_rx = Some(crate::dsp::tone::ToneDetector::new(f, 40));
                    self.tone_presence = Some(crate::dsp::tone::ToneDetector::new(f, 160));
                    self.tone_on = 0;
                    self.pending_rev = None;
                    // On écoute aussi les INFO0 : si l'autre les répète, on
                    // rejoint la répétition (§11.5.1.1 / §11.5.2.1).
                    self.info_rx = Some(crate::info::InfoRx::new(!answer, self.remote_info0_bits()));
                    self.probe = None;
                    if answer {
                        self.set_tx(TxMode::Tone(ToneTx::tone_a()), "A");
                    } else {
                        self.set_tx(TxMode::Tone(ToneTx::tone_b()), "B");
                    }
                    self.t_mark = self.n;
                    self.goto(St::RtWaitTone);
                }
            }
            St::RtWaitTone => {
                if let Some(Ok(bits)) = &f.info {
                    let r = crate::info::Info0::from_info(bits);
                    self.info0_remote = Some(r);
                    self.start_info0_repeat("INFO0 reçu pendant le réentraînement");
                    return;
                }
                if !answer {
                    // Appelant : ton A présent → détection de son inversion
                    if self.tone_for(20) {
                        self.rx_name("A");
                        if let Some(t) = self.tone_rx.as_mut() {
                            t.rearm();
                        }
                        self.set_phase(Phase::Phase2);
                        self.goto(St::C2WaitRev1);
                    }
                } else if self.tone_for(20) && self.n >= self.t_mark + 50 * MS {
                    // Appelé : ton B présent et A émis depuis 50 ms → inversion
                    self.rx_name("B");
                    let at = self.n + 1;
                    if let TxMode::Tone(tt) = &mut self.tx.mode {
                        tt.schedule_reversal(at);
                    }
                    self.tx_name("Ā");
                    self.t_mark = at;
                    if let Some(t) = self.tone_rx.as_mut() {
                        t.rearm();
                    }
                    self.set_phase(Phase::Phase2);
                    self.goto(St::A2WaitBRev);
                }
                if self.st == St::RtWaitTone && self.elapsed() > 5000 * MS {
                    self.fail("réentraînement : l'autre modem ne répond pas");
                }
            }
            _ => {}
        }
    }

    pub(crate) fn apply_p34_actions(&mut self, actions: Vec<P34Action>) {
        for a in actions {
            match a {
                P34Action::Phase(p) => self.set_phase(p),
                P34Action::Tx(s) => self.tx_name(&s),
                P34Action::Rx(s) => self.rx_name(&s),
                P34Action::Log(l, m) => self.log(l, &m),
                P34Action::Params(f) => {
                    f(&mut self.status.params);
                    self.params_changed();
                }
                P34Action::Connected { tx_rate, rx_rate } => {
                    self.recoveries = 0;
                    self.status.leds.cd = true;
                    self.status.leds.hs = tx_rate.min(rx_rate) >= 14400;
                    self.status.params.data_rate_tx = Some(tx_rate);
                    self.status.params.data_rate_rx = Some(rx_rate);
                    self.status.params.trellis_states = Some(16);
                    self.params_changed();
                    self.events.push(ModemEvent::Connected { tx_rate, rx_rate });
                }
                P34Action::Data(d) => {
                    self.status.bytes_rx += d.len() as u64;
                    self.events.push(ModemEvent::Data(d));
                }
                P34Action::Fail(r) => {
                    self.recover(&r);
                    return;
                }
                P34Action::Hangup(r) => {
                    self.hangup(&r);
                    return;
                }
                P34Action::Retrain(r) => {
                    self.initiate_retrain(&r, true);
                    return;
                }
                P34Action::NoV90(r) => {
                    // Le prochain INFO1a choisira le V.34 (§9.2.2.1.9)
                    self.log(LogLevel::Warn, &r);
                    self.v90_refused = true;
                    self.initiate_retrain(&r, true);
                    return;
                }
            }
        }
    }
}
