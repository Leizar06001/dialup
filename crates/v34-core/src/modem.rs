//! Le modem : machine d'états V.8 / V.34 cadencée à l'échantillon.
//!
//! À chaque échantillon : les détecteurs actifs analysent le signal reçu,
//! la machine d'états réagit (avec des instants précis à 125 µs près),
//! puis l'émetteur produit l'échantillon émis.

use crate::api::*;
use crate::dsp::fsk::{FskDemod, V21Channel};
use crate::dsp::tone::{PowerMeter, ToneDetector};
use crate::info::{Info0, Info1a, Info1c, InfoRx};
use crate::probe::{ProbeAnalyzer, ProbeReport};
use crate::tx::{FskTx, Transmitter, TxMode};
use crate::v8::{AnsamDetector, AnsamGen, Menu, V8Parser, V8Rx, PCM_ANALOG, PCM_DIGITAL};

pub(crate) const MS: u64 = 8; // échantillons par milliseconde

/// Amplitude crête minimale d'un ton détecté : −43 dBm ≈ nominal − 33 dB.
pub(crate) const TONE_MIN_AMP: f32 = NOMINAL_RMS * std::f32::consts::SQRT_2 * 0.0224;

/// États détaillés de la machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum St {
    Idle,
    // --- Phase 1 (V.8)
    CallWaitAnsam,
    CallTe,
    CallCm,
    CallCj,
    AnsSilence,
    AnsAnsam,
    AnsJm,
    P1End,
    // --- Phase 2 appelant
    C2Info0,
    C2WaitRev1,
    C2Rev1Sched,
    C2WaitRev2,
    C2RxProbe,
    C2WaitA2,
    C2Rev2Sched,
    C2L1,
    C2L2,
    C2Info1c,
    C2WaitInfo1a,
    // --- Phase 2 appelé
    A2Info0,
    A2WaitBRev,
    A2Rev2Sched,
    A2L1,
    A2L2,
    A2ToneA,
    A2WaitBRev2,
    A2RxProbe,
    A2WaitInfo1c,
    A2Info1a,
    // --- Réentraînement (§11.5) : silence 70 ms puis ton A/B
    RtSilence,
    RtWaitTone,
    // --- Répétition des INFO0 avec acquittement (§11.2.2, §11.8)
    X2Info0Rep,
    // --- Phases 3, 4, données (voir phase34.rs)
    P3,
    Failed,
}

/// Ce que les détecteurs ont vu sur l'échantillon courant.
#[derive(Default)]
pub(crate) struct RxFlags {
    pub ansam: bool,
    pub v8: Option<V8Rx>,
    pub info: Option<Result<Vec<bool>, ()>>,
    pub reversal: Option<u64>,
}

pub struct Modem {
    pub(crate) role: Role,
    pub(crate) cfg: ModemConfig,
    pub(crate) status: ModemStatus,
    pub(crate) events: Vec<ModemEvent>,
    /// Compteur d'échantillons (base de temps commune émission/réception)
    pub(crate) n: u64,
    pub(crate) st: St,
    /// Instant d'entrée dans l'état courant
    pub(crate) t_st: u64,
    /// Instant générique (échéance, inversion programmée…)
    pub(crate) t_mark: u64,
    pub(crate) tx: Transmitter,

    // Détecteurs (présents seulement quand utiles)
    pub(crate) ansam_det: Option<AnsamDetector>,
    pub(crate) fsk_rx: Option<(FskDemod, V8Parser)>,
    pub(crate) info_rx: Option<InfoRx>,
    /// Détecteur d'inversions de phase (fenêtre courte, 5 ms)
    pub(crate) tone_rx: Option<ToneDetector>,
    /// Détecteur de présence du même ton (fenêtre 20 ms : ses zéros tombent
    /// sur tous les tons de L1/L2, donc insensible à l'écho du sondage)
    pub(crate) tone_presence: Option<ToneDetector>,
    pub(crate) power: PowerMeter,
    /// Durée de présence continue du ton surveillé (échantillons)
    pub(crate) tone_on: u64,
    pub(crate) probe: Option<ProbeAnalyzer>,

