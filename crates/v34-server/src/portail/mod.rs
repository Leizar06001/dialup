//! Site d'inscription façon fournisseur d'accès de 1998 (binaire
//! `v34-portail`, prévu pour tourner dans un conteneur derrière Caddy).
//!
//! - inscription avec choix du numéro, validation de l'adresse e-mail ;
//! - le compte est créé SANS accès Internet : l'administrateur reçoit un
//!   e-mail avec deux liens signés [Accorder] / [Refuser], et dispose d'une
//!   page /admin (connexion avec son mot de passe d'abonné) ;
//! - page de téléchargement d'après la dernière version publiée sur GitHub.
//!
//! Les comptes sont écrits dans le fichier partagé avec le central
//! ([`crate::comptes`]) ; la configuration du central est lue (lecture
//! seule) pour connaître les noms et numéros déjà pris et vérifier le mot de
//! passe de l'administrateur.
//!
//! Les liens d'e-mail mènent à une page de confirmation avec un bouton
//! (POST) : un antivirus ou un aperçu de messagerie qui « visite » le lien
//! ne déclenche rien.

pub mod courriel;
pub mod pages;

use std::collections::HashMap;
use url::form_urlencoded;
use std::io::Read;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use v34_net::auth::{to_hex, valid_name, PasswordHash, DEFAULT_ITERATIONS};
use v34_net::crypto::{ct_eq, hmac_sha256, random_bytes};

use crate::comptes::{maintenant, Comptes};
use crate::config::Config;
use crate::{debug, info, warn};

use courriel::{Lettre, Postier, SmtpConfig};
use pages::{esc, numero_lisible};

/// Corps de requête accepté au maximum (formulaires)
const CORPS_MAX: u64 = 32 * 1024;
/// Durée d'une session d'administration (s)
const SESSION_S: u64 = 12 * 3600;
/// Validité des liens [Accorder] / [Refuser] (s)
const DECISION_S: u64 = 30 * 24 * 3600;
/// E-mails envoyés au maximum par heure (tout le site)
const COURRIELS_PAR_HEURE: usize = 60;
const NOMS_RESERVES: &[&str] = &["admin", "administrateur", "root", "fai", "salon", "web1998", "operateur", "opérateur", "support", "postmaster", "abuse"];

/// Réglages du site (variables d'environnement, voir `deploy/portail/.env.example`).
#[derive(Clone, Debug)]
pub struct Reglages {
    pub ecoute: String,
    /// Adresse publique, sans « / » final (liens des e-mails)
    pub url: String,
    pub nom: String,
    /// Adresse du central à saisir dans l'appli
    pub central: String,
    /// Abonné administrateur (déclaré dans la configuration du central)
    pub admin: String,
    /// Destinataire des demandes d'accès Internet
    pub admin_courriel: String,
    /// Dépôt GitHub « propriétaire/dépôt » des versions de l'appli
    pub depot: String,
    pub inscriptions_par_heure: usize,
    /// Configuration du central (lecture seule)
    pub config: PathBuf,
    /// Fichier des comptes (partagé avec le central)
    pub comptes: PathBuf,
    /// Croire l'en-tête X-Forwarded-For (derrière Caddy)
    pub proxy_de_confiance: bool,
}

impl Reglages {
    pub fn depuis_env(get: impl Fn(&str) -> Option<String>) -> Result<Reglages, String> {
        let v = |k: &str, defaut: &str| get(k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| defaut.to_string());
        let url = v("PORTAIL_URL", "");
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.ends_with('/') {
            return Err("PORTAIL_URL : adresse publique du site, par ex. https://dialup.example.org (sans / final)".into());
        }
        let hote = url.split("://").nth(1).unwrap_or("").split(['/', ':']).next().unwrap_or("").to_string();
        let r = Reglages {
            ecoute: v("PORTAIL_ECOUTE", "0.0.0.0:8034"),
            nom: v("PORTAIL_NOM", "Tinyverse Dialup"),
            central: v("PORTAIL_CENTRAL", &format!("{hote}:3434")),
            admin: v("PORTAIL_ADMIN", ""),
            admin_courriel: v("PORTAIL_ADMIN_COURRIEL", ""),
            depot: v("PORTAIL_DEPOT", ""),
            inscriptions_par_heure: v("PORTAIL_INSCRIPTIONS_PAR_HEURE", "3").parse().map_err(|_| "PORTAIL_INSCRIPTIONS_PAR_HEURE invalide")?,
            config: PathBuf::from(v("V34_CONFIG", crate::config::DEFAULT_PATH)),
            comptes: PathBuf::from(v("V34_COMPTES", "/var/lib/v34-server/comptes.toml")),
            proxy_de_confiance: !matches!(v("PORTAIL_PROXY_DE_CONFIANCE", "oui").as_str(), "non" | "0" | "false"),
            url,
        };
        r.ecoute.parse::<SocketAddr>().map_err(|_| format!("PORTAIL_ECOUTE invalide : {}", r.ecoute))?;
        Ok(r)
    }
}

