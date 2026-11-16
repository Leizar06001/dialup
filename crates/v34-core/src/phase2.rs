//! Phase 2 V.34 (§11.2) : INFO0, mesure du délai aller-retour par les
//! inversions de phase des tons A/B, sondage L1/L2 dans les deux sens,
//! puis INFO1c (résultats) / INFO1a (décision).

use crate::api::*;
use crate::dsp::tone::ToneDetector;
use crate::info::{CrcSeen, Info0, Info1a, Info1c, InfoRx, InfoTx};
use crate::modem::{Modem, RxFlags, St, MS};
use crate::probe::{carrier_hz, ProbeAnalyzer, ProbeGen, SYMBOL_RATES_NOMINAL};
use crate::tx::{ToneTx, TxMode};

/// Écart toléré à la capacité (dB) par notre récepteur (égaliseur linéaire,
/// décision symbole par symbole).
const GAP_DB: f32 = 9.0;
/// Délai entre réception d'une inversion et émission de la réponse : 40 ms.
const REV_DELAY: u64 = 40 * MS;
const L1_LEN: u64 = 160 * MS;
/// Marge sur l'attente du ton B après notre L2 (§11.2.2.2.3 : 600 ms plus
/// un aller-retour) : derrière un adaptateur téléphonique, l'aller-retour
/// réel dépasse celui mesuré sur les inversions (un U.S. Robotics répond
/// ~1 s après le début de L2, pour 960 ms permises).
const L2_B_MARGIN: u64 = 400 * MS;
/// Durée d'analyse de L2 (≤ 500 ms autorisés).
const L2_ANALYSIS: u32 = 20; // blocs de 20 ms = 400 ms
/// … quand l'aller-retour est long (Internet, adaptateur téléphonique) :
/// l'appelé n'attend notre ton B que 600 ms plus SON aller-retour depuis le
/// début de son L2 (§11.2.2.2.3). Le nôtre vaut le sien à la gigue près ;
/// écouter 400 ms de L2 ne lui laissait que ~180 ms pour détecter B, et un
/// U.S. Robotics derrière un ATA le ratait un appel sur deux (puis restait
/// sur son ton A sans l'inverser). 200 ms d'analyse suffisent au sondage.
const L2_ANALYSIS_LONG_RTD: u32 = 10;
/// Aller-retour à partir duquel on écourte l'analyse de L2
const LONG_RTD: u64 = 250 * MS;

impl Modem {
    pub(crate) fn start_phase2(&mut self) {
        self.set_phase(Phase::Phase2);
        let answer = self.p2_answer();
        let info0 = self.our_info0();
        self.info0_local = Some(info0.clone());
        let mut itx = InfoTx::new(answer);
        itx.queue(&self.info_out(info0.to_bits()));
        self.set_tx(TxMode::Info(itx), if answer { "INFO0a" } else { "INFO0c" });
        if self.info_rx.is_none() {
            self.prepare_phase2_rx();
        }
        self.log(LogLevel::Info, &format!("Phase 2 — envoi {} (DPSK 600 bit/s, porteuse {} Hz)", if answer { "INFO0a" } else { "INFO0c" }, if answer { 2400 } else { 1200 }));
        self.goto(if answer { St::A2Info0 } else { St::C2Info0 });
    }

    /// §11.2.1.x.1 : le récepteur est conditionné PENDANT le silence de
    /// 75 ms qui termine la phase 1 (sinon on rate le début de l'INFO0).
    pub(crate) fn prepare_phase2_rx(&mut self) {
        let answer = self.p2_answer();
        // On reçoit les INFO de l'autre : 2400 Hz si on est l'appelant.
        self.info_rx = Some(self.new_info_rx(!answer, self.remote_info0_bits()));
        let f = if answer { 1200.0 } else { 2400.0 };
        self.tone_rx = Some(ToneDetector::new(f, 40));
        self.tone_presence = Some(ToneDetector::new(f, 160));
        self.tone_on = 0;
    }

    /// Notre INFO0 : en V.90, le modem numérique envoie l'INFO0d, avec sa
    /// puissance maximale (−0,5 dBm0) et sa loi (A, réseau français).
    pub(crate) fn our_info0(&self) -> Info0 {
        let mut i = Info0::ours(&self.cfg);
        if self.v90 && !self.p2_answer() {
            i.v90d = Some(crate::info::Info0d { nominal_power: 0, max_power: 0, power_at_codec: false, alaw: true, up3429: self.cfg.allow_3429 });
        }
        i
    }

    /// Longueur des bits d'information de l'INFO0 de l'autre.
    pub(crate) fn remote_info0_bits(&self) -> usize {
        if self.v90 && self.p2_answer() {
            Info0::INFO0D_BITS
        } else {
            Info0::INFO_BITS
        }
    }

    /// Une de nos trames INFO, avec son CRC dans l'ordre en vigueur.
    pub(crate) fn info_out(&self, mut bits: Vec<bool>) -> Vec<bool> {
        if self.crc_legacy {
            let pos = crate::info::info_crc_pos(bits.len());
            crate::info::swap_crc(&mut bits, pos);
        }
        bits
    }

