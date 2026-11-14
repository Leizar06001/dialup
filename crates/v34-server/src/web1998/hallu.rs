//! « V.Hallucination » : chaque image redessinée de mémoire en quelques
//! dizaines de formes (triangles et ellipses semi-transparents sur un fond
//! uni), envoyée en SVG de 2,5 Ko au plus au lieu d'un GIF de 15 Ko.
//!
//! Le résultat ressemble à l'original comme un rêve ressemble à la veille :
//! les couleurs et les masses y sont, les détails beaucoup moins. C'est
//! voulu. Et c'est garanti sans intelligence artificielle (ni naturelle) :
//! rien que de l'escalade de colline, à la manière de « primitive » ou de
//! « geometrize » :
//!
//! 1. l'image est réduite à une petite taille de travail (96 px de côté au
//!    plus), posée sur le gris des pages si elle est transparente ; le fond
//!    est sa couleur moyenne ;
//! 2. pour chaque nouvelle forme, quelques milliers de formes tirées au
//!    hasard ; la meilleure est ensuite affinée par petites mutations
//!    (déplacer un sommet, changer un rayon), gardées seulement si elles
//!    font mieux, jusqu'à une série d'échecs. « Mieux » se mesure en gain
//!    par octet de SVG : le budget est en octets, et un cercle s'écrit plus
//!    court qu'une ellipse ;
//! 3. la couleur ne se cherche pas : pour une forme et une opacité données,
//!    celle qui minimise l'erreur quadratique se calcule directement, puis
//!    est arrondie à la notation courte `#rgb` (4 096 couleurs, très 1998) ;
//! 4. la forme n'est ajoutée que si elle améliore l'image ; on s'arrête au
//!    nombre de formes voulu (moins pour une petite image) ou au poids visé.
//!
//! L'erreur d'une forme se calcule sans parcourir ses pixels : la forme est
//! découpée en segments horizontaux, et des sommes cumulées par ligne
//! (cible, image courante et leurs produits) donnent la somme sur un
//! segment en deux lectures. Évaluer une forme coûte de l'ordre de sa
//! hauteur en lignes : quelques centaines de milliers d'essais par image
//! tiennent en une dizaine de millisecondes (Mac récent ; compter trois ou
//! quatre fois plus sur un petit VPS).
//!
//! Déterministe : le hasard vient d'un générateur pseudo-aléatoire
//! initialisé par l'URL de l'image ([`seed`]) : même image, même rêve (le
//! cache et les tests y tiennent). Le travail est borné en nombre d'essais ;
//! un plafond de temps ([`HalluOptions::time_limit`]) ne sert qu'en dernier
//! recours, sur une machine très lente ou très chargée.
//!
//! SVG : `viewBox` à la taille de travail (coordonnées entières de deux
//! chiffres), `width`/`height` à la taille d'affichage du GIF équivalent,
//! formes dans un groupe d'opacité commune, avec un léger flou gaussien
//! pour l'effet « rêve » (60 octets).

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use image::{imageops, DynamicImage, RgbImage};

use super::images;

/// Gris des pages 1998 (`bgcolor="#c0c0c0"`) : fond des images transparentes.
const PAGE_GREY: [u32; 3] = [0xc0, 0xc0, 0xc0];
/// Angle minimal d'un triangle (cosinus de 15°) : pas d'aiguilles.
const MAX_COS: f32 = 0.966;
/// Gain minimal (somme des carrés) pour garder une forme.
const MIN_GAIN: f64 = 1.0;
/// Recherches infructueuses tolérées avant d'abandonner.
const MAX_FAILURES: usize = 3;
/// Formes au moins, même pour une icône.
const MIN_SHAPES: usize = 8;

/// Réglages de l'hallucination.
#[derive(Clone, Copy, Debug)]
pub struct HalluOptions {
    /// Taille d'affichage maximale (px), comme le GIF
    pub max_width: u32,
    pub max_height: u32,
    /// Plus grand côté de l'image de travail (px)
    pub work_size: u32,
    /// Formes au plus pour une image affichée à la taille maximale (une
    /// image plus petite en reçoit moins, au prorata de sa diagonale)
    pub max_shapes: usize,
    /// Poids maximal du SVG (octets)
    pub max_bytes: usize,
    /// Formes tirées au hasard pour chaque forme gardée
    pub candidates: usize,
    /// Escalade : mutations ratées de suite avant d'arrêter
    pub max_age: usize,
    /// Escalade : mutations au plus par forme
    pub max_mutations: usize,
    /// Opacité des formes
    pub alpha: f32,
    /// Flou gaussien (pixels de travail ; 0 = net)
    pub blur: f32,
    /// Plafond de temps (dernier recours : le résultat dépend alors de la
    /// vitesse de la machine)
    pub time_limit: Duration,
}

