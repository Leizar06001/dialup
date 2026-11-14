//! Images « 1998 » : conversion en GIF entrelacé de 256 couleurs au plus,
//! et petites images générées (GIF animé « en construction », compteur).
//!
//! Conversion ([`to_gif`]) : décodage (PNG, JPEG, GIF, WebP, BMP), réduction
//! à la largeur maximale (320 px par défaut, hauteur bornée à 1,5 fois),
//! transparence ramenée à un seul index, palette exacte si l'image a peu de
//! couleurs, sinon quantification NeuQuant (sans tramage : le LZW du GIF
//! compresse mieux les aplats). Le GIF est ENTRELACÉ (lignes 0, 8, 16…
//! puis 4, 12… puis 2, 6… puis les impaires) : à 3 Ko/s le navigateur
//! dessine d'abord une version grossière qui s'affine, comme en 1998.
//! Si le résultat dépasse le poids visé, on réessaie avec moins de couleurs
//! (128, 64), puis plus petit en gardant 64 couleurs : l'échelle est
//! estimée d'après le poids obtenu (proportionnel au nombre de pixels), pour
//! rendre l'image la plus grande qui tienne. À défaut, le plus léger des
//! essais est rendu.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use color_quant::NeuQuant;
use image::{imageops, ImageFormat, ImageReader, Limits, RgbaImage};

/// Réglages de conversion.
#[derive(Clone, Copy, Debug)]
pub struct GifOptions {
    /// Largeur maximale (px)
    pub max_width: u32,
    /// Hauteur maximale (px)
    pub max_height: u32,
    /// Poids visé (octets)
    pub target: usize,
}

impl GifOptions {
    pub fn new(max_width: u32, target: usize) -> Self {
        Self { max_width, max_height: max_width * 3 / 2, target }
    }
}

impl Default for GifOptions {
    fn default() -> Self {
        Self::new(320, 15 * 1024)
    }
}

/// Nombres de couleurs essayés à pleine taille.
const COLORS: [usize; 3] = [256, 128, 64];
/// Couleurs gardées quand il faut réduire la taille.
const SMALL_COLORS: usize = 64;
/// Essais de réduction de taille au plus.
const SHRINK_TRIES: usize = 5;

/// Convertit une image en GIF entrelacé ≤ 256 couleurs.
pub fn to_gif(data: &[u8], opts: &GifOptions) -> Result<Vec<u8>, String> {
    let reader = ImageReader::new(Cursor::new(data)).with_guessed_format().map_err(|e| e.to_string())?;
    let format = reader.format().ok_or("format d'image inconnu")?;
    // petit GIF déjà aux dimensions : renvoyé tel quel (animations comprises)
    if format == ImageFormat::Gif && data.len() <= opts.target {
        let dims = ImageReader::new(Cursor::new(data)).with_guessed_format().ok().and_then(|r| r.into_dimensions().ok());
        if dims.is_some_and(|(w, h)| w <= opts.max_width && h <= opts.max_height) {
            return Ok(data.to_vec());
        }
    }
    let mut reader = reader;
    let mut limits = Limits::default();
    limits.max_image_width = Some(12_000);
    limits.max_image_height = Some(12_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| format!("image illisible : {e}"))?.to_rgba8();
    let (w, h) = fit(img.dimensions(), opts.max_width, opts.max_height);
    let base = if (w, h) == img.dimensions() { img } else { imageops::thumbnail(&img, w, h) };

    let mut best: Option<Vec<u8>> = None;
    let mut keep = |gif: Vec<u8>| -> Option<Vec<u8>> {
        if gif.len() <= opts.target {
            return Some(gif);
        }
        if best.as_ref().is_none_or(|b| gif.len() < b.len()) {
            best = Some(gif);
        }
        None
    };
    let mut last = 0;
    for colors in COLORS {
        let gif = encode_rgba(&base, colors, true)?;
        last = gif.len();
        if let Some(ok) = keep(gif) {
            return Ok(ok);
        }
    }
    if base.width() * base.height() > 32 * 32 {
        // poids ≈ proportionnel au nombre de pixels : échelle estimée, puis
        // réduite de 15 % à chaque essai manqué
        let mut scale = ((opts.target as f64 / last as f64).sqrt() * 0.95).min(0.95);
        for _ in 0..SHRINK_TRIES {
            let sw = ((w as f64 * scale).round() as u32).max(8);
            let sh = ((h as f64 * scale).round() as u32).max(8);
            if let Some(ok) = keep(encode_rgba(&imageops::thumbnail(&base, sw, sh), SMALL_COLORS, true)?) {
                return Ok(ok);
            }
            scale *= 0.85;
        }
    }
    best.ok_or_else(|| "conversion impossible".into())
}

