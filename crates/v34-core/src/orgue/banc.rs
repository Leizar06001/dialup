//! Banc d'essai : deux organistes reliés par la ligne simulée (deux sens
//! + l'écho de chacun), données pseudo-aléatoires, bilan par sens.

use super::{decalage_donnees, partition, Duplex, Mode, Organiste, Voix};
use crate::line::{EchoPath, LineParams, LineSim};

/// Bits pseudo-aléatoires reproductibles (xorshift).
pub fn prbs(graine: u32, n: usize) -> Vec<u8> {
    let mut x = graine.wrapping_mul(0x9E37_79B9) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 7) as u8 & 1
        })
        .collect()
}

/// Bilan d'un sens de transmission.
#[derive(Clone, Debug, Default)]
pub struct Sens {
    /// Le récepteur s'est calé sur le motif
    pub cale: bool,
    pub erreurs: usize,
    pub compares: usize,
    /// Débit net (bit/s) moyen sur la boucle mélodique, selon le chargement
    pub debit_net: f32,
    pub bits_moyens: f32,
    /// SNR mesuré sur les décisions en données (dB)
    pub snr_donnees_db: f32,
    pub decalage_hz: f32,
    pub symboles: usize,
    pub effacements: usize,
    /// Diagnostic sur la tonique (do) : par harmonique, fréquence (Hz),
    /// SNR sur la tenue du motif, SINR sur son tremblant, SINR retenu pour
    /// le chargement (lissé, planchers), SNR vu en données (dB), bits portés
    pub tonique: Vec<(f32, f32, f32, f32, f32, u8)>,
}

impl Sens {
    pub fn teb(&self) -> f64 {
        if self.compares == 0 {
            return 1.0;
        }
        self.erreurs as f64 / self.compares as f64
    }
}