impl HalluOptions {
    pub fn new(max_width: u32) -> Self {
        Self {
            max_width,
            max_height: max_width * 3 / 2,
            work_size: 96,
            max_shapes: 60,
            max_bytes: 2560,
            candidates: 2000,
            max_age: 200,
            max_mutations: 4000,
            alpha: 0.6,
            blur: 0.6,
            time_limit: Duration::from_millis(500),
        }
    }

    /// Formes accordées à une image affichée en `w` × `h`.
    fn shapes_for(&self, w: u32, h: u32) -> usize {
        let full = self.max_width as f64 * self.max_width as f64 * 0.75;
        let n = self.max_shapes as f64 * ((w as f64 * h as f64) / full).sqrt();
        (n.round() as usize).clamp(MIN_SHAPES.min(self.max_shapes), self.max_shapes)
    }
}

impl Default for HalluOptions {
    fn default() -> Self {
        Self::new(320)
    }
}

/// Graine tirée d'une URL (FNV-1a 64 bits : stable d'une version de Rust à
/// l'autre, contrairement au hachage de la bibliothèque standard).
pub fn seed(url: &str) -> u64 {
    url.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Convertit une image (PNG, JPEG, GIF, WebP, BMP) en SVG halluciné.
pub fn to_svg(data: &[u8], opts: &HalluOptions, seed: u64) -> Result<Vec<u8>, String> {
    let (drawing, dw, dh) = hallucinate(&images::decode(data)?, opts, seed);
    Ok(drawing.svg(dw, dh, opts).into_bytes())
}

/// Hallucine une image décodée : le dessin, et sa taille d'affichage (celle
/// qu'aurait le GIF).
pub fn hallucinate(img: &DynamicImage, opts: &HalluOptions, seed: u64) -> (Drawing, u32, u32) {
    let (dw, dh) = images::fit((img.width(), img.height()), opts.max_width, opts.max_height);
    let work = working_image(img, dw, dh, opts.work_size);
    (approximate(&work, opts.shapes_for(dw, dh), opts, seed), dw, dh)
}

/// Image de travail : réduite (plus grand côté `size`, jamais agrandie
/// au-delà de la taille d'affichage), transparence posée sur le gris des pages.
pub fn working_image(img: &DynamicImage, dw: u32, dh: u32, size: u32) -> RgbImage {
    let k = (size as f64 / dw.max(dh) as f64).min(1.0);
    let w = ((dw as f64 * k).round() as u32).max(1);
    let h = ((dh as f64 * k).round() as u32).max(1);
    let rgb = if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            let p = rgba.get_pixel(x, y).0;
            let a = p[3] as u32;
            let mix = |k: usize| ((p[k] as u32 * a + PAGE_GREY[k] * (255 - a) + 127) / 255) as u8;
            image::Rgb([mix(0), mix(1), mix(2)])
        })
    } else {
        img.to_rgb8()
    };
    if rgb.dimensions() == (w, h) {
        rgb
    } else {
        imageops::thumbnail(&rgb, w, h)
    }
}

/// Forme, en pixels de l'image de travail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Sommets x1 y1 x2 y2 x3 y3
    Triangle([i16; 6]),
    /// Centre et rayons (axes horizontal et vertical)
    Ellipse([i16; 4]),
}

impl Shape {
    /// Forme dessinable : ellipse de rayons ≥ 1, triangle sans angle aigu
    /// de moins de 15° (les aiguilles font « bug », pas « rêve »).
    fn valid(&self) -> bool {
        match *self {
            Shape::Ellipse([_, _, rx, ry]) => rx >= 1 && ry >= 1,
            Shape::Triangle(v) => {
                let p = [(v[0] as f32, v[1] as f32), (v[2] as f32, v[3] as f32), (v[4] as f32, v[5] as f32)];
                (0..3).all(|i| {
                    let (a, b, c) = (p[i], p[(i + 1) % 3], p[(i + 2) % 3]);
                    let (ux, uy, vx, vy) = (b.0 - a.0, b.1 - a.1, c.0 - a.0, c.1 - a.1);
                    let n = ((ux * ux + uy * uy) * (vx * vx + vy * vy)).sqrt();
                    n > 0.0 && (ux * vx + uy * vy) / n < MAX_COS
                })
            }
        }
    }