    // Résultats de négociation
    pub(crate) remote_menu: Option<Menu>,
    pub(crate) info0_local: Option<Info0>,
    pub(crate) info0_remote: Option<Info0>,
    pub(crate) rtd: Option<u64>,
    pub(crate) probe_report: Option<ProbeReport>,
    pub(crate) info1c: Option<Info1c>,
    pub(crate) info1a: Option<Info1a>,
    pub(crate) p34: Option<Box<crate::phase34::P34>>,
    /// Annuleur d'écho (notre émission qui revient dans notre récepteur)
    pub(crate) ec: crate::dsp::echo::EchoCanceller,
    /// Couche liaison conservée pendant un réentraînement
    pub(crate) saved_link: Option<crate::lapm::Lapm>,
    /// Détecteur du ton de réentraînement de l'autre (A si on appelle, B sinon)
    pub(crate) retrain_det: Option<ToneDetector>,
    pub(crate) retrain_on: u64,
    pub(crate) rt_initiator: bool,
    /// Reprises automatiques déjà tentées depuis la dernière connexion
    pub(crate) recoveries: u32,
    /// INFO0 reçus de l'autre pendant la phase en cours
    pub(crate) info0_count: u32,
    /// Durée du dernier « ton pur » avant sa rupture, et instant de la rupture
    pub(crate) tone_run_prev: u64,
    pub(crate) tone_drop_at: u64,
    /// Amplitude du ton surveillé quand il est pur (référence)
    pub(crate) tone_amp_ref: f32,
    /// Inversion en attente de confirmation : (instant, échéance, amplitude)
    pub(crate) pending_rev: Option<(u64, u64, f32)>,
    /// 56k (V.90) convenu en V.8 : nous sommes le modem analogique
    /// (appelant) ou numérique (appelé). Les rôles des phases 2 à 4 sont
    /// alors inversés par rapport au V.34 (voir [`Modem::p2_answer`]).
    pub(crate) v90: bool,
    /// Le 56k a échoué sur cet appel : on se contente du V.34
    pub(crate) v90_refused: bool,
}

