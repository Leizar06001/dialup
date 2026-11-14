//! Mode données V.34 (§8 tramage, §9 codeur) et décodeur correspondant.
//!
//! Chaîne d'émission (sans précodage ni codage non linéaire, Θ = 0) :
//! bits → brouilleur → parseur (trames de mise en correspondance) →
//! shell mapper (K bits → 8 indices d'anneau) → codeur différentiel →
//! mapper (étiquette du quart de superconstellation + rotation) →
//! codeur en treillis 16 états (bit U0) → points 2D.
//!
//! Le décodeur décide chaque point (le plus proche), puis inverse
//! exactement la chaîne. Le treillis est « recalculé » à partir des
//! décisions (pas encore de décodage de Viterbi : on n'exploite pas le
//! gain de codage, d'où un choix de débit prudent).

use std::collections::HashMap;

use crate::dsp::Cpx;
use crate::train::{quarter_point, Scrambler};

/// Trames de données par supertrame (Table 7).
pub const J_FRAMES: [usize; 6] = [7, 8, 7, 7, 7, 8];
/// Trames de mise en correspondance par trame de données (Table 7).
pub const P_FRAMES: [usize; 6] = [12, 12, 14, 15, 16, 15];

/// Paramètres de tramage et de constellation pour un couple
/// (vitesse de symbole, débit) — §8.2, §9.2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataParams {
    pub sr_idx: usize,
    pub rate: u32,
    pub j: usize,
    pub p: usize,
    /// Bits par trame de données
    pub n: usize,
    /// Bits par trame « haute »
    pub b: usize,
    /// Trames hautes par trame de données
    pub r: usize,
    /// Motif de commutation : true = trame haute
    pub swp: Vec<bool>,
    pub k: usize,
    pub q: usize,
    pub m: usize,
    /// Nombre de points de la constellation 2D
    pub l: usize,
}

impl DataParams {
    pub fn new(sr_idx: usize, rate: u32, expanded: bool) -> Self {
        let j = J_FRAMES[sr_idx];
        let p = P_FRAMES[sr_idx];
        let n = rate as usize * 28 / (100 * j); // N = R·0,28/J (8-1)
        let b = n.div_ceil(p);
        let r = n - (b - 1) * p;
        let mut swp = Vec::with_capacity(p);
        let mut c = 0;
        for _ in 0..p {
            c += r;
            if c < p {
                swp.push(false);
            } else {
                swp.push(true);
                c -= p;
            }
        }
        let (k, q) = if b <= 12 {
            (0, 0)
        } else {
            let mut q = 0;
            loop {
                let k = b - 12 - 8 * q;
                if k < 32 {
                    break (k, q);
                }
                q += 1;
            }
        };
        let base = 2f64.powf(k as f64 / 8.0);
        let m_min = base.ceil() as usize;
        let m_exp = ((1.25 * base).round() as usize).max(m_min);
        let m = if k == 0 { 1 } else if expanded { m_exp } else { m_min };
        let l = 4 * m * (1 << q);
        Self { sr_idx, rate, j, p, n, b, r, swp, k, q, m, l }
    }

    /// Le débit est-il réalisable à cette vitesse de symbole (Table 8) ?
    /// 2400 bit/s n'existe qu'à 2400 bauds (sinon moins de 8 bits/trame).
    pub fn valid(sr_idx: usize, rate: u32) -> bool {
        let max = crate::probe::MAX_RATE_FOR_SR[sr_idx];
        let min = if sr_idx == 0 { 2400 } else { 4800 };
        rate >= min && rate <= max && rate % 2400 == 0
    }
}

// ---------------------------------------------------------------------------
// Shell mapper (§9.4)
// ---------------------------------------------------------------------------

/// Tables g2, g4, g8, z8 pour un nombre d'anneaux M.
#[derive(Clone, Debug)]
pub struct Shell {
    m: usize,
    g2: Vec<u64>,
    g4: Vec<u64>,
    z8: Vec<u64>,
}

impl Shell {
    pub fn new(m: usize) -> Self {
        let g2: Vec<u64> = (0..=2 * (m - 1)).map(|p| (m as i64 - (p as i64 - m as i64 + 1).abs()) as u64).collect();
        let conv = |a: &[u64]| -> Vec<u64> {
            let n = 2 * (a.len() - 1);
            (0..=n).map(|p| (0..=p).filter(|&i| i < a.len() && p - i < a.len()).map(|i| a[i] * a[p - i]).sum()).collect()
        };
        let g4 = conv(&g2);
        let g8 = conv(&g4);
        let mut z8 = Vec::with_capacity(g8.len() + 1);
        let mut acc = 0u64;
        for &v in &g8 {
            z8.push(acc);
            acc += v;
        }
        z8.push(acc);
        Self { m, g2, g4, z8 }
    }

