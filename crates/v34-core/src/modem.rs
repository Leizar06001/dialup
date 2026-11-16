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
    /// Après la « seconde » inversion de A : son L1 doit suivre (sinon,
    /// c'était la première d'un nouveau cycle)
    C2CheckL1,
    C2RxProbe,
    C2WaitA2,
    /// Ton B coupé un instant, pour que l'appelé le voie arriver (voir
    /// `phase2.rs`, C2WaitA2)
    C2BPause,
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
    /// Phase 2 : ton B déjà relancé une fois (C2WaitA2)
    pub(crate) b_relance: bool,
    /// Phase 2 : instant de la seconde inversion de A (C2CheckL1), et
    /// puissance reçue de son ton juste après
    pub(crate) rev2_at: u64,
    pub(crate) rev2_p: f32,
    /// Puissance reçue récente (fenêtre de 5 ms)
    pub(crate) p_now: f32,
    /// Trou du réseau en cours (zéros exacts d'affilée) et instant du dernier
    pub(crate) zero_run: u32,
    pub(crate) hole_at: Option<u64>,
    pub(crate) probe_report: Option<ProbeReport>,
    pub(crate) info1c: Option<Info1c>,
    pub(crate) info1a: Option<Info1a>,
    pub(crate) p34: Option<Box<crate::phase34::P34>>,
    /// Annuleur d'écho (notre émission qui revient dans notre récepteur)
    pub(crate) ec: crate::dsp::echo::EchoCanceller,
    /// Écho lointain (à travers un adaptateur téléphonique : un aller-retour
    /// Internet plus tard), entraîné avec `ec` sur la même erreur
    pub(crate) far_ec: crate::dsp::echo::FarEchoCanceller,
    far_training: bool,
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
    /// Nos séquences portent leur CRC dans l'ancien ordre (voir `CrcMode`)
    pub(crate) crc_legacy: bool,
    /// Ordre du CRC du dernier INFO0 reçu de l'autre
    pub(crate) crc_seen: Option<crate::info::CrcSeen>,
    /// Un INFO1 de l'autre est arrivé abîmé (CRC faux) : il en émet, il le
    /// répétera, on l'attend plus longtemps
    pub(crate) info1_bad: bool,
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
    /// Télécopie en cours (à la place de la liaison de données)
    pub(crate) fax: Option<Box<crate::fax::session::Fax>>,
    /// Page préparée : le prochain Dial l'envoie par fax
    pub(crate) fax_page: Option<(Vec<Vec<bool>>, bool)>,
    /// Appelé : écoute du CNG (1100 Hz) d'un télécopieur qui appelle
    pub(crate) cng: Option<(ToneDetector, PowerMeter, u64)>,
    /// Minitel (V.23) en cours, et l'oreille qui reconnaît son sifflement
    pub(crate) minitel: Option<Box<crate::v23::Minitel>>,
    pub(crate) ecoute_v23: Option<crate::v23::EcouteMinitel>,
    /// Morse convenu en V.8 (services non normalisés), vitesse en mots/minute
    pub(crate) morse_convenu: Option<u8>,
    /// Liaison morse en cours (à la place des phases 2 à 4 du V.34)
    pub(crate) morse: Option<Box<crate::morse::session::Morse>>,
}

