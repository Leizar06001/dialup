//! Compteur de chargement, pour la barre d'état du navigateur.
//!
//! Le proxy voit passer les octets de chaque flux. Pour l'hôte virtuel
//! `web1998` (HTTP/1.0 en clair, une réponse par connexion), il lit en plus
//! l'en-tête de chaque réponse : sa taille (`Content-Length`) et, pour une
//! page, le nombre d'images qu'elle contient (en-tête [`ELEMENTS_HEADER`],
//! ajouté par le FAI). Le navigateur peut ainsi afficher un vrai
//! pourcentage. Pour le reste (HTTPS chiffré), on ne compte que les octets.

use std::collections::HashMap;

/// En-tête des pages web 1998 : nombre d'images distinctes de la page.
pub const ELEMENTS_HEADER: &str = "X-Tinyverse-Elements";
/// Hôte virtuel suivi en détail.
const HOST: &str = "web1998";
/// Au-delà, l'en-tête n'est pas du HTTP qu'on sait lire : on abandonne.
const MAX_HEAD: usize = 8192;
/// Taille supposée d'une image pas encore demandée, tant qu'aucune n'est
/// connue (les GIF du FAI font une quinzaine de Ko)
const IMAGE_GUESS: u64 = 12 * 1024;

/// Instantané pour l'interface.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Progress {
    /// Octets reçus par le proxy depuis le début du tunnel (tous flux)
    pub rx_bytes: u64,
    /// Connexions ouvertes en ce moment
    pub streams: u32,
    /// Dernière page web 1998 (aucune tant qu'on n'en a pas chargé)
    pub page: Option<PageProgress>,
}

/// Avancement de la dernière page web 1998.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct PageProgress {
    /// Change à chaque nouveau document
    pub seq: u32,
    /// Octets du document reçus / annoncés
    pub doc_rx: u64,
    pub doc_len: Option<u64>,
    pub doc_done: bool,
    /// Images annoncées par le FAI (absent : ancien serveur)
    pub elements: Option<u32>,
    /// Images terminées / en cours de chargement
    pub done: u32,
    pub active: u32,
    /// Octets d'images reçus (terminées et en cours)
    pub elements_rx: u64,
    /// Tailles annoncées des images commencées
    pub elements_len: u64,
    /// Avancement de toute la page en octets (document + images, celles pas
    /// encore demandées estimées d'après les autres), de 0 à 1, quand le
    /// nombre d'images est connu
    pub fraction: Option<f32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Doc,
    Element,
}

struct Watch {
    /// En-tête en cours de lecture
    head: Vec<u8>,
    /// Réponse reconnue (en-tête lu) : document ou image
    kind: Option<Kind>,
    /// Page à laquelle la réponse appartient
    page: u32,
    len: Option<u64>,
    rx: u64,
    finished: bool,
}

/// Compteur tenu par le moteur du proxy.
#[derive(Default)]
pub struct Meter {
    watches: HashMap<u16, Watch>,
    page: Option<PageProgress>,
    rx_bytes: u64,
}

impl Meter {
    pub fn new() -> Meter {
        Meter::default()
    }

    /// Un flux vient d'être ouvert vers `host:port`.
    pub fn opened(&mut self, id: u16, host: &str, port: u16) {
        if port == 80 && host.eq_ignore_ascii_case(HOST) {
            self.watches.insert(id, Watch { head: Vec::new(), kind: None, page: 0, len: None, rx: 0, finished: false });
        }
    }

    /// Octets reçus du modem pour le flux `id`.
    pub fn data(&mut self, id: u16, bytes: &[u8]) {
        self.rx_bytes += bytes.len() as u64;
        let Some(mut w) = self.watches.remove(&id) else { return };
        let body: Vec<u8>;
        let mut rest = bytes;
        if w.kind.is_none() {
            w.head.extend_from_slice(bytes);
            let Some(end) = w.head.windows(4).position(|x| x == b"\r\n\r\n") else {
                if w.head.len() <= MAX_HEAD {
                    self.watches.insert(id, w);
                }
                return;
            };
            body = w.head.split_off(end + 4);
            let head = std::mem::take(&mut w.head);
            if !self.start(&mut w, &head) {
                return;
            }
            rest = &body;
        }
        self.body(&mut w, rest.len() as u64);
        if !w.finished {
            self.watches.insert(id, w);
        }
    }

