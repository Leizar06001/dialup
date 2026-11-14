//! Phases 3 et 4 du V.90 (§9.3, §9.4/V.90), puis mode données 56k.
//!
//! Le modem analogique (l'abonné, rôle « appelé » du V.34) émet en V.34 :
//! S, S̄, PP, TRN, puis Ja (descripteur du DIL), S, S̄, CPt, CP, E, B1 et
//! données ; il reçoit en PCM ([`PcmRx`]). Le modem numérique (le FAI,
//! rôle « appelant ») reçoit en V.34 (égaliseur appris sur PP/TRN) et
//! émet en PCM ([`PcmTx`]) : Sd, TRN1d, Jd, DIL, Ri, TRN2d, MP, Ed, B1d.
//!
//! ```text
//! abonné  S S̄ PP TRN Ja Ja…   ·  S………… S̄  ·(DIL)·  S S̄ CPt CPt…  CP CP′ E B1 données
//! FAI      ·  ·  ·  ·  ·  Sd S̄d TRN1d Jd Jd… J′d DIL DIL… Ri Ri… R̄i TRN2d MP MP′ Ed B1d données
//! ```

use super::*;
use crate::info::Info0d;
use crate::v90::design::{self, power_limit, Channel, Limits};
use crate::v90::pcmrx::{PcmRx, RxEvent};
use crate::v90::pcmtx::{PcmTx, TxEvent};
use crate::v90::seq::{up_mask_upto, Cp, DilDescriptor, Jd, Mp90};
use crate::v90::{Law, Mapping};

/// UINFO demandé par l'abonné : Ucode du point de l'apprentissage à deux
/// niveaux (> 66, §8.2.3.2).
pub(crate) const UINFO: u8 = 96;
/// Silence d'une séquence de l'autre au-delà duquel on abandonne (ms).
const STALL_MS: u64 = 9000;
/// Écart entre niveaux voisins des données, en multiples du bruit mesuré
/// sur le DIL. La ligne (coupée sous 200 Hz et vers 3,7 kHz) renvoie près
/// de 90 % d'un symbole deux échantillons plus tard : une seule erreur de
/// décision peut se recopier de proche en proche dans l'égaliseur et la
/// réception décroche. À 9 σ, une erreur toutes les vingt secondes environ,
/// une perte toutes les 4 minutes en ligne urbaine ; à 11 σ, moins d'une
/// par heure, pour un cran de débit en moins (40 000 au lieu de 41 333).
const MARGE_DONNEES: f64 = 11.0;

/// Motif pseudo-aléatoire de `n` bits (registre à décalage, polynôme
/// x^7 + x^6 + 1), à peu près autant de 1 que de 0.
fn motif(n: usize, graine: u8) -> Vec<bool> {
    let mut r = graine | 1;
    (0..n)
        .map(|_| {
            let b = ((r >> 6) ^ (r >> 5)) & 1;
            r = ((r << 1) | b) & 0x7F;
            b == 1
        })
        .collect()
}

/// Le DIL que l'abonné demande, à la manière des modems de 1999 (voir
/// l'enregistrement d'une vraie connexion) : des motifs de signes et de
/// références de 120 symboles, répétés dans chaque segment de 720 symboles
/// — le signal est périodique, on entend un « peigne » de raies espacées
/// de 67 Hz, ce bourdonnement caractéristique de la fin d'une connexion
/// 56k. Une moitié des symboles sont des références (UINFO) : le volume
/// reste à peu près constant. Les Ucodes d'apprentissage couvrent toute
/// l'échelle, dans le désordre.
/// - express : 8 segments (0,7 s) ;
/// - d'époque : 28 segments (2,5 s).
pub(crate) fn our_dil(express: bool) -> DilDescriptor {
    let n = if express { 8 } else { 28 };
    // Ucodes répartis de 6 à 123, mélangés (pas de crescendo)
    let mut train: Vec<u8> = (0..n).map(|k| (6 + k * 117 / (n - 1)) as u8).collect();
    let mut r = 0x5Du32;
    for k in (1..train.len()).rev() {
        r = r.wrapping_mul(1103515245).wrapping_add(12345);
        train.swap(k, (r >> 16) as usize % (k + 1));
    }
    DilDescriptor { sp: motif(120, 0x2B), tp: motif(120, 0x47), h: [119; 8], refs: [UINFO; 8], train }
}