impl Modem {
    pub fn new(role: Role, cfg: ModemConfig) -> Self {
        // Sans rien savoir de l'autre : l'ancien ordre de CRC (nos modems
        // d'avant restent compris), sauf si l'on imite un vrai modem.
        let crc_legacy_initial = cfg.crc_mode != crate::api::CrcMode::Standard;
        let status = ModemStatus {
            role,
            phase: Phase::Idle,
            leds: Leds { mr: true, tr: true, aa: role == Role::Answer, ..Leds::default() },
            tx_signal: String::new(),
            rx_signal: String::new(),
            params: LinkParams::default(),
            constellation: Vec::new(),
            pcm: None,
            bytes_tx: 0,
            bytes_rx: 0,
            link: None,
            morse: None,
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
            b_relance: false,
            rev2_at: 0,
            rev2_p: 0.0,
            p_now: 0.0,
            zero_run: 0,
            hole_at: None,
            probe_report: None,
            info1c: None,
            info1a: None,
            p34: None,
            ec: crate::dsp::echo::EchoCanceller::new(),
            far_ec: crate::dsp::echo::FarEchoCanceller::new(),
            far_training: false,
            saved_link: None,
            retrain_det: None,
            retrain_on: 0,
            rt_initiator: false,
            recoveries: 0,
            info0_count: 0,
            crc_legacy: crc_legacy_initial,
            crc_seen: None,
            info1_bad: false,
            tone_run_prev: 0,
            tone_drop_at: 0,
            tone_amp_ref: 0.0,
            pending_rev: None,
            v90: false,
            v90_refused: false,
            fax: None,
            fax_page: None,
            cng: None,
            minitel: None,
            ecoute_v23: None,
            morse_convenu: None,
            morse: None,
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
        let Some(cmd) = self.commande_morse(cmd) else { return };
        match cmd {
            ModemCommand::PreparerFax { lignes, fine } => {
                self.fax_page = Some((lignes, fine));
            }
            ModemCommand::Dial if self.fax_page.is_some() => {
                let (lignes, fine) = self.fax_page.take().unwrap_or_default();
                self.reset_link();
                self.role = Role::Call;
                self.status.role = Role::Call;
                self.status.leds.oh = true;
                self.set_phase(Phase::Fax);
                self.log(LogLevel::Info, &format!("Télécopie : {} lignes à envoyer — CNG, en attente du télécopieur", lignes.len()));
                self.fax = Some(Box::new(crate::fax::session::Fax::emetteur(&lignes, &self.cfg.ident, fine)));
            }
            ModemCommand::Answer if self.cfg.minitel => self.repondre_minitel(),
            ModemCommand::Hangup | ModemCommand::Send(_) if self.minitel.is_some() => self.commande_minitel(cmd),
            ModemCommand::Dial => {
                self.reset_link();
                self.role = Role::Call;
                self.status.role = Role::Call;
                self.status.leds.oh = true;
                self.set_phase(Phase::Phase1);
                self.log(LogLevel::Info, "Décroché — attente de la tonalité de réponse (ANSam)");
                self.ansam_det = Some(AnsamDetector::new());
                // (ou le sifflement d'un serveur Minitel)
                self.ecoute_v23 = Some(crate::v23::EcouteMinitel::new());
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
                // Un télécopieur qui appelle se reconnaît à son CNG (1100 Hz pur)
                self.cng = Some((ToneDetector::new(1100.0, 320), PowerMeter::new(320), 0));
            }
            ModemCommand::Hangup if self.fax.is_some() => {
                self.fin_fax("raccroché");
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
        if self.fax.is_some() {
            return self.process_fax(rx, tx);
        }
        if self.minitel.is_some() {
            return self.process_minitel(rx, tx);
        }
        if self.morse.is_some() {
            return self.process_morse(rx, tx);
        }
        for (i, (r, t)) in rx.iter().zip(tx.iter_mut()).enumerate() {
            *t = self.step(*r);
            if self.morse.is_some() {
                // V.8 vient de finir : le reste du bloc part en morse
                return self.process_morse(&rx[i + 1..], &mut tx[i + 1..]);
            }
            if self.ecouter_cng(*r) {
                // Le reste du bloc part en télécopie
                return self.process_fax(&rx[i + 1..], &mut tx[i + 1..]);
            }
            if self.ecouter_minitel(*r) {
                // … ou en Minitel
                return self.process_minitel(&rx[i + 1..], &mut tx[i + 1..]);
            }
        }
        if let Some(p) = self.p34.as_ref() {
            self.status.constellation = p.constellation();
            self.status.pcm = p.pcm_view();
            self.status.link = Some(p.link_stats());
        } else if let Some(l) = self.saved_link.as_ref() {
            self.status.link = Some(l.stats());
        }
    }

    /// Appelé, avant la fin de la négociation V.8 : un CNG (1100 Hz pur
    /// pendant 300 ms) ? Alors c'est un télécopieur : on passe en réception
    /// de fax (notre ANSam a servi de CED).
    fn ecouter_cng(&mut self, r: f32) -> bool {
        let Some((det, pm, duree)) = self.cng.as_mut() else { return false };
        if !matches!(self.status.phase, Phase::Phase1) {
            self.cng = None;
            return false;
        }
        det.push(r);
        let p = pm.push(r);
        // ton pur : presque toute la puissance reçue est à 1100 Hz (le CM
        // d'un modem de données, à 980/1180 Hz, ne passe pas)
        let pur = det.amplitude() > TONE_MIN_AMP && det.amplitude() * det.amplitude() > 1.4 * p;
        *duree = if pur { *duree + 1 } else { 0 };
        if *duree < 300 * MS {
            return false;
        }
        self.cng = None;
        self.reset_link();
        self.role = Role::Answer;
        self.status.role = Role::Answer;
        self.status.leds.oh = true;
        self.set_phase(Phase::Fax);
        self.log(LogLevel::Info, "CNG entendu : c'est un télécopieur, réception d'une télécopie");
        self.fax = Some(Box::new(crate::fax::session::Fax::recepteur(&self.cfg.ident, true)));
        true
    }

    fn process_fax(&mut self, rx: &[f32], tx: &mut [f32]) {
        let Some(f) = self.fax.as_mut() else { return };
        f.process(rx, tx);
        self.n += rx.len() as u64;
        let evs = f.evenements();
        let (emis, recu) = (f.emission(), f.reception());
        let fini = f.fini();
        let emet = tx.iter().any(|x| x.abs() > 1e-6);
        self.status.leds.sd = emet;
        self.status.leds.rd = !emet;
        self.status.leds.cd = true;
        // (karaoké, afficheur TX/RX : ce qui part et ce qui arrive vraiment)
        if self.status.tx_signal != emis {
            self.status.tx_signal = emis.to_string();
        }
        if self.status.rx_signal != recu {
            self.status.rx_signal = recu.to_string();
        }
        for e in evs {
            if let crate::fax::session::FaxEvenement::Fin { ok, raison } = &e {
                self.log(if *ok { LogLevel::Info } else { LogLevel::Warn }, &format!("Télécopie : {raison}"));
            }
            self.events.push(ModemEvent::Fax(e));
        }
        if fini {
            self.fin_fax("télécopie terminée");
        }
    }

    /// Fin d'une télécopie : on raccroche, et on le dit (le lien réseau
    /// libère alors l'appel au central).
    fn fin_fax(&mut self, raison: &str) {
        if let Some(mut f) = self.fax.take() {
            // coupé avant la fin de la procédure : on dit où on en était
            f.couper();
            for e in f.evenements() {
                self.events.push(ModemEvent::Fax(e));
            }
        }
        self.hangup(raison);
        // (hors négociation de données, `hangup` ne prévient pas)
        if !self.events.iter().any(|e| matches!(e, ModemEvent::Disconnected { .. })) {
            self.events.push(ModemEvent::Disconnected { reason: raison.to_string() });
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

    pub(crate) fn reset_link(&mut self) {
        let role = self.role;
        let cfg = self.cfg.clone();
        let n = self.n;
        let events = std::mem::take(&mut self.events);
        let fax_page = self.fax_page.take();
        *self = Modem::new(role, cfg);
        self.n = n;
        self.events = events;
        self.fax_page = fax_page;
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
            // juste après un trou du réseau, le ton qui reprend n'a pas la
            // phase que le détecteur attendait : ce n'est pas une inversion
            // (sinon : délai aller-retour faux, 15 ms ou 1,9 s)
            if self.hole_at.is_some_and(|h| self.n.saturating_sub(h) < 60 * MS) {
                self.log(LogLevel::Detail, "Inversion ignorée : trou du réseau juste avant");
                return None;
            }
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

    /// Notre échantillon émis, pour les deux annuleurs d'écho.
    #[inline]
    fn push_tx_ec(&mut self, y: f32) {
        self.ec.push_tx(y);
        self.far_ec.push_tx(y, self.ec.mu);
    }

    fn step(&mut self, x_line: f32) -> f32 {
        // Annuleur d'écho : il s'entraîne pendant nos signaux large bande
        // émis quand l'autre se tait (L1/L2 en phase 2, S/PP/TRN en phase 3).
        // (Pas en phase 2 : L1/L2 est périodique, l'annuleur n'y apprendrait
        // l'écho qu'aux 21 fréquences du sondage — V.34 l'entraîne en phase 3.)
        let mu = match self.st {
            St::P3 => self.p34.as_ref().map(|p| p.ec_mu()).unwrap_or(0.0),
            _ => 0.0,
        };
        self.ec.mu = mu;
        // l'écho lointain apprend un aller-retour après l'écho proche
        let mu_far = self.far_ec.follow_delayed_mu();
        // écho proche et écho lointain, retirés ensemble et appris sur la
        // même erreur. Le lointain est normalisé par la puissance des deux
        // fenêtres (comme un seul filtre : réglage validé avec le vrai
        // modem). Le proche, par la sienne seulement : normalisé par la
        // somme, il apprenait deux fois moins vite dès que l'apprentissage
        // du lointain chevauchait le sien (aller-retour plus court que notre
        // TRN), et finissait 5 dB moins bon sur une ligne sans écho lointain.
        //
        // Un trou du réseau (trame perdue, coupure) arrive en zéros exacts,
        // ce qu'un signal reçu ne donne pour ainsi dire jamais (la loi A n'a
        // même pas de zéro) : rien à y apprendre ni à mesurer. Sinon une
        // coupure pendant l'apprentissage du lointain lui donnait une
        // atténuation négative, et on le coupait à tort.
        let x_near = x_line - self.ec.estimate();
        let x = x_near - self.far_ec.estimate();
        if (mu > 0.0 || mu_far > 0.0) && x_line != 0.0 {
            if mu > 0.0 {
                self.ec.adapt(x, self.ec.power());
            }
            if mu_far > 0.0 {
                let norm = if mu > 0.0 { self.ec.power() } else { 0.0 } + self.far_ec.power();
                self.far_ec.adapt(x, norm);
            }
            self.ec.note(x_line, x);
            if mu_far > 0.0 {
                self.far_ec.note(x_near, x);
            }
        }
        if mu_far > 0.0 {
            self.far_training = true;
        } else if std::mem::take(&mut self.far_training) {
            let erle = self.far_ec.erle_db();
            if erle < crate::dsp::echo::FAR_MIN_ERLE_DB {
                self.far_ec.disable();
                self.log(LogLevel::Detail, &format!("Pas d'écho lointain (atténuation {erle:.1} dB) : annuleur lointain coupé"));
            } else {
                self.log(LogLevel::Detail, &format!("Annuleur d'écho lointain entraîné (un aller-retour après le nôtre) : atténuation {erle:.1} dB"));
            }
        }
        let mut f = RxFlags::default();
        let p_tot = self.power.push(x);
        self.p_now = p_tot;
        // Trou du réseau : une trame perdue arrive en zéros exacts (un
        // signal reçu n'en donne jamais 2 ms d'affilée)
        if x_line == 0.0 {
            self.zero_run += 1;
            if self.zero_run >= 2 * MS as u32 {
                self.hole_at = Some(self.n);
            }
        } else {
            self.zero_run = 0;
        }
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
                        self.push_tx_ec(y);
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
                self.push_tx_ec(out);
                self.n += 1;
                return out;
            }
            _ => self.phase2(&f),
        }

        let y = self.tx.next_sample(self.n);
        self.push_tx_ec(y);
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
                    // (morse : le CM est plus long, on ne traîne pas)
                    let te = if self.cfg.v90 && !self.cfg.express && !self.cfg.morse { 2300 } else { 1000 };
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
                    if let Some(mpm) = menu.morse() {
                        self.log(LogLevel::Info, &format!("CM : service non normalisé (V.8 §6.6) France, fournisseur « TV » (TINYVERSE) : on propose le morse à {mpm} mots/minute"));
                    }
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
                    self.morse_dans_jm(jm);
                    self.remote_menu = Some(jm.clone());
                    // (morse : CJ trois fois, un clic d'orage n'en mange qu'un)
                    let cj = crate::v8::cj_bits().repeat(if self.morse_convenu.is_some() { 3 } else { 1 });
                    if let TxMode::Fsk(fsk) = &mut self.tx.mode {
                        fsk.finish_octet_then(&cj);
                    }
                    self.tx_name("CJ");
                    self.goto(St::CallCj);
                } else if self.elapsed() > if self.cfg.morse { 10_000 } else { 6000 } * MS {
                    // (un CM et un JM qui parlent morse durent une seconde chacun)
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
                    self.status.params.v8_modes = common.names();
                    // morse proposé (service non normalisé) : accepté si DIP 4
                    let nsf = self.morse_pour_jm(cm);
                    let jm = Menu {
                        call_function: cm.call_function,
                        modes: common,
                        lapm: cm.lapm && ours.lapm,
                        access: if self.v90 { ours.access } else { None },
                        pcm: if self.v90 { Some(PCM_DIGITAL) } else { None },
                        nsf,
                    };
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
                } else if self.elapsed() > if self.cfg.morse { 6000 } else { 5000 } * MS {
                    // (5 ± 1 s, §8.2.2/V.8 : un CM qui propose le morse est long)
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
                    match self.morse_convenu {
                        Some(mpm) => self.demarrer_morse(mpm),
                        None => self.start_phase2(),
                    }
                }
            }
            _ => {}
        }
    }
}