    /// Segments horizontaux couverts (pixels dont le centre est dans la forme).
    fn spans(&self, w: i32, h: i32, out: &mut Vec<Span>) {
        out.clear();
        match *self {
            Shape::Triangle(v) => {
                let p = [(v[0] as f32, v[1] as f32), (v[2] as f32, v[3] as f32), (v[4] as f32, v[5] as f32)];
                let ymin = p[0].1.min(p[1].1).min(p[2].1);
                let ymax = p[0].1.max(p[1].1).max(p[2].1);
                for y in ((ymin - 0.5).ceil() as i32).max(0)..=((ymax - 0.5).floor() as i32).min(h - 1) {
                    // sommets entiers, centre de ligne demi-entier : jamais sur un sommet
                    let yc = y as f32 + 0.5;
                    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                    for i in 0..3 {
                        let (a, b) = (p[i], p[(i + 1) % 3]);
                        if (a.1 < yc) != (b.1 < yc) {
                            let x = a.0 + (yc - a.1) * (b.0 - a.0) / (b.1 - a.1);
                            lo = lo.min(x);
                            hi = hi.max(x);
                        }
                    }
                    push_span(out, y, lo, hi, w);
                }
            }
            Shape::Ellipse([cx, cy, rx, ry]) => {
                let (cx, cy, rx, ry) = (cx as f32, cy as f32, rx as f32, ry as f32);
                for y in ((cy - ry - 0.5).ceil() as i32).max(0)..=((cy + ry - 0.5).floor() as i32).min(h - 1) {
                    let dy = (y as f32 + 0.5 - cy) / ry;
                    let q = 1.0 - dy * dy;
                    if q > 0.0 {
                        let half = rx * q.sqrt();
                        push_span(out, y, cx - half, cx + half, w);
                    }
                }
            }
        }
    }

    /// Longueur de l'élément SVG, sans l'écrire (voir [`Shape::write_svg`]).
    fn svg_len(&self) -> usize {
        let digits = |v: i16| {
            let (mut a, mut n) = (v.unsigned_abs(), 1 + (v < 0) as usize);
            while a >= 10 {
                a /= 10;
                n += 1;
            }
            n
        };
        match *self {
            Shape::Triangle(v) => 30 + v.iter().map(|&x| digits(x)).sum::<usize>(),
            Shape::Ellipse([cx, cy, r, ry]) if r == ry => 38 + digits(cx) + digits(cy) + digits(r),
            Shape::Ellipse(e) => 46 + e.iter().map(|&x| digits(x)).sum::<usize>(),
        }
    }

    /// Élément SVG (couleur `#rgb`).
    fn write_svg(&self, out: &mut String, c: [u8; 3]) {
        let hex = |v: u8| char::from_digit((v / 17) as u32, 16).unwrap_or('0');
        let fill = format!("#{}{}{}", hex(c[0]), hex(c[1]), hex(c[2]));
        let _ = match *self {
            Shape::Triangle([x1, y1, x2, y2, x3, y3]) => write!(out, "<path fill=\"{fill}\" d=\"M{x1} {y1} {x2} {y2} {x3} {y3}\"/>"),
            Shape::Ellipse([cx, cy, r, ry]) if r == ry => write!(out, "<circle fill=\"{fill}\" cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\"/>"),
            Shape::Ellipse([cx, cy, rx, ry]) => write!(out, "<ellipse fill=\"{fill}\" cx=\"{cx}\" cy=\"{cy}\" rx=\"{rx}\" ry=\"{ry}\"/>"),
        };
    }
}

/// Pixels `x0..x1` de la ligne `y`.
#[derive(Clone, Copy, Debug)]
struct Span {
    y: u32,
    x0: u32,
    x1: u32,
}

/// Segment des pixels dont le centre est entre `lo` et `hi`, coupé à l'image.
fn push_span(out: &mut Vec<Span>, y: i32, lo: f32, hi: f32, w: i32) {
    let x0 = ((lo - 0.5).ceil() as i32).max(0);
    let x1 = ((hi - 0.5).floor() as i32).min(w - 1);
    if x0 <= x1 {
        out.push(Span { y: y as u32, x0: x0 as u32, x1: x1 as u32 + 1 });
    }
}