/// Requête HTTP déjà lue (indépendante du serveur HTTP : testable).
#[derive(Clone, Debug, Default)]
pub struct Requete {
    pub methode: String,
    pub chemin: String,
    pub requete: String,
    pub cookie: Option<String>,
    pub corps: Vec<u8>,
    pub ip: String,
}

impl Requete {
    fn param(&self, nom: &str) -> String {
        form_urlencoded::parse(self.requete.as_bytes()).find(|(k, _)| k == nom).map(|(_, v)| v.into_owned()).unwrap_or_default()
    }
    fn champ(&self, nom: &str) -> String {
        form_urlencoded::parse(&self.corps).find(|(k, _)| k == nom).map(|(_, v)| v.into_owned()).unwrap_or_default()
    }
    fn cookie(&self, nom: &str) -> Option<String> {
        self.cookie.as_deref()?.split(';').filter_map(|c| c.trim().split_once('=')).find(|(k, _)| *k == nom).map(|(_, v)| v.to_string())
    }
}

#[derive(Debug)]
pub struct Reponse {
    pub statut: u16,
    pub type_contenu: &'static str,
    pub corps: Vec<u8>,
    pub entetes: Vec<(&'static str, String)>,
}

impl Reponse {
    fn redirection(vers: &str) -> Reponse {
        Reponse { statut: 303, type_contenu: "text/plain; charset=utf-8", corps: Vec::new(), entetes: vec![("Location", vers.to_string())] }
    }
    fn texte(statut: u16, t: &str) -> Reponse {
        Reponse { statut, type_contenu: "text/plain; charset=utf-8", corps: t.as_bytes().to_vec(), entetes: Vec::new() }
    }
}

struct CacheConfig {
    modifie: Option<SystemTime>,
    verifie: Instant,
    cfg: Option<Arc<Config>>,
}

struct Version {
    tag: String,
    page: String,
    fichiers: Vec<pages::Fichier>,
}

pub struct Portail {
    r: Reglages,
    comptes: Arc<Comptes>,
    postier: Postier,
    config: Mutex<CacheConfig>,
    /// Horodatages des actions limitées, par clé (« inscription:IP »…)
    limites: Mutex<HashMap<String, Vec<u64>>>,
    /// Sessions d'administration : empreinte du jeton → expiration
    sessions: Mutex<HashMap<String, u64>>,
    versions: Mutex<Option<(Instant, Option<Arc<Version>>)>>,
    visites: AtomicU64,
}

fn empreinte(s: &str) -> String {
    to_hex(&hmac_sha256(b"v34-portail", &[s.as_bytes()]))
}

impl Portail {
    pub fn new(r: Reglages, comptes: Arc<Comptes>, smtp: Option<SmtpConfig>) -> Arc<Portail> {
        Arc::new(Portail {
            r,
            comptes,
            postier: Postier::new(smtp),
            config: Mutex::new(CacheConfig { modifie: None, verifie: Instant::now() - Duration::from_secs(60), cfg: None }),
            limites: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            versions: Mutex::new(None),
            visites: AtomicU64::new(0),
        })
    }

    pub fn reglages(&self) -> &Reglages {
        &self.r
    }

    /// Configuration du central (relue quand le fichier change).
    fn config(&self) -> Option<Arc<Config>> {
        let mut c = self.config.lock().unwrap();
        if c.verifie.elapsed() < Duration::from_secs(2) && c.cfg.is_some() {
            return c.cfg.clone();
        }
        c.verifie = Instant::now();
        let m = std::fs::metadata(&self.r.config).and_then(|m| m.modified()).ok();
        if m != c.modifie || c.cfg.is_none() {
            match Config::load(&self.r.config) {
                Ok(cfg) => {
                    c.cfg = Some(Arc::new(cfg));
                    c.modifie = m;
                }
                Err(e) => warn!("configuration du central illisible : {e}"),
            }
        }
        c.cfg.clone()
    }

    /// Les inscriptions sont-elles possibles (e-mails et configuration) ?
    fn inscriptions_ouvertes(&self) -> bool {
        self.postier.actif() && self.config().is_some()
    }

    /// Action limitée : au plus `max` par `fenetre_s` pour cette clé.
    fn autoriser(&self, cle: &str, max: usize, fenetre_s: u64) -> bool {
        let now = maintenant();
        let mut l = self.limites.lock().unwrap();
        if l.len() > 10_000 {
            l.retain(|_, v| v.iter().any(|&t| t + 86_400 > now));
        }
        let v = l.entry(cle.to_string()).or_default();
        v.retain(|&t| t + fenetre_s > now);
        if v.len() >= max {
            return false;
        }
        v.push(now);
        true
    }