    /// Le flux `id` est fermé (fin normale ou coupure).
    pub fn closed(&mut self, id: u16) {
        if let Some(mut w) = self.watches.remove(&id) {
            if w.kind.is_some() {
                self.finish(&mut w);
            }
        }
    }

    /// Erreur de transmission : tous les flux sont coupés.
    pub fn reset(&mut self) {
        for (_, mut w) in std::mem::take(&mut self.watches) {
            if w.kind.is_some() {
                self.finish(&mut w);
            }
        }
    }

    pub fn snapshot(&self, streams: u32) -> Progress {
        let page = self.page.clone().map(|mut p| {
            p.fraction = p.elements.map(|n| {
                let doc = if p.doc_done { p.doc_rx } else { p.doc_len.unwrap_or(p.doc_rx).max(p.doc_rx) };
                let started = p.done + p.active;
                let guess = if started > 0 && p.elements_len > 0 { p.elements_len / started as u64 } else { IMAGE_GUESS };
                let total = doc + p.elements_len + n.saturating_sub(started) as u64 * guess;
                let got = p.doc_rx + p.elements_rx;
                if total == 0 {
                    1.0
                } else {
                    (got as f64 / total as f64).min(1.0) as f32
                }
            });
            p
        });
        Progress { rx_bytes: self.rx_bytes, streams, page }
    }

    /// En-tête lu : document (nouvelle page) ou image de la page en cours.
    /// Faux si ce n'est pas une réponse HTTP.
    fn start(&mut self, w: &mut Watch, head: &[u8]) -> bool {
        let head = String::from_utf8_lossy(head);
        let mut lines = head.split("\r\n");
        if !lines.next().is_some_and(|l| l.starts_with("HTTP/")) {
            return false;
        }
        let (mut len, mut elements, mut html) = (None, None, false);
        for l in lines {
            let Some((k, v)) = l.split_once(':') else { continue };
            let (k, v) = (k.trim(), v.trim());
            if k.eq_ignore_ascii_case("content-length") {
                len = v.parse::<u64>().ok();
            } else if k.eq_ignore_ascii_case(ELEMENTS_HEADER) {
                elements = v.parse::<u32>().ok();
            } else if k.eq_ignore_ascii_case("content-type") {
                html = v.to_ascii_lowercase().starts_with("text/html");
            }
        }
        w.len = len;
        if elements.is_some() || html {
            let seq = self.page.as_ref().map_or(1, |p| p.seq.wrapping_add(1));
            self.page = Some(PageProgress { seq, doc_len: len, elements, ..Default::default() });
            w.kind = Some(Kind::Doc);
            w.page = seq;
        } else {
            w.kind = Some(Kind::Element);
            w.page = self.page.as_ref().map_or(0, |p| p.seq);
            if let Some(p) = self.page.as_mut().filter(|p| p.seq == w.page) {
                p.active += 1;
                p.elements_len += len.unwrap_or(0);
            }
        }
        if len == Some(0) {
            self.finish(w);
        }
        true
    }

    fn body(&mut self, w: &mut Watch, n: u64) {
        if w.finished || n == 0 {
            return;
        }
        w.rx += n;
        if let Some(p) = self.page.as_mut().filter(|p| p.seq == w.page) {
            match w.kind {
                Some(Kind::Doc) => p.doc_rx += n,
                Some(Kind::Element) => p.elements_rx += n,
                None => {}
            }
        }
        if w.len.is_some_and(|l| w.rx >= l) {
            self.finish(w);
        }
    }

