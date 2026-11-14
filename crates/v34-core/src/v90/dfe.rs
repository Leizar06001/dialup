//! Égaliseur à retour de décision (DFE) et annuleur d'écho du récepteur PCM.
//!
//! Le signal descendant est en bande de base, un symbole par échantillon
//! (8 kHz). La boucle analogique (filtres de la ligne, transformateurs) en
//! coupe le continu et le haut de bande : un égaliseur linéaire amplifierait
//! le bruit à ces fréquences ; la contre-réaction sur les décisions passées
//! efface au contraire la longue traîne du passe-haut sans bruit ajouté.
//!
//! Les coefficients sont calculés d'un coup par moindres carrés sur une
//! séquence connue (équations normales, Cholesky), puis suivis par un petit
//! gradient (LMS) pendant les données. Unités : celles des échantillons du
//! modem numérique (Ucode linéaire / 32767), si bien que l'erreur mesurée se
//! compare directement aux écarts entre niveaux PCM.

use std::collections::VecDeque;

/// Résout `a·x = b` (a symétrique définie positive, n×n en ligne). Ajoute
/// `ridge` à la diagonale. `None` si la matrice est dégénérée.
pub fn solve_spd(a: &mut [f64], b: &mut [f64], n: usize, ridge: f64) -> Option<()> {
    for i in 0..n {
        a[i * n + i] += ridge;
    }
    // Cholesky : a = L·Lᵀ (L dans le triangle inférieur)
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d <= 0.0 || !d.is_finite() {
            return None;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in j + 1..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = s / d;
        }
    }
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * n + k] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in i + 1..n {
            s -= a[k * n + i] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    Some(())
}

/// Égaliseur : FFE de `nf` coefficients + retour de `nb` décisions.
#[derive(Clone, Debug)]
pub struct Dfe {
    pub nf: usize,
    pub nb: usize,
    /// Position du symbole courant dans la fenêtre de l'FFE (échantillons
    /// après lui) : laisse voir les échos précurseurs
    pub cursor: usize,
    f: Vec<f64>,
    b: Vec<f64>,
    rx: VecDeque<f64>,
    past: VecDeque<f64>,
    /// Pas du suivi LMS (0 : figé)
    pub mu: f64,
    /// Puissance moyenne d'entrée (normalisation du LMS)
    pow: f64,
}

impl Dfe {
    pub fn new(nf: usize, nb: usize, cursor: usize) -> Dfe {
        assert!(cursor < nf);
        Dfe {
            nf,
            nb,
            cursor,
            f: vec![0.0; nf],
            b: vec![0.0; nb],
            rx: VecDeque::from(vec![0.0; nf]),
            past: VecDeque::from(vec![0.0; nb]),
            mu: 0.0,
            pow: 1e-6,
        }
    }