    /// Récepteur des trames INFO de l'autre (ordres de CRC selon la config).
    pub(crate) fn new_info_rx(&self, answer_signal: bool, info_bits: usize) -> InfoRx {
        InfoRx::new(answer_signal, info_bits).with_crc_mode(self.cfg.crc_mode)
    }

    /// Un INFO0 valide de l'autre vient d'arriver : en mode `Auto`, on émet
    /// désormais dans l'ordre de CRC qu'il utilise. Vrai si on a changé
    /// (notre INFO0 précédent lui était illisible : il faut le répéter).
    fn follow_crc(&mut self) -> bool {
        let seen = self.info_rx.as_ref().and_then(InfoRx::crc_seen);
        self.crc_seen = seen;
        if self.cfg.crc_mode != crate::api::CrcMode::Auto {
            return false;
        }
        let legacy = match seen {
            Some(CrcSeen::Standard) => false,
            Some(CrcSeen::Legacy) => true,
            _ => return false,
        };
        if legacy == self.crc_legacy {
            return false;
        }
        self.crc_legacy = legacy;
        self.log(
            LogLevel::Info,
            if legacy { "L'autre modem met le CRC poids faible en premier (nos modems d'avant) : on fait comme lui" } else { "L'autre modem suit la norme (CRC poids fort en premier) : on fait comme lui" },
        );
        true
    }

    fn info_tx_done(&self) -> bool {
        matches!(&self.tx.mode, TxMode::Info(i) if i.done())
    }

    fn got_info0(&mut self, f: &RxFlags) {
        if let Some(Ok(bits)) = &f.info {
            self.info0_count += 1;
            let ack = Info0::from_info(bits).ack;
            // INFO0 répété par l'autre : il n'a pas eu le nôtre → on répète
            // aussi, avec l'acquittement (§11.2.2.x.1).
            let repeated = self.info0_remote.is_some() && (self.info0_count >= 2 || ack) && self.st != St::X2Info0Rep && !ack;
            // L'autre suit un autre ordre de CRC que le nôtre : notre INFO0
            // lui était illisible, on le répète dans son ordre.
            let switched = self.follow_crc();
            if let Some(r) = self.info0_remote.as_mut() {
                r.ack = ack;
            }
            if self.info0_remote.is_none() {
                let i0 = Info0::from_info(bits);
                let me = if !self.p2_answer() { "INFO0a" } else { "INFO0c" };
                self.rx_name(me);
                let mut rates = vec!["2400", "3000", "3200"];
                if i0.sr2743 {
                    rates.push("2743");
                }
                if i0.sr2800 {
                    rates.push("2800");
                }
                if i0.sr3429 && i0.allow3429 {
                    rates.push("3429");
                }
                self.log(LogLevel::Info, &format!("{me} reçu : vitesses de symbole {}", rates.join("/")));
                self.info0_remote = Some(i0);
                if let Some(t) = self.tone_rx.as_mut() {
                    t.rearm();
                }
                self.tone_on = 0;
            }
            if switched && self.st != St::X2Info0Rep {
                self.start_info0_repeat("ordre du CRC changé");
            } else if repeated {
                self.start_info0_repeat("l'autre répète son INFO0");
            }
        } else if let Some(Err(())) = &f.info {
            self.log(LogLevel::Warn, "INFO0 reçu avec CRC faux");
        }
    }

    fn start_probe_rx(&mut self, start_at: u64) {
        self.t_mark = start_at;
        self.probe = None;
    }

    fn probe_feed_ready(&mut self) -> bool {
        if self.probe.is_none() && self.n >= self.t_mark {
            self.probe = Some(ProbeAnalyzer::new());
            self.rx_name("L2");
        }
        let need = if self.rtd.unwrap_or(0) > LONG_RTD { L2_ANALYSIS_LONG_RTD } else { L2_ANALYSIS };
        matches!(&self.probe, Some(p) if p.blocks() >= need)
    }

    fn finish_probe(&mut self) {
        let p = self.probe.take().unwrap();
        let r = p.report(self.cfg.max_rate, GAP_DB);
        let best: Vec<String> = r
            .results
            .iter()
            .enumerate()
            .filter(|(_, x)| x.max_rate > 0)
            .map(|(i, x)| format!("{}:{}", SYMBOL_RATES_NOMINAL[i], x.max_rate as u32 * 2400))
            .collect();
        self.log(LogLevel::Info, &format!("Sondage L1/L2 analysé — débits projetés (bauds:bit/s) {}", best.join(" ")));
        if let Some(fo) = r.freq_offset_hz {
            if fo.abs() > 0.1 {
                self.log(LogLevel::Detail, &format!("Décalage de fréquence mesuré : {fo:+.2} Hz"));
            }
        }
        self.status.params.line_response = r.tones.iter().map(|&(f, g, _)| (f, g)).collect();
        self.status.params.projected_rates = r.results.iter().enumerate().map(|(i, x)| (SYMBOL_RATES_NOMINAL[i], x.max_rate as u32 * 2400)).collect();
        self.params_changed();
        self.probe_report = Some(r);
    }

