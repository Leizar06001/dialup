//! Visiophone « V.Hallucination » : la webcam de chaque abonné, redessinée
//! en une quarantaine de triangles et d'ellipses semi-transparents par le
//! codec des images du web 1998 ([`v34_server::web1998::hallu`]), et
//! envoyée en quelques centaines d'octets par image. Ressemblant comme un
//! rêve ressemble à la veille ; une image de 64 × 48 pixels bruts pèserait
//! 9 Ko, celle-ci en pèse 300.
//!
//! Format d'une image (charge d'une trame vidéo du protocole BBS) :
//!
//! ```text
//! octet 0      version (1)
//! octets 1-2   largeur, hauteur de la grille de travail (1 à 96)
//! octet 3      opacité commune des formes (0-255)
//! octets 4-6   couleur du fond (R, V, B)
//! octet 7      nombre de formes
//! puis les formes, bit à bit (poids fort d'abord, sans alignement) :
//!   0     triangle : x1 y1 x2 y2 x3 y3
//!   1 0   ellipse  : cx cy rx ry
//!   1 1   cercle   : cx cy r
//!   chaque coordonnée sur 7 bits, décalée de 16 (-16 à 111), chaque rayon
//!   sur 7 bits (1 à 127), puis la couleur #rgb sur 12 bits.
//! ```
//!
//! Triangle 55 bits, ellipse 42, cercle 35 : 40 formes tiennent en
//! 283 octets au pire (en-tête compris). Le SVG ne sert qu'à l'affichage.
//!
//! Cadence : une image ne part que si la file d'émission du modem tient en
//! un quart de seconde de ligne, qu'aucun fichier n'est en cours d'envoi, et au plus
//! [`MAX_FPS`] fois par seconde sans dépasser [`SHARE`] du débit : le chat
//! passe toujours devant, les fichiers aussi.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::Serialize;
use v34_server::web1998::hallu::{self, Drawing, HalluOptions, Shape};

use crate::bbs::{self, Frame};

/// Grille de travail (4:3, comme la webcam)
pub const WORK_W: u32 = 64;
pub const WORK_H: u32 = 48;
/// Formes par image
pub const MAX_SHAPES: usize = 40;
/// Images par seconde au plus
pub const MAX_FPS: f64 = 3.0;
/// Part du débit d'émission que la vidéo s'accorde au plus
pub const SHARE: f64 = 0.5;
/// File d'émission (octets pas encore émis ou pas encore acquittés) au-delà
/// de laquelle on attend : ce que la ligne émet en ce temps-là. Une image
/// peut partir avant l'acquittement de la précédente, et un message du chat
/// n'attend jamais plus d'un quart de seconde derrière la vidéo.
pub const QUEUE_TIME: f64 = 0.25;

/// Seuil de la file d'émission (octets) pour un débit donné.
fn queue_max(rate_bps: u32) -> usize {
    ((rate_bps.max(2400) as f64 / 8.0 * QUEUE_TIME) as usize).max(64)
}
/// Version du format
const VERSION: u8 = 1;
/// Plus grand côté accepté de la grille (coordonnées sur 7 bits)
const MAX_SIDE: u8 = 96;
/// Décalage des coordonnées (marge hors de l'image)
const OFFSET: i32 = 16;
/// Opacité des formes et flou de l'affichage (pixels de travail)
const ALPHA: f32 = 0.6;
const BLUR: f32 = 0.7;
/// Taille d'affichage du SVG (il est mis à l'échelle de toute façon)
const SHOW_W: u32 = 320;
const SHOW_H: u32 = 240;
/// Annonce sans réponse depuis ce temps : l'autre n'a pas de visiophone
const PEER_TIMEOUT: Duration = Duration::from_secs(8);
/// Fenêtre des statistiques (images/s, octets/image, débit)
const STATS_WINDOW: Duration = Duration::from_secs(4);

