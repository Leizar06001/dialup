//! # Tinyblog : les blogs des abonnés, sur l'hôte virtuel `http://tinyblog/`
//!
//! Façon blogs des années 2000 : un blog par abonné, des articles (titre,
//! texte mis en forme par codes, une image), des « j'kiffe » et des
//! commentaires. Lisible seulement par modem : comme `web1998:80` et
//! `salon:6667`, l'hôte `tinyblog:80` n'existe pas sur Internet, le bout
//! serveur du tunnel du FAI le sert lui-même. Il sait donc qui visite (le
//! nom d'abonnement, déjà authentifié par le central) : pas de mot de passe.
//!
//! | Chemin | Contenu |
//! |---|---|
//! | `GET /` | accueil : derniers articles, les plus kiffés, tous les blogs |
//! | `GET /<abonné>` (`?page=n`) | son blog, 5 articles par page |
//! | `GET /gerer` | écrire un article, régler son blog, supprimer |
//! | `POST /gerer/article` | nouvel article (formulaire multipart : titre, texte, image) |
//! | `POST /gerer/reglages`, `/gerer/supprimer?id=` | réglages, suppression |
//! | `POST /<abonné>/kiffe?id=`, `/<abonné>/com?id=` | j'kiffe (ou plus), commentaire |
//! | `POST /<abonné>/com/supprimer?id=&k=` | supprimer un commentaire (l'auteur du blog ou du commentaire) |
//! | `GET /img/<fichier>` | images (GIF, réduites à l'envoi) |
//!
//! Succès : « Blogueur » (premier article), « Lâche tes com's » (5
//! commentaires), « Star du net » (10 j'kiffe sur son blog).

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use url::form_urlencoded;

use crate::caches::Exploits;
use crate::debug;

pub mod pages;
pub mod store;
#[cfg(test)]
mod tests;

use store::{Article, Com, Store};

pub const HOST: &str = "tinyblog";
pub const PORT: u16 = 80;

/// Corps de requête le plus gros accepté (une image envoyée par le modem)
const CORPS_MAX: usize = 600 * 1024;
const TITRE_MAX: usize = 80;
const TEXTE_MAX: usize = 4000;
const DESCRIPTION_MAX: usize = 300;
const COM_MAX: usize = 300;
const COMS_PAR_ARTICLE: usize = 200;
const ARTICLES_MAX: usize = 300;
/// Un commentaire toutes les 5 s au plus, par abonné
const COM_INTERVALLE: Duration = Duration::from_secs(5);

pub fn is_virtual_host(host: &str, port: u16) -> bool {
    port == PORT && host.strip_suffix('.').unwrap_or(host).eq_ignore_ascii_case(HOST)
}

/// Une réponse HTTP.
#[derive(Debug)]
pub struct Reponse {
    pub statut: u16,
    pub type_: &'static str,
    pub corps: Vec<u8>,
    pub location: Option<String>,
}

impl Reponse {
    fn html(statut: u16, page: String) -> Reponse {
        Reponse { statut, type_: "text/html; charset=utf-8", corps: page.into_bytes(), location: None }
    }
    fn vers(chemin: String) -> Reponse {
        Reponse { statut: 303, type_: "text/html; charset=utf-8", corps: Vec::new(), location: Some(chemin) }
    }
    fn erreur(statut: u16, titre: &str, texte: &str) -> Reponse {
        Reponse::html(statut, pages::erreur(titre, texte))
    }
}

/// Une requête lue.
pub struct Requete {
    pub methode: String,
    pub cible: String,
    pub type_contenu: String,
    pub corps: Vec<u8>,
}

pub struct Tinyblog {
    store: Store,
    exploits: Exploits,
    derniers_coms: Mutex<HashMap<String, Instant>>,
}

impl std::fmt::Debug for Tinyblog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tinyblog")
    }
}

