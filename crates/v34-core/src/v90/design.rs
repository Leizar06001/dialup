//! Choix des niveaux PCM par le modem analogique (il les envoie au modem
//! numérique dans la séquence CP, §8.5.2).
//!
//! Deux niveaux voisins doivent rester distinguables malgré le bruit mesuré
//! après égalisation : on impose un écart minimal `d = marge × σ` (et un plus
//! petit niveau ≥ d/2, car son opposé est de l'autre côté de zéro). Plus la
//! ligne est bruitée, moins il reste de niveaux, et moins de bits par
//! symbole. La puissance moyenne reste sous la limite annoncée par le modem
//! numérique (tableau 15/V.90).

use super::codes::{self, Law, UCODES};
use super::frame::{rate_for, Mapping, FRAME};

/// Limite de puissance moyenne (valeur efficace linéaire 16 bits) pour une
/// puissance d'émission maximale de `-(pas + 1) × 0,5 dBm0` (tableau 15/V.90,
/// `pas` = bits 33:37 d'INFO0d, 0 = -0,5 dBm0).
pub fn power_limit(step: u8) -> f64 {
    const T: [f64; 32] = [
        15124.0, 14276.0, 13480.0, 12724.0, 12012.0, 11340.0, 10708.0, 10108.0, 9544.0, 9008.0, 8504.0, 8028.0, 7580.0, 7156.0, 6756.0, 6380.0, 6020.0, 5684.0, 5368.0, 5068.0, 4784.0,
        4516.0, 4264.0, 4024.0, 3800.0, 3588.0, 3388.0, 3196.0, 3020.0, 2852.0, 2692.0, 2540.0,
    ];
    T[(step as usize).min(31)]
}

/// Puissance moyenne d'un jeu de constellations (formule du §8.5.2) : chaque
/// niveau compte autant de fois que le codeur à modules le choisit parmi les
/// 2^K entrées possibles. `levels[u]` : valeur linéaire du Ucode u.
pub fn average_power(map: &Mapping, levels: &[f64; UCODES]) -> f64 {
    let q: u128 = 1u128 << map.k;
    let mut a: u128 = 1; // A_i = M0 × … × M(i-1)
    let mut total = 0.0;
    for set in &map.sets {
        let m = set.len() as u128;
        let cycle = a * m;
        let (full, rem) = (q / cycle, q % cycle);
        for (j, &u) in set.iter().enumerate() {
            let j = j as u128;
            let extra = rem.saturating_sub(j * a).min(a);
            let n = full * a + extra;
            total += n as f64 * levels[u as usize] * levels[u as usize];
        }
        a = a.saturating_mul(m);
    }
    total / (FRAME as f64 * q as f64)
}

/// Ce que le modem analogique sait au moment de choisir.
#[derive(Clone, Debug)]
pub struct Channel {
    pub law: Law,
    /// Valeur reçue de chaque Ucode, ramenée à l'échelle d'émission
    /// (mesurée pendant le DIL ; à défaut, la valeur nominale)
    pub levels: [f64; UCODES],
    /// Bruit après égalisation (même échelle, linéaire 16 bits)
    pub sigma: f64,
}

impl Channel {
    /// Canal idéal (niveaux nominaux) avec un bruit donné.
    pub fn nominal(law: Law, sigma: f64) -> Channel {
        let mut levels = [0.0; UCODES];
        for (u, l) in levels.iter_mut().enumerate() {
            *l = codes::linear(law, u as u8) as f64;
        }
        Channel { law, levels, sigma }
    }
}

/// Réglages du choix.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Écart minimal entre niveaux, en multiples de σ (≈ 9 : une erreur de
    /// symbole sur 10^5 environ)
    pub margin: f64,
    /// Limite de puissance (valeur efficace linéaire 16 bits)
    pub power_rms: f64,
    /// Débit maximal (bit/s)
    pub max_rate: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { margin: 9.0, power_rms: power_limit(0), max_rate: 56000 }
    }
}

/// Niveaux utilisables : du plus petit (≥ d/2) au plus grand, écartés d'au
/// moins `d`, en Ucodes croissants.
fn ladder(ch: &Channel, d: f64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut next = d / 2.0;
    for u in 0..UCODES {
        if ch.levels[u] >= next {
            out.push(u as u8);
            next = ch.levels[u] + d;
        }
    }
    out
}