/// Forme évaluée.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    shape: Shape,
    /// Variation de l'erreur (négative : amélioration)
    delta: f64,
    color: [u8; 3],
    /// Variation par octet de SVG : le budget est en octets, une ellipse
    /// (51 o) doit rapporter plus qu'un triangle (41 o) pour être choisie
    rate: f64,
}

impl Candidate {
    fn better_than(&self, other: &Candidate) -> bool {
        self.rate < other.rate
    }
}

/// Composante arrondie à la notation courte (multiple de 17 : `#rgb`).
fn quantize(c: f64) -> u8 {
    (c / 17.0).round().clamp(0.0, 15.0) as u8 * 17
}

/// Image en cours de dessin, avec les sommes cumulées par ligne.
struct Canvas {
    w: usize,
    h: usize,
    target: Vec<[i32; 3]>,
    cur: Vec<[i32; 3]>,
    /// Par ligne, `w + 1` entrées : sommes des pixels `0..x` de
    /// t₀ t₁ t₂ (cible), c₀ c₁ c₂ (courante), Σₖ tₖcₖ et Σₖ cₖ²
    pre: Vec<[i32; 8]>,
    alpha: f64,
}

impl Canvas {
    fn new(img: &RgbImage, background: [u8; 3], alpha: f64) -> Canvas {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let target: Vec<[i32; 3]> = img.pixels().map(|p| [p[0] as i32, p[1] as i32, p[2] as i32]).collect();
        let bg = background.map(|c| c as i32);
        let mut c = Canvas { w, h, target, cur: vec![bg; w * h], pre: vec![[0; 8]; h * (w + 1)], alpha };
        c.refresh(0, h);
        c
    }

    /// Recalcule les sommes cumulées des lignes `y0..y1`.
    fn refresh(&mut self, y0: usize, y1: usize) {
        for y in y0..y1 {
            let mut acc = [0i32; 8];
            let row = y * (self.w + 1);
            self.pre[row] = acc;
            for x in 0..self.w {
                let (t, c) = (self.target[y * self.w + x], self.cur[y * self.w + x]);
                for k in 0..3 {
                    acc[k] += t[k];
                    acc[3 + k] += c[k];
                    acc[6] += t[k] * c[k];
                    acc[7] += c[k] * c[k];
                }
                self.pre[row + x + 1] = acc;
            }
        }
    }

    /// Évalue une forme (`None` si elle n'est pas dessinable ou ne couvre
    /// aucun pixel).
    fn score(&self, shape: Shape, spans: &mut Vec<Span>) -> Option<Candidate> {
        if !shape.valid() {
            return None;
        }
        shape.spans(self.w as i32, self.h as i32, spans);
        let (delta, color) = self.evaluate(spans)?;
        Some(Candidate { shape, delta, color, rate: delta / shape.svg_len() as f64 })
    }

    /// Variation de l'erreur (somme des carrés) si l'on peint ces segments
    /// avec la meilleure couleur, et cette couleur. Avec a l'opacité, t la
    /// cible et c la courante, le pixel devient (1-a)c + a·k ; sur la forme,
    /// pour chaque composante, l'erreur varie de
    ///   2aΣtc + (a²-2a)Σc² - 2ak·D + n·a²k²,  avec D = Σt - (1-a)Σc,
    /// minimale pour k = D / (a·n).
    fn evaluate(&self, spans: &[Span]) -> Option<(f64, [u8; 3])> {
        let mut s = [0i64; 8];
        let mut n = 0i64;
        let stride = self.w + 1;
        for sp in spans {
            let row = sp.y as usize * stride;
            let (a, b) = (&self.pre[row + sp.x0 as usize], &self.pre[row + sp.x1 as usize]);
            for k in 0..8 {
                s[k] += (b[k] - a[k]) as i64;
            }
            n += (sp.x1 - sp.x0) as i64;
        }
        if n == 0 {
            return None;
        }
        let (a, n) = (self.alpha, n as f64);
        let mut delta = 2.0 * a * s[6] as f64 + (a * a - 2.0 * a) * s[7] as f64;
        let mut color = [0u8; 3];
        for k in 0..3 {
            let d = s[k] as f64 - (1.0 - a) * s[3 + k] as f64;
            color[k] = quantize(d / (a * n));
            let c = color[k] as f64;
            delta += n * a * a * c * c - 2.0 * a * c * d;
        }
        Some((delta, color))
    }