    fn signer(&self, parts: &[&str]) -> String {
        let p: Vec<&[u8]> = parts.iter().flat_map(|s| [s.as_bytes(), b"\0"]).collect();
        to_hex(&hmac_sha256(self.comptes.secret(), &p))
    }

    fn page(&self, statut: u16, titre: &str, contenu: &str) -> Reponse {
        let n = self.visites.load(Ordering::Relaxed);
        Reponse { statut, type_contenu: "text/html; charset=utf-8", corps: pages::gabarit(&self.r.nom, titre, n, contenu).into_bytes(), entetes: Vec::new() }
    }

    fn message(&self, statut: u16, titre: &str, texte: &str, ok: bool) -> Reponse {
        self.page(statut, titre, &pages::message(titre, texte, ok))
    }

    /// Traite une requête.
    pub fn repondre(&self, q: &Requete) -> Reponse {
        match (q.methode.as_str(), q.chemin.as_str()) {
            ("GET" | "HEAD", "/") => {
                self.visites.fetch_add(1, Ordering::Relaxed);
                self.page(200, "Accueil", &pages::accueil(&self.r.nom, &self.r.central, self.inscriptions_ouvertes()))
            }
            ("GET" | "HEAD", "/inscription") => self.formulaire(&pages::Formulaire::default(), &[]),
            ("POST", "/inscription") => self.inscrire(q),
            ("GET" | "HEAD", "/valider") => match self.comptes.inscription(&q.param("t")) {
                Some(i) => self.page(200, "Activation", &pages::confirmer_inscription(&q.param("t"), &i.nom, &i.numero)),
                None => self.message(404, "Lien invalide", "Ce lien d'activation est invalide, déjà utilisé ou expiré (48 heures). Recommencez l'inscription.", false),
            },
            ("POST", "/valider") => self.valider(q),
            ("GET" | "HEAD", "/decision") => self.decision(q, false),
            ("POST", "/decision") => self.decision(q, true),
            ("GET" | "HEAD", "/telechargement") => self.telechargement(),
            ("GET" | "HEAD", "/aide") => self.page(200, "Aide", &pages::aide(&self.r.central, &self.r.depot)),
            ("GET" | "HEAD", "/admin") => self.admin(q, None),
            ("POST", "/admin/connexion") => self.admin_connexion(q),
            ("POST", "/admin/fai") | ("POST", "/admin/supprimer") | ("POST", "/admin/deconnexion") => self.admin_action(q),
            ("GET" | "HEAD", "/construction.gif") => gif(crate::web1998::images::construction_gif()),
            ("GET" | "HEAD", "/compteur.gif") => {
                let n: String = q.param("n").chars().filter(char::is_ascii_digit).take(9).collect();
                gif(crate::web1998::images::counter_gif(&n))
            }
            ("GET" | "HEAD", "/robots.txt") => Reponse::texte(200, "User-agent: *\nDisallow: /valider\nDisallow: /decision\nDisallow: /admin\n"),
            ("GET" | "HEAD", "/favicon.ico") => Reponse::texte(404, ""),
            (_, "/" | "/inscription" | "/valider" | "/decision" | "/telechargement" | "/aide" | "/admin") => Reponse::texte(405, "méthode non autorisée"),
            _ => self.message(404, "Fichier introuvable", "Cette page n'existe pas (ou elle est encore en construction).", false),
        }
    }

    // ───────────────────────── inscription ─────────────────────────

    /// Numéros libres au hasard (10 chiffres, 01 à 07 ou 09).
    fn suggestions(&self, cfg: Option<&Config>, n: usize) -> Vec<String> {
        let mut v = Vec::new();
        let annuaire = cfg.and_then(|c| c.annuaire().ok());
        for _ in 0..200 {
            if v.len() >= n {
                break;
            }
            let b: [u8; 9] = random_bytes();
            let pref = [b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'9'][(b[0] % 8) as usize] as char;
            let num: String = std::iter::once('0').chain(std::iter::once(pref)).chain(b[1..].iter().map(|x| (b'0' + x % 10) as char)).collect();
            let pris = annuaire.as_ref().is_some_and(|a| a.numero_pris(&num)) || cfg.is_some_and(|c| c.service_for(&num).is_some()) || self.comptes.conflit("", &num, "").is_some();
            if !pris && !v.contains(&num) {
                v.push(num);
            }
        }
        v
    }