    fn freq_offset_field(&self) -> i16 {
        match self.probe_report.as_ref().and_then(|r| r.freq_offset_hz) {
            Some(f) => ((f / 0.02).round() as i32).clamp(-511, 511) as i16,
            None => -512,
        }
    }

    /// Répétition des INFO0 (§11.2.2.1.1, §11.2.2.2.1, §11.8) : chacun
    /// répète son INFO0, avec le bit 28 à 1 dès qu'il a reçu celui de
    /// l'autre ; on repart vers les tons quand on reçoit un INFO0 acquitté.
    pub(crate) fn start_info0_repeat(&mut self, why: &str) {
        let answer = self.p2_answer();
        self.log(LogLevel::Warn, &format!("{why} : répétition des INFO0 (§11.2.2)"));
        self.set_phase(Phase::Phase2);
        if self.info_rx.is_none() || self.tone_rx.is_none() {
            self.prepare_phase2_rx();
        }
        let mut i0 = self.info0_local.clone().unwrap_or_else(|| self.our_info0());
        i0.ack = self.info0_remote.is_some();
        self.info0_local = Some(i0.clone());
        let mut itx = InfoTx::new(answer);
        itx.queue(&self.info_out(i0.to_bits()));
        self.set_tx(TxMode::Info(itx), if answer { "INFO0a ×" } else { "INFO0c ×" });
        self.t_mark = 0; // drapeau « INFO0 acquitté reçu »
        self.goto(St::X2Info0Rep);
    }

    fn info0_repeat_step(&mut self, f: &RxFlags) {
        let answer = self.p2_answer();
        if let Some(Ok(bits)) = &f.info {
            let r = Info0::from_info(bits);
            self.follow_crc();
            if self.info0_remote.is_none() {
                self.rx_name(if answer { "INFO0c" } else { "INFO0a" });
                self.log(LogLevel::Info, "INFO0 de l'autre reçu (répétition)");
            }
            if r.ack {
                self.t_mark = 1;
            }
            self.info0_remote = Some(r);
        }
        // Fin d'un INFO0 : on en renvoie un (acquitté si on a le sien), sauf
        // si l'autre a acquitté le nôtre → on passe aux tons A/B.
        // Sortie : INFO0 acquitté reçu, OU ton de l'autre entendu alors qu'on
        // a son INFO0 (« alternatively… », §11.2.2.1.1 / §11.2.2.2.1).
        if self.info0_remote.is_some() && self.tone_for(20) {
            self.t_mark = 1;
        }
        if self.info_tx_done() {
            if self.t_mark == 1 && self.info0_remote.is_some() {
                self.info0_count = 0;
                if answer {
                    self.set_tx(TxMode::Tone(ToneTx::tone_a()), "A");
                    self.t_mark = self.n;
                    self.goto(St::A2Info0);
                } else {
                    self.set_tx(TxMode::Tone(ToneTx::tone_b()), "B");
                    self.goto(St::C2Info0);
                }
                if let Some(t) = self.tone_rx.as_mut() {
                    t.rearm();
                }
                self.tone_on = 0;
                return;
            }
            let mut i0 = self.info0_local.clone().unwrap();
            i0.ack = self.info0_remote.is_some();
            // L'autre répète sans acquitter, et on n'a pu déduire son ordre
            // de CRC (trame palindrome) : il ne lit peut-être pas le nôtre,
            // on essaie l'autre ordre.
            if self.cfg.crc_mode == crate::api::CrcMode::Auto && self.crc_seen == Some(CrcSeen::Either) && self.t_mark != 1 {
                self.crc_legacy = !self.crc_legacy;
            }
            let mut itx = InfoTx::new(answer);
            itx.queue(&self.info_out(i0.to_bits()));
            self.tx.mode = TxMode::Info(itx);
        }
        if self.elapsed() > 6000 * MS {
            self.fail("échange des INFO0 impossible");
        }
    }