    /// Peint les segments (comme le navigateur : mélange, arrondi à l'entier).
    fn paint(&mut self, spans: &[Span], color: [u8; 3]) {
        let a = self.alpha;
        let (mut y0, mut y1) = (self.h, 0);
        for sp in spans {
            let y = sp.y as usize;
            (y0, y1) = (y0.min(y), y1.max(y + 1));
            for p in &mut self.cur[y * self.w + sp.x0 as usize..y * self.w + sp.x1 as usize] {
                for k in 0..3 {
                    p[k] = ((1.0 - a) * p[k] as f64 + a * color[k] as f64).round() as i32;
                }
            }
        }
        self.refresh(y0, y1.max(y0));
    }

    /// Écart quadratique moyen avec la cible (0-255).
    fn rmse(&self) -> f64 {
        let sse: i64 = self.target.iter().zip(&self.cur).map(|(t, c)| (0..3).map(|k| ((t[k] - c[k]) as i64).pow(2)).sum::<i64>()).sum();
        (sse as f64 / (3 * self.w * self.h).max(1) as f64).sqrt()
    }
}

/// Générateur pseudo-aléatoire SplitMix64 (petit, rapide, reproductible).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Entier dans `0..n` (n ≥ 1).
    fn below(&mut self, n: u32) -> i32 {
        (((self.next() >> 32) * n as u64) >> 32) as i32
    }

    /// Entier dans `-d..=d`.
    fn around(&mut self, d: i32) -> i32 {
        self.below(2 * d as u32 + 1) - d
    }

    /// Réel dans [0, 1[.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Tirages et mutations dans une image de travail `w` × `h`.
struct Shaper {
    w: i32,
    h: i32,
    /// Plus grand côté
    side: i32,
    /// Marge autorisée hors de l'image pour les sommets
    margin: i32,
}

impl Shaper {
    fn new(w: usize, h: usize) -> Shaper {
        let side = w.max(h) as i32;
        Shaper { w: w as i32, h: h as i32, side, margin: (side / 8).max(1) }
    }

    fn clamp_x(&self, x: i32) -> i16 {
        x.clamp(-self.margin, self.w - 1 + self.margin) as i16
    }

    fn clamp_y(&self, y: i32) -> i16 {
        y.clamp(-self.margin, self.h - 1 + self.margin) as i16
    }

    /// Forme au hasard : surtout petite, parfois très grande ; un triangle
    /// une fois sur deux, sinon une ellipse, ronde une fois sur trois (un
    /// cercle s'écrit plus court).
    fn random(&self, rng: &mut Rng) -> Shape {
        let (x, y) = (rng.below(self.w as u32), rng.below(self.h as u32));
        let u = rng.unit();
        let r = 1 + (self.side as f32 * (0.05 + 0.45 * u * u)) as i32;
        match rng.below(6) {
            0..=2 => {
                let mut v = [0i16; 6];
                for i in 0..3 {
                    v[2 * i] = self.clamp_x(x + rng.around(r));
                    v[2 * i + 1] = self.clamp_y(y + rng.around(r));
                }
                Shape::Triangle(v)
            }
            3 => {
                let r = 1 + rng.below(r as u32) as i16;
                Shape::Ellipse([x as i16, y as i16, r, r])
            }
            _ => Shape::Ellipse([x as i16, y as i16, (1 + rng.below(r as u32)) as i16, (1 + rng.below(r as u32)) as i16]),
        }
    }

    /// Petite variation : un sommet, le centre ou un rayon bouge, de 1 ou
    /// 2 pixels (retouche) ou d'un sixième de l'image (saut) ; une ellipse
    /// peut aussi s'arrondir, un cercle grossir sans cesser d'être rond.
    fn mutate(&self, s: Shape, rng: &mut Rng) -> Shape {
        let d = if rng.below(2) == 0 { 2 } else { (self.side / 6).max(2) };
        match s {
            Shape::Triangle(mut v) => {
                let i = 2 * rng.below(3) as usize;
                v[i] = self.clamp_x(v[i] as i32 + rng.around(d));
                v[i + 1] = self.clamp_y(v[i + 1] as i32 + rng.around(d));
                Shape::Triangle(v)
            }
            Shape::Ellipse([cx, cy, rx, ry]) => {
                let (mut e, side) = ([cx as i32, cy as i32, rx as i32, ry as i32], self.side);
                match rng.below(5) {
                    0 => {
                        e[0] = (e[0] + rng.around(d)).clamp(0, self.w - 1);
                        e[1] = (e[1] + rng.around(d)).clamp(0, self.h - 1);
                    }
                    1 => e[2] = (e[2] + rng.around(d)).clamp(1, side),
                    2 => e[3] = (e[3] + rng.around(d)).clamp(1, side),
                    3 => {
                        let k = rng.around(d);
                        e[2] = (e[2] + k).clamp(1, side);
                        e[3] = (e[3] + k).clamp(1, side);
                    }
                    _ => (e[2], e[3]) = ((e[2] + e[3]) / 2, (e[2] + e[3]) / 2),
                }
                Shape::Ellipse(e.map(|v| v as i16))
            }
        }
    }
}