    fn formulaire(&self, f: &pages::Formulaire, erreurs: &[String]) -> Reponse {
        if !self.inscriptions_ouvertes() {
            return self.message(503, "Inscriptions fermées", "Les inscriptions sont momentanément fermées. Revenez plus tard !", false);
        }
        let cfg = self.config();
        let ts = maintenant().to_string();
        let jeton = format!("{ts}.{}", &self.signer(&["formulaire", &ts])[..20]);
        self.page(if erreurs.is_empty() { 200 } else { 400 }, "Inscription", &pages::inscription(f, erreurs, &self.suggestions(cfg.as_deref(), 4), &jeton))
    }

    fn inscrire(&self, q: &Requete) -> Reponse {
        let Some(cfg) = self.config().filter(|_| self.postier.actif()) else {
            return self.message(503, "Inscriptions fermées", "Les inscriptions sont momentanément fermées. Revenez plus tard !", false);
        };
        let numero: String = q.champ("numero").chars().filter(|c| !matches!(c, ' ' | '.' | '-')).collect();
        let f = pages::Formulaire { nom: q.champ("nom").trim().to_string(), courriel: q.champ("courriel").trim().to_string(), numero: numero.clone() };
        // Robots : champ piège rempli, ou formulaire envoyé trop vite / trop vieux.
        let t = q.champ("t");
        let age = t.split_once('.').filter(|(ts, sig)| ct_eq(sig.as_bytes(), self.signer(&["formulaire", ts])[..20].as_bytes())).and_then(|(ts, _)| ts.parse::<u64>().ok()).map(|ts| maintenant().saturating_sub(ts));
        if !q.champ("site").is_empty() || age.is_none_or(|a| a < 3) {
            debug!("inscription refusée (robot ?) depuis {}", q.ip);
            return self.formulaire(&f, &["Formulaire invalide : rechargez la page et réessayez.".into()]);
        }
        if age.is_some_and(|a| a > 3 * 3600) {
            return self.formulaire(&f, &["Le formulaire a expiré : réessayez.".into()]);
        }
        let mut erreurs = Vec::new();
        let annuaire = cfg.annuaire().ok();
        if !valid_name(&f.nom) {
            erreurs.push("Nom d'abonné : 1 à 32 caractères parmi lettres sans accent, chiffres, « . », « _ », « - ».".into());
        } else if NOMS_RESERVES.iter().any(|r| r.eq_ignore_ascii_case(&f.nom)) || annuaire.as_ref().is_some_and(|a| a.nom_pris(&f.nom)) {
            erreurs.push(format!("Le nom « {} » est déjà pris.", f.nom));
        }
        let b = numero.as_bytes();
        if numero.len() != 10 || !numero.bytes().all(|c| c.is_ascii_digit()) || b[0] != b'0' || !matches!(b[1], b'1'..=b'7' | b'9') {
            erreurs.push("Numéro : 10 chiffres commençant par 01 à 07 ou 09.".into());
        } else if cfg.service_for(&numero).is_some() || annuaire.as_ref().is_some_and(|a| a.numero_pris(&numero)) {
            erreurs.push(format!("Le numéro {} est déjà attribué : choisissez-en un autre.", numero_lisible(&numero)));
        }
        if !courriel_valide(&f.courriel) {
            erreurs.push("Adresse électronique invalide.".into());
        }
        let mdp = q.champ("mdp");
        if mdp.chars().count() < 8 || mdp.chars().count() > 128 {
            erreurs.push("Mot de passe : 8 à 128 caractères.".into());
        } else if mdp != q.champ("mdp2") {
            erreurs.push("Les deux mots de passe sont différents.".into());
        }
        if q.champ("cgu") != "1" {
            erreurs.push("Vous devez accepter les conditions générales (et laisser la ligne à maman).".into());
        }
        if erreurs.is_empty() {
            match self.comptes.conflit(&f.nom, &numero, &f.courriel) {
                Some("nom") => erreurs.push(format!("Le nom « {} » est déjà pris.", f.nom)),
                Some("numero") => erreurs.push(format!("Le numéro {} est déjà attribué : choisissez-en un autre.", numero_lisible(&numero))),
                Some(_) => erreurs.push("Cette adresse électronique a déjà un abonnement (ou une inscription en attente).".into()),
                None => {}
            }
        }
        if !erreurs.is_empty() {
            return self.formulaire(&f, &erreurs);
        }
        if !self.autoriser(&format!("inscription:{}", q.ip), self.r.inscriptions_par_heure, 3600) {
            return self.formulaire(&f, &["Trop d'inscriptions depuis votre adresse : réessayez dans une heure.".into()]);
        }
        if !self.autoriser("courriels", COURRIELS_PAR_HEURE, 3600) {
            return self.message(503, "Bureau de poste saturé", "Trop d'e-mails envoyés cette heure-ci : réessayez plus tard.", false);
        }
        let hash = PasswordHash::new(&mdp, DEFAULT_ITERATIONS).to_string();
        let jeton = match self.comptes.inscrire(&f.nom, &numero, &hash, &f.courriel) {
            Ok(j) => j,
            Err(e) => return self.message(503, "Inscription impossible", &e, false),
        };
        let lien = format!("{}/valider?t={jeton}", self.r.url);
        self.postier.poster(Lettre {
            a: f.courriel.clone(),
            sujet: format!("{} : activez votre abonnement", self.r.nom),
            corps: format!(
                "Bonjour {nom},\n\nPour activer votre abonnement {fai} et obtenir le numéro {num}, ouvrez ce lien dans les 48 heures :\n\n{lien}\n\nSi vous n'êtes pas à l'origine de cette demande, ignorez simplement ce message : rien ne sera créé.\n\n-- \n{fai}\n{url}\n",
                nom = f.nom,
                fai = self.r.nom,
                num = numero_lisible(&numero),
                url = self.r.url
            ),
        });
        info!("inscription en attente : {} ({}) depuis {}", f.nom, numero, q.ip);
        self.page(200, "Inscription envoyée", &pages::inscription_envoyee(&f.courriel, &numero))
    }