    pub(crate) fn phase2(&mut self, f: &RxFlags) {
        // Garde-fou global : la phase 2 dure normalement < 2 s
        if self.elapsed() > 4000 * MS && !matches!(self.st, St::C2WaitInfo1a | St::A2WaitInfo1c | St::X2Info0Rep) {
            self.recover(&format!("phase 2 bloquée ({:?})", self.st));
            return;
        }
        match self.st {
            // ============================ APPELANT ============================
            St::X2Info0Rep => self.info0_repeat_step(f),
            St::C2Info0 => {
                self.got_info0(f);
                if self.st != St::C2Info0 {
                    return;
                }
                // Ton A présent alors qu'on n'a pas reçu INFO0a : on l'a raté
                if self.info0_remote.is_none() && matches!(self.tx.mode, TxMode::Tone(_)) && self.tone_for(100) {
                    self.start_info0_repeat("INFO0a non reçu");
                    return;
                }
                if matches!(self.tx.mode, TxMode::Info(_)) && self.info_tx_done() {
                    self.set_tx(TxMode::Tone(ToneTx::tone_b()), "B");
                }
                if self.info0_remote.is_some() && matches!(self.tx.mode, TxMode::Tone(_)) && self.tone_for(20) {
                    self.rx_name("A");
                    if let Some(t) = self.tone_rx.as_mut() {
                        t.rearm();
                    }
                    self.goto(St::C2WaitRev1);
                }
            }
            St::C2WaitRev1 => {
                // Un INFO0a répété = l'appelé n'a pas eu notre INFO0c (§11.2.2.1.1)
                self.got_info0(f);
                if self.st != St::C2WaitRev1 {
                    return;
                }
                if let Some(t) = self.confirmed_reversal(f) {
                    self.rx_name("Ā");
                    let at = t + REV_DELAY;
                    if let TxMode::Tone(tt) = &mut self.tx.mode {
                        tt.schedule_reversal(at);
                    }
                    self.t_mark = at;
                    self.log(LogLevel::Detail, "Inversion de A reçue → inversion de B 40 ms plus tard");
                    self.goto(St::C2Rev1Sched);
                }
            }
            St::C2Rev1Sched => {
                if self.n == self.t_mark {
                    self.tx_name("B̄");
                }
                if self.n >= self.t_mark + 10 * MS {
                    self.set_tx(TxMode::Silence, "silence");
                    if let Some(t) = self.tone_rx.as_mut() {
                        t.rearm();
                    }
                    self.goto(St::C2WaitRev2);
                }
            }
            St::C2WaitRev2 => {
                self.got_info0(f);
                if self.st != St::C2WaitRev2 {
                    return;
                }
                if let Some(t) = self.confirmed_reversal(f) {
                    if t > self.t_mark {
                        // confirmée 9 ms après : la fenêtre du mesureur ne
                        // contient plus que le ton inversé, stable
                        self.rev2_at = t;
                        self.rev2_p = self.p_now;
                        self.goto(St::C2CheckL1);
                    }
                } else if self.elapsed() > 2000 * MS {
                    self.recover("seconde inversion du ton A non reçue");
                }
            }
            St::C2CheckL1 => {
                // §11.2.1.2.5 : après la seconde inversion, l'appelé émet son
                // ton A encore un peu (10 ms selon la norme, ~65 ms pour un
                // U.S. Robotics), puis L1, 6 dB plus fort. On guette L1 : il
                // date le début de L2. Si le ton A dure, cette inversion
                // était la PREMIÈRE d'un nouveau cycle : il n'avait pas vu la
                // nôtre (un vrai modem derrière un ATA, quand la répétition
                // des INFO0 l'a décalé d'un tour). On y répond comme telle.
                let t = self.rev2_at;
                if self.n < t + 12 * MS {
                    return;
                }
                let l1 = self.p_now > 2.5 * self.rev2_p.max(1e-9);
                if !l1 && self.n < t + 150 * MS {
                    return;
                }
                if !l1 {
                    self.log(LogLevel::Warn, "Ton A toujours là après son inversion : c'était sa première, on inverse B");
                    let at = if let TxMode::Tone(tt) = &mut self.tx.mode {
                        let at = (t + REV_DELAY).max(self.n);
                        tt.schedule_reversal(at);
                        at
                    } else {
                        // notre B s'est déjà tu : on le reprend, assez
                        // longtemps pour qu'il le reconnaisse, et on l'inverse
                        let at = (t + REV_DELAY).max(self.n + 60 * MS);
                        let mut tt = ToneTx::tone_b();
                        tt.schedule_reversal(at);
                        self.set_tx(TxMode::Tone(tt), "B");
                        at
                    };
                    self.t_mark = at;
                    self.goto(St::C2Rev1Sched);
                    return;
                }
                let l1_at = self.n.saturating_sub(5 * MS);
                {
                    {
                        let rtd = (t - self.t_mark).saturating_sub(REV_DELAY);
                        self.rtd = Some(rtd);
                        self.far_ec.set_round_trip(rtd as usize);
                        self.status.params.rtd_ms = Some(self.ms(rtd));
                        self.params_changed();
                        self.rx_name("A");
                        self.log(LogLevel::Info, &format!("Délai aller-retour mesuré : {:.1} ms", self.ms(rtd)));
                        // L1 dure 160 ms ; on analyse L2 en laissant 15 ms de marge.
                        self.start_probe_rx(l1_at + L1_LEN + 15 * MS);
                        self.rx_name("L1");
                        self.goto(St::C2RxProbe);
                    }
                }
            }
            St::C2RxProbe => {
                if self.probe_feed_ready() {
                    self.finish_probe();
                    self.set_tx(TxMode::Tone(ToneTx::tone_b()), "B");
                    if let Some(t) = self.tone_rx.as_mut() {
                        t.rearm();
                    }
                    self.tone_on = 0;
                    self.b_relance = false;
                    self.goto(St::C2WaitA2);
                }
            }
            St::C2BPause => {
                if self.elapsed() >= 80 * MS {
                    self.set_tx(TxMode::Tone(ToneTx::tone_b()), "B");
                    if let Some(t) = self.tone_rx.as_mut() {
                        t.rearm();
                    }
                    self.goto(St::C2WaitA2);
                }
            }
            St::C2WaitA2 => {
                if self.tone_for(15) {
                    self.rx_name("A");
                }
                if let Some(t) = self.confirmed_reversal(f) {
                    {
                        self.rx_name("Ā");
                        let at = t + REV_DELAY;
                        if let TxMode::Tone(tt) = &mut self.tx.mode {
                            tt.schedule_reversal(at);
                        }
                        self.t_mark = at;
                        self.goto(St::C2Rev2Sched);
                    }
                } else if !self.b_relance && self.elapsed() > self.rtd.unwrap_or(0) + 450 * MS && self.tone_for(300) {
                    // Son ton A dure sans s'inverser alors qu'il entend notre B
                    // depuis longtemps : il l'a raté (parti avant la fin de son
                    // attente, §11.2.2.2.3) et ne réagit qu'à un B qui arrive.
                    // On le coupe 80 ms et on le relance, une fois.
                    self.b_relance = true;
                    self.log(LogLevel::Warn, "Ton A sans inversion : l'appelé n'a pas vu notre ton B, on le relance");
                    self.set_tx(TxMode::Silence, "silence");
                    self.goto(St::C2BPause);
                } else if self.elapsed() > (900 * MS + self.rtd.unwrap_or(0)) {
                    // §11.2.2.1.4 : on fait comme si l'inversion était reçue
                    self.log(LogLevel::Warn, "Inversion de A non détectée, on continue (§11.2.2.1.4)");
                    let at = self.n + REV_DELAY;
                    if let TxMode::Tone(tt) = &mut self.tx.mode {
                        tt.schedule_reversal(at);
                    }
                    self.t_mark = at;
                    self.goto(St::C2Rev2Sched);
                }
            }
            St::C2Rev2Sched => {
                if self.n == self.t_mark {
                    self.tx_name("B̄");
                }
                if self.n >= self.t_mark + 10 * MS {
                    self.set_tx(TxMode::Probe(ProbeGen::new(6.0)), "L1");
                    self.log(LogLevel::Info, "Envoi du sondage L1 (+6 dB, 160 ms) puis L2");
                    self.t_mark = self.n + L1_LEN;
                    self.goto(St::C2L1);
                }
            }
            St::C2L1 => {
                if self.n >= self.t_mark {
                    if let TxMode::Probe(p) = &mut self.tx.mode {
                        p.set_boost(0.0);
                    }
                    self.tx_name("L2");
                    if let Some(t) = self.tone_rx.as_mut() {
                        t.rearm();
                    }
                    self.tone_on = 0;
                    self.goto(St::C2L2);
                }
            }
            St::C2L2 => {
                let max = 550 * MS + self.rtd.unwrap_or(0);
                // L'appelé n'a pu analyser notre L2 avant de l'avoir reçu :
                // un ton A entendu moins d'un aller-retour après le début de
                // L2 est le sien d'avant (un vrai modem qui n'a pas inversé
                // son A), pas sa réponse. Sans cette garde, L2 durait 20 ms.
                let fresh = self.elapsed() >= self.rtd.unwrap_or(0) + 100 * MS;
                let a_now = self.tone_for(20) && fresh;
                if a_now || self.elapsed() > max {
                    if a_now {
                        self.rx_name("A");
                    }
                    let r = self.probe_report.as_ref().unwrap();
                    let i1c = Info1c { min_power_red: 0, add_power_red: 0, md_len: 0, results: r.results, freq_offset: self.freq_offset_field() };
                    let mut itx = InfoTx::new(false);
                    itx.queue(&self.info_out(i1c.to_bits()));
                    self.set_tx(TxMode::Info(itx), "INFO1c");
                    self.log(LogLevel::Info, "Envoi INFO1c (résultats du sondage)");
                    self.info1c = Some(i1c);
                    self.info_rx = Some(self.new_info_rx(true, Info1a::INFO_BITS));
                    self.info1_bad = false;
                    self.goto(St::C2Info1c);
                }
            }
            St::C2Info1c => {
                if self.info_tx_done() {
                    self.set_tx(TxMode::Silence, "silence");
                    self.goto(St::C2WaitInfo1a);
                }
                self.check_info1a(f);
            }
            St::C2WaitInfo1a => {
                self.check_info1a(f);
                // INFO1a abîmé déjà vu : l'appelé en émet, il le répétera
                let extra = if self.info1_bad { 3000 * MS } else { 0 };
                if self.st == St::C2WaitInfo1a && self.elapsed() > (1500 * MS + self.rtd.unwrap_or(0) + extra) {
                    self.recover("INFO1a non reçu");
                }
            }

            // ============================ APPELÉ ============================
            St::A2Info0 => {
                self.got_info0(f);
                if self.st != St::A2Info0 {
                    return;
                }
                if self.info0_remote.is_none() && matches!(self.tx.mode, TxMode::Tone(_)) && self.tone_for(100) {
                    self.start_info0_repeat("INFO0c non reçu");
                    return;
                }
                if matches!(self.tx.mode, TxMode::Info(_)) && self.info_tx_done() {
                    self.set_tx(TxMode::Tone(ToneTx::tone_a()), "A");
                    self.t_mark = self.n;
                }
                let a_long_enough = matches!(self.tx.mode, TxMode::Tone(_)) && self.n >= self.t_mark + 50 * MS;
                if self.info0_remote.is_some() && a_long_enough && self.tone_for(20) {
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
                    self.goto(St::A2WaitBRev);
                }
            }
            St::A2WaitBRev => {
                self.got_info0(f);
                if self.st != St::A2WaitBRev {
                    return;
                }
                if let Some(t) = self.confirmed_reversal(f) {
                    if t > self.t_mark {
                        self.rx_name("B̄");
                        let rtd = (t - self.t_mark).saturating_sub(REV_DELAY);
                        self.rtd = Some(rtd);
                        self.far_ec.set_round_trip(rtd as usize);
                        self.status.params.rtd_ms = Some(self.ms(rtd));
                        self.params_changed();
                        self.log(LogLevel::Info, &format!("Délai aller-retour mesuré : {:.1} ms", self.ms(rtd)));
                        let at = t + REV_DELAY;
                        if let TxMode::Tone(tt) = &mut self.tx.mode {
                            tt.schedule_reversal(at);
                        }
                        self.t_mark = at;
                        self.goto(St::A2Rev2Sched);
                    }
                } else if self.elapsed() > 2000 * MS {
                    self.recover("inversion du ton B non reçue");
                }
            }
            St::A2Rev2Sched => {
                if self.n == self.t_mark {
                    self.tx_name("A");
                }
                if self.n >= self.t_mark + 10 * MS {
                    self.set_tx(TxMode::Probe(ProbeGen::new(6.0)), "L1");
                    self.log(LogLevel::Info, "Envoi du sondage L1 (+6 dB, 160 ms) puis L2");
                    self.t_mark = self.n + L1_LEN;
                    self.goto(St::A2L1);
                }
            }
            St::A2L1 => {
                if self.n >= self.t_mark {
                    if let TxMode::Probe(p) = &mut self.tx.mode {
                        p.set_boost(0.0);
                    }
                    self.tx_name("L2");
                    if let Some(t) = self.tone_rx.as_mut() {
                        t.rearm();
                    }
                    self.tone_on = 0;
                    self.goto(St::A2L2);
                }
            }
            St::A2L2 => {
                if self.tone_for(20) {
                    self.rx_name("B");
                    // Ton A 50 ms, inversion, encore 10 ms, puis silence.
                    let mut a = ToneTx::tone_a();
                    let at = self.n + 50 * MS;
                    a.schedule_reversal(at);
                    self.set_tx(TxMode::Tone(a), "A");
                    self.t_mark = at;
                    self.goto(St::A2ToneA);
                } else if self.elapsed() > 600 * MS + self.rtd.unwrap_or(0) + L2_B_MARGIN {
                    self.recover("ton B non reçu après L2");
                }
            }
            St::A2ToneA => {
                if self.n == self.t_mark {
                    self.tx_name("Ā");
                }
                if self.n >= self.t_mark + 10 * MS {
                    self.set_tx(TxMode::Silence, "silence");
                    if let Some(t) = self.tone_rx.as_mut() {
                        t.rearm();
                    }
                    self.goto(St::A2WaitBRev2);
                }
            }
            St::A2WaitBRev2 => {
                if let Some(t) = self.confirmed_reversal(f) {
                    if t > self.t_mark {
                        self.rx_name("B̄");
                        self.start_probe_rx(t + 10 * MS + L1_LEN + 10 * MS);
                        self.goto(St::A2RxProbe);
                    }
                } else if self.elapsed() > 2000 * MS {
                    self.recover("inversion du ton B (avant L1) non reçue");
                }
            }
            St::A2RxProbe => {
                if self.n == self.t_mark.saturating_sub(L1_LEN) {
                    self.rx_name("L1");
                }
                if self.probe_feed_ready() {
                    self.finish_probe();
                    self.set_tx(TxMode::Tone(ToneTx::tone_a()), "A");
                    self.info_rx = Some(self.new_info_rx(false, Info1c::INFO_BITS));
                    self.info1_bad = false;
                    self.goto(St::A2WaitInfo1c);
                }
            }
            St::A2WaitInfo1c => {
                match &f.info {
                    Some(Ok(bits)) => {
                        let i1c = Info1c::from_info(bits);
                        self.rx_name("INFO1c");
                        self.log(LogLevel::Info, "INFO1c reçu — choix des vitesses de symbole");
                        self.info1c = Some(i1c);
                        self.decide_info1a();
                    }
                    Some(Err(())) => {
                        self.info1_bad = true;
                        self.log(LogLevel::Warn, "INFO1c reçu avec CRC faux");
                    }
                    None => {
                        // INFO1c abîmé déjà vu : l'appelant en émet, il le
                        // répétera (un vrai modem, ~3,5 s plus tard)
                        let extra = if self.info1_bad { 3000 * MS } else { 0 };
                        if self.elapsed() > 2000 * MS + 2 * self.rtd.unwrap_or(0) + extra {
                            self.recover("INFO1c non reçu");
                        }
                    }
                }
            }
            St::A2Info1a => {
                if self.info_tx_done() {
                    self.info_rx = None;
                    self.tone_rx = None;
                    self.tone_presence = None;
                    self.start_phase3();
                }
            }
            _ => {}
        }
    }