/// Pauses du modem analogique en mode d'époque (ms) : réflexion après Jd
/// (§9.3.2.7 : jusqu'à 5 s permises), SCR pendant le calcul des niveaux
/// (§9.4.2.2 : jusqu'à 4 s).
const THINK_AFTER_JD_MS: u64 = 2500;
const SCR_BEFORE_CP_MS: f64 = 4000.0;
/// TRN de phase 3 de l'abonné en mode d'époque (≥ 512T ; la norme permet
/// jusqu'à 4 s de MD + TRN) : long souffle, et l'annuleur d'écho en profite.
const TRN3_EPOQUE: usize = 8000;

/// Débit montant : le plus grand activé des deux côtés, sous `limit`.
fn up_rate(mask: u16, limit: u32) -> u32 {
    let mut r = 0;
    for k in 0..13u32 {
        let rate = 4800 + 2400 * k;
        if mask & (1 << k) != 0 && rate <= limit {
            r = rate;
        }
    }
    r
}

/// Cherche, en remontant dans `buf`, une trame « 17 uns + 0 » que `parse`
/// sait lire.
fn find_frame<T>(buf: &[bool], parse: impl Fn(&[bool]) -> Option<T>, back: usize) -> Option<T> {
    if buf.len() < 18 {
        return None;
    }
    let lo = buf.len().saturating_sub(back);
    for c in (lo..buf.len() - 18).rev() {
        if (c == 0 || !buf[c - 1]) && buf[c..c + 17].iter().all(|&b| b) && !buf[c + 17] {
            if let Some(t) = parse(&buf[c..]) {
                return Some(t);
            }
        }
    }
    None
}

pub(crate) struct V90 {
    pub(crate) digital: bool,
    law: Law,
    /// Débits max autorisés : descendant (PCM) et montant (V.34)
    max_pcm: u32,
    max_up: u32,
    /// Puissance max du modem numérique (bits 33:37 d'INFO0d)
    power_step: u8,
    /// 56k express (pas de lenteurs d'époque)
    express: bool,
    /// Mode d'époque : instant où l'abonné enverra S (après Jd)
    s_at: Option<u64>,
    /// Mode d'époque : SCR à intercaler avant le CP
    scr_pending: bool,
    /// Modem analogique : annuleur d'écho appris (notre TRN de phase 3 est
    /// fini) ; ensuite le FAI parle presque toujours, il ne bouge plus
    pub(crate) ec_done: bool,
    pub(crate) up_rate: u32,
    pub(crate) down_rate: u32,
    // --- modem numérique
    ptx: Option<PcmTx>,
    bitbuf: Vec<bool>,
    s_trans: u32,
    /// Un vrai S (≥ 24 symboles) vu depuis la première transition
    s_after: bool,
    our_mp: Option<Mp90>,
    cp_rx: Option<Cp>,
    cp_ack_rx: bool,
    sent_mp_ack: bool,
    finishing: bool,
    e_rx: bool,
    data_tx: bool,
    // --- modem analogique
    prx: Option<PcmRx>,
    dil: DilDescriptor,
    seq: Vec<bool>,
    seq_pos: usize,
    seq_ack: bool,
    next_seq: Option<(Vec<bool>, bool)>,
    cp: Option<Cp>,
    mp_rx: Option<Mp90>,
    mp_ack_rx: bool,
    ed_rx: bool,
    sent_cp_ack: bool,
    e_sent: bool,
    data_rx: bool,
    // --- commun
    connected: bool,
    t_progress: u64,
    /// Surveillance de la liaison en données : (instant, trames bonnes,
    /// trames rejetées) au début de la fenêtre
    watch: (u64, u64, u64),
}

impl V90 {
    /// Position dans TRN1d (modem numérique), pour l'annuleur d'écho.
    pub(crate) fn trn1d_index(&self) -> Option<usize> {
        self.ptx.as_ref().and_then(|t| t.trn1d_index())
    }

