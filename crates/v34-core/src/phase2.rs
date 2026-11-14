//! Phase 2 V.34 (§11.2) : INFO0, mesure du délai aller-retour par les
//! inversions de phase des tons A/B, sondage L1/L2 dans les deux sens,
//! puis INFO1c (résultats) / INFO1a (décision).

use crate::api::*;
use crate::dsp::tone::ToneDetector;
use crate::info::{Info0, Info1a, Info1c, InfoRx, InfoTx};
use crate::modem::{Modem, RxFlags, St, MS};
use crate::probe::{carrier_hz, ProbeAnalyzer, ProbeGen, SYMBOL_RATES_NOMINAL};
use crate::tx::{ToneTx, TxMode};

/// Écart toléré à la capacité (dB) par notre récepteur (égaliseur linéaire,
/// décision symbole par symbole).
const GAP_DB: f32 = 9.0;
/// Délai entre réception d'une inversion et émission de la réponse : 40 ms.
const REV_DELAY: u64 = 40 * MS;
const L1_LEN: u64 = 160 * MS;
/// Durée d'analyse de L2 (≤ 500 ms autorisés).
const L2_ANALYSIS: u32 = 20; // blocs de 20 ms = 400 ms

impl Modem {
    pub(crate) fn start_phase2(&mut self) {
        self.set_phase(Phase::Phase2);
        let answer = self.p2_answer();
        let info0 = self.our_info0();
        self.info0_local = Some(info0.clone());
        let mut itx = InfoTx::new(answer);
        itx.queue(&info0.to_bits());
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
        self.info_rx = Some(InfoRx::new(!answer, self.remote_info0_bits()));
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

    fn info_tx_done(&self) -> bool {
        matches!(&self.tx.mode, TxMode::Info(i) if i.done())
    }

    fn got_info0(&mut self, f: &RxFlags) {
        if let Some(Ok(bits)) = &f.info {
            self.info0_count += 1;
            let ack = Info0::from_info(bits).ack;
            if self.info0_remote.is_some() && (self.info0_count >= 2 || ack) && self.st != St::X2Info0Rep {
                // INFO0 répété par l'autre : il n'a pas eu le nôtre → on
                // répète aussi, avec l'acquittement (§11.2.2.x.1).
                if !ack {
                    self.start_info0_repeat("l'autre répète son INFO0");
                }
            }
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
        matches!(&self.probe, Some(p) if p.blocks() >= L2_ANALYSIS)
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
        itx.queue(&i0.to_bits());
        self.set_tx(TxMode::Info(itx), if answer { "INFO0a ×" } else { "INFO0c ×" });
        self.t_mark = 0; // drapeau « INFO0 acquitté reçu »
        self.goto(St::X2Info0Rep);
    }

    fn info0_repeat_step(&mut self, f: &RxFlags) {
        let answer = self.p2_answer();
        if let Some(Ok(bits)) = &f.info {
            let r = Info0::from_info(bits);
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
            let mut itx = InfoTx::new(answer);
            itx.queue(&i0.to_bits());
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
                        let rtd = (t - self.t_mark).saturating_sub(REV_DELAY);
                        self.rtd = Some(rtd);
                        self.status.params.rtd_ms = Some(self.ms(rtd));
                        self.params_changed();
                        self.rx_name("A");
                        self.log(LogLevel::Info, &format!("Délai aller-retour mesuré : {:.1} ms", self.ms(rtd)));
                        // L1 commence 10 ms après l'inversion, dure 160 ms ; on
                        // analyse L2 en laissant 10 ms de marge.
                        self.start_probe_rx(t + 10 * MS + L1_LEN + 10 * MS);
                        self.rx_name("L1");
                        self.goto(St::C2RxProbe);
                    }
                } else if self.elapsed() > 2000 * MS {
                    self.recover("seconde inversion du ton A non reçue");
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
                if self.tone_for(20) || self.elapsed() > max {
                    if self.tone_for(20) {
                        self.rx_name("A");
                    }
                    let r = self.probe_report.as_ref().unwrap();
                    let i1c = Info1c { min_power_red: 0, add_power_red: 0, md_len: 0, results: r.results, freq_offset: self.freq_offset_field() };
                    let mut itx = InfoTx::new(false);
                    itx.queue(&i1c.to_bits());
                    self.set_tx(TxMode::Info(itx), "INFO1c");
                    self.log(LogLevel::Info, "Envoi INFO1c (résultats du sondage)");
                    self.info1c = Some(i1c);
                    self.info_rx = Some(InfoRx::new(true, Info1a::INFO_BITS));
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
                if self.st == St::C2WaitInfo1a && self.elapsed() > (1500 * MS + self.rtd.unwrap_or(0)) {
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
                } else if self.elapsed() > 600 * MS + self.rtd.unwrap_or(0) {
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
                    self.info_rx = Some(InfoRx::new(false, Info1c::INFO_BITS));
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
                    Some(Err(())) => self.log(LogLevel::Warn, "INFO1c reçu avec CRC faux"),
                    None => {
                        if self.elapsed() > 2000 * MS + 2 * self.rtd.unwrap_or(0) {
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
            Some(Err(())) => self.log(LogLevel::Warn, "INFO1a reçu avec CRC faux"),
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
                itx.queue(&i1a.to_bits());
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
        itx.queue(&i1a.to_bits());
        self.set_tx(TxMode::Info(itx), "INFO1a");
        self.info1a = Some(i1a);
        self.goto(St::A2Info1a);
    }
}