/// Réglages du codec pour la vidéo : petite grille, nombre de formes fixe
/// (pas de budget en octets de SVG), quelques millisecondes par image.
fn options() -> HalluOptions {
    HalluOptions {
        work_size: WORK_W.max(WORK_H),
        max_shapes: MAX_SHAPES,
        max_bytes: 1 << 20,
        candidates: 1500,
        max_age: 150,
        max_mutations: 3000,
        alpha: ALPHA,
        blur: BLUR,
        time_limit: Duration::from_millis(150),
        ..HalluOptions::new(SHOW_W)
    }
}

// ------------------------------------------------------------ format binaire

/// Écriture bit à bit, poids fort d'abord.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, bits: u32) {
        for i in (0..bits).rev() {
            self.acc = (self.acc << 1) | ((v >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                self.out.push(self.acc as u8);
                (self.acc, self.n) = (0, 0);
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push((self.acc << (8 - self.n)) as u8);
        }
        self.out
    }
}

/// Lecture bit à bit (`None` au-delà de la fin).
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn get(&mut self, bits: u32) -> Option<u32> {
        let mut v = 0;
        for _ in 0..bits {
            let byte = *self.data.get(self.pos / 8)?;
            v = (v << 1) | ((byte >> (7 - self.pos % 8)) & 1) as u32;
            self.pos += 1;
        }
        Some(v)
    }
}

fn coord(v: i16) -> u32 {
    (v as i32 + OFFSET).clamp(0, 127) as u32
}

fn radius(v: i16) -> u32 {
    (v as i32).clamp(1, 127) as u32
}

/// Composante quantifiée sur 4 bits (le codec arrondit déjà à `#rgb`).
fn nibble(c: u8) -> u32 {
    ((c as u32 + 8) / 17).min(15)
}

/// Image rêvée prête à partir : le dessin et l'opacité de ses formes.
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub alpha: f32,
    pub background: [u8; 3],
    pub shapes: Vec<(Shape, [u8; 3])>,
}

impl Picture {
    pub fn from_drawing(d: &Drawing, alpha: f32) -> Picture {
        Picture { width: d.width, height: d.height, alpha, background: d.background, shapes: d.shapes.clone() }
    }

    /// Format binaire (voir l'en-tête du module). Au plus 255 formes ; les
    /// valeurs hors format sont ramenées dans les bornes.
    pub fn encode(&self) -> Vec<u8> {
        let side = |v: u32| v.clamp(1, MAX_SIDE as u32) as u8;
        let n = self.shapes.len().min(255);
        let mut out = vec![VERSION, side(self.width), side(self.height), (self.alpha.clamp(0.0, 1.0) * 255.0).round() as u8];
        out.extend(self.background);
        out.push(n as u8);
        let mut b = Bits::default();
        for (shape, c) in &self.shapes[..n] {
            match *shape {
                Shape::Triangle(v) => {
                    b.put(0, 1);
                    v.iter().for_each(|&x| b.put(coord(x), 7));
                }
                Shape::Ellipse([cx, cy, rx, ry]) if rx == ry => {
                    b.put(0b11, 2);
                    b.put(coord(cx), 7);
                    b.put(coord(cy), 7);
                    b.put(radius(rx), 7);
                }
                Shape::Ellipse([cx, cy, rx, ry]) => {
                    b.put(0b10, 2);
                    b.put(coord(cx), 7);
                    b.put(coord(cy), 7);
                    b.put(radius(rx), 7);
                    b.put(radius(ry), 7);
                }
            }
            c.iter().for_each(|&k| b.put(nibble(k), 4));
        }
        out.extend(b.finish());
        out
    }