    fn g2(&self, p: i64) -> u64 {
        if p < 0 || p as usize >= self.g2.len() { 0 } else { self.g2[p as usize] }
    }

    fn g4(&self, p: i64) -> u64 {
        if p < 0 || p as usize >= self.g4.len() { 0 } else { self.g4[p as usize] }
    }

    /// K bits (entier R0) → 8 indices d'anneau [m00, m01, m10, m11, …].
    pub fn map(&self, r0: u64) -> [usize; 8] {
        let m = self.m as i64;
        // A : plus grand entier tel que z8(A) ≤ R0
        let mut a = 0usize;
        while a + 1 < self.z8.len() - 1 && self.z8[a + 1] <= r0 {
            a += 1;
        }
        let a = a as i64;
        let (m2, m4) = (2 * (m - 1), 4 * (m - 1));
        // B : plus grand entier tel que R1 ≥ 0 (seuls les termes non nuls,
        // g4(B)·g4(A−B) > 0, comptent : B ∈ [max(0, A−4(M−1)), min(A, 4(M−1))])
        let mut r1 = r0 - self.z8[a as usize];
        let (b_lo, b_hi) = ((a - m4).max(0), a.min(m4));
        let mut b = b_lo;
        while b < b_hi && r1 >= self.g4(b) * self.g4(a - b) {
            r1 -= self.g4(b) * self.g4(a - b);
            b += 1;
        }
        let r2 = r1 % self.g4(b);
        let r3 = (r1 - r2) / self.g4(b);
        let mut r4 = r2;
        let (c_lo, c_hi) = ((b - m2).max(0), b.min(m2));
        let mut c = c_lo;
        while c < c_hi && r4 >= self.g2(c) * self.g2(b - c) {
            r4 -= self.g2(c) * self.g2(b - c);
            c += 1;
        }
        let mut r5 = r3;
        let ab = a - b;
        let (d_lo, d_hi) = ((ab - m2).max(0), ab.min(m2));
        let mut d = d_lo;
        while d < d_hi && r5 >= self.g2(d) * self.g2(ab - d) {
            r5 -= self.g2(d) * self.g2(ab - d);
            d += 1;
        }
        let e = (r4 % self.g2(c)) as i64;
        let f = ((r4 - e as u64) / self.g2(c)) as i64;
        let g = (r5 % self.g2(d)) as i64;
        let h = ((r5 - g as u64) / self.g2(d)) as i64;
        let pair = |sum: i64, x: i64| -> (usize, usize) {
            if sum < m {
                (x as usize, (sum - x) as usize)
            } else {
                let second = m - 1 - x;
                ((sum - second) as usize, second as usize)
            }
        };
        let (m00, m01) = pair(c, e);
        let (m10, m11) = pair(b - c, f);
        let (m20, m21) = pair(d, g);
        let (m30, m31) = pair(a - b - d, h);
        [m00, m01, m10, m11, m20, m21, m30, m31]
    }

    /// Inverse : 8 indices d'anneau → entier R0.
    pub fn demap(&self, r: &[usize; 8]) -> u64 {
        let m = self.m as i64;
        let ri: Vec<i64> = r.iter().map(|&x| x as i64).collect();
        let c = ri[0] + ri[1];
        let b = c + ri[2] + ri[3];
        let d = ri[4] + ri[5];
        let a = b + d + ri[6] + ri[7];
        let first = |sum: i64, x0: i64, x1: i64| -> i64 { if sum < m { x0 } else { m - 1 - x1 } };
        let e = first(c, ri[0], ri[1]);
        let f = first(b - c, ri[2], ri[3]);
        let g = first(d, ri[4], ri[5]);
        let h = first(a - b - d, ri[6], ri[7]);
        let r4 = e as u64 + f as u64 * self.g2(c);
        let r5 = g as u64 + h as u64 * self.g2(d);
        let r2 = r4 + (0..c).map(|p| self.g2(p) * self.g2(b - p)).sum::<u64>();
        let r3 = r5 + (0..d).map(|p| self.g2(p) * self.g2(a - b - p)).sum::<u64>();
        let r1 = r2 + r3 * self.g4(b);
        r1 + self.z8.get(a as usize).copied().unwrap_or(0) + (0..b).map(|p| self.g4(p) * self.g4(a - p)).sum::<u64>()
    }
}