    fn new(digital: bool, law: Law, max_pcm: u32, max_up: u32, power_step: u8, express: bool) -> V90 {
        V90 {
            digital,
            law,
            max_pcm,
            max_up,
            power_step,
            express,
            s_at: None,
            scr_pending: false,
            ec_done: false,
            up_rate: 0,
            down_rate: 0,
            ptx: None,
            bitbuf: Vec::new(),
            s_trans: 0,
            s_after: false,
            our_mp: None,
            cp_rx: None,
            cp_ack_rx: false,
            sent_mp_ack: false,
            finishing: false,
            e_rx: false,
            data_tx: false,
            prx: None,
            dil: our_dil(express),
            seq: Vec::new(),
            seq_pos: 0,
            seq_ack: false,
            next_seq: None,
            cp: None,
            mp_rx: None,
            mp_ack_rx: false,
            ed_rx: false,
            sent_cp_ack: false,
            e_sent: false,
            data_rx: false,
            connected: false,
            t_progress: 0,
            watch: (0, 0, 0),
        }
    }
}

impl P34 {
    /// Modem analogique (l'abonné) : il commence la phase 3 comme l'appelé
    /// du V.34 (S, S̄, PP, TRN) ; le DIL et les niveaux viendront de son
    /// récepteur PCM.
    pub(crate) fn start_v90_analog(&mut self, info0d: Option<Info0d>, max_pcm: u32, max_up: u32, express: bool) {
        let law = if info0d.is_some_and(|d| !d.alaw) { Law::Mu } else { Law::A };
        let power = info0d.map_or(0, |d| d.max_power);
        let mut v = V90::new(false, law, max_pcm, max_up, power, express);
        let mut prx = PcmRx::new(law, UINFO, Some(v.dil.clone()));
        prx.set_dil_passes(1);
        v.prx = Some(prx);
        v.t_progress = self.n;
        self.v90 = Some(Box::new(v));
        let mut burst = self.silence_syms(70.0);
        burst.extend(seq_s(128, false));
        burst.extend(seq_s(16, true));
        burst.extend(seq_pp().iter().map(|x| *x * std::f32::consts::SQRT_2));
        let trn = if express { TRN3_LEN } else { TRN3_EPOQUE };
        self.send_burst(burst, TxSt::Trn { left: trn }, "S S̄ PP");
        self.goto(Step::A3Send);
    }

    /// Modem numérique (le FAI) : silence, réception du V.34 de l'abonné
    /// (S, S̄, PP, TRN, puis Ja).
    pub(crate) fn start_v90_digital(&mut self, max_pcm: u32, max_up: u32) {
        let mut v = V90::new(true, Law::A, max_pcm, max_up, 0, true);
        v.ptx = Some(PcmTx::new(Law::A, UINFO));
        v.t_progress = self.n;
        self.v90 = Some(Box::new(v));
        self.rx_st = RxSt::WaitS;
        self.goto(Step::C3WaitS);
        self.act(P34Action::Tx("silence".into()));
    }

    fn v90_mut(&mut self) -> &mut V90 {
        self.v90.as_mut().expect("mode V.90")
    }

    /// Un échantillon en mode V.90.
    pub(crate) fn v90_step(&mut self, x: f32, n: u64) -> f32 {
        self.n = n;
        let digital = self.v90_mut().digital;
        if digital {
            if let Some(s) = self.rx.push(x) {
                self.on_rx_symbol(s);
            }
        } else {
            let v = self.v90.as_mut().unwrap();
            let prx = v.prx.as_mut().unwrap();
            prx.push(x);
            let events = prx.take_events();
            let data = std::mem::take(&mut prx.data);
            for ev in events {
                self.v90_on_pcm_event(ev);
            }
            if !data.is_empty() {
                for b in data {
                    self.link.rx_bit(b);
                }
                let got = self.link.take_received();
                if !got.is_empty() {
                    self.act(P34Action::Data(got));
                }
            }
        }
        // mode d'époque : S après le temps de réflexion qui suit Jd
        if self.v90.as_ref().unwrap().s_at.is_some_and(|t| self.n >= t) {
            self.v90_mut().s_at = None;
            self.tx_st = TxSt::SForever;
            self.act(P34Action::Tx("S".into()));
        }
        // garde-fou : la poignée de main ne doit jamais rester bloquée
        let v = self.v90.as_ref().unwrap();
        if !v.connected && (self.n - v.t_progress) / 8 > STALL_MS {
            let why = format!("56k : poignée de main bloquée ({:?})", self.step);
            self.v90_mut().t_progress = self.n;
            self.act(P34Action::NoV90(why));
        }
        if let Some(t) = self.hangup_at {
            if self.n >= t {
                self.hangup_at = None;
                self.act(P34Action::Hangup("raccrochage".into()));
            }
        }
        // En données : rien de bon ne passe plus depuis 3 s, mais des trames
        // fausses arrivent (trames PCM décalées par un échantillon perdu,
        // émission montante détruite…) → réentraînement.
        if self.v90.as_ref().unwrap().connected {
            let st = self.link.stats();
            let n = self.n;
            let v = self.v90.as_mut().unwrap();
            if st.frames_rx > v.watch.1 || v.watch.0 == 0 {
                v.watch = (n, st.frames_rx, st.fcs_errors);
            } else if n - v.watch.0 > 3 * 8000 {
                let bad = st.fcs_errors - v.watch.2;
                v.watch = (n, st.frames_rx, st.fcs_errors);
                if bad >= 3 {
                    self.log(format!("Liaison 56k muette depuis 3 s ({bad} trames fausses) : réentraînement"));
                    self.act(P34Action::Retrain("liaison 56k muette".into()));
                }
            }
        }
        if digital {
            let v = self.v90.as_mut().unwrap();
            let link = &mut self.link;
            let ptx = v.ptx.as_mut().unwrap();
            let y = ptx.next_sample(&mut || link.next_tx_bit());
            for ev in ptx.take_events() {
                self.v90_on_tx_event(ev);
            }
            y
        } else {
            self.fill_tx();
            self.tx.next_sample()
        }
    }