    /// Relit une image reçue (`None` si elle est tronquée ou d'un autre
    /// format : elle vient du réseau, on ne lui fait pas confiance).
    pub fn decode(data: &[u8]) -> Option<Picture> {
        let (&[version, w, h, alpha, r, g, bl, n], rest) = data.split_first_chunk::<8>()?;
        if version != VERSION || !(1..=MAX_SIDE).contains(&w) || !(1..=MAX_SIDE).contains(&h) {
            return None;
        }
        let mut rd = Reader { data: rest, pos: 0 };
        let mut shapes = Vec::with_capacity(n as usize);
        let c = |rd: &mut Reader| rd.get(7).map(|v| (v as i32 - OFFSET) as i16);
        let r7 = |rd: &mut Reader| rd.get(7).map(|v| v.max(1) as i16);
        let k = |rd: &mut Reader| rd.get(4).map(|v| v as u8 * 17);
        for _ in 0..n {
            let shape = if rd.get(1)? == 0 {
                Shape::Triangle([c(&mut rd)?, c(&mut rd)?, c(&mut rd)?, c(&mut rd)?, c(&mut rd)?, c(&mut rd)?])
            } else if rd.get(1)? == 1 {
                let (cx, cy, r) = (c(&mut rd)?, c(&mut rd)?, r7(&mut rd)?);
                Shape::Ellipse([cx, cy, r, r])
            } else {
                Shape::Ellipse([c(&mut rd)?, c(&mut rd)?, r7(&mut rd)?, r7(&mut rd)?])
            };
            shapes.push((shape, [k(&mut rd)?, k(&mut rd)?, k(&mut rd)?]));
        }
        Some(Picture { width: w as u32, height: h as u32, alpha: alpha as f32 / 255.0, background: [r, g, bl], shapes })
    }

    /// SVG pour l'affichage (avec le flou « rêve »).
    pub fn svg(&self) -> String {
        let d = Drawing { width: self.width, height: self.height, background: self.background, shapes: self.shapes.clone(), rmse: 0.0 };
        d.svg(SHOW_W, SHOW_H, &HalluOptions { alpha: self.alpha, ..options() })
    }
}

/// Une image de la caméra, hallucinée.
#[derive(Clone, Debug, Serialize)]
pub struct Encoded {
    /// Format binaire, à envoyer (sans l'en-tête de trame)
    #[serde(skip)]
    pub wire: Vec<u8>,
    /// Ce que verra l'autre (décodé depuis `wire`)
    pub svg: String,
    pub bytes: usize,
    pub shapes: usize,
    /// Temps de calcul (ms)
    pub ms: f32,
}

/// Hallucine une image RGBA de la caméra (ou de la mire).
pub fn encode_rgba(rgba: &[u8], w: u32, h: u32) -> Result<Encoded, String> {
    if w > 1280 || h > 960 {
        return Err(format!("image trop grande pour le visiophone ({w}×{h})"));
    }
    let t0 = Instant::now();
    let opts = options();
    // Même graine à chaque image : une scène immobile donne à peu près le
    // même rêve d'une image à l'autre (moins de scintillement).
    let drawing = hallu::from_rgba(rgba, w, h, MAX_SHAPES, &opts, hallu::seed("visiophone"))?;
    let wire = Picture::from_drawing(&drawing, opts.alpha).encode();
    let pic = Picture::decode(&wire).ok_or("image illisible après codage")?;
    Ok(Encoded { svg: pic.svg(), bytes: wire.len() + 4, shapes: pic.shapes.len(), wire, ms: t0.elapsed().as_secs_f32() * 1000.0 })
}

// ------------------------------------------------------------ régulation

/// Ce que le visiophone doit savoir de la liaison.
#[derive(Clone, Copy, Debug, Default)]
pub struct LinkCtx {
    /// Appel entre abonnés, en mode données (pas de tunnel Internet)
    pub online: bool,
    /// Octets en attente d'émission ou d'acquittement (LAPM)
    pub tx_queue: usize,
    /// Débit d'émission négocié (bit/s)
    pub rate_bps: u32,
    /// Un fichier est en cours d'envoi
    pub file_busy: bool,
}

/// Le correspondant, vu d'ici.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Peer {
    /// Pas (encore) de nouvelles
    Unknown,
    /// Il a un visiophone, caméra allumée ou non
    Visio { camera: bool },
}

/// Images passées dans un sens, pour les statistiques.
#[derive(Default)]
struct Meter(VecDeque<(Instant, usize)>);

impl Meter {
    fn push(&mut self, now: Instant, bytes: usize) {
        self.0.push_back((now, bytes));
        while self.0.front().is_some_and(|(t, _)| now.duration_since(*t) > STATS_WINDOW) {
            self.0.pop_front();
        }
    }