/// Modules par intervalle (les plus petits possibles) pour coder K bits
/// avec au plus `max` niveaux.
fn moduli(k: u32, max: usize) -> Option<[usize; FRAME]> {
    if max < 2 {
        return None;
    }
    let target = 1u128 << k;
    let base = (2f64.powf(k as f64 / FRAME as f64)).floor().max(1.0) as usize;
    if base > max {
        return None;
    }
    let mut m = [base; FRAME];
    let mut i = 0;
    while m.iter().map(|&x| x as u128).product::<u128>() < target {
        if m[i] >= max {
            return None;
        }
        m[i] += 1;
        i = (i + 1) % FRAME;
    }
    Some(m)
}

/// Meilleur jeu de constellations sans mise en forme spectrale (S = 6) :
/// le plus grand K qui tient dans la limite de puissance et le débit
/// maximal ; à K égal, les niveaux les plus écartés possible (meilleure
/// marge contre le bruit). `None` si même 28 000 bit/s est impossible.
pub fn design(ch: &Channel, lim: &Limits) -> Option<Mapping> {
    let d0 = (lim.margin * ch.sigma).max(1.0);
    let base = ladder(ch, d0);
    for k in (15..=36u32).rev() {
        if rate_for(k, 6) > lim.max_rate {
            continue;
        }
        let Some(m) = moduli(k, base.len()) else { continue };
        let mmax = *m.iter().max().unwrap();
        // écart le plus grand qui laisse encore assez de niveaux sous la
        // limite de puissance (recherche par dichotomie)
        let build = |d: f64| -> Option<Mapping> {
            let lad = ladder(ch, d);
            if lad.len() < mmax {
                return None;
            }
            let sets = std::array::from_fn(|i| {
                let mut s: Vec<u8> = lad[..m[i]].to_vec();
                s.reverse();
                s
            });
            let map = Mapping { law: ch.law, sets, k, s: 6 };
            (average_power(&map, &ch.levels) <= lim.power_rms * lim.power_rms).then_some(map)
        };
        let Some(mut best) = build(d0) else { continue };
        let (mut lo, mut hi) = (d0, d0 * 64.0);
        for _ in 0..30 {
            let mid = (lo + hi) / 2.0;
            match build(mid) {
                Some(map) => {
                    best = map;
                    lo = mid;
                }
                None => hi = mid,
            }
        }
        return Some(best);
    }
    None
}

/// Plus petit écart entre deux points signés d'un ensemble (y compris
/// autour de zéro) : sert à vérifier une constellation reçue.
pub fn min_distance(set: &[u8], levels: &[f64; UCODES]) -> f64 {
    let mut v: Vec<f64> = set.iter().map(|&u| levels[u as usize]).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut d = v.first().map_or(0.0, |x| 2.0 * x);
    for w in v.windows(2) {
        d = d.min(w[1] - w[0]);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puissance_moyenne_uniforme() {
        // 2^K = ∏ Mi exactement : chaque niveau sort aussi souvent
        let map = Mapping::uniform(Law::A, &[100, 90, 80, 70], 12);
        let ch = Channel::nominal(Law::A, 1.0);
        let p = average_power(&map, &ch.levels);
        let attendu = [100u8, 90, 80, 70].iter().map(|&u| (codes::linear(Law::A, u) as f64).powi(2)).sum::<f64>() / 4.0;
        assert!((p - attendu).abs() / attendu < 1e-12);
        // 2^K < ∏ Mi : les petites étiquettes (grands niveaux) sortent plus
        let map = Mapping::uniform(Law::A, &[100, 90, 80], 9);
        let p = average_power(&map, &ch.levels);
        let moyenne = [100u8, 90, 80].iter().map(|&u| (codes::linear(Law::A, u) as f64).powi(2)).sum::<f64>() / 3.0;
        assert!(p > moyenne);
    }

    #[test]
    fn ligne_parfaite_56k() {
        let ch = Channel::nominal(Law::A, 0.5);
        let map = design(&ch, &Limits::default()).unwrap();
        assert_eq!(map.rate(), 56000);
        assert!(map.is_valid());
    }

    #[test]
    fn le_bruit_reduit_le_debit() {
        let mut prev = 56001;
        for sigma in [1.0, 20.0, 60.0, 150.0, 300.0] {
            let ch = Channel::nominal(Law::A, sigma);
            let map = design(&ch, &Limits::default()).unwrap();
            assert!(map.is_valid());
            assert!(map.rate() <= prev, "σ {sigma} : {} bit/s", map.rate());
            prev = map.rate();
            for s in &map.sets {
                assert!(min_distance(s, &ch.levels) >= 9.0 * sigma - 1e-9);
            }
        }
        assert!(prev < 50000);
        // trop de bruit : pas de V.90
        assert!(design(&Channel::nominal(Law::A, 3000.0), &Limits::default()).is_none());
    }
}