    fn valider(&self, q: &Requete) -> Reponse {
        let cfg = self.config();
        let libre = |nom: &str, num: &str| {
            cfg.as_ref().is_some_and(|c| c.service_for(num).is_none() && c.annuaire().is_ok_and(|a| !a.nom_pris(nom) && !a.numero_pris(num)))
        };
        let c = match self.comptes.valider(&q.champ("t"), libre) {
            Ok(c) => c,
            Err(e) => return self.message(400, "Activation impossible", &e, false),
        };
        info!("abonnement activé : {} ({}), accès Internet à décider", c.nom, c.numero);
        if !self.r.admin_courriel.is_empty() {
            let exp = (maintenant() + DECISION_S).to_string();
            let lien = |d: &str| {
                let sig = self.signer(&["decision", &c.nom, d, &exp]);
                format!("{}/decision?nom={}&d={d}&exp={exp}&sig={sig}", self.r.url, enc(&c.nom))
            };
            self.postier.poster(Lettre {
                a: self.r.admin_courriel.clone(),
                sujet: format!("{} : {} demande l'accès Internet", self.r.nom, c.nom),
                corps: format!(
                    "Nouvel abonné : {nom} <{mail}>, numéro {num}.\n\nAccès Internet (FAI) :\n\n  Accorder : {oui}\n\n  Refuser  : {non}\n\n(liens valables 30 jours ; ou depuis la page d'administration : {url}/admin)\n",
                    nom = c.nom,
                    mail = c.courriel,
                    num = numero_lisible(&c.numero),
                    oui = lien("oui"),
                    non = lien("non"),
                    url = self.r.url
                ),
            });
        }
        self.page(200, "Bienvenue", &pages::bienvenue(&c.nom, &c.numero, &self.r.central, true))
    }

    // ───────────────────────── décision de l'administrateur ─────────────────────────

    fn decision(&self, q: &Requete, appliquer: bool) -> Reponse {
        let get = |k: &str| if appliquer { q.champ(k) } else { q.param(k) };
        let (nom, d, exp, sig) = (get("nom"), get("d"), get("exp"), get("sig"));
        let valide = matches!(d.as_str(), "oui" | "non")
            && exp.parse::<u64>().is_ok_and(|e| e > maintenant())
            && ct_eq(sig.as_bytes(), self.signer(&["decision", &nom, &d, &exp]).as_bytes());
        if !valide {
            return self.message(403, "Lien invalide", "Ce lien est invalide ou expiré. Passez par la page d'administration.", false);
        }
        let Some(c) = self.comptes.compte(&nom) else {
            return self.message(404, "Abonné inconnu", "Ce compte n'existe plus.", false);
        };
        let oui = d == "oui";
        if !appliquer {
            let champs = format!(
                r#"<input type="hidden" name="nom" value="{}"><input type="hidden" name="d" value="{}"><input type="hidden" name="exp" value="{}"><input type="hidden" name="sig" value="{}">"#,
                esc(&nom),
                esc(&d),
                esc(&exp),
                esc(&sig)
            );
            return self.page(200, "Accès Internet", &pages::confirmer_decision(&c.nom, &c.numero, &c.courriel, oui, &champs));
        }
        self.regler_acces(&c.nom, oui)
    }