    /// Calcul des coefficients sur une séquence d'apprentissage : `rx[t]`
    /// échantillons reçus, `sym[n]` symboles émis (même échelle que la
    /// sortie voulue), le symbole n arrivant à l'échantillon `n + delay`.
    /// Utilise les symboles `from..to`. Renvoie l'erreur quadratique moyenne.
    pub fn train(&mut self, rx: &[f32], sym: &[f64], delay: usize, from: usize, to: usize) -> Option<f64> {
        let (nf, nb) = (self.nf, self.nb);
        let n = nf + nb;
        let mut a = vec![0.0; n * n];
        let mut c = vec![0.0; n];
        let mut u = vec![0.0; n];
        let mut rows = 0usize;
        let mut pow = 0.0;
        for k in from.max(nb)..to {
            // dernier échantillon de la fenêtre : k + delay + cursor
            let last = k + delay + self.cursor;
            if last >= rx.len() || last + 1 < nf {
                continue;
            }
            for (j, uj) in u.iter_mut().take(nf).enumerate() {
                *uj = rx[last - j] as f64;
            }
            for j in 0..nb {
                u[nf + j] = -sym[k - 1 - j];
            }
            for i in 0..n {
                c[i] += u[i] * sym[k];
                for j in 0..=i {
                    a[i * n + j] += u[i] * u[j];
                }
            }
            pow += u[0] * u[0];
            rows += 1;
        }
        if rows < n * 2 {
            return None;
        }
        for i in 0..n {
            for j in 0..i {
                a[j * n + i] = a[i * n + j];
            }
        }
        let scale = (0..n).map(|i| a[i * n + i]).sum::<f64>() / n as f64;
        let mut w = c.clone();
        solve_spd(&mut a, &mut w, n, scale * 1e-7 + 1e-15)?;
        self.f.copy_from_slice(&w[..nf]);
        self.b.copy_from_slice(&w[nf..]);
        self.pow = (pow / rows as f64).max(1e-12);
        // erreur résiduelle sur les mêmes données
        let mut e2 = 0.0;
        for k in from.max(nb)..to {
            let last = k + delay + self.cursor;
            if last >= rx.len() || last + 1 < nf {
                continue;
            }
            let mut y = 0.0;
            for j in 0..nf {
                y += self.f[j] * rx[last - j] as f64;
            }
            for j in 0..nb {
                y -= self.b[j] * sym[k - 1 - j];
            }
            e2 += (sym[k] - y) * (sym[k] - y);
        }
        Some(e2 / rows as f64)
    }

    /// Remet l'historique à zéro (coefficients gardés).
    pub fn clear(&mut self) {
        self.rx.iter_mut().for_each(|x| *x = 0.0);
        self.past.iter_mut().for_each(|x| *x = 0.0);
    }

    /// Ajoute un échantillon reçu.
    pub fn push(&mut self, r: f32) {
        self.rx.pop_back();
        self.rx.push_front(r as f64);
    }

    /// Sortie de l'égaliseur pour le symbole attendu `cursor` échantillons
    /// avant le dernier reçu.
    pub fn output(&self) -> f64 {
        let ff: f64 = self.f.iter().zip(&self.rx).map(|(f, x)| f * x).sum();
        let fb: f64 = self.b.iter().zip(&self.past).map(|(b, d)| b * d).sum();
        ff - fb
    }

    /// Enregistre la décision prise pour la dernière sortie `y` (et suit le
    /// canal si `mu > 0`).
    pub fn decided(&mut self, y: f64, d: f64) {
        if self.mu > 0.0 {
            let e = d - y;
            let g = self.mu * e / (self.pow * self.nf as f64);
            for (f, x) in self.f.iter_mut().zip(&self.rx) {
                *f += g * x;
            }
            let gb = self.mu * e / (self.pow * self.nf as f64);
            for (b, p) in self.b.iter_mut().zip(&self.past) {
                *b -= gb * p;
            }
        }
        self.past.pop_back();
        self.past.push_front(d);
    }
}

/// Annuleur d'écho : modèle FIR de l'écho de notre propre émission.
#[derive(Clone, Debug)]
pub struct EchoCanceller {
    w: Vec<f64>,
    hist: VecDeque<f64>,
    pub mu: f64,
    pow: f64,
}

impl EchoCanceller {
    pub fn new(taps: usize) -> EchoCanceller {
        EchoCanceller { w: vec![0.0; taps], hist: VecDeque::from(vec![0.0; taps]), mu: 0.0, pow: 1e-6 }
    }