/// Image rêvée : fond uni et formes, dans l'ordre de dessin.
#[derive(Clone, Debug)]
pub struct Drawing {
    /// Taille de travail (unités du `viewBox`)
    pub width: u32,
    pub height: u32,
    pub background: [u8; 3],
    pub shapes: Vec<(Shape, [u8; 3])>,
    /// Écart quadratique moyen avec l'image de travail (0-255)
    pub rmse: f64,
}

/// Approxime `img` par au plus `max_shapes` formes (et au plus
/// `opts.max_bytes` de SVG).
pub fn approximate(img: &RgbImage, max_shapes: usize, opts: &HalluOptions, seed: u64) -> Drawing {
    let deadline = Instant::now() + opts.time_limit;
    let n = (img.width() as u64 * img.height() as u64).max(1);
    let mut sums = [0u64; 3];
    for p in img.pixels() {
        for k in 0..3 {
            sums[k] += p[k] as u64;
        }
    }
    let background = sums.map(|s| ((s + n / 2) / n) as u8);
    let mut canvas = Canvas::new(img, background, opts.alpha as f64);
    let shaper = Shaper::new(canvas.w, canvas.h);
    let mut rng = Rng(seed);
    let mut spans = Vec::with_capacity(canvas.h);
    let mut drawing = Drawing { width: img.width(), height: img.height(), background, shapes: Vec::new(), rmse: 0.0 };
    // poids du SVG sans formes (au pire : affichage en 4 chiffres, décimale au viewBox)
    let mut bytes = drawing.svg(9999, 9999, opts).len() + group_open(opts).len() + "</g>".len() + 2;
    let mut failures = 0;
    while drawing.shapes.len() < max_shapes && failures < MAX_FAILURES && Instant::now() < deadline {
        // tirage au hasard…
        let mut best: Option<Candidate> = None;
        for _ in 0..opts.candidates {
            let c = canvas.score(shaper.random(&mut rng), &mut spans);
            if c.is_some_and(|c| best.is_none_or(|b| c.better_than(&b))) {
                best = c;
            }
        }
        let Some(mut best) = best else {
            failures += 1;
            continue;
        };
        // … puis escalade
        let mut age = 0;
        for _ in 0..opts.max_mutations {
            if age >= opts.max_age {
                break;
            }
            match canvas.score(shaper.mutate(best.shape, &mut rng), &mut spans) {
                Some(c) if c.better_than(&best) => {
                    best = c;
                    age = 0;
                }
                _ => age += 1,
            }
        }
        if best.delta > -MIN_GAIN {
            failures += 1;
            continue;
        }
        let len = best.shape.svg_len();
        if bytes + len > opts.max_bytes {
            break;
        }
        bytes += len;
        best.shape.spans(canvas.w as i32, canvas.h as i32, &mut spans);
        canvas.paint(&spans, best.color);
        drawing.shapes.push((best.shape, best.color));
    }
    drawing.rmse = canvas.rmse();
    drawing
}

/// Début du groupe des formes : flou (s'il y en a) et opacité communs.
fn group_open(opts: &HalluOptions) -> String {
    let alpha = short(opts.alpha as f64);
    if opts.blur > 0.0 {
        format!("<filter id=\"b\"><feGaussianBlur stdDeviation=\"{}\"/></filter><g fill-opacity=\"{alpha}\" filter=\"url(#b)\">", short(opts.blur as f64))
    } else {
        format!("<g fill-opacity=\"{alpha}\">")
    }
}