    fn get(&self, now: Instant) -> Flow {
        let recent: Vec<_> = self.0.iter().filter(|(t, _)| now.duration_since(*t) <= STATS_WINDOW).collect();
        if recent.is_empty() {
            return Flow::default();
        }
        let total: usize = recent.iter().map(|(_, n)| n).sum();
        let secs = STATS_WINDOW.as_secs_f32();
        Flow { fps: recent.len() as f32 / secs, bytes: (total / recent.len()) as u32, rate: total as f32 / secs }
    }
}

/// Débit d'images dans un sens.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
pub struct Flow {
    /// Images par seconde
    pub fps: f32,
    /// Octets par image (trame comprise)
    pub bytes: u32,
    /// Octets par seconde
    pub rate: f32,
}

/// État du visiophone pour l'IHM.
#[derive(Clone, Debug, Default, Serialize)]
pub struct VisioUi {
    pub online: bool,
    pub camera: bool,
    /// "?" (pas de nouvelles), "attente" (annoncé, pas de réponse),
    /// "muet" (pas de réponse : appli trop ancienne), "ok"
    pub peer: &'static str,
    pub peer_camera: bool,
    /// Le moteur prendrait une image maintenant
    pub ready: bool,
    /// Raison de la pause des envois ("fichier", "file")
    pub pause: Option<&'static str>,
    pub tx: Flow,
    pub rx: Flow,
}

/// Une image reçue, pour l'IHM (événement `v34:visio`).
#[derive(Clone, Debug, Serialize)]
pub struct RemoteFrame {
    pub svg: String,
    pub bytes: usize,
    pub shapes: usize,
}

/// Le visiophone d'un appel : annonces, cadence, statistiques.
pub struct Visio {
    /// Notre caméra (ou la mire) envoie des images
    camera: bool,
    /// Annonce envoyée pendant cet appel (et quand)
    announced: Option<Instant>,
    peer: Peer,
    last_sent: Option<Instant>,
    last_len: usize,
    tx: Meter,
    rx: Meter,
}

impl Default for Visio {
    fn default() -> Self {
        Visio { camera: false, announced: None, peer: Peer::Unknown, last_sent: None, last_len: 0, tx: Meter::default(), rx: Meter::default() }
    }
}

impl Visio {
    /// Nouvel appel : on oublie le correspondant (la caméra reste comme elle est).
    pub fn reset(&mut self) {
        *self = Visio { camera: self.camera, ..Visio::default() };
    }

    fn announce(&mut self, now: Instant) -> Vec<u8> {
        self.announced = Some(now);
        bbs::encode(&Frame::Visio { camera: self.camera })
    }

    /// Caméra allumée ou éteinte : on prévient le correspondant (et on
    /// s'annonce si ce n'est pas déjà fait).
    pub fn set_camera(&mut self, on: bool, ctx: &LinkCtx, now: Instant) -> Option<Vec<u8>> {
        if self.camera == on {
            return None;
        }
        self.camera = on;
        (ctx.online && (on || self.announced.is_some())).then(|| self.announce(now))
    }

    /// Délai minimal entre deux images : cadence maximale, et part du débit.
    fn interval(&self, ctx: &LinkCtx) -> Duration {
        let by_rate = self.last_len as f64 * 8.0 / (ctx.rate_bps.max(2400) as f64 * SHARE);
        Duration::from_secs_f64(by_rate.max(1.0 / MAX_FPS))
    }