fn maintenant() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Tinyblog {
    pub fn new(dir: PathBuf, exploits: Exploits) -> Arc<Tinyblog> {
        Arc::new(Tinyblog { store: Store::new(dir), exploits, derniers_coms: Mutex::new(HashMap::new()) })
    }

    /// Connexion du navigateur de `abonne` vers le serveur interne (le
    /// tunnel l'ouvre quand l'abonné demande `tinyblog:80`).
    pub fn open_stream(self: &Arc<Self>, abonne: &str) -> io::Result<TcpStream> {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let client = TcpStream::connect(l.local_addr()?)?;
        let (serveur, _) = l.accept()?;
        let me = self.clone();
        let abonne = abonne.to_string();
        std::thread::Builder::new().name("tinyblog".into()).spawn(move || me.servir(&abonne, serveur))?;
        Ok(client)
    }

    fn servir(&self, abonne: &str, mut s: TcpStream) {
        // l'image d'un article arrive à la vitesse du modem : patience
        let _ = s.set_read_timeout(Some(Duration::from_secs(240)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(600)));
        let r = match lire_requete(&mut s) {
            Ok(q) => {
                let r = self.traiter(abonne, &q);
                debug!("tinyblog : {abonne} {} {} → {} ({} o)", q.methode, q.cible, r.statut, r.corps.len());
                r
            }
            Err(e) => Reponse::erreur(400, "Requête illisible", &e),
        };
        let _ = ecrire_reponse(&mut s, &r);
        let _ = s.shutdown(Shutdown::Write);
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = [0u8; 4096];
        while matches!(s.read(&mut buf), Ok(n) if n > 0) {}
    }

    fn succes(&self, abonne: &str, id: &'static str) {
        self.exploits.lock().unwrap_or_else(|e| e.into_inner()).push((abonne.to_string(), id));
    }

    /// Traite une requête (sans réseau : pratique pour les tests).
    pub fn traiter(&self, abonne: &str, q: &Requete) -> Reponse {
        let cible = match q.cible.strip_prefix("http://") {
            Some(rest) => rest.find('/').map_or("/", |i| &rest[i..]).to_string(),
            None => q.cible.clone(),
        };
        let (chemin, requete) = cible.split_once('?').unwrap_or((&cible, ""));
        let param = |n: &str| form_urlencoded::parse(requete.as_bytes()).find(|(k, _)| k == n).map(|(_, v)| v.into_owned());
        let id = param("id").and_then(|v| v.parse::<u32>().ok());
        let parties: Vec<&str> = chemin.trim_matches('/').split('/').filter(|p| !p.is_empty()).collect();
        let post = q.methode == "POST";
        match (q.methode.as_str(), parties.as_slice()) {
            ("GET" | "HEAD", []) => Reponse::html(200, pages::accueil(abonne, &self.store.tous())),
            ("GET" | "HEAD", ["gerer"]) => {
                let b = self.store.lire(abonne).unwrap_or_default();
                Reponse::html(200, pages::gerer(abonne, &b, None))
            }
            ("GET" | "HEAD", ["img", f]) => match self.store.lire_image(f) {
                Some(d) => Reponse { statut: 200, type_: "image/gif", corps: d, location: None },
                None => Reponse::erreur(404, "Image introuvable", "Cette image a disparu."),
            },
            ("GET" | "HEAD", [nom]) => match self.store.lire(nom) {
                Some(b) => {
                    let page = param("page").and_then(|p| p.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                    Reponse::html(200, pages::blog(abonne, nom, &b, page))
                }
                None if *nom == abonne => Reponse::html(200, pages::blog(abonne, nom, &Default::default(), 0)),
                None => Reponse::erreur(404, "Tinyblog introuvable", &format!("Personne n'a encore de Tinyblog au nom de « {nom} ».")),
            },
            ("POST", ["gerer", "reglages"]) => self.reglages(abonne, &q.corps),
            ("POST", ["gerer", "article"]) => self.publier(abonne, q),
            ("POST", ["gerer", "supprimer"]) => {
                let image = self.store.modifier(abonne, |b| {
                    let pos = b.articles.iter().position(|a| Some(a.id) == id)?;
                    b.articles.remove(pos).image
                });
                if let Some(Some(f)) = image {
                    self.store.supprimer_image(&f);
                }
                Reponse::vers("/gerer".into())
            }
            ("POST", [nom, "kiffe"]) => self.kiffer(abonne, nom, id),
            ("POST", [nom, "com"]) => self.commenter(abonne, nom, id, &q.corps),
            ("POST", [nom, "com", "supprimer"]) => {
                let k = param("k").and_then(|v| v.parse::<usize>().ok());
                self.store.modifier(nom, |b| {
                    if let (Some(a), Some(k)) = (b.articles.iter_mut().find(|a| Some(a.id) == id), k) {
                        // l'auteur du blog, ou celui du commentaire
                        if k < a.coms.len() && (abonne == *nom || a.coms[k].de == abonne) {
                            a.coms.remove(k);
                        }
                    }
                });
                Reponse::vers(format!("/{nom}#a{}", id.unwrap_or(0)))
            }
            _ if post => Reponse::erreur(404, "Action inconnue", "Cette action n'existe pas sur Tinyblog."),
            _ => Reponse::erreur(404, "Page introuvable", "Cette page n'existe pas (ou elle est encore en construction)."),
        }
    }

    fn reglages(&self, abonne: &str, corps: &[u8]) -> Reponse {
        let champ = |n: &str| form_urlencoded::parse(corps).find(|(k, _)| k == n).map(|(_, v)| v.into_owned()).unwrap_or_default();
        let theme = champ("theme");
        let ok = self.store.modifier(abonne, |b| {
            b.titre = borne(&champ("titre"), TITRE_MAX);
            b.description = borne(&champ("description"), DESCRIPTION_MAX);
            if pages::THEMES.iter().any(|t| t.id == theme) {
                b.theme = theme.clone();
            }
            if b.cree == 0 {
                b.cree = maintenant();
            }
        });
        if ok.is_none() {
            return Reponse::erreur(400, "Nom d'abonné invalide", "Ce nom ne peut pas avoir de Tinyblog.");
        }
        Reponse::vers("/gerer".into())
    }

    fn publier(&self, abonne: &str, q: &Requete) -> Reponse {
        let Some(champs) = multipart(&q.type_contenu, &q.corps) else {
            return Reponse::erreur(400, "Formulaire illisible", "Le formulaire n'est pas arrivé en entier. La ligne a peut-être coupé.");
        };
        let texte_de = |n: &str| champs.iter().find(|c| c.nom == n).map(|c| String::from_utf8_lossy(&c.donnees).into_owned()).unwrap_or_default();
        let titre = borne(&texte_de("titre"), TITRE_MAX);
        let texte = borne(&texte_de("texte"), TEXTE_MAX);
        if titre.trim().is_empty() && texte.trim().is_empty() {
            let b = self.store.lire(abonne).unwrap_or_default();
            return Reponse::html(400, pages::gerer(abonne, &b, Some("Un article sans titre ni texte ? Même en 1998, c'est un peu court.")));
        }
        // l'image, réduite tout de suite : 320 points de large, ~25 Ko
        let image = champs.iter().find(|c| c.nom == "image" && !c.donnees.is_empty()).map(|c| crate::web1998::images::to_gif(&c.donnees, &crate::web1998::images::GifOptions::new(320, 25 * 1024)));
        if let Some(Err(e)) = &image {
            let b = self.store.lire(abonne).unwrap_or_default();
            return Reponse::html(400, pages::gerer(abonne, &b, Some(&format!("Image illisible ({e}). JPEG, PNG ou GIF, s'il te plaît."))));
        }
        let image = image.and_then(Result::ok);
        let r = self.store.modifier(abonne, |b| {
            if b.articles.len() >= ARTICLES_MAX {
                return None;
            }
            if b.cree == 0 {
                b.cree = maintenant();
            }
            b.suivant = b.suivant.max(b.articles.iter().map(|a| a.id).max().unwrap_or(0)) + 1;
            let id = b.suivant;
            Some((id, b.articles.is_empty()))
        });
        let Some(Some((id, premier))) = r else {
            return Reponse::erreur(400, "Tinyblog plein", "Ton Tinyblog a atteint 300 articles. Fais un peu de ménage !");
        };
        let fichier = image.and_then(|gif| {
            let f = format!("{abonne}-{id}.gif");
            self.store.enregistrer_image(&f, &gif).then_some(f)
        });
        let titre = if titre.trim().is_empty() { "Sans titre".to_string() } else { titre };
        self.store.modifier(abonne, |b| b.articles.push(Article { id, date: maintenant(), titre, texte, image: fichier, kiffs: Vec::new(), coms: Vec::new() }));
        if premier {
            self.succes(abonne, "blogueur");
        }
        Reponse::vers(format!("/{abonne}#a{id}"))
    }

    fn kiffer(&self, abonne: &str, nom: &str, id: Option<u32>) -> Reponse {
        let total = self.store.modifier(nom, |b| {
            if let Some(a) = b.articles.iter_mut().find(|a| Some(a.id) == id) {
                if let Some(pos) = a.kiffs.iter().position(|k| k == abonne) {
                    a.kiffs.remove(pos);
                } else {
                    a.kiffs.push(abonne.to_string());
                }
            }
            b.kiffs()
        });
        if total.is_some_and(|t| t >= 10) {
            self.succes(nom, "star");
        }
        Reponse::vers(format!("/{nom}#a{}", id.unwrap_or(0)))
    }

    fn commenter(&self, abonne: &str, nom: &str, id: Option<u32>, corps: &[u8]) -> Reponse {
        let texte = form_urlencoded::parse(corps).find(|(k, _)| k == "texte").map(|(_, v)| borne(&v, COM_MAX)).unwrap_or_default();
        let retour = Reponse::vers(format!("/{nom}#a{}", id.unwrap_or(0)));
        if texte.trim().is_empty() {
            return retour;
        }
        {
            let mut d = self.derniers_coms.lock().unwrap_or_else(|e| e.into_inner());
            if d.get(abonne).is_some_and(|t| t.elapsed() < COM_INTERVALLE) {
                return Reponse::erreur(429, "Doucement !", "Un com's toutes les cinq secondes, pas plus : le modem a besoin de souffler.");
            }
            d.insert(abonne.to_string(), Instant::now());
        }
        let ajoute = self.store.modifier(nom, |b| match b.articles.iter_mut().find(|a| Some(a.id) == id) {
            Some(a) if a.coms.len() < COMS_PAR_ARTICLE => {
                a.coms.push(Com { de: abonne.to_string(), date: maintenant(), texte: texte.trim().to_string() });
                true
            }
            _ => false,
        });
        if ajoute == Some(true) && self.store.compter_com(abonne) >= 5 {
            self.succes(abonne, "coms");
        }
        retour
    }
}

/// Coupe à `n` caractères, sans caractères de contrôle (sauf les retours à
/// la ligne).
fn borne(s: &str, n: usize) -> String {
    s.chars().filter(|c| !c.is_control() || *c == '\n').take(n).collect()
}

// ───────────────────────── HTTP ─────────────────────────

fn lire_requete(s: &mut TcpStream) -> Result<Requete, String> {
    let mut buf = Vec::with_capacity(4096);
    let mut morceau = [0u8; 8192];
    let fin_entetes = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 32 * 1024 {
            return Err("en-têtes trop longs".into());
        }
        let n = s.read(&mut morceau).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connexion fermée".into());
        }
        buf.extend_from_slice(&morceau[..n]);
    };
    let entetes = String::from_utf8_lossy(&buf[..fin_entetes]).into_owned();
    let mut lignes = entetes.lines();
    let premiere = lignes.next().unwrap_or("");
    let mut mots = premiere.split_whitespace();
    let methode = mots.next().unwrap_or("").to_string();
    let cible = mots.next().unwrap_or("/").to_string();
    let mut longueur = 0usize;
    let mut type_contenu = String::new();
    for l in lignes {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => longueur = v.trim().parse().unwrap_or(0),
                "content-type" => type_contenu = v.trim().to_string(),
                _ => {}
            }
        }
    }
    if longueur > CORPS_MAX {
        return Err(format!("envoi trop gros ({} Ko au plus)", CORPS_MAX / 1024));
    }
    let mut corps = buf[fin_entetes..].to_vec();
    while corps.len() < longueur {
        let n = s.read(&mut morceau).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("envoi interrompu".into());
        }
        corps.extend_from_slice(&morceau[..n]);
    }
    corps.truncate(longueur);
    Ok(Requete { methode, cible, type_contenu, corps })
}

