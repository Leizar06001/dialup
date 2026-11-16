//! Asservissement de la cadence locale sur l'horloge de l'autre côté.
//!
//! Chaque machine fabrique ses échantillons au rythme de sa propre horloge,
//! et deux horloges ne sont jamais exactement d'accord (quelques dizaines de
//! ppm). Sur un vrai réseau téléphonique, c'est le central qui donne
//! l'horloge à tout le monde. Ici, le récepteur surveille le niveau de son
//! tampon de gigue ([`crate::JitterBuffer::level`]) : s'il monte, il traite
//! ponctuellement un bloc de plus ; s'il baisse, un de moins. Aucun
//! échantillon n'est inventé ni jeté : les deux modems voient exactement le
//! même flux, sans glissement.

/// Écart moyen (trames) toléré avant de corriger
const DEADBAND: f32 = 2.5;
/// Écart (trames) au-delà duquel on rattrape à chaque tic
const CATCH_UP: f32 = 10.0;
/// Tics d'attente entre deux corrections ordinaires (200 ms)
const COOLDOWN: u32 = 10;
/// Lissage du niveau (par tic de 20 ms : constante de temps ~0,4 s)
const SMOOTH: f32 = 0.05;
/// Mode ancré : tics de lecture avant de relever le niveau de référence (1 s)
const SETTLE: u32 = 50;

#[derive(Clone, Debug)]
pub struct Pacer {
    avg: Option<f32>,
    wait: u32,
    deadband: f32,
    /// Référence = niveau du début plutôt que la cible du tampon
    anchored: bool,
    reference: Option<f32>,
    settle: u32,
    /// Corrections appliquées (blocs en plus, blocs en moins)
    pub extra: u64,
    pub skipped: u64,
}

impl Default for Pacer {
    fn default() -> Self {
        Self { avg: None, wait: 0, deadband: DEADBAND, anchored: false, reference: None, settle: 0, extra: 0, skipped: 0 }
    }
}

impl Pacer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Face à une horloge matérielle (adaptateur téléphonique) : notre
    /// tampon est gardé au niveau qu'il avait après une seconde de lecture,
    /// à `frames` trames près. Rien ne se perd en route : ce que notre
    /// tampon gagne, celui de l'émetteur le perd. Le garder à son niveau de
    /// départ garde donc aussi celui de l'adaptateur, petit et fixe, au sien
    /// (avec la cible pour référence, il pourrait s'en écarter de plusieurs
    /// trames avant la première correction).
    pub fn anchored(frames: f32) -> Self {
        Self { deadband: frames.max(0.5), anchored: true, ..Self::default() }
    }

    /// À appeler une fois par tic de 20 ms avec le niveau du tampon
    /// (trames présentes, cible). Renvoie la correction en blocs à ajouter au
    /// bloc normal du tic : +1, 0 ou −1.
    pub fn update(&mut self, level: Option<(usize, usize)>) -> f64 {
        let Some((depth, target)) = level else {
            self.avg = None;
            self.wait = 0;
            self.reference = None;
            self.settle = 0;
            return 0.0;
        };
        // Tampon vide : coupure du réseau, la rafale n'est pas encore
        // arrivée. Ce niveau ne dit rien de l'horloge de l'autre ; le suivre
        // nous ferait sauter des blocs, à rattraper aussitôt après.
        if depth == 0 {
            return 0.0;
        }
        let mut avg = self.avg.unwrap_or(depth as f32);
        avg += (depth as f32 - avg) * SMOOTH;
        self.avg = Some(avg);
        let reference = match (self.anchored, self.reference) {
            (false, _) => target as f32,
            (true, Some(r)) => r,
            (true, None) => {
                self.settle += 1;
                if self.settle >= SETTLE {
                    self.reference = Some(avg);
                }
                return 0.0;
            }
        };
        if self.wait > 0 {
            self.wait -= 1;
            return 0.0;
        }
        let err = avg - reference;
        if err > self.deadband {
            // Trop plein : l'autre va plus vite, ou on a pris du retard.
            self.avg = Some(avg - 1.0);
            self.wait = if err > CATCH_UP { 0 } else { COOLDOWN };
            self.extra += 1;
            1.0
        } else if err < -self.deadband {
            self.avg = Some(avg + 1.0);
            self.wait = COOLDOWN;
            self.skipped += 1;
            -1.0
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_a_faster_sender() {
        // L'émetteur produit 1 % de trames en plus : le tampon monte, le
        // récepteur rattrape avec des blocs supplémentaires et le niveau
        // reste près de la cible.
        let mut p = Pacer::new();
        let (mut depth, target) = (7.0f64, 7usize);
        let mut produced = 0.0f64;
        for _ in 0..30_000 {
            produced += 1.01;
            while produced >= 1.0 {
                depth += 1.0;
                produced -= 1.0;
            }
            let blocks = 1.0 + p.update(Some((depth.max(0.0) as usize, target)));
            depth -= blocks;
            assert!((depth - target as f64).abs() < 8.0, "niveau {depth}");
        }
        assert!(p.extra > 250 && p.skipped < 20, "{} / {}", p.extra, p.skipped);
    }

    #[test]
    fn anchored_keeps_the_starting_level() {
        // Émetteur 1 % plus rapide, tampon parti SOUS sa cible (5 pour 7) :
        // ancré, le niveau reste à une ou deux trames de son départ (avec
        // la cible pour référence, il monterait d'abord de trois trames).
        let mut p = Pacer::anchored(1.0);
        let (mut depth, target) = (5.0f64, 7usize);
        let mut produced = 0.0f64;
        let mut worst = 0.0f64;
        for _ in 0..30_000 {
            produced += 1.01;
            while produced >= 1.0 {
                depth += 1.0;
                produced -= 1.0;
            }
            let blocks = 1.0 + p.update(Some((depth.max(0.0) as usize, target)));
            depth -= blocks;
            worst = worst.max((depth - 5.0).abs());
        }
        assert!(worst <= 2.0, "écart maximal {worst}");
        assert!(p.extra > 250, "{}", p.extra);
    }

    #[test]
    fn ignores_short_jitter() {
        // Un pic de gigue isolé (tampon vide 3 tics puis rempli d'un coup)
        // ne déclenche aucune correction.
        let mut p = Pacer::new();
        let mut c = 0.0;
        for t in 0..500 {
            let depth = match t % 100 {
                50..=52 => 4,
                53 => 10,
                _ => 7,
            };
            c += p.update(Some((depth, 7))).abs();
        }
        assert_eq!(c, 0.0);
    }

    #[test]
    fn holds_during_an_outage() {
        // Coupure du réseau d'une demi-seconde (tampon vide 27 tics), puis
        // la rafale le remet à son niveau : aucune correction, ancré ou non.
        for mut p in [Pacer::new(), Pacer::anchored(1.0)] {
            let mut c = 0.0;
            for t in 0..600 {
                let depth = if (300..327).contains(&t) { 0 } else { 7 };
                c += p.update(Some((depth, 7))).abs();
            }
            assert_eq!(c, 0.0);
        }
    }
}