// ---------------------------------------------------------------------------
// Treillis 16 états (§9.6.3, Figures 9 et 10, Table 13)
// ---------------------------------------------------------------------------

/// Étiquettes de sous-ensemble (Figure 9) : LABELS[ligne][colonne] avec
/// lignes Im = 3, 1, −1, −3 et colonnes Re = −3, −1, 1, 3 (période 8).
const LABELS: [[u8; 4]; 4] = [
    [0b001, 0b110, 0b101, 0b010],
    [0b100, 0b011, 0b000, 0b111],
    [0b101, 0b010, 0b001, 0b110],
    [0b000, 0b111, 0b100, 0b011],
];

/// Table 13 : (s(2m), s(2m+1)) → [Y4 Y3 Y2 Y1] (Y1 = bit 0).
const TABLE13: [[u8; 8]; 8] = [
    [0b0000, 0b0000, 0b0001, 0b0001, 0b1000, 0b1000, 0b1001, 0b1001],
    [0b0011, 0b0010, 0b0010, 0b0011, 0b1011, 0b1010, 0b1010, 0b1011],
    [0b0101, 0b0101, 0b0100, 0b0100, 0b1101, 0b1101, 0b1100, 0b1100],
    [0b0110, 0b0111, 0b0111, 0b0110, 0b1110, 0b1111, 0b1111, 0b1110],
    [0b1000, 0b1000, 0b1001, 0b1001, 0b0000, 0b0000, 0b0001, 0b0001],
    [0b1011, 0b1010, 0b1010, 0b1011, 0b0011, 0b0010, 0b0010, 0b0011],
    [0b1101, 0b1101, 0b1100, 0b1100, 0b0101, 0b0101, 0b0100, 0b0100],
    [0b1110, 0b1111, 0b1111, 0b1110, 0b0110, 0b0111, 0b0111, 0b0110],
];

pub fn subset_label(x: i32, y: i32) -> u8 {
    let col = ((x + 3) / 2).rem_euclid(4) as usize;
    let row = ((3 - y) / 2).rem_euclid(4) as usize;
    LABELS[row][col]
}

/// Codeur convolutionnel systématique 16 états (Figure 10).
#[derive(Clone, Copy, Debug, Default)]
pub struct Trellis16 {
    d: [bool; 4],
}

impl Trellis16 {
    /// Sortie Y0(m) (ne dépend que des entrées passées).
    pub fn y0(&self) -> bool {
        self.d[3]
    }

    /// Avance d'un intervalle 4D avec les deux points de sortie du canal.
    pub fn update(&mut self, y0pt: (i32, i32), y1pt: (i32, i32)) {
        let y0 = self.d[3];
        self.update_with_y0(y0pt, y1pt, y0);
    }

    /// Côté réception : avance avec le Y0 OBSERVÉ dans le symbole reçu (la
    /// parité des rotations des deux points), au lieu du Y0 mémorisé. Sans
    /// erreur c'est le même ; mais l'état ne dépend plus alors que des
    /// 4 derniers intervalles 4D : après une rafale d'erreurs (trou de
    /// signal, choc), le décodeur se recale tout seul au lieu de rester faux
    /// jusqu'au prochain réapprentissage.
    pub fn update_with_y0(&mut self, y0pt: (i32, i32), y1pt: (i32, i32), y0: bool) {
        let s0 = subset_label(y0pt.0, y0pt.1) as usize;
        let s1 = subset_label(y1pt.0, y1pt.1) as usize;
        let yb = TABLE13[s0][s1];
        let (y1, y2) = (yb & 1 == 1, yb & 2 == 2);
        let [d1, d2, d3, _] = self.d;
        self.d = [y0, d1 ^ y2 ^ y0, d2 ^ y2, d3 ^ y1];
    }
}

/// Motifs d'inversion de bit pour la synchro de supertrame (Table 12).
fn inversion_pattern(j: usize) -> &'static [bool] {
    const P8: [bool; 16] = [false, true, true, true, false, true, true, true, true, true, true, true, true, false, true, false];
    const P7: [bool; 14] = [false, true, true, true, false, true, true, true, true, true, true, true, true, false];
    if j == 8 { &P8 } else { &P7 }
}