/// Signaux enregistrés (pour écouter).
#[derive(Clone, Debug, Default)]
pub struct Enregistrement {
    pub appelant: Vec<f32>,
    pub appele: Vec<f32>,
    /// Ce qu'entend un combiné branché côté appelant : sa voix + celle
    /// d'en face après la ligne
    pub ligne: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct Bilan {
    /// [appelant → appelé, appelé → appelant]
    pub sens: [Sens; 2],
    /// Atténuation d'écho apprise sur le motif [appelant, appelé] (dB),
    /// telle que l'annuleur la voit (plafonnée par le bruit de ligne)
    pub erle_db: [f32; 2],
    /// Pendant le duplex : écho résiduel sous le signal d'en face
    /// [chez l'appelant, chez l'appelé] (dB, mesuré par le banc)
    pub echo_sous_signal_db: [f32; 2],
    /// Pendant le duplex : atténuation réelle de l'écho (dB)
    pub erle_duplex_db: [f32; 2],
    /// Début des données de l'appelant (échantillons)
    pub debut_donnees: usize,
    pub enregistrement: Option<Enregistrement>,
}

/// Un appel complet : motifs, puis `secondes` de données dans les deux
/// sens. `enregistrer` : durée à garder (échantillons depuis le début).
pub fn appel(p: &LineParams, mode: Mode, duplex: Duplex, secondes: f32, graine: u64, enregistrer: usize) -> Bilan {
    let mut a = Organiste::new(Voix::Appelant, duplex, mode);
    let mut b = Organiste::new(Voix::Appele, duplex, mode);
    // Réglages d'exploration (variables d'environnement du banc)
    let env = |n: &str| std::env::var(n).ok().and_then(|v| v.parse::<f32>().ok());
    for o in [&mut a, &mut b] {
        if let Some(m) = env("ORGUE_MU_LENT") {
            o.mu_lent = m;
        }
        if let Some(m) = env("ORGUE_MU_RAPIDE") {
            o.mu_rapide = m;
        }
        if let Some(m) = env("ORGUE_MARGE") {
            o.rx.marge_db = m;
        }
    }
    let (mut a2b, mut b2a) = (LineSim::with_seed(p.clone(), graine), LineSim::with_seed(p.clone(), graine ^ 0xA5A5));
    let (mut echo_a, mut echo_b) = (EchoPath::new(p.clone()), EchoPath::new(p.clone()));
    let debut = partition::duree_motif() + decalage_donnees(Voix::Appelant);
    let fin = debut + (secondes * 8000.0) as usize + 4000;
    // de quoi ne jamais manquer de bits (8 bits × 60 harmoniques × 60 symboles/s)
    let n_bits = (secondes as usize + 2) * 30_000;
    let (ba, bb) = (prbs(graine as u32 * 2 + 1, n_bits), prbs(graine as u32 * 2 + 2, n_bits));
    a.pousser_bits(ba.iter().copied());
    b.pousser_bits(bb.iter().copied());
    const B: usize = 160;
    let (mut a_rx, mut b_rx) = ([0.0f32; B], [0.0f32; B]);
    let (mut a_tx, mut b_tx) = ([0.0f32; B], [0.0f32; B]);
    let (mut recu_a, mut recu_b) = (Vec::new(), Vec::new());
    let (mut donne_a, mut donne_b) = (false, false);
    let mut enr = (enregistrer > 0).then(Enregistrement::default);
    // Écho : [signal d'en face, écho, résidu] par poste, pendant le duplex
    let mut ech = [[0.0f64; 3]; 2];
    let (mut a_sans, mut b_sans) = ([0.0f32; B], [0.0f32; B]);
    let debug = std::env::var_os("ORGUE_DEBUG").is_some();
    let mut dbg = [[1e-30f64; 3]; 2];
    let mut t = 0;
    while t < fin {
        a.traiter(&a_rx, &mut a_tx);
        b.traiter(&b_rx, &mut b_tx);
        if t >= debut + 8000 {
            for (k, (o, rx, sans)) in [(&a, &a_rx, &a_sans), (&b, &b_rx, &b_sans)].into_iter().enumerate() {
                for i in 0..B {
                    ech[k][0] += (sans[i] as f64).powi(2);
                    ech[k][1] += ((rx[i] - sans[i]) as f64).powi(2);
                    ech[k][2] += ((o.nettoye()[i] - sans[i]) as f64).powi(2);
                }
            }
        }
        if debug {
            for (k, (o, rx, sans)) in [(&a, &a_rx, &a_sans), (&b, &b_rx, &b_sans)].into_iter().enumerate() {
                for i in 0..B {
                    dbg[k][0] += (sans[i] as f64).powi(2);
                    dbg[k][1] += ((rx[i] - sans[i]) as f64).powi(2);
                    dbg[k][2] += ((o.nettoye()[i] - sans[i]) as f64).powi(2);
                }
            }
            if t % 4000 == 0 {
                let f = |v: [f64; 3]| format!("loin {:5.1} écho {:5.1} résidu {:5.1}", 10.0 * (v[0] / 4000.0).log10(), 10.0 * (v[1] / 4000.0).log10(), 10.0 * (v[2] / 4000.0).log10());
                eprintln!("t {:5.1} s  A: {} {:?} | B: {} {:?}", t as f32 / 8000.0, f(dbg[0]), a.rx.etat, f(dbg[1]), b.rx.etat);
                dbg = [[1e-30; 3]; 2];
            }
        }
        a2b.process(&a_tx, &mut b_rx);
        b_sans = b_rx;
        echo_b.process(&b_tx, &mut b_rx);
        b2a.process(&b_tx, &mut a_rx);
        a_sans = a_rx;
        if let Some(e) = enr.as_mut() {
            if t < enregistrer {
                e.appelant.extend_from_slice(&a_tx);
                e.appele.extend_from_slice(&b_tx);
                e.ligne.extend(a_tx.iter().zip(&a_rx).map(|(x, y)| x + y));
            }
        }
        echo_a.process(&a_tx, &mut a_rx);
        recu_a.extend(a.prendre_bits());
        recu_b.extend(b.prendre_bits());
        // Chargements rendus hors bande (prototype)
        if !donne_a {
            if let Some(c) = b.chargement_pour_l_autre() {
                a.imposer_chargement(c);
                donne_a = true;
            }
        }
        if !donne_b {
            if let Some(c) = a.chargement_pour_l_autre() {
                b.imposer_chargement(c);
                donne_b = true;
            }
        }
        t += B;
    }
    let sens = |env: &[u8], rec: &[u8], tx: &Organiste, rx: &Organiste| -> Sens {
        let n = env.len().min(rec.len());
        let s = &rx.rx.stats;
        if debug {
            // Où tombent les erreurs : (symbole, degré, harmonique)
            let ch = tx.tx.chargement();
            let mut pos = 0usize;
            for (i, sy) in partition::Ordonnanceur::new(tx.voix).enumerate() {
                if pos >= n {
                    break;
                }
                if sy.reference {
                    continue;
                }
                for (k, &b) in ch.bits[sy.degre.index()].iter().enumerate() {
                    let e = (pos..(pos + b as usize).min(n)).filter(|&j| env[j] != rec[j]).count();
                    if e > 0 {
                        eprintln!("  erreur : symbole {i} note {} {:?} harmonique {k} ({b} bits) : {e}", sy.note, sy.degre);
                    }
                    pos += b as usize;
                }
            }
        }
        Sens {
            cale: rx.rx.synchro.is_some(),
            erreurs: env[..n].iter().zip(&rec[..n]).filter(|(x, y)| x != y).count(),
            compares: n,
            debit_net: tx.tx.chargement().debit_net(),
            bits_moyens: tx.tx.chargement().bits_moyens(),
            snr_donnees_db: s.snr_donnees_db(),
            decalage_hz: rx.rx.synchro.as_ref().map(|s| s.decalage_hz).unwrap_or(f32::NAN),
            symboles: s.symboles,
            effacements: s.effacements,
            tonique: {
                let d = partition::Degre::Do;
                let rangs = partition::harmoniques(tx.voix, duplex, d);
                let f0 = partition::f0(tx.voix, d);
                let motif = rx.rx.mesure.as_ref().map(|m| m.snr_db[d.index()].clone()).unwrap_or_default();
                let retenu = rx.rx.mesure.as_ref().map(|m| m.snr_lisse_db[d.index()].clone()).unwrap_or_default();
                let tremblant = rx.rx.mesure.as_ref().map(|m| m.sinr_db[d.index()].clone()).unwrap_or_default();
                let donnees = s.par_harmonique.get(d.index()).cloned().unwrap_or_default();
                let bits = &tx.tx.chargement().bits[d.index()];
                rangs
                    .iter()
                    .enumerate()
                    .map(|(i, &h)| {
                        let sm = motif.get(i).copied().unwrap_or(f32::NAN);
                        let sd = donnees.get(i).map(|(e, p)| (10.0 * (p / e.max(1e-30)).log10()) as f32).unwrap_or(f32::NAN);
                        let sr = retenu.get(i).copied().unwrap_or(f32::NAN);
                        let st = tremblant.get(i).copied().unwrap_or(f32::NAN);
                        (h as f32 * f0, sm, st, sr, sd, bits[i])
                    })
                    .collect()
            },
        }
    };
    Bilan {
        sens: [sens(&ba, &recu_b, &a, &b), sens(&bb, &recu_a, &b, &a)],
        erle_db: [a.erle_motif.unwrap_or(f32::NAN), b.erle_motif.unwrap_or(f32::NAN)],
        echo_sous_signal_db: ech.map(|e| (10.0 * (e[0] / e[2].max(1e-30)).log10()) as f32),
        erle_duplex_db: ech.map(|e| (10.0 * (e[1] / e[2].max(1e-30)).log10()) as f32),
        debut_donnees: debut,
        enregistrement: enr,
    }
}