fn ecrire_reponse(s: &mut TcpStream, r: &Reponse) -> io::Result<()> {
    let raison = match r.statut {
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        404 => "Not Found",
        429 => "Too Many Requests",
        _ => "Erreur",
    };
    let mut h = format!(
        "HTTP/1.0 {} {raison}\r\nServer: Tinyblog/1998\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n",
        r.statut,
        r.type_,
        r.corps.len()
    );
    if let Some(l) = &r.location {
        h.push_str(&format!("Location: {l}\r\n"));
    }
    h.push_str("\r\n");
    s.write_all(h.as_bytes())?;
    s.write_all(&r.corps)
}

/// Un champ d'un formulaire multipart.
pub struct Champ {
    pub nom: String,
    pub donnees: Vec<u8>,
}

/// Formulaire multipart/form-data : les champs (nom, contenu).
pub fn multipart(type_contenu: &str, corps: &[u8]) -> Option<Vec<Champ>> {
    let frontiere = type_contenu.split(';').find_map(|p| p.trim().strip_prefix("boundary="))?.trim_matches('"');
    let delim = format!("--{frontiere}").into_bytes();
    let mut champs = Vec::new();
    let mut pos = trouver(corps, &delim, 0)? + delim.len();
    loop {
        if corps.get(pos..pos + 2) == Some(b"--") {
            break; // fin
        }
        let debut_entetes = pos + 2; // \r\n
        let fin_entetes = trouver(corps, b"\r\n\r\n", debut_entetes)?;
        let entetes = String::from_utf8_lossy(corps.get(debut_entetes..fin_entetes)?);
        let suivant = trouver(corps, &delim, fin_entetes + 4)?;
        let donnees = corps.get(fin_entetes + 4..suivant.saturating_sub(2))?.to_vec();
        let nom = entetes.split(';').find_map(|p| p.trim().strip_prefix("name=")).unwrap_or("").trim_matches('"').to_string();
        champs.push(Champ { nom, donnees });
        pos = suivant + delim.len();
    }
    Some(champs)
}

fn trouver(h: &[u8], aiguille: &[u8], depuis: usize) -> Option<usize> {
    h.get(depuis..)?.windows(aiguille.len()).position(|w| w == aiguille).map(|p| p + depuis)
}