    fn check_info1a(&mut self, f: &RxFlags) {
        match &f.info {
            Some(Ok(bits)) if Info1a::from_info(bits).uinfo.is_some() => {
                // V.90 (§9.2.1.1.8) : nous émettrons en PCM, nous recevons le
                // V.34 de l'abonné à la vitesse qu'il a choisie
                let i1a = Info1a::from_info(bits);
                self.rx_name("INFO1a");
                let up = i1a.sr_a2c as usize;
                self.log(LogLevel::Info, &format!("INFO1a reçu : 56k (V.90) — descendant en PCM, montant {} bauds", SYMBOL_RATES_NOMINAL[up.min(5)]));
                let rx_high = self.info1c.as_ref().is_some_and(|c| c.results[up.min(5)].high_carrier);
                self.apply_symbol_rates(up.min(5), false, up.min(5), rx_high);
                self.status.params.symbol_rate_tx = Some(8000);
                self.status.params.carrier_tx_hz = None;
                self.info1a = Some(i1a);
                self.info_rx = None;
                self.tone_rx = None;
                self.tone_presence = None;
                self.start_phase3();
            }
            Some(Ok(bits)) => {
                let i1a = Info1a::from_info(bits);
                self.rx_name("INFO1a");
                let sr_tx = SYMBOL_RATES_NOMINAL[i1a.sr_c2a as usize];
                let sr_rx = SYMBOL_RATES_NOMINAL[i1a.sr_a2c as usize];
                self.log(LogLevel::Info, &format!("INFO1a reçu : émission {sr_tx} bauds, réception {sr_rx} bauds"));
                // Notre porteuse d'émission est imposée par INFO1a ; celle de
                // réception est celle que nous avons demandée dans INFO1c.
                let i1c = self.info1c.as_ref().unwrap();
                let rx_high = i1c.results[i1a.sr_a2c as usize].high_carrier;
                self.apply_symbol_rates(i1a.sr_c2a as usize, i1a.high_carrier, i1a.sr_a2c as usize, rx_high);
                self.status.params.preemphasis_tx = Some(i1a.preemph);
                self.info1a = Some(i1a);
                self.info_rx = None;
                self.tone_rx = None;
                self.tone_presence = None;
                self.start_phase3();
            }
            Some(Err(())) => {
                self.info1_bad = true;
                self.log(LogLevel::Warn, "INFO1a reçu avec CRC faux");
            }
            None => {}
        }
    }