    fn v90_progress(&mut self) {
        let n = self.n;
        self.v90_mut().t_progress = n;
    }

    fn v90_check_connected(&mut self) {
        let v = self.v90.as_ref().unwrap();
        let ok = if v.digital { v.e_rx && v.data_tx } else { v.e_sent && v.data_rx };
        if !ok || v.connected {
            return;
        }
        let (up, down) = (v.up_rate, v.down_rate);
        let digital = v.digital;
        self.v90_mut().connected = true;
        let (tx, rx) = if digital { (down, up) } else { (up, down) };
        self.tx_rate = tx;
        self.rx_rate = rx;
        // T401 : une seconde de notre émission (l'aller-retour du réseau
        // dépasse souvent 300 ms ; trop court, on réémettrait sans cesse)
        self.link.set_t401_bits(tx.max(4800) as u64);
        self.goto(Step::Connected);
        self.act(P34Action::Phase(Phase::Data));
        self.log(format!("CONNECT 56k (V.90) — descendant {down} bit/s, montant {up} bit/s"));
        self.act(P34Action::Connected { tx_rate: tx, rx_rate: rx });
    }

    // ------------------------------------------------------------------
    // Modem analogique
    // ------------------------------------------------------------------

    /// Fin de notre TRN de phase 3 : Ja (descripteur du DIL) en boucle.
    pub(crate) fn v90_on_trn_sent(&mut self) {
        let v = self.v90_mut();
        if v.digital {
            self.tx_st = TxSt::Silence;
            return;
        }
        v.ec_done = true;
        let bits = v.dil.to_bits();
        self.v90_start_seq(bits, false, "Ja", false);
    }

    /// Commence à émettre une séquence de bits en boucle (4 points,
    /// différentiel, brouillée comme J / MP).
    fn v90_start_seq(&mut self, bits: Vec<bool>, ack: bool, name: &str, reset: bool) {
        if reset {
            self.mapper.reset_diff();
        }
        self.mapper.sixteen = false;
        self.tx.set_constellation_power(2.0);
        let v = self.v90_mut();
        v.seq = bits;
        v.seq_pos = 0;
        v.seq_ack = ack;
        self.tx_st = TxSt::V90Seq;
        self.act(P34Action::Tx(name.to_string()));
    }

    /// Un symbole de la séquence en cours.
    pub(crate) fn v90_fill_seq(&mut self) {
        let v = self.v90.as_mut().unwrap();
        if v.seq.is_empty() {
            self.tx.push(&[Cpx::ZERO]);
            return;
        }
        let mut chunk = [true; 2];
        for c in chunk.iter_mut() {
            *c = v.seq[v.seq_pos];
            v.seq_pos += 1;
            if v.seq_pos == v.seq.len() {
                v.seq_pos = 0;
            }
        }
        let end = v.seq_pos == 0 || v.seq_pos == 1;
        let p = self.mapper.diff_symbol(&chunk);
        self.tx.push(&[p]);
        if end {
            self.v90_seq_done();
        }
    }

