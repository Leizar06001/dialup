//! Sons « réseau téléphonique » générés côté appli : tonalité, DTMF, retour d'appel.
//! Ils ne passent pas par le modem : on les entend seulement dans le HP local,
//! comme sur un vrai modem pendant la numérotation.

use std::collections::VecDeque;
use std::f32::consts::TAU;

use v34_core::SAMPLE_RATE;

/// Action déclenchée quand le script atteint une étape `Action`.
#[derive(Clone, Debug, PartialEq)]
pub enum SetupAction {
    /// Le modem local appelle (ModemCommand::Dial) — le distant n'a pas encore décroché.
    LocalDial,
    /// Le modem distant décroche (ModemCommand::Answer).
    RemoteAnswer,
    /// Appel entrant : le modem distant compose (ModemCommand::Dial).
    RemoteDial,
    /// Appel entrant : le modem local décroche (ModemCommand::Answer).
    LocalAnswer,
    /// Ligne de console (ex. "RING").
    Console(String),
    /// Réseau : la numérotation est finie, on appelle via le central.
    NetDial(String),
    /// Réseau : on décroche l'appel entrant.
    NetAnswer,
    /// Fin d'un appel qui n'aboutit pas (après la tonalité) : code
    /// résultat, et le modem raccroche.
    Fail(String),
}

#[derive(Clone, Debug)]
enum Step {
    Tone { f1: f32, f2: f32, samples: usize, level: f32 },
    Silence(usize),
    Action(SetupAction),
}

/// Script de sons + actions, consommé échantillon par échantillon.
#[derive(Default)]
pub struct ToneScript {
    steps: VecDeque<Step>,
    pos: usize,
    phase1: f32,
    phase2: f32,
}

const fn ms(n: u32) -> usize {
    (n * SAMPLE_RATE / 1000) as usize
}

/// Fréquences DTMF (ligne, colonne).
pub fn dtmf_freqs(c: char) -> Option<(f32, f32)> {
    let rows = [697.0, 770.0, 852.0, 941.0];
    let cols = [1209.0, 1336.0, 1477.0, 1633.0];
    let keys = ["123A", "456B", "789C", "*0#D"];
    let c = c.to_ascii_uppercase();
    for (r, line) in keys.iter().enumerate() {
        if let Some(col) = line.find(c) {
            return Some((rows[r], cols[col]));
        }
    }
    None
}

impl ToneScript {
    pub fn is_active(&self) -> bool {
        !self.steps.is_empty()
    }

    pub fn clear(&mut self) {
        self.steps.clear();
        self.pos = 0;
    }

    /// Appel sortant : tonalité (440 Hz), numérotation DTMF, retour d'appel français.
    /// Les '.' d'une adresse IP sont composés comme '*', ',' = pause de 2 s (Hayes).
    pub fn outgoing(number: &str) -> Self {
        let mut s = Self::default();
        let tone = |f1, f2, samples, level| Step::Tone { f1, f2, samples, level };
        s.steps.push_back(tone(440.0, 0.0, ms(900), 0.18));
        s.steps.push_back(Step::Silence(ms(150)));
        for c in number.chars() {
            let c = if c == '.' { '*' } else { c };
            if c == ',' {
                s.steps.push_back(Step::Silence(ms(2000)));
            } else if let Some((f1, f2)) = dtmf_freqs(c) {
                s.steps.push_back(tone(f1, f2, ms(85), 0.16));
                s.steps.push_back(Step::Silence(ms(75)));
            }
        }
        s.steps.push_back(Step::Action(SetupAction::LocalDial));
        s.steps.push_back(Step::Silence(ms(600)));
        // Retour d'appel France : 440 Hz, 1,5 s on (puis on décroche pendant le silence)
        s.steps.push_back(tone(440.0, 0.0, ms(1500), 0.12));
        s.steps.push_back(Step::Silence(ms(900)));
        s.steps.push_back(Step::Action(SetupAction::RemoteAnswer));
        s
    }

    /// Réseau : tonalité + numérotation DTMF, puis appel via le central (le
    /// retour d'appel est joué ensuite tant que l'autre ne décroche pas).
    pub fn outgoing_net(number: &str) -> Self {
        let mut s = Self::outgoing(number);
        // On garde tonalité + DTMF, on remplace la suite par l'appel réseau.
        while let Some(st) = s.steps.back() {
            if matches!(st, Step::Action(SetupAction::LocalDial)) {
                s.steps.pop_back();
                break;
            }
            s.steps.pop_back();
        }
        s.steps.push_back(Step::Action(SetupAction::NetDial(number.to_string())));
        s
    }

    /// Retour d'appel (réseau) : 1,5 s de 440 Hz puis 3,5 s de silence.
    pub fn ringback() -> Self {
        let mut s = Self::default();
        s.steps.push_back(Step::Tone { f1: 440.0, f2: 0.0, samples: ms(1500), level: 0.12 });
        s.steps.push_back(Step::Silence(ms(3500)));
        s
    }

    /// Réseau : appel entrant — deux sonneries puis on décroche.
    pub fn ringing_net(from: &str) -> Self {
        let mut s = Self::default();
        for _ in 0..2 {
            s.steps.push_back(Step::Action(SetupAction::Console(format!("RING (appel de {from})"))));
            // Sonnerie « téléphone » : 50 Hz modulé, rendue par deux tons proches
            s.steps.push_back(Step::Tone { f1: 425.0, f2: 450.0, samples: ms(1200), level: 0.15 });
            s.steps.push_back(Step::Silence(ms(800)));
        }
        s.steps.push_back(Step::Action(SetupAction::NetAnswer));
        s
    }