impl Modem {
    pub fn new(role: Role, cfg: ModemConfig) -> Self {
        let status = ModemStatus {
            role,
            phase: Phase::Idle,
            leds: Leds { mr: true, tr: true, aa: role == Role::Answer, ..Leds::default() },
            tx_signal: String::new(),
            rx_signal: String::new(),
            params: LinkParams::default(),
            constellation: Vec::new(),
            bytes_tx: 0,
            bytes_rx: 0,
            link: None,
        };
        Self {
            role,
            cfg,
            status,
            events: Vec::new(),
            n: 0,
            st: St::Idle,
            t_st: 0,
            t_mark: 0,
            tx: Transmitter::default(),
            ansam_det: None,
            fsk_rx: None,
            info_rx: None,
            tone_rx: None,
            tone_presence: None,
            power: PowerMeter::new(40),
            tone_on: 0,
            probe: None,
            remote_menu: None,
            info0_local: None,
            info0_remote: None,
            rtd: None,
            probe_report: None,
            info1c: None,
            info1a: None,
            p34: None,
            ec: crate::dsp::echo::EchoCanceller::new(),
            saved_link: None,
            retrain_det: None,
            retrain_on: 0,
            rt_initiator: false,
            recoveries: 0,
            info0_count: 0,
            tone_run_prev: 0,
            tone_drop_at: 0,
            tone_amp_ref: 0.0,
            pending_rev: None,
            v90: false,
            v90_refused: false,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// Rôle tenu en phases 2 à 4 : celui de V.8, sauf en V.90 où le modem
    /// analogique (l'appelant) se comporte comme l'appelé du V.34 et le
    /// numérique comme l'appelant (§9.2.1.1.8 et §9.2.2.1.9/V.90).
    pub(crate) fn p2_answer(&self) -> bool {
        (self.role == Role::Answer) != self.v90
    }

    pub fn command(&mut self, cmd: ModemCommand) {
        match cmd {
            ModemCommand::Dial => {
                self.reset_link();
                self.role = Role::Call;
                self.status.role = Role::Call;
                self.status.leds.oh = true;
                self.set_phase(Phase::Phase1);
                self.log(LogLevel::Info, "Décroché — attente de la tonalité de réponse (ANSam)");
                self.ansam_det = Some(AnsamDetector::new());
                self.goto(St::CallWaitAnsam);
                self.set_tx(TxMode::Silence, "silence");
            }
            ModemCommand::Answer => {
                self.reset_link();
                self.role = Role::Answer;
                self.status.role = Role::Answer;
                self.status.leds.oh = true;
                self.set_phase(Phase::Phase1);
                self.log(LogLevel::Info, "Décroché (réponse) — silence ≥ 200 ms puis ANSam");
                self.t_mark = self.n + self.cfg.answer_silence_ms.max(200) as u64 * MS;
                self.goto(St::AnsSilence);
                self.set_tx(TxMode::Silence, "silence");
            }
            ModemCommand::Hangup => {
                if self.st == St::P3 {
                    if let Some(p) = self.p34.as_mut() {
                        if p.request_cleardown() {
                            return;
                        }
                    }
                }
                self.hangup("raccroché");
            }
            ModemCommand::Send(bytes) => {
                self.status.bytes_tx += bytes.len() as u64;
                if let Some(p) = self.p34.as_mut() {
                    p.send(&bytes);
                } else if let Some(l) = self.saved_link.as_mut() {
                    // Pendant un réentraînement : mis en file, partira après
                    l.send(&bytes);
                }
            }
            ModemCommand::Retrain => {
                if self.st == St::P3 {
                    self.initiate_retrain("demandé par l'utilisateur", true);
                }
            }
            ModemCommand::Renegotiate => {
                if let Some(p) = self.p34.as_mut() {
                    p.request_renegotiation();
                }
            }
        }
    }

    /// Traite un bloc audio : `rx` = ce qui arrive de la ligne,
    /// `tx` = ce que le modem émet (même longueur).
    pub fn process(&mut self, rx: &[f32], tx: &mut [f32]) {
        for (r, t) in rx.iter().zip(tx.iter_mut()) {
            *t = self.step(*r);
        }
        if let Some(p) = self.p34.as_ref() {
            self.status.constellation = p.constellation();
            self.status.link = Some(p.link_stats());
        } else if let Some(l) = self.saved_link.as_ref() {
            self.status.link = Some(l.stats());
        }
    }

    pub fn drain_events(&mut self) -> Vec<ModemEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn status(&self) -> ModemStatus {
        self.status.clone()
    }

    // ------------------------------------------------------------------
    // Outils internes
    // ------------------------------------------------------------------

    fn reset_link(&mut self) {
        let role = self.role;
        let cfg = self.cfg.clone();
        let n = self.n;
        let events = std::mem::take(&mut self.events);
        *self = Modem::new(role, cfg);
        self.n = n;
        self.events = events;
    }

    pub(crate) fn hangup(&mut self, reason: &str) {
        let was_connected = self.st != St::Idle;
        self.reset_link();
        self.status.leds.oh = false;
        self.set_phase(Phase::Idle);
        self.set_tx(TxMode::Silence, "silence");
        if was_connected {
            self.events.push(ModemEvent::Disconnected { reason: reason.to_string() });
            self.log(LogLevel::Info, &format!("Raccroché : {reason}"));
        }
    }

    /// Reprise sur erreur pendant la négociation : réentraînement (ou
    /// répétition des INFO0) au lieu d'abandonner, 3 fois au plus.
    pub(crate) fn recover(&mut self, why: &str) {
        if self.recoveries >= 3 || matches!(self.status.phase, Phase::Phase1 | Phase::Idle) {
            self.fail(why);
            return;
        }
        self.recoveries += 1;
        self.log(LogLevel::Warn, &format!("Reprise {}/3 : {why}", self.recoveries));
        self.initiate_retrain(why, true);
    }

    pub(crate) fn fail(&mut self, why: &str) {
        self.log(LogLevel::Warn, &format!("Échec : {why}"));
        self.set_phase(Phase::Failed);
        self.st = St::Failed;
        self.events.push(ModemEvent::Disconnected { reason: why.to_string() });
        self.reset_link();
        self.status.leds.oh = false;
        self.status.phase = Phase::Failed;
        self.set_tx(TxMode::Silence, "silence");
    }

    pub(crate) fn goto(&mut self, st: St) {
        self.st = st;
        self.t_st = self.n;
    }

    pub(crate) fn elapsed(&self) -> u64 {
        self.n - self.t_st
    }

    pub(crate) fn set_phase(&mut self, p: Phase) {
        if self.status.phase != p {
            self.status.phase = p;
            self.events.push(ModemEvent::PhaseChanged(p));
        }
    }

    pub(crate) fn set_tx(&mut self, mode: TxMode, name: &str) {
        self.tx.mode = mode;
        self.tx_name(name);
    }

    pub(crate) fn tx_name(&mut self, name: &str) {
        if self.status.tx_signal != name {
            self.status.tx_signal = name.to_string();
            self.events.push(ModemEvent::TxSignal(name.to_string()));
        }
    }

    pub(crate) fn rx_name(&mut self, name: &str) {
        if self.status.rx_signal != name {
            self.status.rx_signal = name.to_string();
            self.events.push(ModemEvent::RxSignal(name.to_string()));
        }
    }

    pub(crate) fn log(&mut self, level: LogLevel, msg: &str) {
        self.events.push(ModemEvent::Log { level, msg: msg.to_string() });
    }

    pub(crate) fn params_changed(&mut self) {
        self.events.push(ModemEvent::ParamsUpdated(self.status.params.clone()));
    }

    pub(crate) fn ms(&self, samples: u64) -> f32 {
        samples as f32 / MS as f32
    }

    /// Une inversion de phase détectée est-elle crédible ? Elle doit
    /// interrompre un ton PUR présent depuis au moins 30 ms (une modulation
    /// DPSK, elle, produit aussi des sauts de phase, mais jamais sur un ton
    /// stable aussi long).
    pub(crate) fn credible_reversal(&self) -> bool {
        self.tone_on >= 30 * MS || (self.tone_run_prev >= 30 * MS && self.n.saturating_sub(self.tone_drop_at) < 40 * MS)
    }

    /// Inversion de phase CONFIRMÉE : crédible, puis le même ton, pur, juste
    /// après (une vraie inversion A→Ā ou B→B̄ continue le ton ; le
    /// début d'une modulation DPSK, non). Renvoie l'instant de l'inversion.
    pub(crate) fn confirmed_reversal(&mut self, f: &RxFlags) -> Option<u64> {
        if let Some(t) = f.reversal {
            if self.credible_reversal() && self.pending_rev.is_none() {
                // Le ton ne dure que 10 ms après l'inversion (§11.2.1) : on
                // vérifie à 9 ms, quand la fenêtre de 5 ms ne contient plus
                // que le ton inversé.
                self.pending_rev = Some((t, t + 9 * MS, self.tone_amp_ref));
            }
        }
        let (t, at, amp) = self.pending_rev?;
        if self.n < at {
            return None;
        }
        self.pending_rev = None;
        let ok = self.tone_rx.as_ref().map(|d| d.amplitude() >= 0.6 * amp && d.stable()).unwrap_or(false);
        if ok {
            Some(t)
        } else {
            self.log(LogLevel::Detail, "Fausse inversion de phase ignorée (début de modulation, pas un ton)");
            None
        }
    }

    /// Ton surveillé présent depuis au moins `ms` millisecondes ?
    pub(crate) fn tone_for(&self, ms: u64) -> bool {
        self.tone_on >= ms * MS
    }

    // ------------------------------------------------------------------
    // Boucle principale
    // ------------------------------------------------------------------

    fn step(&mut self, x_line: f32) -> f32 {
        // Annuleur d'écho : il s'entraîne pendant nos signaux large bande
        // émis quand l'autre se tait (L1/L2 en phase 2, S/PP/TRN en phase 3).
        // (Pas en phase 2 : L1/L2 est périodique, l'annuleur n'y apprendrait
        // l'écho qu'aux 21 fréquences du sondage — V.34 l'entraîne en phase 3.)
        self.ec.mu = match self.st {
            St::P3 => self.p34.as_ref().map(|p| p.ec_mu()).unwrap_or(0.0),
            _ => 0.0,
        };
        let x = self.ec.process(x_line);
        let mut f = RxFlags::default();
        let p_tot = self.power.push(x);
        if let Some(d) = self.ansam_det.as_mut() {
            f.ansam = d.push(x);
        }
        if let Some((d, p)) = self.fsk_rx.as_mut() {
            if let Some(b) = d.push(x) {
                f.v8 = p.push(b);
            }
        }
        if let Some(r) = self.info_rx.as_mut() {
            f.info = r.push(x);
        }
        if let Some(t) = self.tone_rx.as_mut() {
            // Le détecteur date l'inversion dans sa propre base de temps
            // (échantillons depuis sa création) : conversion vers self.n.
            let n = self.n;
            f.reversal = t.push(x).map(|tr| n - (t.samples() - 1 - tr));
            // Présence du ton : niveau > −43 dBm (≈ 33 dB sous le nominal,
            // seuil du circuit 109 de V.34) ET phase stable. On ne compare
            // pas à la puissance totale : l'écho de notre propre émission
            // peut être plus fort que le signal distant sur une longue ligne.
            let _ = p_tot;
        }
        if let Some(t) = self.tone_presence.as_mut() {
            t.push(x);
            let a = t.amplitude();
            if a > TONE_MIN_AMP && t.stable() {
                self.tone_on += 1;
                self.tone_amp_ref = a;
            } else {
                if self.tone_on > 0 {
                    self.tone_run_prev = self.tone_on;
                    self.tone_drop_at = self.n;
                }
                self.tone_on = 0;
            }
        }
        if let Some(p) = self.probe.as_mut() {
            p.push(x);
        }

        match self.st {
            St::Idle | St::Failed => {}
            St::CallWaitAnsam | St::CallTe | St::CallCm | St::CallCj | St::AnsSilence | St::AnsAnsam | St::AnsJm | St::P1End => self.phase1(&f),
            St::RtSilence | St::RtWaitTone => self.retrain_step(&f),
            St::P3 => {
                // Ton de réentraînement de l'autre (> 50 ms) : on répond (§11.5.x.2)
                if let Some(det) = self.retrain_det.as_mut() {
                    det.push(x);
                    let watch = self.p34.as_ref().map(|p| p.retrain_watch()).unwrap_or(false);
                    // Le ton doit DOMINER le signal reçu : une raie du signal PP
                    // peut tomber pile à 1200 Hz, mais elle n'en porte qu'1/48.
                    let a = det.amplitude();
                    if watch && a > TONE_MIN_AMP && det.stable() && a * a * 0.5 > 0.5 * p_tot {
                        self.retrain_on += 1;
                    } else {
                        self.retrain_on = 0;
                    }
                    if self.retrain_on > 50 * MS {
                        self.retrain_on = 0;
                        self.initiate_retrain("l'autre modem demande un réentraînement", false);
                        let y = self.tx.next_sample(self.n);
                        self.ec.push_tx(y);
                        self.n += 1;
                        return y;
                    }
                }
                let p = self.p34.as_mut().unwrap();
                let out = p.step(x, self.n);
                let report = std::mem::take(&mut p.ec_report);
                let actions = p.take_actions();
                if report {
                    let erle = self.ec.erle_db();
                    self.log(LogLevel::Detail, &format!("Annuleur d'écho entraîné sur notre TRN : atténuation {erle:.1} dB"));
                }
                self.apply_p34_actions(actions);
                self.ec.push_tx(out);
                self.n += 1;
                return out;
            }
            _ => self.phase2(&f),
        }

        let y = self.tx.next_sample(self.n);
        self.ec.push_tx(y);
        self.n += 1;
        y
    }

    // ------------------------------------------------------------------
    // Phase 1 : V.8
    // ------------------------------------------------------------------

    fn phase1(&mut self, f: &RxFlags) {
        match self.st {
            St::CallWaitAnsam => {
                if f.ansam {
                    self.rx_name("ANSam");
                    self.log(LogLevel::Info, "ANSam détecté (2100 Hz modulé 15 Hz, inversions de phase)");
                    // Modem 56k d'époque : il écoute l'ANSam plus longtemps
                    // avant de répondre (comme ceux de 1999), dans les 5 s
                    // que l'appelé lui accorde.
                    let te = if self.cfg.v90 && !self.cfg.express { 2300 } else { 1000 };
                    self.log(LogLevel::Detail, &format!("Silence Te = {:.1} s (neutralisation des annuleurs d'écho du réseau)", te as f32 / 1000.0));
                    self.t_mark = self.n + te * MS;
                    self.goto(St::CallTe);
                } else if self.elapsed() > 15_000 * MS {
                    self.fail("pas de réponse (aucune tonalité ANSam)");
                }
            }
            St::CallTe => {
                if self.n >= self.t_mark {
                    let menu = Menu::ours_for(&self.cfg, false);
                    let mut fsk = FskTx::new(V21Channel::Low);
                    fsk.set_repeat(menu.sequence_bits());
                    self.set_tx(TxMode::Fsk(fsk), "CM");
                    self.fsk_rx = Some((FskDemod::new(V21Channel::High), V8Parser::new()));
                    self.ansam_det = None;
                    self.log(LogLevel::Info, &format!("Envoi CM (V.21 bas, 300 bit/s) : {}", menu.modes.names().join(", ")));
                    self.goto(St::CallCm);
                }
            }
            St::CallCm => {
                if let Some(V8Rx::Confirmed(jm)) = &f.v8 {
                    self.rx_name("JM");
                    let names = jm.modes.names();
                    self.log(LogLevel::Info, &format!("JM reçu ×2 : modes communs {}", names.join(", ")));
                    self.status.params.v8_modes = names;
                    self.params_changed();
                    if !jm.modes.v34 {
                        self.fail("l'autre modem ne propose pas V.34");
                        return;
                    }
                    // §9.1.1/V.90 : 56k si nous sommes analogique et l'autre numérique
                    self.v90 = Menu::ours_for(&self.cfg, false).pcm.is_some() && jm.pcm.is_some_and(|p| p & PCM_DIGITAL != 0);
                    if self.v90 {
                        self.log(LogLevel::Info, "Le FAI est raccordé en numérique : 56k (V.90) possible, nous serons le modem analogique");
                        self.status.params.v8_modes.insert(0, "V.90".into());
                    }
                    self.remote_menu = Some(jm.clone());
                    if let TxMode::Fsk(fsk) = &mut self.tx.mode {
                        fsk.finish_octet_then(&crate::v8::cj_bits());
                    }
                    self.tx_name("CJ");
                    self.goto(St::CallCj);
                } else if self.elapsed() > 6000 * MS {
                    self.fail("pas de JM");
                }
            }
            St::CallCj => {
                let idle = matches!(&self.tx.mode, TxMode::Fsk(fsk) if fsk.idle());
                if idle {
                    self.log(LogLevel::Detail, "CJ envoyé — silence 75 ms");
                    self.set_tx(TxMode::Silence, "silence");
                    self.fsk_rx = None;
                    self.prepare_phase2_rx();
                    self.t_mark = self.n + 75 * MS;
                    self.goto(St::P1End);
                }
            }
            St::AnsSilence => {
                if self.n >= self.t_mark {
                    self.set_tx(TxMode::Ansam(AnsamGen::new()), "ANSam");
                    self.fsk_rx = Some((FskDemod::new(V21Channel::Low), V8Parser::new()));
                    self.log(LogLevel::Info, "Envoi ANSam : 2100 Hz, AM 15 Hz, inversion de phase toutes les 450 ms");
                    self.goto(St::AnsAnsam);
                }
            }
            St::AnsAnsam => {
                if let Some(V8Rx::Confirmed(cm)) = &f.v8 {
                    self.rx_name("CM");
                    let ours = Menu::ours_for(&self.cfg, true);
                    let common = cm.modes.intersect(&ours.modes);
                    self.log(LogLevel::Info, &format!("CM reçu ×2 : l'appelant propose {}{}", if cm.pcm.is_some_and(|p| p & PCM_ANALOG != 0) { "V.90, " } else { "" }, cm.modes.names().join(", ")));
                    // §9.1.1/V.90 : nous sommes numérique et l'autre analogique
                    self.v90 = ours.pcm.is_some_and(|p| p & PCM_DIGITAL != 0) && cm.pcm.is_some_and(|p| p & PCM_ANALOG != 0);
                    let jm = Menu {
                        call_function: cm.call_function,
                        modes: common,
                        lapm: cm.lapm && ours.lapm,
                        access: if self.v90 { ours.access } else { None },
                        pcm: if self.v90 { Some(PCM_DIGITAL) } else { None },
                    };
                    self.status.params.v8_modes = common.names();
                    if self.v90 {
                        self.log(LogLevel::Info, "L'abonné propose le 56k : nous serons le modem numérique (V.90)");
                        self.status.params.v8_modes.insert(0, "V.90".into());
                    }
                    self.params_changed();
                    self.remote_menu = Some(cm.clone());
                    let mut fsk = FskTx::new(V21Channel::High);
                    fsk.set_repeat(jm.sequence_bits());
                    self.set_tx(TxMode::Fsk(fsk), "JM");
                    self.log(LogLevel::Info, &format!("Envoi JM (V.21 haut) : {}", common.names().join(", ")));
                    self.goto(St::AnsJm);
                } else if self.elapsed() > 5000 * MS {
                    self.fail("pas de CM pendant ANSam (5 s)");
                }
            }
            St::AnsJm => {
                if let Some(V8Rx::Cj) = f.v8 {
                    self.rx_name("CJ");
                    self.log(LogLevel::Detail, "CJ reçu — fin de V.8, silence 75 ms");
                    self.set_tx(TxMode::Silence, "silence");
                    self.fsk_rx = None;
                    self.prepare_phase2_rx();
                    self.t_mark = self.n + 75 * MS;
                    self.goto(St::P1End);
                } else if self.elapsed() > 6000 * MS {
                    self.fail("pas de CJ");
                }
            }
            St::P1End => {
                if self.n >= self.t_mark {
                    self.start_phase2();
                }
            }
            _ => {}
        }
    }
}