    fn finish(&mut self, w: &mut Watch) {
        if w.finished {
            return;
        }
        w.finished = true;
        if let Some(p) = self.page.as_mut().filter(|p| p.seq == w.page) {
            match w.kind {
                Some(Kind::Doc) => p.doc_done = true,
                Some(Kind::Element) => {
                    p.active = p.active.saturating_sub(1);
                    p.done += 1;
                    // image coupée en route : on ne l'attend plus
                    if let Some(l) = w.len {
                        p.elements_len = p.elements_len.saturating_sub(l.saturating_sub(w.rx));
                    }
                }
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reponse(ctype: &str, body: &[u8], extra: &str) -> Vec<u8> {
        let mut v = format!("HTTP/1.0 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n", body.len()).into_bytes();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn page_et_images_au_pourcentage() {
        let mut m = Meter::new();
        m.opened(1, "web1998", 80);
        m.opened(9, "example.org", 443); // HTTPS : octets comptés seulement
        m.data(9, &[0u8; 100]);
        let doc = reponse("text/html; charset=utf-8", &[b'x'; 1000], "X-Tinyverse-Elements: 2\r\n");
        // l'en-tête arrive en deux morceaux, coupé au milieu de \r\n\r\n
        let cut = doc.windows(4).position(|x| x == b"\r\n\r\n").unwrap() + 2;
        m.data(1, &doc[..cut]);
        assert!(m.snapshot(2).page.is_none(), "en-tête incomplet");
        m.data(1, &doc[cut..cut + 2 + 500]);
        let p = m.snapshot(2).page.unwrap();
        assert_eq!((p.seq, p.doc_rx, p.doc_len, p.elements), (1, 500, Some(1000), Some(2)));
        // images pas encore demandées : taille supposée
        let attendu = 500.0 / (1000.0 + 2.0 * IMAGE_GUESS as f32);
        assert!((p.fraction.unwrap() - attendu).abs() < 1e-4, "{p:?}");
        m.data(1, &doc[cut + 2 + 500..]);
        assert!(m.snapshot(2).page.unwrap().doc_done);

        // deux images, dont une à moitié
        let img = reponse("image/gif", &[7u8; 400], "");
        m.opened(2, "WEB1998", 80);
        m.opened(3, "web1998", 80);
        m.data(2, &img);
        m.data(3, &img[..img.len() - 200]);
        let p = m.snapshot(3).page.unwrap();
        assert_eq!((p.done, p.active, p.elements_rx, p.elements_len), (1, 1, 600, 800));
        assert!((p.fraction.unwrap() - 1600.0 / 1800.0).abs() < 1e-4, "{p:?}");
        m.closed(3); // coupure : l'image compte comme terminée
        let s = m.snapshot(0);
        assert_eq!(s.page.as_ref().unwrap().fraction, Some(1.0));
        assert_eq!(s.rx_bytes, 100 + doc.len() as u64 + 2 * img.len() as u64 - 200);

        // page suivante : on repart de zéro
        m.opened(4, "web1998", 80);
        m.opened(5, "web1998", 80);
        m.data(5, &img[..50]); // en-tête d'image pas encore complet
        m.data(4, &reponse("text/html", b"<p>", "X-Tinyverse-Elements: 0\r\n"));
        let p = m.snapshot(1).page.unwrap();
        assert_eq!((p.seq, p.done, p.fraction), (2, 0, Some(1.0)));
    }

    #[test]
    fn ancien_serveur_sans_nombre_d_images() {
        let mut m = Meter::new();
        m.opened(1, "web1998", 80);
        m.data(1, &reponse("text/html", &[b'x'; 10], ""));
        let p = m.snapshot(0).page.unwrap();
        assert!(p.doc_done && p.elements.is_none() && p.fraction.is_none());
        m.opened(2, "web1998", 80);
        m.data(2, b"pas du HTTP\r\n\r\n");
        m.data(2, b"encore");
        assert_eq!(m.snapshot(0).page.unwrap().active, 0);
    }
}