/// Nombre court : `0.5` → `.5`, `1.0` → `1`, `63.94` → `63.9`.
fn short(x: f64) -> String {
    let s = format!("{x:.1}");
    let s = s.strip_suffix(".0").unwrap_or(&s);
    s.strip_prefix("0.").map_or_else(|| s.to_string(), |r| format!(".{r}"))
}

impl Drawing {
    /// Le SVG, affiché en `dw` × `dh`. Le `viewBox` a exactement les
    /// proportions de l'affichage (pas de bande vide sur un bord).
    pub fn svg(&self, dw: u32, dh: u32, opts: &HalluOptions) -> String {
        let (w, h) = (self.width as f64, self.height as f64);
        let (vw, vh) = if dw as f64 * h >= dh as f64 * w { (w, w * dh as f64 / dw.max(1) as f64) } else { (h * dw as f64 / dh.max(1) as f64, h) };
        let [r, g, b] = self.background;
        let mut s = String::with_capacity(opts.max_bytes + 64);
        let _ = write!(
            s,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"{dw}\" height=\"{dh}\">\
             <rect width=\"100%\" height=\"100%\" fill=\"#{r:02x}{g:02x}{b:02x}\"/>",
            short(vw),
            short(vh)
        );
        if !self.shapes.is_empty() {
            s.push_str(&group_open(opts));
            for (shape, c) in &self.shapes {
                shape.write_svg(&mut s, *c);
            }
            s.push_str("</g>");
        }
        s.push_str("</svg>");
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Rgb, Rgba};
    use std::io::Cursor;

    /// Paysage de synthèse : ciel en dégradé, soleil, montagne, prairie, un
    /// peu de bruit (de quoi occuper un peintre pressé).
    fn paysage(w: u32, h: u32) -> RgbImage {
        let mut s = 7u32;
        ImageBuffer::from_fn(w, h, |x, y| {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let noise = (s >> 24) as i32 % 16 - 8;
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let mut c = if fy < 0.6 { [60.0 + 150.0 * fy, 120.0 + 150.0 * fy, 230.0] } else { [40.0, 140.0 - 60.0 * (fy - 0.6), 50.0] };
            if (fx - 0.75).powi(2) + (fy - 0.2).powi(2) < 0.01 {
                c = [255.0, 220.0, 60.0];
            }
            if fy > 0.25 && fy < 0.6 && (fx - 0.35).abs() < (fy - 0.25) * 1.2 {
                c = [90.0, 80.0, 90.0];
            }
            Rgb(c.map(|v| (v as i32 + noise).clamp(0, 255) as u8))
        })
    }

    fn png(img: &RgbImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn plus_de_formes_moins_d_erreur() {
        let opts = HalluOptions::default();
        let img = working_image(&DynamicImage::ImageRgb8(paysage(640, 480)), 320, 240, opts.work_size);
        assert_eq!(img.dimensions(), (96, 72));
        let fond = approximate(&img, 0, &opts, 1);
        let peu = approximate(&img, 5, &opts, 1);
        let beaucoup = approximate(&img, 40, &opts, 1);
        assert!(fond.shapes.is_empty() && peu.shapes.len() == 5 && beaucoup.shapes.len() == 40);
        assert!(peu.rmse < fond.rmse * 0.85, "5 formes : {:.1} contre {:.1} pour le fond seul", peu.rmse, fond.rmse);
        assert!(beaucoup.rmse < peu.rmse * 0.8, "40 formes : {:.1} contre {:.1} avec 5", beaucoup.rmse, peu.rmse);
        assert!(beaucoup.rmse < 25.0, "ressemblant (un peu) : {:.1}", beaucoup.rmse);
    }

    #[test]
    fn svg_compact_et_deterministe() {
        let opts = HalluOptions::default();
        let src = png(&paysage(640, 480));
        let t0 = Instant::now();
        let svg = to_svg(&src, &opts, seed("http://exemple.fr/paysage.png")).unwrap();
        let ms = t0.elapsed().as_millis();
        eprintln!("hallucination 640×480 : {} o en {ms} ms", svg.len());
        assert!(ms < 2000, "{ms} ms");
        let text = String::from_utf8(svg.clone()).unwrap();
        assert!(text.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 96 72\" width=\"320\" height=\"240\">"), "{text}");
        assert!(text.ends_with("</g></svg>"));
        assert!(svg.len() <= opts.max_bytes, "{} o", svg.len());
        assert!(svg.len() > 1000, "des formes, quand même : {text}");
        let shapes = text.matches("<path ").count() + text.matches("<ellipse ").count() + text.matches("<circle ").count();
        assert!((30..=opts.max_shapes).contains(&shapes), "{shapes} formes");
        assert_eq!(to_svg(&src, &opts, seed("http://exemple.fr/paysage.png")).unwrap(), svg, "même URL, même rêve");
        assert_ne!(to_svg(&src, &opts, seed("http://exemple.fr/autre.png")).unwrap(), svg, "autre URL, autre rêve");
    }