    /// Apprentissage pendant que l'autre bout se tait : `rx[t]` ne contient
    /// que l'écho de `tx[t]` (et du bruit). Renvoie la puissance résiduelle
    /// relative (écho après annulation / écho avant).
    pub fn train(&mut self, tx: &[f32], rx: &[f32]) -> Option<f64> {
        let n = self.w.len();
        let mut a = vec![0.0; n * n];
        let mut c = vec![0.0; n];
        let mut before = 0.0;
        let mut pow = 0.0;
        for t in n..tx.len().min(rx.len()) {
            for i in 0..n {
                let xi = tx[t - i] as f64;
                c[i] += xi * rx[t] as f64;
                for j in 0..=i {
                    a[i * n + j] += xi * tx[t - j] as f64;
                }
            }
            before += (rx[t] as f64).powi(2);
            pow += (tx[t] as f64).powi(2);
        }
        for i in 0..n {
            for j in 0..i {
                a[j * n + i] = a[i * n + j];
            }
        }
        let scale = (0..n).map(|i| a[i * n + i]).sum::<f64>() / n as f64;
        let mut w = c;
        solve_spd(&mut a, &mut w, n, scale * 1e-8 + 1e-15)?;
        self.w = w;
        let rows = (tx.len().min(rx.len()) - n) as f64;
        self.pow = (pow / rows).max(1e-12);
        let mut after = 0.0;
        for t in n..tx.len().min(rx.len()) {
            let est: f64 = (0..n).map(|i| self.w[i] * tx[t - i] as f64).sum();
            after += (rx[t] as f64 - est).powi(2);
        }
        Some(if before > 0.0 { after / before } else { 0.0 })
    }

    /// Retire l'écho estimé de l'échantillon reçu `rx`, `tx` étant
    /// l'échantillon émis au même instant.
    pub fn process(&mut self, tx: f32, rx: f32) -> f32 {
        self.hist.pop_back();
        self.hist.push_front(tx as f64);
        let est: f64 = self.w.iter().zip(&self.hist).map(|(w, x)| w * x).sum();
        let e = rx as f64 - est;
        if self.mu > 0.0 {
            let g = self.mu * e / (self.pow * self.w.len() as f64);
            for (w, x) in self.w.iter_mut().zip(&self.hist) {
                *w += g * x;
            }
        }
        e as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cholesky() {
        // [[4,2],[2,3]] x = [2,1] → x = [0.5, 0]
        let mut a = vec![4.0, 2.0, 2.0, 3.0];
        let mut b = vec![2.0, 1.0];
        solve_spd(&mut a, &mut b, 2, 0.0).unwrap();
        assert!((b[0] - 0.5).abs() < 1e-12 && b[1].abs() < 1e-12);
    }

    #[test]
    fn dfe_efface_une_traine() {
        // canal à longue traîne (passe-haut du 1er ordre) + précurseur
        let n = 6000;
        let mut seed = 7u64;
        let sym: Vec<f64> = (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((seed >> 33) % 7) as f64 * 0.1 - 0.3
            })
            .collect();
        let mut rx = vec![0f32; n + 10];
        let mut hp = 0.0;
        let mut prev = 0.0;
        for (t, &s) in sym.iter().enumerate() {
            // passe-haut y = x - x[-1] + 0.95 y[-1], retard 3
            let y = s - prev + 0.95 * hp;
            hp = y;
            prev = s;
            rx[t + 3] += y as f32;
            rx[t + 2] += (0.2 * y) as f32; // précurseur
        }
        let mut dfe = Dfe::new(16, 40, 4);
        let mse = dfe.train(&rx, &sym, 3, 100, 4000).unwrap();
        assert!(mse < 1e-6, "erreur résiduelle {mse}");
        // en continu, décisions exactes
        let mut errs = 0;
        for (t, &r) in rx.iter().enumerate() {
            dfe.push(r);
            let Some(k) = (t + 1).checked_sub(3 + dfe.cursor + 1) else { continue };
            if k >= n {
                break;
            }
            let y = dfe.output();
            let d = ((y + 0.3) / 0.1).round().clamp(0.0, 6.0) * 0.1 - 0.3;
            if (d - sym[k]).abs() > 1e-9 && k > 200 {
                errs += 1;
            }
            dfe.decided(y, d);
        }
        assert_eq!(errs, 0);
    }
}