// ---------------------------------------------------------------------------
// Puissance moyenne de la constellation (avec la loi d'anneaux du shell)
// ---------------------------------------------------------------------------

/// Puissance moyenne des points émis (coordonnées entières). Estimée en
/// tirant des trames pseudo-aléatoires (graine fixe : émetteur et récepteur
/// obtiennent exactement la même valeur).
pub fn average_power(p: &DataParams) -> f32 {
    let ring_size = 1usize << p.q;
    let ring_power: Vec<f32> = (0..p.m)
        .map(|r| (0..ring_size).map(|i| quarter_point(r * ring_size + i).norm_sqr()).sum::<f32>() / ring_size as f32)
        .collect();
    if p.k == 0 {
        return ring_power[0];
    }
    let shell = Shell::new(p.m);
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut hist = vec![0u64; p.m];
    for _ in 0..4096 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let r0 = seed & ((1u64 << p.k) - 1);
        for idx in shell.map(r0) {
            hist[idx] += 1;
        }
    }
    let total: u64 = hist.iter().sum();
    hist.iter().zip(&ring_power).map(|(&h, &pw)| h as f32 / total as f32 * pw).sum()
}

// ---------------------------------------------------------------------------
// Codeur
// ---------------------------------------------------------------------------

pub struct DataEncoder {
    pub prm: DataParams,
    shell: Shell,
    scr: Scrambler,
    z: u32,
    trellis: Trellis16,
    /// Intervalle 4D courant depuis le début de B1
    m4d: u64,
    /// Indice de la trame de mise en correspondance dans la trame de données
    frame_in_df: usize,
    /// Trames de B1 restant à émettre (uns brouillés)
    b1_left: usize,
    pub power: f32,
}

impl DataEncoder {
    /// `call` : brouilleur de l'appelant (GPC) ou de l'appelé (GPA).
    pub fn new(prm: DataParams, call: bool) -> Self {
        let power = average_power(&prm);
        Self { shell: Shell::new(prm.m), b1_left: prm.p, prm, scr: Scrambler::new(call), z: 0, trellis: Trellis16::default(), m4d: 0, frame_in_df: 0, power }
    }

    pub fn in_b1(&self) -> bool {
        self.b1_left > 0
    }

    /// Bits utilisateur consommés par la prochaine trame (b ou b − 1).
    pub fn next_frame_bits(&self) -> usize {
        if self.prm.swp[self.frame_in_df] { self.prm.b } else { self.prm.b - 1 }
    }