    /// Fin du SCR (mode d'époque) : CP, ou CP′ si la MP est déjà là.
    pub(crate) fn v90_scr_done(&mut self) {
        let v = self.v90.as_mut().unwrap();
        let next = v.next_seq.take().or_else(|| v.cp.clone().map(|c| (c.to_bits(), false)));
        let Some((bits, ack)) = next else {
            self.tx_st = TxSt::Silence;
            return;
        };
        v.seq = bits;
        v.seq_pos = 0;
        v.seq_ack = ack;
        self.tx_st = TxSt::V90Seq;
        self.act(P34Action::Tx(if ack { "CP′".into() } else { "CP".into() }));
    }

    /// Fin d'une répétition de la séquence émise.
    fn v90_seq_done(&mut self) {
        let v = self.v90.as_mut().unwrap();
        if v.scr_pending {
            v.scr_pending = false;
            let left = (SCR_BEFORE_CP_MS / 1000.0 * self.sym_rate_tx) as usize;
            self.tx_st = TxSt::V90Scr { left };
            self.act(P34Action::Tx("SCR".into()));
            return;
        }
        if v.seq_ack {
            v.sent_cp_ack = true;
        }
        if v.sent_cp_ack && (v.mp_ack_rx || v.ed_rx) && !v.e_sent {
            v.e_sent = true;
            self.tx_st = TxSt::E;
            self.act(P34Action::Tx("E".into()));
            self.v90_progress();
            return;
        }
        if let Some((bits, ack)) = v.next_seq.take() {
            v.seq = bits;
            v.seq_pos = 0;
            v.seq_ack = ack;
            let name = if ack { "CP′" } else { "CP" };
            self.act(P34Action::Tx(name.into()));
        }
    }

    fn v90_on_pcm_event(&mut self, ev: RxEvent) {
        match ev {
            RxEvent::Signal(name) => {
                self.act(P34Action::Rx(name.to_string()));
                self.v90_progress();
            }
            RxEvent::SdBar => {
                // §9.3.2.4 : S→S̄d vu, on arrête Ja
                self.tx_st = TxSt::Silence;
                self.act(P34Action::Tx("silence".into()));
            }
            RxEvent::Trained { sigma } => {
                self.detail(format!("Récepteur 56k entraîné sur TRN1d : bruit {sigma:.0} (pas de niveau le plus fin : 16)"));
            }
            RxEvent::Jd(jd) => {
                let max = (21..=42u32).rev().find(|d| jd.rates & (1 << (d - 21)) != 0).map_or(0, |d| crate::v90::frame::rate_for(d - 6, 6));
                if self.v90.as_ref().unwrap().express {
                    self.log(format!("Jd reçu : le FAI sait émettre jusqu'à {max} bit/s — envoi de S"));
                    self.tx_st = TxSt::SForever;
                    self.act(P34Action::Tx("S".into()));
                } else {
                    self.log(format!("Jd reçu : le FAI sait émettre jusqu'à {max} bit/s — le modem réfléchit, puis S"));
                    let at = self.n + THINK_AFTER_JD_MS * 8;
                    self.v90_mut().s_at = Some(at);
                }
                self.v90_mut().cp = None;
                let mut c = Cp::for_mapping(&Mapping::uniform(Law::A, &[100], 0), true, 0);
                c.d = jd.rates; // mémo provisoire du masque de débits du FAI
                self.v90_mut().cp = Some(c);
            }
            RxEvent::JdPrime => {
                self.send_burst(seq_s(16, true), TxSt::Silence, "S̄");
                self.log("J′d reçu : écoute du DIL (apprentissage des niveaux du central)");
            }
            RxEvent::DilDone { sigma, gain } => {
                self.detail(format!("DIL mesuré : bruit {sigma:.0}, gain {:.3}", gain));
                self.v90_design(sigma);
            }
            RxEvent::RiBar => {
                let v = self.v90_mut();
                if v.express {
                    if let Some(cp) = v.cp.clone() {
                        v.next_seq = Some((cp.to_bits(), false));
                    }
                } else {
                    // §9.4.2.2 : fin du CPt en cours, puis SCR le temps de
                    // « calculer » les niveaux, puis CP
                    v.scr_pending = true;
                }
                self.goto(Step::P4Mp);
            }
            RxEvent::Retrained { sigma } => {
                self.detail(format!("Récepteur 56k réentraîné sur TRN2d : bruit {sigma:.0}"));
            }
            RxEvent::Mp(mp) => {
                let v = self.v90.as_mut().unwrap();
                if v.mp_rx.is_none() {
                    let cp = v.cp.clone().unwrap();
                    let up = up_rate(cp.up_rates & mp.up_rates, (mp.up_drn as u32 * 2400).min(MAX_RATE_FOR_SR[self.sr_tx_idx]));
                    v.up_rate = up;
                    // CP′ (acquittement) dès la fin du CP en cours
                    let mut ack = cp.clone();
                    ack.ack = true;
                    v.next_seq = Some((ack.to_bits(), true));
                    self.detail(format!("MP reçue : montant jusqu'à {} bit/s → {up} bit/s", mp.up_drn as u32 * 2400));
                }
                let v = self.v90.as_mut().unwrap();
                if mp.ack {
                    v.mp_ack_rx = true;
                }
                v.mp_rx = Some(mp);
            }
            RxEvent::Ed => self.v90_mut().ed_rx = true,
            RxEvent::DataStarted => {
                self.v90_mut().data_rx = true;
                self.v90_check_connected();
            }
            RxEvent::Lost(why) => self.act(P34Action::NoV90(format!("56k : {why}"))),
            RxEvent::Debug(line) => self.detail(line),
            RxEvent::Degraded { sigma } => {
                self.log(format!("Réception 56k perdue (erreur {sigma:.0}) : réentraînement"));
                self.act(P34Action::Retrain("réception 56k perdue".into()));
            }
        }
    }