    fn regler_acces(&self, nom: &str, oui: bool) -> Reponse {
        let avant = self.comptes.compte(nom);
        let c = match self.comptes.regler_fai(nom, oui) {
            Ok(c) => c,
            Err(e) => return self.message(400, "Impossible", &e, false),
        };
        info!("accès Internet de {} : {}", c.nom, if oui { "accordé" } else { "refusé" });
        // L'abonné n'est prévenu que si quelque chose change pour lui.
        if avant.is_some_and(|a| a.fai != oui || !a.fai_decide) {
            let corps = if oui {
                format!("Bonjour {},\n\nBonne nouvelle : votre accès Internet est activé ! Composez le 08 36 65 65 65 dans le logiciel (bouton FAI), puis ouvrez le NAVIGATEUR.\n\nBon surf à 28 800 bit/s !\n\n-- \n{}\n{}\n", c.nom, self.r.nom, self.r.url)
            } else {
                format!("Bonjour {},\n\nVotre accès Internet n'est pas activé pour le moment. Vous pouvez toujours appeler les autres abonnés et la ligne de test.\n\n-- \n{}\n{}\n", c.nom, self.r.nom, self.r.url)
            };
            self.postier.poster(Lettre { a: c.courriel.clone(), sujet: format!("{} : accès Internet {}", self.r.nom, if oui { "activé" } else { "non activé" }), corps });
        }
        let t = if oui { format!("Accès Internet accordé à {} (pris en compte par le central sous 2 secondes).", c.nom) } else { format!("Accès Internet refusé à {}.", c.nom) };
        self.message(200, "C'est fait", &t, true)
    }

    // ───────────────────────── administration ─────────────────────────

    fn session(&self, q: &Requete) -> Option<String> {
        let jeton = q.cookie("v34adm")?;
        let now = maintenant();
        let mut s = self.sessions.lock().unwrap();
        s.retain(|_, &mut exp| exp > now);
        s.contains_key(&empreinte(&jeton)).then_some(jeton)
    }

    fn csrf(&self, jeton: &str) -> String {
        self.signer(&["csrf", jeton])[..32].to_string()
    }

    fn admin(&self, q: &Requete, info: Option<&str>) -> Reponse {
        if self.r.admin.is_empty() {
            return self.message(404, "Administration désactivée", "Aucun administrateur n'est configuré (PORTAIL_ADMIN).", false);
        }
        let Some(jeton) = self.session(q) else {
            return self.page(200, "Administration", &pages::admin_connexion(&self.r.admin, None));
        };
        let jour = |t: u64| {
            let j = t / 86_400;
            // date civile (algorithme de Howard Hinnant)
            let z = j as i64 + 719_468;
            let era = z.div_euclid(146_097);
            let doe = z - era * 146_097;
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let d = doy - (153 * mp + 2) / 5 + 1;
            let m = if mp < 10 { mp + 3 } else { mp - 9 };
            format!("{d:02}/{m:02}/{}", yoe + era * 400 + i64::from(m <= 2))
        };
        let mut comptes: Vec<pages::LigneAdmin> = self
            .comptes
            .liste()
            .into_iter()
            .map(|c| pages::LigneAdmin { nom: c.nom, numero: c.numero, courriel: c.courriel, cree: jour(c.cree), fai: c.fai, decide: c.fai_decide })
            .collect();
        comptes.sort_by(|a, b| (a.decide, &a.nom).cmp(&(b.decide, &b.nom)));
        let attente: Vec<(String, String, String)> = self.comptes.en_attente().into_iter().map(|i| (i.nom, i.numero, i.courriel)).collect();
        let config: Vec<(String, String)> = self.config().map(|c| c.abonnes.iter().map(|a| (a.nom.clone(), a.numero.clone())).collect()).unwrap_or_default();
        let mut r = self.page(200, "Administration", &pages::admin_tableau(&self.csrf(&jeton), &comptes, &attente, &config, info));
        r.entetes.push(("Cache-Control", "no-store".into()));
        r
    }

    fn admin_connexion(&self, q: &Requete) -> Reponse {
        if self.r.admin.is_empty() {
            return Reponse::texte(404, "");
        }
        if !self.autoriser(&format!("connexion:{}", q.ip), 5, 900) {
            return self.page(429, "Administration", &pages::admin_connexion(&self.r.admin, Some("Trop d'essais : attendez un quart d'heure.")));
        }
        let mdp = q.champ("mdp");
        let ok = self.config().and_then(|c| c.annuaire().ok()).and_then(|a| a.get(&self.r.admin).map(|(_, h)| h.clone())).is_some_and(|h| {
            ct_eq(&PasswordHash::with_salt(&mdp, &h.salt, h.iterations).stored_key, &h.stored_key)
        });
        if !ok {
            warn!("connexion admin refusée depuis {}", q.ip);
            return self.page(403, "Administration", &pages::admin_connexion(&self.r.admin, Some("Mot de passe incorrect.")));
        }
        let jeton = to_hex(&random_bytes::<32>());
        self.sessions.lock().unwrap().insert(empreinte(&jeton), maintenant() + SESSION_S);
        info!("connexion admin depuis {}", q.ip);
        let secure = if self.r.url.starts_with("https://") { "; Secure" } else { "" };
        let mut r = Reponse::redirection("/admin");
        r.entetes.push(("Set-Cookie", format!("v34adm={jeton}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_S}{secure}")));
        r
    }