    /// Réseau, réponse automatique coupée : ça sonne (une trentaine de
    /// secondes au plus) jusqu'à ce que l'utilisateur décroche (ATA).
    pub fn ringing_manual(from: &str) -> Self {
        let mut s = Self::default();
        for _ in 0..15 {
            s.steps.push_back(Step::Action(SetupAction::Console(format!("RING (appel de {from}) — ATA pour décrocher"))));
            s.steps.push_back(Step::Tone { f1: 425.0, f2: 450.0, samples: ms(1200), level: 0.15 });
            s.steps.push_back(Step::Silence(ms(800)));
        }
        s
    }

    /// Tonalité d'occupation française : 440 Hz, 0,5 s sur 1 s, puis le
    /// code résultat.
    pub fn busy(result: String) -> Self {
        let mut s = Self::default();
        s.steps.push_back(Step::Silence(ms(200)));
        for _ in 0..4 {
            s.steps.push_back(Step::Tone { f1: 440.0, f2: 0.0, samples: ms(500), level: 0.14 });
            s.steps.push_back(Step::Silence(ms(500)));
        }
        s.steps.push_back(Step::Action(SetupAction::Fail(result)));
        s
    }

    /// Réseau encombré : la même note, en cadence rapide.
    pub fn congestion(result: String) -> Self {
        let mut s = Self::default();
        s.steps.push_back(Step::Silence(ms(200)));
        for _ in 0..8 {
            s.steps.push_back(Step::Tone { f1: 440.0, f2: 0.0, samples: ms(250), level: 0.14 });
            s.steps.push_back(Step::Silence(ms(250)));
        }
        s.steps.push_back(Step::Action(SetupAction::Fail(result)));
        s
    }

    /// Numéro non attribué : les trois notes montantes d'information
    /// spéciale (950, 1400, 1800 Hz), qui précédaient le message enregistré.
    pub fn unassigned(result: String) -> Self {
        let mut s = Self::default();
        s.steps.push_back(Step::Silence(ms(300)));
        for _ in 0..3 {
            for f in [950.0, 1400.0, 1800.0] {
                s.steps.push_back(Step::Tone { f1: f, f2: 0.0, samples: ms(330), level: 0.13 });
                s.steps.push_back(Step::Silence(ms(30)));
            }
            s.steps.push_back(Step::Silence(ms(1000)));
        }
        s.steps.push_back(Step::Action(SetupAction::Fail(result)));
        s
    }

    /// Appel entrant : « RING » puis le modem local décroche.
    pub fn incoming() -> Self {
        let mut s = Self::default();
        s.steps.push_back(Step::Action(SetupAction::RemoteDial));
        s.steps.push_back(Step::Silence(ms(400)));
        s.steps.push_back(Step::Action(SetupAction::Console("RING".into())));
        s.steps.push_back(Step::Silence(ms(1500)));
        s.steps.push_back(Step::Action(SetupAction::LocalAnswer));
        s
    }

    /// Ajoute les sons du script dans `out` et renvoie les actions atteintes.
    pub fn render(&mut self, out: &mut [f32]) -> Vec<SetupAction> {
        let mut actions = Vec::new();
        let sr = SAMPLE_RATE as f32;
        let mut i = 0;
        while i < out.len() {
            let Some(step) = self.steps.front() else { break };
            match step.clone() {
                Step::Action(a) => {
                    actions.push(a);
                    self.steps.pop_front();
                    self.pos = 0;
                }
                Step::Silence(n) => {
                    let take = (n - self.pos).min(out.len() - i);
                    self.pos += take;
                    i += take;
                    if self.pos >= n {
                        self.steps.pop_front();
                        self.pos = 0;
                    }
                }
                Step::Tone { f1, f2, samples, level } => {
                    let take = (samples - self.pos).min(out.len() - i);
                    for k in 0..take {
                        // Rampe de 3 ms pour éviter les clics
                        let p = self.pos + k;
                        let ramp = (p.min(samples - p) as f32 / ms(3) as f32).min(1.0);
                        let mut v = self.phase1.sin();
                        if f2 > 0.0 {
                            v = 0.5 * (v + self.phase2.sin());
                        }
                        out[i + k] += v * level * ramp;
                        self.phase1 = (self.phase1 + TAU * f1 / sr) % TAU;
                        self.phase2 = (self.phase2 + TAU * f2 / sr) % TAU;
                    }
                    self.pos += take;
                    i += take;
                    if self.pos >= samples {
                        self.steps.pop_front();
                        self.pos = 0;
                    }
                }
            }
        }
        // Actions en fin de bloc (étapes de durée nulle restantes)
        while let Some(Step::Action(a)) = self.steps.front().cloned() {
            actions.push(a);
            self.steps.pop_front();
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dtmf_table() {
        assert_eq!(dtmf_freqs('5'), Some((770.0, 1336.0)));
        assert_eq!(dtmf_freqs('#'), Some((941.0, 1477.0)));
        assert_eq!(dtmf_freqs('x'), None);
    }

    #[test]
    fn outgoing_script_reaches_actions_in_order() {
        let mut s = ToneScript::outgoing("12");
        let mut all = Vec::new();
        let mut buf = vec![0.0; 160];
        for _ in 0..1000 {
            buf.fill(0.0);
            all.extend(s.render(&mut buf));
            if !s.is_active() {
                break;
            }
        }
        assert_eq!(all, vec![SetupAction::LocalDial, SetupAction::RemoteAnswer]);
    }
}