    /// Choix des niveaux (CPt pour la phase 4, CP pour les données) d'après
    /// le bruit mesuré, puis S, S̄ et CPt.
    fn v90_design(&mut self, sigma: f64) {
        let v = self.v90.as_ref().unwrap();
        let (law, power, max_pcm) = (v.law, power_limit(v.power_step), v.max_pcm);
        let jd_rates = v.cp.as_ref().map_or(u32::MAX, |c| c.d);
        let jd = Jd { rates: jd_rates, cp16_train: false, cp16_reneg: false, lookahead: 1 };
        let ch = Channel::nominal(law, sigma);
        let marge = MARGE_DONNEES;
        let data = design::design(&ch, &Limits { margin: marge, power_rms: power, max_rate: max_pcm }).and_then(|m| {
            // débit accepté par le FAI (Jd)
            let d = jd.best_d(m.bits_per_frame())?;
            if d == m.bits_per_frame() {
                Some(m)
            } else {
                design::design_range(&ch, &Limits { margin: marge, power_rms: power, max_rate: max_pcm }, d - 6, d - 6)
            }
        });
        let train = design::design_range(&ch, &Limits { margin: 14.0, power_rms: power, max_rate: 32000 }, 6, 18);
        let (Some(data), Some(train)) = (data, train) else {
            self.act(P34Action::NoV90(format!("bruit trop fort pour le 56k ({sigma:.0}) : repli en V.34")));
            return;
        };
        let up_mask = up_mask_upto(self.v90.as_ref().unwrap().max_up.min(MAX_RATE_FOR_SR[self.sr_tx_idx]));
        let cpt = Cp::for_mapping(&train, false, up_mask);
        let cp = Cp::for_mapping(&data, true, up_mask);
        self.log(format!("Niveaux choisis : {} bit/s en données ({} niveaux par symbole), {} bit/s pour l'apprentissage", data.rate(), data.sets[0].len() * 2, train.rate()));
        let v = self.v90.as_mut().unwrap();
        v.down_rate = data.rate();
        let prx = v.prx.as_mut().unwrap();
        prx.set_train_map(train);
        prx.set_data_map(data);
        v.cp = Some(cp);
        // §9.3.2.10 puis §9.4.2.1 : S 128T, S̄ 16T, puis CPt (brouilleur et
        // codeur différentiel remis à zéro)
        let mut burst = seq_s(128, false);
        burst.extend(seq_s(16, true));
        self.act(P34Action::Phase(Phase::Phase4));
        self.send_burst(burst, TxSt::Silence, "S S̄");
        self.v90_mut().next_seq = None;
        self.after_burst = Some(TxSt::V90Seq);
        self.mapper.reset_diff();
        let v = self.v90_mut();
        v.seq = cpt.to_bits();
        v.seq_pos = 0;
        v.seq_ack = false;
        self.goto(Step::P4Trn);
    }