    #[test]
    fn petites_images_transparentes_et_unies() {
        let opts = HalluOptions::default();
        // icône transparente : pas agrandie, posée sur le gris, peu de formes
        let icone = ImageBuffer::from_fn(24, 24, |x, y| if (x as i32 - 12).pow(2) + (y as i32 - 12).pow(2) < 64 { Rgba([200u8, 0, 0, 255]) } else { Rgba([0, 0, 0, 0]) });
        let mut src = Vec::new();
        icone.write_to(&mut Cursor::new(&mut src), ImageFormat::Png).unwrap();
        let svg = String::from_utf8(to_svg(&src, &opts, 3).unwrap()).unwrap();
        assert!(svg.contains("viewBox=\"0 0 24 24\" width=\"24\" height=\"24\""), "{svg}");
        assert!(svg.len() < 800, "{} o : {svg}", svg.len());
        // image unie : le fond suffit, aucune forme n'améliore rien
        let uni = png(&ImageBuffer::from_pixel(50, 30, Rgb([10u8, 200, 30])));
        let svg = String::from_utf8(to_svg(&uni, &opts, 3).unwrap()).unwrap();
        assert_eq!(svg, "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 50 30\" width=\"50\" height=\"30\"><rect width=\"100%\" height=\"100%\" fill=\"#0ac81e\"/></svg>");
        // image de 1 pixel de haut, et proportions non entières
        assert!(to_svg(&png(&paysage(300, 1)), &opts, 3).is_ok());
        let svg = String::from_utf8(to_svg(&png(&paysage(500, 333)), &opts, 3).unwrap()).unwrap();
        assert!(svg.contains("viewBox=\"0 0 96 63.9\" width=\"320\" height=\"213\""), "{}", &svg[..120]);
        assert!(to_svg(b"pas une image", &opts, 3).is_err());
    }

    #[test]
    fn formes_tramees() {
        let mut spans = Vec::new();
        // triangle rectangle de 10 px de côté : la moitié du carré, environ
        Shape::Triangle([0, 0, 10, 0, 0, 10]).spans(20, 20, &mut spans);
        let area: u32 = spans.iter().map(|s| s.x1 - s.x0).sum();
        assert!((45..=55).contains(&area), "{area}");
        // cercle de rayon 5 : π·25 ≈ 78 ; puis coupé par le bord gauche
        Shape::Ellipse([10, 10, 5, 5]).spans(20, 20, &mut spans);
        let area: u32 = spans.iter().map(|s| s.x1 - s.x0).sum();
        assert!((70..=86).contains(&area), "{area}");
        Shape::Ellipse([0, 10, 5, 5]).spans(20, 20, &mut spans);
        assert!(spans.iter().all(|s| s.x0 == 0 && s.x1 <= 5));
        assert!(!Shape::Triangle([0, 0, 50, 1, 0, 2]).valid(), "aiguille refusée");
        assert!(Shape::Triangle([0, 0, 10, 0, 5, 9]).valid());
        let mut s = String::new();
        Shape::Triangle([-3, 4, 50, 0, 7, 99]).write_svg(&mut s, [255, 0, 136]);
        Shape::Ellipse([5, 6, 7, 7]).write_svg(&mut s, [0, 17, 34]);
        assert_eq!(s, "<path fill=\"#f08\" d=\"M-3 4 50 0 7 99\"/><circle fill=\"#012\" cx=\"5\" cy=\"6\" r=\"7\"/>");
        for shape in [Shape::Triangle([-3, 4, 50, 0, 7, -199]), Shape::Ellipse([5, 60, 7, 7]), Shape::Ellipse([-5, 100, 1007, 12])] {
            let mut s = String::new();
            shape.write_svg(&mut s, [0, 0, 0]);
            assert_eq!(shape.svg_len(), s.len(), "{s}");
        }
        assert_eq!((short(0.5), short(1.0), short(63.94), short(96.0)), (".5".into(), "1".into(), "63.9".into(), "96".into()));
    }
}