/// Dimensions réduites (proportions gardées) pour tenir dans max_w × max_h.
fn fit((w, h): (u32, u32), max_w: u32, max_h: u32) -> (u32, u32) {
    if w <= max_w && h <= max_h {
        return (w.max(1), h.max(1));
    }
    let r = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
    (((w as f64 * r).round() as u32).max(1), ((h as f64 * r).round() as u32).max(1))
}

/// Encode une image RGBA en GIF d'au plus `colors` couleurs (transparence
/// comprise).
pub fn encode_rgba(img: &RgbaImage, colors: usize, interlaced: bool) -> Result<Vec<u8>, String> {
    let (w, h) = img.dimensions();
    if w > u16::MAX as u32 || h > u16::MAX as u32 {
        return Err("image trop grande".into());
    }
    let colors = colors.clamp(2, 256);
    let px = img.as_raw();
    let transparent = px.chunks_exact(4).any(|p| p[3] < 128);
    let opaque_colors = if transparent { colors - 1 } else { colors };
    let key = |p: &[u8]| u32::from_be_bytes([p[0], p[1], p[2], 0]);

    // palette exacte si l'image a peu de couleurs (logos, captures…)
    let mut distinct = HashSet::new();
    for p in px.chunks_exact(4).filter(|p| p[3] >= 128) {
        distinct.insert(key(p));
        if distinct.len() > opaque_colors {
            break;
        }
    }
    let (palette, indices): (Vec<u8>, Vec<u8>) = if distinct.len() <= opaque_colors {
        let mut list: Vec<u32> = distinct.into_iter().collect();
        list.sort_unstable();
        let pos: HashMap<u32, u8> = list.iter().enumerate().map(|(i, &c)| (c, i as u8)).collect();
        let t = list.len() as u8;
        let palette = list.iter().flat_map(|c| {
            let b = c.to_be_bytes();
            [b[0], b[1], b[2]]
        });
        let indices = px.chunks_exact(4).map(|p| if p[3] < 128 { t } else { pos[&key(p)] }).collect();
        (palette.chain(if transparent { vec![0, 0, 0] } else { vec![] }).collect(), indices)
    } else {
        let samples: Vec<u8> = px.chunks_exact(4).filter(|p| p[3] >= 128).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
        let samplefac = if samples.len() / 4 < 20_000 { 1 } else { 10 };
        let nq = NeuQuant::new(samplefac, opaque_colors, &samples);
        let mut memo: HashMap<u32, u8> = HashMap::new();
        let t = opaque_colors as u8;
        let indices = px
            .chunks_exact(4)
            .map(|p| {
                if p[3] < 128 {
                    return t;
                }
                *memo.entry(key(p)).or_insert_with(|| nq.index_of(&[p[0], p[1], p[2], 255]) as u8)
            })
            .collect();
        let mut palette = nq.color_map_rgb();
        if transparent {
            palette.extend_from_slice(&[0, 0, 0]);
        }
        (palette, indices)
    };
    let t_index = transparent.then(|| (palette.len() / 3 - 1) as u8);
    encode_indexed(w as u16, h as u16, &palette, indices, t_index, interlaced)
}

/// Ordre des lignes d'un GIF entrelacé.
fn interlace(indices: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(indices.len());
    for (start, step) in [(0, 8), (4, 8), (2, 4), (1, 2)] {
        for y in (start..h).step_by(step) {
            out.extend_from_slice(&indices[y * w..(y + 1) * w]);
        }
    }
    out
}

/// GIF d'une seule image à partir d'indices de palette (ligne par ligne).
/// L'encodeur `gif` ne réordonne pas les lignes : on le fait ici.
fn encode_indexed(w: u16, h: u16, palette: &[u8], indices: Vec<u8>, transparent: Option<u8>, interlaced: bool) -> Result<Vec<u8>, String> {
    let buffer = if interlaced { interlace(&indices, w as usize, h as usize) } else { indices };
    let mut out = Vec::new();
    let mut enc = gif::Encoder::new(&mut out, w, h, palette).map_err(|e| e.to_string())?;
    let frame = gif::Frame { width: w, height: h, interlaced, transparent, buffer: Cow::Owned(buffer), ..gif::Frame::default() };
    enc.write_frame(&frame).map_err(|e| e.to_string())?;
    enc.into_inner().map_err(|e| e.to_string())?;
    Ok(out)
}