    /// Pourquoi on n'enverrait pas d'image maintenant (`None` : on peut).
    fn hold(&self, ctx: &LinkCtx, now: Instant) -> Option<&'static str> {
        if ctx.file_busy {
            Some("fichier")
        } else if ctx.tx_queue > queue_max(ctx.rate_bps) {
            Some("file")
        } else if self.last_sent.is_some_and(|t| now.duration_since(t) < self.interval(ctx)) {
            Some("cadence")
        } else {
            None
        }
    }

    /// Le moteur prendrait-il une image maintenant ?
    pub fn ready(&self, ctx: &LinkCtx, now: Instant) -> bool {
        self.camera
            && ctx.online
            && match (self.announced, self.peer) {
                (None, _) => true, // la première sert à s'annoncer
                (Some(_), Peer::Visio { .. }) => self.hold(ctx, now).is_none(),
                (Some(_), Peer::Unknown) => false,
            }
    }

    /// Une image hallucinée par l'IHM : les octets à envoyer, s'il y a lieu
    /// (l'annonce d'abord, les images ensuite, quand la ligne le permet).
    pub fn offer(&mut self, wire: Vec<u8>, ctx: &LinkCtx, now: Instant) -> Option<Vec<u8>> {
        if !self.camera || !ctx.online {
            return None;
        }
        if self.announced.is_none() {
            return Some(self.announce(now));
        }
        if !self.ready(ctx, now) {
            return None;
        }
        let bytes = bbs::encode(&Frame::Video(wire));
        self.last_sent = Some(now);
        self.last_len = bytes.len();
        self.tx.push(now, bytes.len());
        Some(bytes)
    }

    /// Annonce du correspondant : il a un visiophone. On lui répond par la
    /// nôtre si on ne s'est pas encore annoncé (sinon, pas de ping-pong).
    pub fn on_announce(&mut self, camera: bool, ctx: &LinkCtx, now: Instant) -> Option<Vec<u8>> {
        self.peer = Peer::Visio { camera };
        (ctx.online && self.announced.is_none()).then(|| self.announce(now))
    }

    /// Image reçue : à afficher (`None` si elle est illisible).
    pub fn on_video(&mut self, data: &[u8], now: Instant) -> Option<RemoteFrame> {
        let pic = Picture::decode(data)?;
        self.peer = Peer::Visio { camera: true };
        self.rx.push(now, data.len() + 4);
        Some(RemoteFrame { svg: pic.svg(), bytes: data.len() + 4, shapes: pic.shapes.len() })
    }

    pub fn ui(&self, ctx: &LinkCtx, now: Instant) -> VisioUi {
        let peer = match (self.peer, self.announced) {
            (Peer::Visio { .. }, _) => "ok",
            (Peer::Unknown, Some(t)) if now.duration_since(t) > PEER_TIMEOUT => "muet",
            (Peer::Unknown, Some(_)) => "attente",
            (Peer::Unknown, None) => "?",
        };
        let sending = self.camera && ctx.online && peer == "ok";
        VisioUi {
            online: ctx.online,
            camera: self.camera,
            peer,
            peer_camera: self.peer == Peer::Visio { camera: true },
            ready: self.ready(ctx, now),
            pause: if sending { self.hold(ctx, now).filter(|r| *r != "cadence") } else { None },
            tx: self.tx.get(now),
            rx: self.rx.get(now),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Visage de synthèse sur fond de mur, 160 × 120 RGBA (comme la mire).
    fn visage(t: f32) -> Vec<u8> {
        let mut out = Vec::with_capacity(160 * 120 * 4);
        for y in 0..120 {
            for x in 0..160 {
                let (fx, fy) = (x as f32, y as f32);
                let cx = 80.0 + 15.0 * t.sin();
                let mut c = if fy < 70.0 { [200, 180, 140] } else { [120, 60, 40] };
                if ((fx - cx) / 28.0).powi(2) + ((fy - 55.0) / 36.0).powi(2) < 1.0 {
                    c = [230, 180, 130];
                }
                if ((fx - cx + 10.0).powi(2) + (fy - 48.0).powi(2)) < 16.0 || ((fx - cx - 10.0).powi(2) + (fy - 48.0).powi(2)) < 16.0 {
                    c = [30, 30, 40];
                }
                if fy > 100.0 {
                    c = [180, 30, 40];
                }
                out.extend([c[0], c[1], c[2], 255]);
            }
        }
        out
    }

    #[test]
    fn aller_retour_et_taille() {
        for t in [0.0, 1.0, 2.5] {
            let e = encode_rgba(&visage(t), 160, 120).unwrap();
            eprintln!("visio : {} o, {} formes, {:.1} ms", e.bytes, e.shapes, e.ms);
            assert_eq!(e.shapes, MAX_SHAPES, "une image chargée garde toutes ses formes");
            assert!(e.wire.len() <= 8 + (MAX_SHAPES * 55).div_ceil(8), "{} o", e.wire.len());
            assert!(e.bytes <= 400, "{} o par image", e.bytes);
            // le binaire décrit exactement le dessin du codec
            let opts = options();
            let d = hallu::from_rgba(&visage(t), 160, 120, MAX_SHAPES, &opts, hallu::seed("visiophone")).unwrap();
            let p = Picture::decode(&e.wire).unwrap();
            assert_eq!((p.width, p.height, p.background), (WORK_W, WORK_H, d.background));
            assert_eq!(p.shapes, d.shapes, "formes et couleurs sans perte");
            assert!((p.alpha - ALPHA).abs() < 0.003);
            assert_eq!(p.encode(), e.wire);
            assert!(e.svg.starts_with("<svg") && e.svg.ends_with("</svg>"));
        }
    }

    #[test]
    fn toutes_les_formes_aux_bornes() {
        let p = Picture {
            width: 96,
            height: 72,
            alpha: 1.0,
            background: [1, 2, 3],
            shapes: vec![
                (Shape::Triangle([-16, 111, 0, 95, -12, 107]), [255, 0, 17]),
                (Shape::Ellipse([0, 71, 1, 127]), [0, 0, 0]),
                (Shape::Ellipse([95, 0, 96, 96]), [238, 221, 204]),
            ],
        };
        let b = p.encode();
        assert_eq!(b.len(), 8 + (55 + 42 + 35usize).div_ceil(8));
        assert_eq!(Picture::decode(&b), Some(p));
        // image vide : l'en-tête seul
        let vide = Picture { width: 64, height: 48, alpha: 0.6, background: [9, 9, 9], shapes: vec![] };
        assert_eq!(vide.encode().len(), 8);
        assert_eq!(Picture::decode(&vide.encode()).unwrap().shapes, vec![]);
    }

    #[test]
    fn images_abimees_refusees() {
        let b = encode_rgba(&visage(0.3), 160, 120).unwrap().wire;
        // tronquée d'un octet ou plus : refusée
        for n in 0..b.len() {
            assert!(Picture::decode(&b[..n]).is_none(), "{n} octets");
        }
        let mut v = b.clone();
        v[0] = 2;
        assert!(Picture::decode(&v).is_none(), "autre version");
        let mut v = b.clone();
        v[1] = 0;
        assert!(Picture::decode(&v).is_none(), "largeur nulle");
        let mut v = b;
        v[7] = 255;
        assert!(Picture::decode(&v).is_none(), "plus de formes annoncées que d'octets");
        assert!(encode_rgba(&[0; 10], 160, 120).is_err());
        // octets quelconques : jamais de panique
        let mut s = 1u32;
        for _ in 0..2000 {
            let n = (s % 64) as usize;
            let junk: Vec<u8> = (0..n)
                .map(|_| {
                    s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (s >> 16) as u8
                })
                .collect();
            let _ = Picture::decode(&junk);
        }
    }

    fn ctx() -> LinkCtx {
        LinkCtx { online: true, tx_queue: 0, rate_bps: 28800, file_busy: false }
    }

    /// Trames décodées d'octets émis.
    fn frames(bytes: &[u8]) -> Vec<Frame> {
        let mut d = bbs::Decoder::default();
        d.push(bytes).into_iter().filter_map(|i| if let bbs::Item::Frame(f) = i { Some(f) } else { None }).collect()
    }

    #[test]
    fn annonce_puis_images_a_la_cadence() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let (mut a, mut b) = (Visio::default(), Visio::default());
        let img = encode_rgba(&visage(0.0), 160, 120).unwrap().wire;
        // caméra éteinte, ou pas d'appel : rien ne part
        assert!(a.offer(img.clone(), &ctx(), t0).is_none());
        assert!(a.set_camera(true, &LinkCtx::default(), t0).is_none());
        assert!(!a.ready(&LinkCtx::default(), t0));
        // en ligne : la première image sert d'annonce
        assert!(a.ready(&ctx(), t0));
        let hello = a.offer(img.clone(), &ctx(), t0).unwrap();
        assert_eq!(frames(&hello), vec![Frame::Visio { camera: true }]);
        assert!(!a.ready(&ctx(), ms(100)), "pas d'image avant la réponse");
        assert_eq!(a.ui(&ctx(), ms(100)).peer, "attente");
        assert_eq!(a.ui(&ctx(), ms(9000)).peer, "muet");
        // B répond une fois (caméra éteinte), et pas deux
        let reply = b.on_announce(true, &ctx(), ms(150)).unwrap();
        assert_eq!(frames(&reply), vec![Frame::Visio { camera: false }]);
        assert!(b.on_announce(true, &ctx(), ms(160)).is_none());
        assert!(a.on_announce(false, &ctx(), ms(200)).is_none(), "A s'était annoncé : pas de ping-pong");
        // les images partent, au plus MAX_FPS par seconde
        let f = a.offer(img.clone(), &ctx(), ms(200)).unwrap();
        let Frame::Video(got) = &frames(&f)[0] else { panic!("trame vidéo attendue") };
        assert!(b.on_video(got, ms(300)).is_some());
        assert!(b.ui(&ctx(), ms(300)).peer_camera);
        assert!(a.offer(img.clone(), &ctx(), ms(300)).is_none(), "trop tôt");
        assert!(a.offer(img.clone(), &ctx(), ms(200 + 340)).is_some());
        // la file d'émission ou un fichier passent devant
        let busy = LinkCtx { tx_queue: 2000, ..ctx() };
        assert!(a.ready(&LinkCtx { tx_queue: 600, ..ctx() }, ms(2000)), "une image en attente d'acquittement ne bloque pas");
        assert!(!a.ready(&busy, ms(2000)));
        assert_eq!(a.ui(&busy, ms(2000)).pause, Some("file"));
        let file = LinkCtx { file_busy: true, ..ctx() };
        assert!(a.offer(img.clone(), &file, ms(2000)).is_none());
        assert_eq!(a.ui(&file, ms(2000)).pause, Some("fichier"));
        // à 2 400 bit/s, la part du débit freine plus que la cadence
        let lent = LinkCtx { rate_bps: 2400, ..ctx() };
        assert!(a.offer(img.clone(), &lent, ms(3000)).is_some());
        assert!(a.offer(img.clone(), &lent, ms(3000 + 900)).is_none());
        assert!(a.offer(img.clone(), &lent, ms(3000 + 2000)).is_some());
        let ui = a.ui(&ctx(), ms(5000));
        assert!(ui.tx.fps > 0.0 && ui.tx.bytes as usize == img.len() + 4, "{ui:?}");
        // caméra coupée : on prévient ; nouvel appel : on oublie le correspondant
        assert_eq!(frames(&a.set_camera(false, &ctx(), ms(6000)).unwrap()), vec![Frame::Visio { camera: false }]);
        a.reset();
        assert_eq!(a.ui(&ctx(), ms(6000)).peer, "?");
    }

    #[test]
    fn debit_raisonnable() {
        // 3 images/s de ~300 octets : ~1 Ko/s, la moitié au plus de 28 800 bit/s
        let (mut a, t0) = (Visio::default(), Instant::now());
        a.set_camera(true, &ctx(), t0);
        a.on_announce(true, &ctx(), t0);
        let img = encode_rgba(&visage(1.0), 160, 120).unwrap().wire;
        let mut sent = 0;
        for i in 0..100 {
            let now = t0 + Duration::from_millis(40 * i);
            if let Some(b) = a.offer(img.clone(), &ctx(), now) {
                sent += b.len();
            }
        }
        let rate = sent as f64 / 4.0;
        assert!(rate <= MAX_FPS * 400.0 && rate <= 28800.0 / 8.0 * SHARE, "{rate:.0} o/s");
        assert!(rate > 500.0, "{rate:.0} o/s");
    }
}