    // ------------------------------------------------------------------
    // Modem numérique
    // ------------------------------------------------------------------

    pub(crate) fn v90_on_s_present(&mut self) {
        let v = self.v90.as_mut().unwrap();
        if v.digital && self.step == Step::P4Trn && v.s_trans == 1 {
            v.s_after = true;
        }
        if v.digital && self.step == Step::A3RxJ {
            // §9.3.1.5 : S de l'abonné pendant Jd → finir Jd, J′d, DIL
            v.ptx.as_mut().unwrap().finish_jd();
            self.goto(Step::P4Trn);
            self.v90_progress();
        }
    }

    /// Transition S→S̄ ; vrai si traitée ici.
    pub(crate) fn v90_on_s_transition(&mut self, _k0: i64) -> bool {
        let v = self.v90.as_mut().unwrap();
        if !v.digital {
            return false;
        }
        match self.step {
            Step::C3WaitS => false, // phase 3 : PP et TRN de l'abonné (code V.34)
            Step::P4Trn => {
                let dil_ok = v.ptx.as_ref().is_some_and(|t| t.dil_passes_sent() >= 1.0);
                if v.s_trans == 1 && (!v.s_after || self.s_run < 96 || !dil_ok) {
                    // la fin de S̄ (16T) n'est pas le S 128T de fin du DIL
                    self.sdet = SDetector::new();
                    self.rx_st = RxSt::WaitS;
                    return true;
                }
                v.s_trans += 1;
                let nt = v.s_trans;
                self.actions.push(P34Action::Log(LogLevel::Detail, format!("S→S̄ de l'abonné n° {nt}")));
                let v = self.v90.as_mut().unwrap();
                if v.s_trans == 1 {
                    // S̄ après J′d : l'abonné écoute le DIL ; on guette son S suivant
                    self.sdet = SDetector::new();
                    self.rx_st = RxSt::WaitS;
                } else {
                    // second S→S̄ : fin du DIL, Ri ; puis CPt en réception
                    v.ptx.as_mut().unwrap().stop_dil();
                    self.rx_st = RxSt::Bits;
                    self.descr = Scrambler::new(self.answer);
                    self.prev_z = 0;
                    v.bitbuf.clear();
                    self.goto(Step::P4Mp);
                }
                self.v90_progress();
                true
            }
            _ => true,
        }
    }

    /// Bit reçu de l'abonné (V.34 désembrouillé).
    pub(crate) fn v90_on_bit(&mut self, b: bool) {
        self.ones_run = if b { self.ones_run + 1 } else { 0 };
        let v = self.v90.as_mut().unwrap();
        if !v.digital {
            return;
        }
        v.bitbuf.push(b);
        if v.bitbuf.len() > 4096 {
            v.bitbuf.drain(..2048);
        }
        match self.step {
            Step::C3WaitS => {
                if let Some(dil) = find_frame(&v.bitbuf, DilDescriptor::from_bits, 1200) {
                    v.bitbuf.clear();
                    let jd = Jd::upto(v.max_pcm);
                    let n = dil.train.len();
                    let len = dil.total_len();
                    v.ptx.as_mut().unwrap().start_phase3(&jd, Some(dil));
                    self.act(P34Action::Rx("Ja".into()));
                    self.log(format!("Ja reçu : l'abonné demande un DIL de {n} segments ({len} symboles) — envoi Sd, TRN1d, Jd"));
                    // on guette maintenant le S de l'abonné (Jd reçu)
                    self.goto(Step::A3RxJ);
                    self.rx_st = RxSt::WaitS;
                    self.sdet = SDetector::new();
                    self.v90_progress();
                }
            }
            Step::P4Mp => {
                if let Some(cp) = find_frame(&v.bitbuf, Cp::from_bits, 1200) {
                    v.bitbuf.clear();
                    self.v90_on_cp(cp);
                }
                let fire = {
                    let v = self.v90.as_ref().unwrap();
                    v.cp_ack_rx && !v.e_rx && self.ones_run >= 20
                };
                if fire {
                    // E de l'abonné : B1 puis données montantes
                    let (up, finishing) = {
                        let v = self.v90.as_mut().unwrap();
                        v.e_rx = true;
                        (v.up_rate, v.finishing)
                    };
                    self.act(P34Action::Rx("E".into()));
                    let prm = DataParams::new(self.sr_rx_idx, up, false);
                    self.dec = Some(DataDecoder::new(prm, self.answer));
                    self.rx_st = RxSt::Data;
                    self.sdet = SDetector::new();
                    if !finishing {
                        self.v90_finish_mp();
                    }
                    self.v90_check_connected();
                }
            }
            _ => {}
        }
    }