    fn admin_action(&self, q: &Requete) -> Reponse {
        let Some(jeton) = self.session(q) else { return Reponse::redirection("/admin") };
        if !ct_eq(q.champ("csrf").as_bytes(), self.csrf(&jeton).as_bytes()) {
            return self.message(403, "Refusé", "Jeton de formulaire invalide : rechargez la page d'administration.", false);
        }
        let nom = q.champ("nom");
        match q.chemin.as_str() {
            "/admin/deconnexion" => {
                self.sessions.lock().unwrap().remove(&empreinte(&jeton));
                let mut r = Reponse::redirection("/");
                r.entetes.push(("Set-Cookie", "v34adm=; Path=/; Max-Age=0".into()));
                r
            }
            "/admin/fai" => {
                let r = self.regler_acces(&nom, q.champ("d") == "oui");
                if r.statut == 200 {
                    self.admin(q, Some(&format!("Accès Internet de {nom} : {}.", if q.champ("d") == "oui" { "accordé" } else { "retiré" })))
                } else {
                    r
                }
            }
            _ => match self.comptes.supprimer(&nom) {
                Ok(c) => {
                    info!("compte supprimé par l'administrateur : {} ({})", c.nom, c.numero);
                    self.admin(q, Some(&format!("Compte {} supprimé ; le numéro {} est libéré.", c.nom, numero_lisible(&c.numero))))
                }
                Err(e) => self.message(400, "Impossible", &e, false),
            },
        }
    }

    // ───────────────────────── téléchargement ─────────────────────────

    fn telechargement(&self) -> Reponse {
        let v = self.version();
        let (version, fichiers) = match &v {
            Some(v) => (Some((v.tag.as_str(), v.page.as_str())), &v.fichiers[..]),
            None => (None, &[][..]),
        };
        self.page(200, "Logiciel", &pages::telechargement(version, fichiers, &self.r.depot))
    }

    /// Dernière version publiée (API GitHub, en cache 10 min ; 2 min après un échec).
    fn version(&self) -> Option<Arc<Version>> {
        if self.r.depot.is_empty() {
            return None;
        }
        {
            let c = self.versions.lock().unwrap();
            if let Some((t, v)) = c.as_ref() {
                let ttl = if v.is_some() { 600 } else { 120 };
                if t.elapsed() < Duration::from_secs(ttl) {
                    return v.clone();
                }
            }
        }
        let v = match recuperer_version(&self.r.depot) {
            Ok(v) => Some(Arc::new(v)),
            Err(e) => {
                warn!("versions de {} : {e}", self.r.depot);
                None
            }
        };
        *self.versions.lock().unwrap() = Some((Instant::now(), v.clone()));
        v
    }

    // ───────────────────────── serveur HTTP ─────────────────────────

    /// Sert le site (bloquant). Quatre fils traitent les requêtes.
    pub fn servir(self: Arc<Self>) -> std::io::Result<()> {
        let server = Arc::new(tiny_http::Server::http(&self.r.ecoute).map_err(|e| std::io::Error::other(e.to_string()))?);
        info!("site {} à l'écoute sur http://{} ({})", self.r.nom, self.r.ecoute, self.r.url);
        let mut fils = Vec::new();
        for i in 0..4 {
            let (s, p) = (server.clone(), self.clone());
            fils.push(std::thread::Builder::new().name(format!("http-{i}")).spawn(move || {
                while let Ok(rq) = s.recv() {
                    p.traiter(rq);
                }
            })?);
        }
        for f in fils {
            let _ = f.join();
        }
        Ok(())
    }