// ───────────────────── images générées ─────────────────────

/// Police 5 × 7 (bit 4 = colonne de gauche) pour les quelques caractères utiles.
fn glyph(c: char) -> [u8; 7] {
    match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        _ => [0; 7],
    }
}

/// Dessine `text` (police 5 × 7, agrandie `scale` fois) dans un tampon d'indices.
fn draw_text(buf: &mut [u8], w: usize, x0: usize, y0: usize, text: &str, scale: usize, color: u8) {
    for (i, c) in text.chars().enumerate() {
        for (row, bits) in glyph(c).iter().enumerate() {
            for col in 0..5 {
                if bits & (0x10 >> col) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let x = x0 + (i * 6 + col) * scale + dx;
                            let y = y0 + row * scale + dy;
                            if x < w && y * w + x < buf.len() {
                                buf[y * w + x] = color;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Petit GIF animé « EN CONSTRUCTION » : bandes de chantier jaunes et
/// noires qui défilent, en boucle (4 images, ~1,4 Ko).
pub fn construction_gif() -> Vec<u8> {
    const W: usize = 100;
    const H: usize = 31;
    // 0 noir, 1 jaune, 2 orange
    let palette = [0x00, 0x00, 0x00, 0xFF, 0xD7, 0x00, 0xFF, 0x80, 0x00, 0xFF, 0xFF, 0xFF];
    let mut out = Vec::new();
    {
        let mut enc = gif::Encoder::new(&mut out, W as u16, H as u16, &palette).expect("écriture en mémoire");
        enc.set_repeat(gif::Repeat::Infinite).expect("écriture en mémoire");
        for f in 0..4 {
            let mut buf = vec![1u8; W * H];
            for y in 0..H {
                for x in 0..W {
                    // bandes de chantier en haut et en bas (décalées à chaque image), cadre noir
                    let band = !(7..H - 7).contains(&y);
                    let border = x == 0 || x == W - 1 || y == 0 || y == H - 1;
                    let stripe = ((x + y + 2 * (4 - f)) / 4).is_multiple_of(2);
                    if border || (band && stripe) {
                        buf[y * W + x] = 0;
                    }
                }
            }
            draw_text(&mut buf, W, (W - 89) / 2, 12, "EN CONSTRUCTION", 1, if f.is_multiple_of(2) { 0 } else { 2 });
            let frame = gif::Frame { width: W as u16, height: H as u16, delay: 25, buffer: Cow::Owned(buf), ..gif::Frame::default() };
            enc.write_frame(&frame).expect("écriture en mémoire");
        }
    }
    out
}

/// Compteur de visites façon odomètre : chiffres verts sur fond noir.
/// `digits` : chiffres seulement (les autres caractères sont ignorés).
pub fn counter_gif(digits: &str) -> Vec<u8> {
    let digits: String = digits.chars().filter(char::is_ascii_digit).take(12).collect();
    let digits = if digits.is_empty() { "0".to_string() } else { digits };
    const CW: usize = 13; // cellule : 1 px de séparation + 12 px
    const H: usize = 20;
    let w = digits.len() * CW + 1;
    // 0 noir, 1 vert, 2 gris
    let palette = [0x00, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x60, 0x60, 0x60, 0xFF, 0xFF, 0xFF];
    let mut buf = vec![0u8; w * H];
    for y in 0..H {
        for x in 0..w {
            if x % CW == 0 || y == 0 || y == H - 1 {
                buf[y * w + x] = 2;
            }
        }
    }
    for (i, c) in digits.chars().enumerate() {
        draw_text(&mut buf, w, i * CW + 2, 3, &c.to_string(), 2, 1);
    }
    encode_indexed(w as u16, H as u16, &palette, buf, None, false).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    /// PNG généré en mémoire : dégradé + motif (beaucoup de couleurs).
    fn png(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let img = ImageBuffer::from_fn(w, h, |x, y| {
            let a = if alpha && x < w / 4 { 0 } else { 255 };
            Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, ((x ^ y) & 0xFF) as u8, a])
        });
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
        out
    }

    struct Decoded {
        width: u16,
        height: u16,
        colors: usize,
        interlaced: bool,
        transparent: Option<u8>,
        frames: usize,
    }

    fn decode(gif_bytes: &[u8]) -> Decoded {
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::Indexed);
        let mut d = opts.read_info(Cursor::new(gif_bytes)).expect("GIF valide");
        let global = d.global_palette().map(|p| p.len() / 3).unwrap_or(0);
        // métadonnées AVANT décodage (le décodeur désentrelace et efface le drapeau)
        let f = d.next_frame_info().unwrap().expect("une image").clone();
        let mut frames = 0;
        loop {
            let mut buf = vec![0u8; d.buffer_size()];
            d.read_into_buffer(&mut buf).unwrap();
            frames += 1;
            if d.next_frame_info().unwrap().is_none() {
                break;
            }
        }
        let colors = f.palette.as_ref().map(|p| p.len() / 3).unwrap_or(global);
        Decoded { width: f.width, height: f.height, colors, interlaced: f.interlaced, transparent: f.transparent, frames }
    }

    #[test]
    fn png_vers_gif_entrelace_256_couleurs() {
        let src = png(800, 600, false);
        assert!(src.len() > 50_000, "source volumineuse : {} o", src.len());
        let opts = GifOptions::default();
        let gif = to_gif(&src, &opts).unwrap();
        assert!(gif.starts_with(b"GIF89a"));
        let d = decode(&gif);
        assert!(d.width <= 320, "largeur {}", d.width);
        assert_eq!((d.width, d.height), (320, 240), "largeur maximale, proportions gardées");
        assert!(d.colors <= 256);
        assert!(d.interlaced, "GIF entrelacé");
        assert!(gif.len() <= opts.target, "poids visé tenu : {} o", gif.len());
    }

    #[test]
    fn transparence_et_petites_images() {
        let gif = to_gif(&png(64, 40, true), &GifOptions::default()).unwrap();
        let d = decode(&gif);
        assert_eq!((d.width, d.height), (64, 40), "pas d'agrandissement");
        assert!(d.transparent.is_some(), "transparence gardée");
        // peu de couleurs : palette exacte, pixels intacts
        let img = ImageBuffer::from_fn(30, 20, |x, _| if x < 15 { Rgba([255u8, 0, 0, 255]) } else { Rgba([0, 0, 255, 255]) });
        let gif = encode_rgba(&img, 256, true).unwrap();
        let back = image::load_from_memory(&gif).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(3, 7), &Rgba([255, 0, 0, 255]));
        assert_eq!(back.get_pixel(20, 19), &Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn entrelacement_correct() {
        // chaque ligne a sa couleur : un mauvais ordre des lignes se verrait
        let img = ImageBuffer::from_fn(8, 37, |_, y| Rgba([(y * 6) as u8, 255 - (y * 6) as u8, 0, 255]));
        let gif = encode_rgba(&img, 256, true).unwrap();
        assert!(decode(&gif).interlaced);
        let back = image::load_from_memory(&gif).unwrap().to_rgba8();
        for y in 0..37 {
            assert_eq!(back.get_pixel(5, y), img.get_pixel(5, y), "ligne {y}");
        }
    }

    #[test]
    fn images_generees() {
        let c = construction_gif();
        let d = decode(&c);
        assert_eq!(d.frames, 4, "GIF animé");
        assert!(c.len() < 2000, "{} o", c.len());
        let n = counter_gif("001998");
        let d = decode(&n);
        assert_eq!(d.width as usize, 6 * 13 + 1);
        assert!(n.len() < 600);
        assert!(to_gif(b"pas une image", &GifOptions::default()).is_err());
    }

    #[test]
    fn reduction_quand_trop_lourd() {
        // bruit pseudo-aléatoire : incompressible en 256 couleurs à 320 px
        let mut s = 12345u32;
        let img = ImageBuffer::from_fn(320, 240, |_, _| {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let b = s.to_be_bytes();
            Rgba([b[0], b[1], b[2], 255])
        });
        let mut src = Vec::new();
        img.write_to(&mut Cursor::new(&mut src), ImageFormat::Png).unwrap();
        let opts = GifOptions::new(320, 15 * 1024);
        let gif = to_gif(&src, &opts).unwrap();
        let d = decode(&gif);
        assert!(gif.len() <= opts.target || d.width < 320, "réduit en couleurs puis en taille : {} o, {} px", gif.len(), d.width);
    }
}