    fn v90_on_cp(&mut self, cp: Cp) {
        let law = self.v90.as_ref().unwrap().law;
        if !cp.data {
            let v = self.v90.as_mut().unwrap();
            if v.our_mp.is_some() {
                return;
            }
            // CPt : R̄i, TRN2d, MP (§9.4.1.2). Débit montant d'après notre
            // égaliseur V.34.
            let snr = self.rx.snr_db();
            let cap = max_rate_for_snr(self.sr_rx_idx, snr, 1.5).min(v.max_up).min(MAX_RATE_FOR_SR[self.sr_rx_idx]).max(4800);
            let mp = Mp90 { up_drn: (cap / 2400) as u8, trellis: 0, nonlinear: false, shaping_expanded: false, ack: false, up_rates: up_mask_upto(v.max_up.min(MAX_RATE_FOR_SR[self.sr_rx_idx])) };
            v.ptx.as_mut().unwrap().start_trn2d(cp.mapping(law), mp.to_bits());
            v.our_mp = Some(mp);
            self.act(P34Action::Rx("CPt".into()));
            self.log(format!("CPt reçu : apprentissage à {} bit/s — envoi R̄i, TRN2d, MP (montant jusqu'à {cap} bit/s, SNR {snr:.1} dB)", crate::v90::frame::rate_for(cp.d.saturating_sub(6), 6)));
            self.v90_progress();
            return;
        }
        let first = self.v90.as_ref().unwrap().cp_rx.is_none();
        if first {
            let (down, up) = {
                let v = self.v90.as_mut().unwrap();
                let mp = v.our_mp.clone().unwrap_or(Mp90 { up_drn: 2, trellis: 0, nonlinear: false, shaping_expanded: false, ack: false, up_rates: 1 });
                v.up_rate = up_rate(mp.up_rates & cp.up_rates, mp.up_drn as u32 * 2400);
                v.down_rate = crate::v90::frame::rate_for(cp.d.saturating_sub(6), 6);
                let ack = Mp90 { ack: true, ..mp };
                v.ptx.as_mut().unwrap().set_mp(ack.to_bits(), true);
                (v.down_rate, v.up_rate)
            };
            self.act(P34Action::Rx("CP".into()));
            self.log(format!("CP reçu : descendant {down} bit/s, montant {up} bit/s — envoi MP′"));
        }
        let new_ack = cp.ack && !self.v90.as_ref().unwrap().cp_ack_rx;
        if new_ack {
            self.act(P34Action::Rx("CP′".into()));
        }
        let finish = {
            let v = self.v90.as_mut().unwrap();
            v.cp_ack_rx |= cp.ack;
            v.cp_rx = Some(cp);
            v.cp_ack_rx && v.sent_mp_ack && !v.finishing
        };
        if finish {
            self.v90_finish_mp();
        }
        self.v90_progress();
    }

    fn v90_finish_mp(&mut self) {
        let v = self.v90.as_mut().unwrap();
        let Some(cp) = v.cp_rx.clone() else { return };
        v.finishing = true;
        let law = v.law;
        v.ptx.as_mut().unwrap().finish_mp(cp.mapping(law));
    }

    fn v90_on_tx_event(&mut self, ev: TxEvent) {
        match ev {
            TxEvent::Signal(name) => {
                if name == "Ri" {
                    self.act(P34Action::Phase(Phase::Phase4));
                }
                self.act(P34Action::Tx(name.to_string()));
            }
            TxEvent::MpSent { ack } => {
                let v = self.v90.as_mut().unwrap();
                if ack {
                    v.sent_mp_ack = true;
                    if v.cp_ack_rx && !v.finishing {
                        self.v90_finish_mp();
                    }
                }
            }
            TxEvent::DataStarted => {
                self.v90_mut().data_tx = true;
                self.v90_check_connected();
            }
        }
    }
}