    pub(crate) fn apply_symbol_rates(&mut self, sr_tx: usize, tx_high: bool, sr_rx: usize, rx_high: bool) {
        let p = &mut self.status.params;
        p.symbol_rate_tx = Some(SYMBOL_RATES_NOMINAL[sr_tx]);
        p.symbol_rate_rx = Some(SYMBOL_RATES_NOMINAL[sr_rx]);
        p.carrier_tx_hz = Some(carrier_hz(sr_tx, tx_high) as f32);
        p.carrier_rx_hz = Some(carrier_hz(sr_rx, rx_high) as f32);
        if let Some(r) = self.probe_report.as_ref() {
            // SNR estimé dans notre sens de réception
            p.snr_db = Some(r.snr_eq_db[sr_rx]);
        }
        self.params_changed();
    }

    /// V.90, modem analogique (§9.2.2.1.9) : 56k si la ligne s'y prête
    /// (pas de décalage de fréquence — signe d'un circuit analogique
    /// entre les deux centraux —, sondage pas trop bruité), avec la
    /// meilleure vitesse de symbole montante parmi 3000, 3200 et 3429.
    fn decide_info1a_v90(&mut self) -> Option<Info1a> {
        let me = self.info0_local.clone()?;
        let other = self.info0_remote.clone()?;
        let i1c = self.info1c.clone()?;
        let r = self.probe_report.as_ref()?;
        let fo = r.freq_offset_hz.unwrap_or(0.0);
        let down = r.results.iter().map(|x| x.max_rate as u32 * 2400).max().unwrap_or(0);
        if fo.abs() > 0.5 {
            self.log(LogLevel::Info, &format!("56k impossible : décalage de fréquence {fo:+.1} Hz (liaison analogique entre centraux)"));
            return None;
        }
        if down < 14400 {
            self.log(LogLevel::Info, &format!("56k impossible : ligne trop bruitée (sondage : {down} bit/s en V.34)"));
            return None;
        }
        let up3429 = other.v90d.is_some_and(|d| d.up3429);
        let usable = |i: usize| match i {
            3 => me.low3000 || me.high3000,
            4 => me.low3200 || me.high3200,
            5 => me.sr3429 && me.allow3429 && other.allow3429 && up3429,
            _ => false,
        };
        let up = (3..=5).filter(|&i| usable(i) && i1c.results[i].max_rate > 0).max_by_key(|&i| (i1c.results[i].max_rate, i))?;
        Some(Info1a {
            min_power_red: 0,
            add_power_red: 0,
            md_len: 0,
            high_carrier: false,
            preemph: 0,
            max_rate: 0,
            sr_a2c: up as u8,
            sr_c2a: 6,
            freq_offset: self.freq_offset_field(),
            uinfo: Some(crate::phase34::v90::UINFO),
        })
    }