    fn traiter(&self, mut rq: tiny_http::Request) {
        let entete = |nom: &str| rq.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(nom)).map(|h| h.value.as_str().to_string());
        let ip_directe = rq.remote_addr().map(|a| a.ip().to_string()).unwrap_or_default();
        let ip = if self.r.proxy_de_confiance { entete("X-Forwarded-For").and_then(|v| v.split(',').next().map(|s| s.trim().to_string())).unwrap_or(ip_directe) } else { ip_directe };
        let (chemin, requete) = match rq.url().split_once('?') {
            Some((c, r)) => (c.to_string(), r.to_string()),
            None => (rq.url().to_string(), String::new()),
        };
        let mut q = Requete { methode: rq.method().as_str().to_ascii_uppercase(), chemin, requete, cookie: entete("Cookie"), corps: Vec::new(), ip };
        let rep = if rq.body_length().is_some_and(|n| n as u64 > CORPS_MAX) {
            Reponse::texte(413, "formulaire trop gros")
        } else {
            let mut corps = Vec::new();
            match rq.as_reader().take(CORPS_MAX + 1).read_to_end(&mut corps) {
                Ok(_) if corps.len() as u64 <= CORPS_MAX => {
                    q.corps = corps;
                    self.repondre(&q)
                }
                _ => Reponse::texte(413, "formulaire trop gros"),
            }
        };
        let head = q.methode == "HEAD";
        let mut r = tiny_http::Response::from_data(if head { Vec::new() } else { rep.corps }).with_status_code(rep.statut);
        let mut ajouter = |k: &str, v: &str| {
            if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                r.add_header(h);
            }
        };
        ajouter("Content-Type", rep.type_contenu);
        ajouter("Content-Security-Policy", "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'");
        ajouter("X-Content-Type-Options", "nosniff");
        ajouter("Referrer-Policy", "no-referrer");
        if !rep.entetes.iter().any(|(k, _)| *k == "Cache-Control") {
            ajouter("Cache-Control", if rep.type_contenu.starts_with("image/") { "public, max-age=86400" } else { "no-cache" });
        }
        for (k, v) in &rep.entetes {
            ajouter(k, v);
        }
        debug!("{} {} {} → {}", q.ip, q.methode, q.chemin, rep.statut);
        let _ = rq.respond(r);
    }
}

fn gif(data: Vec<u8>) -> Reponse {
    Reponse { statut: 200, type_contenu: "image/gif", corps: data, entetes: Vec::new() }
}

/// Encodage d'un paramètre d'URL.
fn enc(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Vérification simple d'une adresse e-mail (le vrai test, c'est le lien).
fn courriel_valide(s: &str) -> bool {
    let Some((local, domaine)) = s.rsplit_once('@') else { return false };
    s.len() <= 254
        && !local.is_empty()
        && local.len() <= 64
        && domaine.contains('.')
        && !domaine.starts_with('.')
        && !domaine.ends_with('.')
        && s.chars().all(|c| c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"' | '\\' | ',' | ';'))
}

/// Interroge l'API GitHub (`releases/latest`) et classe les fichiers par système.
fn recuperer_version(depot: &str) -> Result<Version, String> {
    let url = format!("https://api.github.com/repos/{depot}/releases/latest");
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
    let rep = agent.get(&url).set("User-Agent", "v34-portail").set("Accept", "application/vnd.github+json").call().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_reader(rep.into_reader().take(2 << 20)).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or("?").to_string();
    let page = v["html_url"].as_str().unwrap_or("").to_string();
    let mut fichiers = Vec::new();
    for a in v["assets"].as_array().map(|a| &a[..]).unwrap_or(&[]) {
        let nom = a["name"].as_str().unwrap_or("").to_string();
        let url = a["browser_download_url"].as_str().unwrap_or("").to_string();
        if !url.starts_with("https://github.com/") {
            continue;
        }
        if let Some(systeme) = classer(&nom) {
            fichiers.push(pages::Fichier { systeme, nom, url, taille: a["size"].as_u64().unwrap_or(0) });
        }
    }
    fichiers.sort_by_key(|f| ordre(f.systeme));
    Ok(Version { tag, page, fichiers })
}

/// Système d'après le nom du fichier publié par la chaîne de compilation.
pub fn classer(nom: &str) -> Option<&'static str> {
    let n = nom.to_ascii_lowercase();
    if n.ends_with(".sig") || n.ends_with(".tar.gz") || n.ends_with(".zip") || n.ends_with(".json") {
        return None;
    }
    if n.ends_with(".dmg") {
        return Some(if n.contains("universal") {
            "Macintosh (tous)"
        } else if n.contains("aarch64") || n.contains("arm64") {
            "Macintosh (Apple Silicon)"
        } else {
            "Macintosh (Intel)"
        });
    }
    if n.ends_with(".exe") {
        return Some("Windows 10/11 (installateur)");
    }
    if n.ends_with(".msi") {
        return Some("Windows 10/11 (MSI)");
    }
    if n.ends_with(".appimage") {
        return Some("Linux (AppImage)");
    }
    if n.ends_with(".deb") {
        return Some("Linux (Debian, Ubuntu)");
    }
    if n.ends_with(".rpm") {
        return Some("Linux (Fedora, RPM)");
    }
    None
}

fn ordre(systeme: &str) -> usize {
    ["Macintosh (tous)", "Macintosh (Apple Silicon)", "Macintosh (Intel)", "Windows 10/11 (installateur)", "Windows 10/11 (MSI)", "Linux (AppImage)", "Linux (Debian, Ubuntu)", "Linux (Fedora, RPM)"]
        .iter()
        .position(|s| *s == systeme)
        .unwrap_or(99)
}

#[cfg(test)]
mod tests;