    /// Produit les 8 points (coordonnées entières) de la trame suivante.
    /// `src` fournit les bits utilisateur (non brouillés) ; pendant B1 il
    /// n'est pas appelé (on émet des uns).
    pub fn frame(&mut self, src: &mut dyn FnMut() -> bool) -> [(i32, i32); 8] {
        let p = &self.prm;
        let high = p.swp[self.frame_in_df];
        let nb = if high { p.b } else { p.b - 1 };
        let b1 = self.b1_left > 0;
        let mut bits: Vec<bool> = (0..nb)
            .map(|_| {
                let u = if b1 { true } else { src() };
                self.scr.scramble(u)
            })
            .collect();
        let mut pos = 0usize;
        let mut take = |n: usize, bits: &mut Vec<bool>| -> u64 {
            let mut v = 0u64;
            for i in 0..n {
                v |= (bits[pos + i] as u64) << i;
            }
            pos += n;
            v
        };
        let p = self.prm.clone();
        let rings: [usize; 8];
        let mut groups = [(false, false, false, [0u64; 2]); 4];
        if p.b > 12 {
            // Parseur §9.3.1 : en trame basse, un 0 est inséré après K − 1 bits
            let s = if high {
                take(p.k, &mut bits)
            } else {
                take(p.k - 1, &mut bits)
            };
            rings = self.shell.map(s);
            for g in groups.iter_mut() {
                let i1 = take(1, &mut bits) == 1;
                let i2 = take(1, &mut bits) == 1;
                let i3 = take(1, &mut bits) == 1;
                let q0 = take(p.q, &mut bits);
                let q1 = take(p.q, &mut bits);
                *g = (i1, i2, i3, [q0, q1]);
            }
        } else {
            // §9.3.2 : 8, 9, 11 ou 12 bits ; I3 présents dans les premiers groupes
            rings = [0; 8];
            let n3 = nb.saturating_sub(8);
            for (jj, g) in groups.iter_mut().enumerate() {
                let i1 = take(1, &mut bits) == 1;
                let i2 = take(1, &mut bits) == 1;
                let i3 = if jj < n3 { take(1, &mut bits) == 1 } else { false };
                *g = (i1, i2, i3, [0, 0]);
            }
        }
        let _ = &mut bits;
        let mut out = [(0i32, 0i32); 8];
        let inv = inversion_pattern(p.j);
        for (jj, &(i1, i2, i3, q)) in groups.iter().enumerate() {
            let i = i2 as u32 + 2 * i3 as u32;
            self.z = (self.z + i) & 3;
            let lbl0 = (q[0] as usize) | (rings[2 * jj] << p.q);
            let lbl1 = (q[1] as usize) | (rings[2 * jj + 1] << p.q);
            let u0 = quarter_point(lbl0).rot_cw(self.z);
            // V0 : inversion au début de chaque demi-trame de données.
            // B1 joue le rôle de la dernière trame d'une supertrame.
            let half = 2 * p.p as u64;
            let v0 = if self.m4d % half == 0 {
                let h = ((self.m4d / half) as usize + 2 * p.j - 2) % (2 * p.j);
                inv[h]
            } else {
                false
            };
            let u0bit = self.trellis.y0() ^ v0;
            let u1 = quarter_point(lbl1).rot_cw(self.z + 2 * i1 as u32 + u0bit as u32);
            let a = (u0.re as i32, u0.im as i32);
            let b = (u1.re as i32, u1.im as i32);
            self.trellis.update(a, b);
            out[2 * jj] = a;
            out[2 * jj + 1] = b;
            self.m4d += 1;
        }
        self.frame_in_df = (self.frame_in_df + 1) % p.p;
        if self.b1_left > 0 {
            self.b1_left -= 1;
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Décodeur
// ---------------------------------------------------------------------------

pub struct DataDecoder {
    pub prm: DataParams,
    shell: Shell,
    descr: Scrambler,
    z_prev: u32,
    trellis: Trellis16,
    m4d: u64,
    frame_in_df: usize,
    b1_left: usize,
    /// Points décidés de la trame en cours
    buf: Vec<(i32, i32)>,
    /// (x, y) → (étiquette dans le quart, rotation horaire)
    lookup: HashMap<(i32, i32), (u16, u8)>,
    pts: Vec<(i32, i32)>,
    pub scale: f32,
    /// Incohérences détectées (rotation impaire, anneau hors plage…)
    pub errors: u64,
}

impl DataDecoder {
    /// `remote_call` : true si l'autre modem est l'appelant (brouilleur GPC).
    pub fn new(prm: DataParams, remote_call: bool) -> Self {
        let mut lookup = HashMap::new();
        let mut pts = Vec::new();
        for lbl in 0..prm.l / 4 {
            for rot in 0..4u32 {
                let p = quarter_point(lbl).rot_cw(rot);
                let key = (p.re as i32, p.im as i32);
                lookup.insert(key, (lbl as u16, rot as u8));
                pts.push(key);
            }
        }
        let scale = 1.0 / average_power(&prm).sqrt();
        Self { shell: Shell::new(prm.m), b1_left: prm.p, prm, descr: Scrambler::new(remote_call), z_prev: 0, trellis: Trellis16::default(), m4d: 0, frame_in_df: 0, buf: Vec::with_capacity(8), lookup, pts, scale, errors: 0 }
    }

    pub fn in_b1(&self) -> bool {
        self.b1_left > 0
    }

    /// Intervalles 4D décodés depuis B1 (pour les statistiques d'erreurs).
    pub fn symbols4d(&self) -> u64 {
        self.m4d
    }

    /// Décision : point de la constellation le plus proche de `z`
    /// (normalisé, puissance moyenne 1). Renvoie le point entier.
    pub fn decide(&self, z: Cpx) -> (i32, i32) {
        let x = z.re / self.scale;
        let y = z.im / self.scale;
        let rx = 2 * ((x - 1.0) / 2.0).round() as i32 + 1;
        let ry = 2 * ((y - 1.0) / 2.0).round() as i32 + 1;
        if self.lookup.contains_key(&(rx, ry)) {
            return (rx, ry);
        }
        // Hors constellation (bord) : recherche exhaustive
        let mut best = self.pts[0];
        let mut bd = f32::MAX;
        for &(px, py) in &self.pts {
            let d = (px as f32 - x).powi(2) + (py as f32 - y).powi(2);
            if d < bd {
                bd = d;
                best = (px, py);
            }
        }
        best
    }

    pub fn to_cpx(&self, p: (i32, i32)) -> Cpx {
        Cpx::new(p.0 as f32 * self.scale, p.1 as f32 * self.scale)
    }

    /// Ajoute un symbole reçu ; renvoie le point décidé (normalisé, pour
    /// l'égaliseur) et, en fin de trame, les bits utilisateur décodés.
    pub fn push(&mut self, z: Cpx) -> (Cpx, Option<Vec<bool>>) {
        let d = self.decide(z);
        self.buf.push(d);
        let out = if self.buf.len() == 8 { Some(self.decode_frame()) } else { None };
        (self.to_cpx(d), out)
    }

    fn decode_frame(&mut self) -> Vec<bool> {
        let p = self.prm.clone();
        let high = p.swp[self.frame_in_df];
        let nb = if high { p.b } else { p.b - 1 };
        let inv = inversion_pattern(p.j);
        let mut rings = [0usize; 8];
        let mut groups = [(false, false, false, [0u64; 2]); 4];
        let qmask = (1u64 << p.q) - 1;
        for jj in 0..4 {
            let a = self.buf[2 * jj];
            let b = self.buf[2 * jj + 1];
            let (l0, rot0) = self.lookup[&a];
            let (l1, rot1) = self.lookup[&b];
            let half = 2 * p.p as u64;
            let v0 = if self.m4d % half == 0 {
                let h = ((self.m4d / half) as usize + 2 * p.j - 2) % (2 * p.j);
                inv[h]
            } else {
                false
            };
            let u0bit = self.trellis.y0() ^ v0;
            let z = rot0 as u32;
            let t = (rot1 as u32 + 8 - z - u0bit as u32) & 3;
            if t & 1 == 1 {
                self.errors += 1;
            }
            let i1 = t >> 1 == 1;
            let i = (z + 4 - self.z_prev) & 3;
            self.z_prev = z;
            let (i2, i3) = (i & 1 == 1, i & 2 == 2);
            // Y0 observé : U0 = parité de (rot1 − rot0), Y0 = U0 ⊕ V0.
            let y0_vu = ((rot1 as u32 + 4 - z) & 1 == 1) ^ v0;
            self.trellis.update_with_y0(a, b, y0_vu);
            self.m4d += 1;
            rings[2 * jj] = (l0 as usize) >> p.q;
            rings[2 * jj + 1] = (l1 as usize) >> p.q;
            groups[jj] = (i1, i2, i3, [l0 as u64 & qmask, l1 as u64 & qmask]);
        }
        self.buf.clear();
        // Réassemblage des bits dans l'ordre du parseur
        let mut bits = Vec::with_capacity(nb);
        let push = |v: u64, n: usize, bits: &mut Vec<bool>| {
            for i in 0..n {
                bits.push((v >> i) & 1 == 1);
            }
        };
        if p.b > 12 {
            if rings.iter().any(|&r| r >= p.m) {
                self.errors += 1;
            }
            let rings_ok = rings.map(|r| r.min(p.m - 1));
            let s = self.shell.demap(&rings_ok);
            push(s, if high { p.k } else { p.k - 1 }, &mut bits);
            for &(i1, i2, i3, q) in &groups {
                push(i1 as u64, 1, &mut bits);
                push(i2 as u64, 1, &mut bits);
                push(i3 as u64, 1, &mut bits);
                push(q[0], p.q, &mut bits);
                push(q[1], p.q, &mut bits);
            }
        } else {
            let n3 = nb.saturating_sub(8);
            for (jj, &(i1, i2, i3, _)) in groups.iter().enumerate() {
                push(i1 as u64, 1, &mut bits);
                push(i2 as u64, 1, &mut bits);
                if jj < n3 {
                    push(i3 as u64, 1, &mut bits);
                }
            }
        }
        bits.truncate(nb);
        let out: Vec<bool> = bits.iter().map(|&b| self.descr.descramble(b)).collect();
        self.frame_in_df = (self.frame_in_df + 1) % p.p;
        if self.b1_left > 0 {
            self.b1_left -= 1;
            return Vec::new(); // B1 : uns brouillés, rien à livrer
        }
        out
    }
}

/// Débit max (multiple de 2400) qu'un SNR d'égaliseur permet à cette
/// vitesse de symbole, pour notre décodeur (décision point par point :
/// pas de gain de codage). Cible : taux d'erreur symbole ≈ 1e-5 + marge.
pub fn max_rate_for_snr(sr_idx: usize, snr_db: f32, margin_db: f32) -> u32 {
    let mut best = 0;
    let mut rate = if sr_idx == 0 { 2400 } else { 4800 };
    while rate <= crate::probe::MAX_RATE_FOR_SR[sr_idx] {
        let p = DataParams::new(sr_idx, rate, false);
        // QAM non codée : SNR ≥ (L − 1)·Q⁻¹(1e-5/4)²/3 ≈ (L−1)·6,9
        let need = 10.0 * ((p.l as f32 - 1.0).max(1.0) * 6.9).log10();
        if snr_db >= need + margin_db {
            best = rate;
        }
        rate += 2400;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_match_table10() {
        // (vitesse, débit) → (K, M min, L min) — extraits de la Table 10
        let cases = [
            (0, 2400, 0, 1, 4),
            (0, 4800, 4, 2, 8),
            (0, 21600, 28, 12, 768),
            (5, 33600, 27, 11, 1408),
            (4, 31200, 26, 10, 1280),
            (3, 28800, 25, 9, 1152),
            (1, 26400, 25, 9, 1152),
            (2, 19200, 27, 11, 176),
            (5, 4800, 0, 1, 4),
            (4, 9600, 12, 3, 12),
        ];
        for (sr, rate, k, m, l) in cases {
            let p = DataParams::new(sr, rate, false);
            assert_eq!((p.k, p.m, p.l), (k, m, l), "S={sr} R={rate}");
        }
        // SWP (Table 8) : 19 200 bit/s à 3000 bauds → 0421 (hex) sur 15 bits
        let p = DataParams::new(3, 19200, false);
        let swp: u32 = p.swp.iter().fold(0, |a, &h| (a << 1) | h as u32);
        assert_eq!((p.b, swp), (52, 0x0421));
        let p = DataParams::new(5, 33600, false);
        let swp: u32 = p.swp.iter().fold(0, |a, &h| (a << 1) | h as u32);
        assert_eq!((p.b, swp), (79, 0x14A5));
    }

    #[test]
    fn shell_roundtrip() {
        for &(k, m) in &[(4usize, 2usize), (12, 3), (27, 11), (31, 15), (28, 14), (25, 9), (31, 18)] {
            let s = Shell::new(m);
            let mut seed = 99u64;
            for _ in 0..3000 {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let r0 = seed & ((1u64 << k) - 1);
                let rings = s.map(r0);
                assert!(rings.iter().all(|&r| r < m), "anneau hors plage K={k} M={m}");
                assert_eq!(s.demap(&rings), r0, "K={k} M={m}");
            }
        }
    }

    #[test]
    fn encoder_decoder_roundtrip() {
        for &(sr, rate) in &[(5usize, 33600u32), (4, 31200), (0, 2400), (5, 4800), (3, 14400), (1, 26400), (2, 9600)] {
            let prm = DataParams::new(sr, rate, false);
            let mut enc = DataEncoder::new(prm.clone(), true);
            let mut dec = DataDecoder::new(prm.clone(), true);
            let mut seed = 7u32;
            let mut sent = Vec::new();
            let mut got = Vec::new();
            for _ in 0..(prm.p * 3) {
                let pts = enc.frame(&mut || {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let b = seed & 1 == 1;
                    sent.push(b);
                    b
                });
                for p in pts {
                    let z = Cpx::new(p.0 as f32 * dec.scale, p.1 as f32 * dec.scale);
                    let (_, bits) = dec.push(z);
                    if let Some(b) = bits {
                        got.extend(b);
                    }
                }
            }
            assert_eq!(dec.errors, 0, "S={sr} R={rate}");
            assert_eq!(got.len(), sent.len(), "S={sr} R={rate}");
            assert_eq!(got, sent, "S={sr} R={rate}");
            // Puissance réellement émise ≈ estimation
            assert!(enc.power > 1.0);
        }
    }

    #[test]
    fn rate_choice_is_sane() {
        assert_eq!(max_rate_for_snr(5, 47.0, 1.5), 33600);
        let r = max_rate_for_snr(5, 26.0, 1.5);
        assert!((9600..=16800).contains(&r), "{r}");
    }
}