    /// Décision de l'appelé (§10.1.2.3.5) : vitesses de symbole des deux sens.
    fn decide_info1a(&mut self) {
        if self.v90 && !self.v90_refused {
            if let Some(i1a) = self.decide_info1a_v90() {
                let up = i1a.sr_a2c as usize;
                let i1c = self.info1c.clone().unwrap();
                self.log(LogLevel::Info, &format!("Décision : 56k (V.90) — descendant en PCM, montant {} bauds (porteuse {:.0} Hz)", SYMBOL_RATES_NOMINAL[up], carrier_hz(up, i1c.results[up].high_carrier)));
                self.apply_symbol_rates(up, i1c.results[up].high_carrier, up, false);
                self.status.params.symbol_rate_rx = Some(8000);
                self.status.params.carrier_rx_hz = None;
                self.status.params.preemphasis_tx = Some(i1c.results[up].preemph);
                let mut itx = InfoTx::new(true);
                itx.queue(&self.info_out(i1a.to_bits()));
                self.set_tx(TxMode::Info(itx), "INFO1a");
                self.info1a = Some(i1a);
                self.goto(St::A2Info1a);
                return;
            }
        }
        let me = self.info0_local.clone().unwrap();
        let other = self.info0_remote.clone().unwrap();
        let i1c = self.info1c.clone().unwrap();
        let mine = self.probe_report.as_ref().unwrap().results;
        // Vitesses utilisables par un émetteur (tx) vers un récepteur (rx)
        let usable = |i: usize, tx: &Info0, rx: &Info0| -> bool {
            match i {
                0 => true,
                1 => tx.sr2743 && rx.sr2743,
                2 => tx.sr2800 && rx.sr2800,
                3 => tx.low3000 || tx.high3000,
                4 => tx.low3200 || tx.high3200,
                5 => tx.sr3429 && rx.sr3429 && tx.allow3429 && rx.allow3429,
                _ => false,
            }
        };
        let max_asym = me.max_asym.min(other.max_asym) as i32;
        let mut best = (0usize, 0usize, 0i32);
        for a2c in 0..6 {
            if !usable(a2c, &me, &other) || i1c.results[a2c].max_rate == 0 {
                continue;
            }
            for c2a in 0..6 {
                if !usable(c2a, &other, &me) || mine[c2a].max_rate == 0 || (a2c as i32 - c2a as i32).abs() > max_asym {
                    continue;
                }
                let score = i1c.results[a2c].max_rate as i32 + mine[c2a].max_rate as i32;
                let tie = score == best.2 && (a2c + c2a) > (best.0 + best.1);
                if score > best.2 || tie {
                    best = (a2c, c2a, score);
                }
            }
        }
        let (a2c, c2a, _) = best;
        let i1a = Info1a {
            min_power_red: 0,
            add_power_red: 0,
            md_len: 0,
            high_carrier: mine[c2a].high_carrier,
            preemph: mine[c2a].preemph,
            max_rate: mine[c2a].max_rate,
            sr_a2c: a2c as u8,
            sr_c2a: c2a as u8,
            freq_offset: self.freq_offset_field(),
            uinfo: None,
        };
        self.log(
            LogLevel::Info,
            &format!(
                "Décision : appelé→appelant {} bauds (porteuse {:.0} Hz), appelant→appelé {} bauds (porteuse {:.0} Hz)",
                SYMBOL_RATES_NOMINAL[a2c],
                carrier_hz(a2c, i1c.results[a2c].high_carrier),
                SYMBOL_RATES_NOMINAL[c2a],
                carrier_hz(c2a, i1a.high_carrier)
            ),
        );
        self.apply_symbol_rates(a2c, i1c.results[a2c].high_carrier, c2a, i1a.high_carrier);
        self.status.params.preemphasis_tx = Some(i1c.results[a2c].preemph);
        let mut itx = InfoTx::new(true);
        itx.queue(&self.info_out(i1a.to_bits()));
        self.set_tx(TxMode::Info(itx), "INFO1a");
        self.info1a = Some(i1a);
        self.goto(St::A2Info1a);
    }
}
