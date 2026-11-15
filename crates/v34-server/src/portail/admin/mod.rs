//! Administration du site (`/admin`) : de quoi faire tourner le réseau
//! depuis un navigateur, sans SSH. Connexion avec le mot de passe
//! d'abonné de l'administrateur (`PORTAIL_ADMIN`, déclaré dans la
//! configuration du central).
//!
//! | Page | Contenu |
//! |---|---|
//! | `/admin` | tableau de bord : pupitre du central (qui est en ligne, qui appelle qui), chiffres, appels et factures du mois, accès Internet à décider, derniers inscrits |
//! | `/admin/abonnes?q=` | tous les abonnés (site et configuration), recherche |
//! | `/admin/abonne?nom=` | fiche : accès Internet, numéro, mot de passe (lien par e-mail ou provisoire), suspension, facture, succès, Tinyblog, numéros cachés, suppression |
//! | `/admin/inscriptions` | inscriptions en attente : renvoyer le lien, supprimer |
//! | `/admin/factures?mois=` | factures de tous les abonnés, journal des communications |
//! | `/admin/facture?nom=&mois=` | relevé détaillé d'un abonné |
//! | `/admin/tinyblog` (`?nom=`) | modération des Tinyblogs : articles, images, commentaires, blogs |
//! | `/admin/caches` | ce que les abonnés laissent sur les numéros cachés ([`depots`]) |
//! | `/admin/livre` | modération du livre d'or |
//! | `/admin/site` | inscriptions ouvertes ou fermées, « Quoi de neuf ? » ([`site`]) |
//!
//! Pas de JavaScript (la politique de sécurité du site l'interdit) : des
//! formulaires POST avec le jeton CSRF de la session ; ce qui efface,
//! coupe ou remplace demande de cocher « sûr » (vérifié ici aussi). Après
//! une action, la page d'où elle vient est réaffichée avec un message.
//!
//! Le site et le central partagent le dossier de données : les comptes
//! (relus par le central sous 2 s), les tickets et succès (écrits par le
//! central seul : ici, lecture seule), les Tinyblogs et les dépôts des
//! numéros cachés (écrits des deux côtés, sous le verrou du dossier,
//! [`crate::verrou`]).

pub mod depots;
mod pages;
pub mod site;

use sha2::{Digest, Sha256};
use v34_net::auth::{to_hex, PasswordHash, DEFAULT_ITERATIONS};
use v34_net::crypto::{ct_eq, random_bytes};
use v34_net::tarif;

use crate::comptes::{maintenant, Compte};
use crate::facturation::{self, Ticket};
use crate::tinyblog::store::{Com, Store};
use crate::{info, warn};

use super::courriel::Lettre;
use super::etat::{self, Etat};
use super::facture;
use super::pages::numero_lisible;
use super::{empreinte, enc, Portail, Reponse, Requete, COURRIELS_PAR_HEURE, SESSION_S};
use pages::{Cadre, Formulaires, LigneAbonne, Onglet};

/// Une page de l'administration.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Vue {
    Tableau,
    /// Liste, avec un filtre
    Abonnes(String),
    Abonne(String),
    Inscriptions,
    Factures(Option<u32>),
    Facture(String, Option<u32>),
    /// Tous les blogs, ou un seul
    Tinyblog(Option<String>),
    Caches,
    Livre,
    Site,
}

impl Vue {
    fn onglet(&self) -> Onglet {
        match self {
            Vue::Tableau => Onglet::Tableau,
            Vue::Abonnes(_) | Vue::Abonne(_) => Onglet::Abonnes,
            Vue::Inscriptions => Onglet::Inscriptions,
            Vue::Factures(_) | Vue::Facture(..) => Onglet::Factures,
            Vue::Tinyblog(_) => Onglet::Tinyblog,
            Vue::Caches => Onglet::Caches,
            Vue::Livre => Onglet::Livre,
            Vue::Site => Onglet::Site,
        }
    }

    fn depuis_get(q: &Requete) -> Option<Vue> {
        let mois = || q.param("mois").parse().ok().filter(|m| facture::mois_valide(*m));
        Some(match q.chemin.as_str() {
            "/admin" | "/admin/" => Vue::Tableau,
            "/admin/abonnes" => Vue::Abonnes(q.param("q").trim().to_string()),
            "/admin/abonne" => Vue::Abonne(q.param("nom")),
            "/admin/inscriptions" => Vue::Inscriptions,
            "/admin/factures" => Vue::Factures(mois()),
            "/admin/facture" => Vue::Facture(q.param("nom"), mois()),
            "/admin/tinyblog" => Vue::Tinyblog(Some(q.param("nom")).filter(|n| !n.is_empty())),
            "/admin/caches" => Vue::Caches,
            "/admin/livre" => Vue::Livre,
            "/admin/site" => Vue::Site,
            _ => return None,
        })
    }

    /// Page où revenir après une action (champ caché `retour` du
    /// formulaire), sinon `defaut`.
    fn retour(q: &Requete, defaut: Vue) -> Vue {
        match q.champ("retour").as_str() {
            "tableau" => Vue::Tableau,
            "abonnes" => Vue::Abonnes(String::new()),
            "abonne" => Vue::Abonne(q.champ("nom")),
            "tinyblog" => Vue::Tinyblog(None),
            "blog" => Vue::Tinyblog(Some(q.champ("nom"))),
            _ => defaut,
        }
    }
}

/// Issue d'une action : la page à afficher et un message (ou une erreur et
/// son statut HTTP).
type Issue = Result<(Vue, String), (Vue, String, u16)>;

/// Empreinte d'un commentaire de Tinyblog (contrôle avant suppression : la
/// liste a pu bouger depuis l'affichage).
pub(super) fn empreinte_com(c: &Com) -> String {
    let mut h = Sha256::new();
    h.update(c.de.as_bytes());
    h.update([0]);
    h.update(c.date.to_le_bytes());
    h.update([0]);
    h.update(c.texte.as_bytes());
    to_hex(&h.finalize())[..16].to_string()
}

/// Mot de passe provisoire : 12 caractères faciles à dicter, par groupes de 4.
fn mot_de_passe_provisoire() -> String {
    const LETTRES: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let b: [u8; 12] = random_bytes();
    let s: Vec<char> = b.iter().map(|x| LETTRES[*x as usize % LETTRES.len()] as char).collect();
    s.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

/// Numéro saisi, sans espaces, points ni tirets.
fn numero_saisi(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, ' ' | '.' | '-')).collect()
}

impl Portail {
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

    /// GET /admin et /admin/… : la page demandée, ou la connexion.
    pub(super) fn admin_page(&self, q: &Requete) -> Reponse {
        if self.r.admin.is_empty() {
            return self.message(404, "Administration désactivée", "Aucun administrateur n'est configuré (PORTAIL_ADMIN).", false);
        }
        let Some(jeton) = self.session(q) else {
            let mut r = self.page(200, "Administration", &pages::connexion(&self.r.admin, None));
            r.entetes.push(("Cache-Control", "no-store".into()));
            return r;
        };
        if q.chemin == "/admin/tinyblog/img" {
            return self.admin_image(q);
        }
        match Vue::depuis_get(q) {
            Some(v) => self.afficher(&jeton, &v, None, None, 200),
            None => self.message(404, "Fichier introuvable", "Cette page de l'administration n'existe pas (ou elle est encore en construction).", false),
        }
    }

    fn admin_connexion(&self, q: &Requete) -> Reponse {
        if self.r.admin.is_empty() {
            return Reponse::texte(404, "");
        }
        if !self.autoriser(&format!("connexion:{}", q.ip), 5, 900) {
            return self.page(429, "Administration", &pages::connexion(&self.r.admin, Some("Trop d'essais : attendez un quart d'heure.")));
        }
        let mdp = q.champ("mdp");
        let ok = self.config().and_then(|c| c.annuaire().ok()).and_then(|a| a.get(&self.r.admin).map(|(_, h)| h.clone())).is_some_and(|h| {
            ct_eq(&PasswordHash::with_salt(&mdp, &h.salt, h.iterations).stored_key, &h.stored_key)
        });
        if !ok {
            warn!("connexion admin refusée depuis {}", q.ip);
            return self.page(403, "Administration", &pages::connexion(&self.r.admin, Some("Mot de passe incorrect.")));
        }
        let jeton = to_hex(&random_bytes::<32>());
        self.sessions.lock().unwrap().insert(empreinte(&jeton), maintenant() + SESSION_S);
        info!("connexion admin depuis {}", q.ip);
        let secure = if self.r.url.starts_with("https://") { "; Secure" } else { "" };
        let mut r = Reponse::redirection("/admin");
        r.entetes.push(("Set-Cookie", format!("v34adm={jeton}; Path=/; HttpOnly; SameSite=Strict; Max-Age={SESSION_S}{secure}")));
        r
    }

    /// POST /admin/… : connexion, déconnexion et actions.
    pub(super) fn admin_post(&self, q: &Requete) -> Reponse {
        if q.chemin == "/admin/connexion" {
            return self.admin_connexion(q);
        }
        if self.r.admin.is_empty() {
            return Reponse::texte(404, "");
        }
        let Some(jeton) = self.session(q) else { return Reponse::redirection("/admin") };
        if !ct_eq(q.champ("csrf").as_bytes(), self.csrf(&jeton).as_bytes()) {
            return self.message(403, "Refusé", "Jeton de formulaire invalide : rechargez la page d'administration.", false);
        }
        let issue = match q.chemin.as_str() {
            "/admin/deconnexion" => {
                self.sessions.lock().unwrap().remove(&empreinte(&jeton));
                let mut r = Reponse::redirection("/");
                r.entetes.push(("Set-Cookie", "v34adm=; Path=/; Max-Age=0".into()));
                return r;
            }
            "/admin/fai" => self.adm_fai(q),
            "/admin/supprimer" => self.adm_supprimer(q),
            "/admin/suspendre" => self.adm_suspendre(q),
            "/admin/numero" => self.adm_numero(q),
            "/admin/mdp/lien" => self.adm_mdp_lien(q),
            "/admin/mdp/provisoire" => self.adm_mdp_provisoire(q),
            "/admin/inscription/renvoyer" => self.adm_inscription_renvoyer(q),
            "/admin/inscription/supprimer" => self.adm_inscription_supprimer(q),
            "/admin/livre/publier" => match self.livre.publier(&q.champ("id")) {
                Ok(m) => {
                    info!("livre d'or : message de « {} » publié", m.pseudo);
                    Ok((Vue::Livre, format!("Message de {} publié dans le livre d'or.", m.pseudo)))
                }
                Err(e) => Err((Vue::Livre, e, 400)),
            },
            "/admin/livre/supprimer" => match self.livre.supprimer(&q.champ("id")) {
                Ok(m) => {
                    info!("livre d'or : message de « {} » supprimé", m.pseudo);
                    Ok((Vue::Livre, format!("Message de {} supprimé du livre d'or.", m.pseudo)))
                }
                Err(e) => Err((Vue::Livre, e, 400)),
            },
            "/admin/tinyblog/article/supprimer" => self.adm_article_supprimer(q),
            "/admin/tinyblog/com/supprimer" => self.adm_com_supprimer(q),
            "/admin/tinyblog/vider" => self.adm_blog_vider(q),
            "/admin/tinyblog/supprimer" => self.adm_blog_supprimer(q),
            "/admin/caches/supprimer" => self.adm_depot_supprimer(q),
            "/admin/caches/supprimer-de" => self.adm_depots_de(q),
            "/admin/site/inscriptions" => {
                let fermer = q.champ("d") == "fermer";
                match self.site.fermer_inscriptions(fermer) {
                    Ok(()) => {
                        info!("inscriptions {} par l'administrateur", if fermer { "fermées" } else { "rouvertes" });
                        Ok((Vue::Site, if fermer { "Inscriptions fermées.".into() } else { "Inscriptions rouvertes.".into() }))
                    }
                    Err(e) => Err((Vue::Site, e, 503)),
                }
            }
            "/admin/site/nouvelle" => match self.site.ajouter_nouvelle(q.champ("date").trim(), &q.champ("titre")) {
                Ok(n) => Ok((Vue::Site, format!("Nouvelle du {} ajoutée à « Quoi de neuf ? ».", n.date))),
                Err(e) => Err((Vue::Site, e, 400)),
            },
            "/admin/site/nouvelle/supprimer" => match self.site.supprimer_nouvelle(&q.champ("id")) {
                Ok(n) => Ok((Vue::Site, format!("Nouvelle « {} » retirée.", n.titre))),
                Err(e) => Err((Vue::Site, e, 400)),
            },
            _ => return Reponse::texte(404, ""),
        };
        match issue {
            Ok((vue, info)) => self.afficher(&jeton, &vue, Some(&info), None, 200),
            Err((vue, e, statut)) => self.afficher(&jeton, &vue, None, Some(&e), statut),
        }
    }

    // ───────────────────────── données ─────────────────────────

    fn etat_central(&self) -> Option<Etat> {
        etat::lire(&self.r.etat, maintenant())
    }

    /// Tickets du mois `mois` (aucun sans dossier de données).
    fn tickets(&self, mois: u32) -> Vec<Ticket> {
        self.donnees().map(|d| facturation::lire_tickets(&d, mois)).unwrap_or_default()
    }

    fn tinyblogs(&self) -> Option<Store> {
        self.donnees().map(|d| Store::new(d.join("tinyblog")))
    }

    /// Tous les abonnés (configuration et site), par ordre alphabétique,
    /// avec leur facture du mois (`tickets`).
    fn lignes(&self, etat: Option<&Etat>, tickets: &[Ticket]) -> Vec<LigneAbonne> {
        let cfg = self.config();
        let annuaire = cfg.as_ref().and_then(|c| c.annuaire().ok());
        let en_ligne = |n: &str| etat.is_some_and(|e| e.en_ligne.iter().any(|x| x == n));
        let facture = |n: &str| tickets.iter().filter(|t| t.appelant == n).map(|t| t.centimes as u64).sum::<u64>();
        let mut v = Vec::new();
        for a in cfg.iter().flat_map(|c| &c.abonnes) {
            v.push(LigneAbonne { nom: a.nom.clone(), numero: a.numero.clone(), compte: None, fai: a.fai, en_ligne: en_ligne(&a.nom), ignore: false, centimes: facture(&a.nom) });
        }
        for c in self.comptes.liste() {
            let ignore = annuaire.as_ref().is_some_and(|a| a.nom_pris(&c.nom) || a.numero_pris(&c.numero));
            v.push(LigneAbonne {
                nom: c.nom.clone(),
                numero: c.numero.clone(),
                fai: c.fai && !ignore,
                en_ligne: en_ligne(&c.nom) && !ignore,
                ignore,
                centimes: facture(&c.nom),
                compte: Some(c),
            });
        }
        v.sort_by(|a, b| a.nom.to_lowercase().cmp(&b.nom.to_lowercase()).then(a.compte.is_some().cmp(&b.compte.is_some())));
        v
    }

    /// Pourquoi les inscriptions sont fermées (`Ok` : ouvertes).
    fn etat_inscriptions(&self) -> Result<(), String> {
        let mut pourquoi = Vec::new();
        if self.site.lire().inscriptions_fermees {
            pourquoi.push("par vous");
        }
        if !self.postier.actif() {
            pourquoi.push("pas d'e-mails : SMTP_HOST vide");
        }
        if self.config().is_none() {
            pourquoi.push("configuration du central illisible");
        }
        if pourquoi.is_empty() {
            Ok(())
        } else {
            Err(pourquoi.join(", "))
        }
    }

    /// Les nouvelles de l'accueil, avec leur origine (toutes, sans limite).
    fn nouvelles_origines(&self) -> Vec<(super::pages::Nouvelle, pages::Origine)> {
        let publiees = if self.r.depot.is_empty() {
            None
        } else {
            let depot = self.r.depot.clone();
            self.nouvelles.sans_attendre(format!("dernières versions de {depot}"), move || super::recuperer_nouvelles(&depot))
        };
        let mut v: Vec<(super::pages::Nouvelle, pages::Origine)> = publiees.iter().flat_map(|p| p.iter().take(3)).map(|n| (n.clone(), pages::Origine::GitHub)).collect();
        for a in self.site.lire().nouvelle {
            v.push((super::pages::Nouvelle { titre: a.titre, date: a.date, lien: String::new() }, pages::Origine::Ajoutee(a.id)));
        }
        for (date, titre) in super::NOUVELLES {
            v.push((super::pages::Nouvelle { titre: (*titre).into(), date: (*date).into(), lien: String::new() }, pages::Origine::Code));
        }
        v.push((super::pages::Nouvelle { titre: "Ouverture du site : les premiers modems chantent !".into(), date: super::OUVERTURE.into(), lien: String::new() }, pages::Origine::Code));
        v.sort_by(|a, b| b.0.date.cmp(&a.0.date));
        v
    }

    /// GET /admin/tinyblog/img?f= : une image de Tinyblog, pour la
    /// modération (le site public ne les sert pas : elles passent par le
    /// modem).
    fn admin_image(&self, q: &Requete) -> Reponse {
        match self.tinyblogs().and_then(|s| s.lire_image(&q.param("f"))) {
            Some(d) => Reponse { statut: 200, type_contenu: "image/gif", corps: d, entetes: vec![("Cache-Control", "private, max-age=300".into())] },
            None => Reponse::texte(404, "image introuvable"),
        }
    }

    // ───────────────────────── affichage ─────────────────────────

    fn afficher(&self, jeton: &str, vue: &Vue, info: Option<&str>, erreur: Option<&str>, statut: u16) -> Reponse {
        let csrf = self.csrf(jeton);
        let f = Formulaires(&csrf);
        let now = maintenant();
        let mois_courant = tarif::mois(now);
        let dir = self.donnees();
        let sans_donnees = || (r#"<div class="erreur">Ce site n'a pas de dossier de données (V34_COMPTES sans dossier) : rien à montrer ici.</div>"#.to_string(), 503);
        let (contenu, statut_page): (String, u16) = match vue {
            Vue::Tableau => {
                let etat = self.etat_central();
                let tickets = self.tickets(mois_courant);
                let lignes = self.lignes(etat.as_ref(), &tickets);
                let blogs = self.tinyblogs().map(|s| s.tous()).unwrap_or_default();
                let articles = blogs.iter().map(|(_, b)| b.articles.len()).sum();
                let coms = blogs.iter().flat_map(|(_, b)| &b.articles).map(|a| a.coms.len()).sum();
                let t = pages::Tableau {
                    etat: etat.as_ref(),
                    maintenant: now,
                    lignes: &lignes,
                    nb_inscriptions: self.comptes.en_attente().len(),
                    nb_a_relire: self.livre.en_attente().len(),
                    blogs: (blogs.len(), articles, coms),
                    mois: mois_courant,
                    tickets: &tickets,
                    inscriptions: self.etat_inscriptions(),
                };
                (pages::tableau(&f, &t), 200)
            }
            Vue::Abonnes(filtre) => {
                let etat = self.etat_central();
                let lignes = self.lignes(etat.as_ref(), &self.tickets(mois_courant));
                let total = lignes.len();
                let cherche = filtre.to_lowercase();
                let chiffres = numero_saisi(filtre);
                let choisies: Vec<LigneAbonne> = lignes
                    .into_iter()
                    .filter(|l| {
                        cherche.is_empty()
                            || l.nom.to_lowercase().contains(&cherche)
                            || (!chiffres.is_empty() && l.numero.contains(&chiffres))
                            || l.compte.as_ref().is_some_and(|c| c.courriel.to_lowercase().contains(&cherche))
                    })
                    .collect();
                (pages::abonnes(&f, &choisies, filtre, total), 200)
            }
            Vue::Abonne(nom) => {
                let etat = self.etat_central();
                let tickets = self.tickets(mois_courant);
                let lignes = self.lignes(etat.as_ref(), &tickets);
                // un compte du site en doublon d'un abonné de la configuration : le compte du site
                match lignes.iter().find(|l| l.nom == *nom && l.compte.is_some()).or_else(|| lignes.iter().find(|l| l.nom == *nom)) {
                    None => (format!(r#"<div class="erreur">Aucun abonné « {} ».</div><p><a href="/admin/abonnes">Tous les abonnés</a></p>"#, super::pages::esc(nom)), 404),
                    Some(l) => {
                        let appel = etat.as_ref().and_then(|e| e.communications.iter().find(|c| c.de == *nom || c.vers == *nom)).map(|c| (if c.de == *nom { c.vers.clone() } else { c.de.clone() }, c.decroche));
                        let siens: Vec<Ticket> = tickets.iter().filter(|t| t.appelant == *nom).cloned().collect();
                        let autres_mois: Vec<u32> = dir.as_ref().map(|d| facturation::mois_factures(d)).unwrap_or_default().into_iter().filter(|m| *m != mois_courant).collect();
                        let carnets = dir.as_ref().map(|d| facturation::lire_carnets(d)).unwrap_or_default();
                        let blog = self.tinyblogs().and_then(|s| s.lire(nom));
                        let nb_depots = dir.as_ref().map(|d| depots::compter_de(&depots::lister(d), nom)).unwrap_or(0);
                        let cfg = self.config();
                        let suggestions = if l.compte.is_some() { self.suggestions(cfg.as_deref(), 4) } else { Vec::new() };
                        let v = pages::Fiche {
                            ligne: l,
                            appel,
                            central: etat.is_some(),
                            maintenant: now,
                            mois: mois_courant,
                            tickets: &siens,
                            autres_mois: &autres_mois,
                            carnet: carnets.get(nom.as_str()),
                            blog: blog.as_ref(),
                            depots: nb_depots,
                            suggestions: &suggestions,
                            postier: self.postier.actif(),
                        };
                        (pages::fiche(&f, &v), 200)
                    }
                }
            }
            Vue::Inscriptions => (pages::inscriptions(&f, &self.comptes.en_attente(), self.postier.actif()), 200),
            Vue::Factures(mois) => {
                let mois = mois.unwrap_or(mois_courant);
                let mut autres: Vec<u32> = dir.as_ref().map(|d| facturation::mois_factures(d)).unwrap_or_default();
                if !autres.contains(&mois_courant) {
                    autres.insert(0, mois_courant);
                }
                autres.retain(|m| *m != mois);
                (pages::factures(mois, &autres, &self.tickets(mois)), 200)
            }
            Vue::Facture(nom, mois) => match &dir {
                None => sans_donnees(),
                Some(d) => {
                    let mois = mois.unwrap_or(mois_courant);
                    let tickets = self.tickets(mois);
                    // le numéro : celui de l'abonné, ou celui des tickets (compte supprimé depuis)
                    let numero = self
                        .lignes(None, &[])
                        .into_iter()
                        .find(|l| l.nom == *nom)
                        .map(|l| l.numero)
                        .or_else(|| tickets.iter().find(|t| t.appelant == *nom).map(|t| t.appelant_num.clone()))
                        .unwrap_or_default();
                    let e = enc(nom);
                    let retour = format!(r#"<a href="/admin/abonne?nom={e}">Fiche de {}</a> · <a href="/admin/factures?mois={mois}">Toutes les factures de {}</a>"#, super::pages::esc(nom), facture::nom_mois(mois));
                    (facture::detail_avec_liens(d, nom, &numero, mois, now, &|m| format!("/admin/facture?nom={e}&mois={m}"), &retour), 200)
                }
            },
            Vue::Tinyblog(None) => match self.tinyblogs() {
                None => sans_donnees(),
                Some(s) => (pages::tinyblogs(&f, &s.tous()), 200),
            },
            Vue::Tinyblog(Some(nom)) => match self.tinyblogs().and_then(|s| s.lire(nom)) {
                Some(b) => (pages::tinyblog(&f, nom, &b), 200),
                None => (format!(r#"<div class="erreur">Pas de Tinyblog au nom de « {} ».</div><p><a href="/admin/tinyblog">Tous les Tinyblogs</a></p>"#, super::pages::esc(nom)), 404),
            },
            Vue::Caches => match &dir {
                None => sans_donnees(),
                Some(d) => (pages::caches(&f, &depots::lister(d)), 200),
            },
            Vue::Livre => {
                let publies: Vec<super::livre::Message> = self.livre.publies().into_iter().take(20).collect();
                (pages::livre(&f, &self.livre.en_attente(), &publies), 200)
            }
            Vue::Site => {
                let h = tarif::paris(now);
                let aujourdhui = format!("{:04}-{:02}-{:02}", h.annee, h.mois, h.jour);
                let nouvelles = self.nouvelles_origines();
                let v = pages::VueSite { fermees: self.site.lire().inscriptions_fermees, postier: self.postier.actif(), config: self.config().is_some(), nouvelles: &nouvelles, aujourdhui: &aujourdhui };
                (pages::site(&f, &v), 200)
            }
        };
        let comptes = self.comptes.liste();
        let nb_config = self.config().map_or(0, |c| c.abonnes.len());
        let cadre = Cadre {
            csrf: &csrf,
            admin: &self.r.admin,
            onglet: vue.onglet(),
            nb_abonnes: nb_config + comptes.len(),
            nb_inscriptions: self.comptes.en_attente().len(),
            nb_a_relire: self.livre.en_attente().len(),
            nb_a_decider: comptes.iter().filter(|c| !c.fai_decide).count(),
        };
        // (après une action réussie, la page visée peut ne plus exister : 200 quand même)
        let statut = if statut == 200 && info.is_none() { statut_page } else { statut };
        let mut r = self.page(statut, "Administration", &pages::cadre(&cadre, info, erreur, &contenu));
        r.entetes.push(("Cache-Control", "no-store".into()));
        r
    }

    // ───────────────────────── actions sur les abonnés ─────────────────────────

    /// Le compte du site visé par l'action (champ `nom`).
    fn compte_vise(&self, q: &Requete, vue: Vue) -> Result<Compte, (Vue, String, u16)> {
        let nom = q.champ("nom");
        self.comptes.compte(&nom).ok_or_else(|| (vue, format!("Aucun compte du site au nom de « {nom} » (les abonnés de config.toml se gèrent avec v34-server)."), 404))
    }

    fn sur(q: &Requete, vue: &Vue) -> Result<(), (Vue, String, u16)> {
        if q.champ("sur") == "1" {
            Ok(())
        } else {
            Err((vue.clone(), "Cochez « sûr » pour confirmer.".into(), 400))
        }
    }

    fn adm_fai(&self, q: &Requete) -> Issue {
        let oui = q.champ("d") == "oui";
        let vue = Vue::retour(q, Vue::Tableau);
        match self.changer_acces(&q.champ("nom"), oui) {
            Ok((c, avant)) => Ok((vue, format!("Accès Internet de {} : {}.", c.nom, if oui { "accordé" } else if avant { "retiré" } else { "refusé" }))),
            Err(e) => Err((vue, e, 400)),
        }
    }

    fn adm_supprimer(&self, q: &Requete) -> Issue {
        let vue = Vue::Abonne(q.champ("nom"));
        Self::sur(q, &vue)?;
        let c = self.comptes.supprimer(&q.champ("nom")).map_err(|e| (vue, e, 400))?;
        self.fermer_sessions(&c.nom, None);
        info!("compte supprimé par l'administrateur : {} ({})", c.nom, c.numero);
        let mut t = format!("Compte {} supprimé ; le numéro {} est libéré.", c.nom, numero_lisible(&c.numero));
        if q.champ("tout") == "1" {
            if self.tinyblogs().and_then(|s| s.supprimer_blog(&c.nom)).is_some() {
                t.push_str(" Son Tinyblog est effacé.");
            }
            if let Some(d) = self.donnees() {
                match depots::supprimer_de(&d, &c.nom) {
                    Ok(0) => {}
                    Ok(n) => t.push_str(&format!(" {n} message(s) ou score(s) effacé(s) des numéros cachés.")),
                    Err(e) => t.push_str(&format!(" Numéros cachés : {e}.")),
                }
            }
        }
        Ok((Vue::Abonnes(String::new()), t))
    }

    fn adm_suspendre(&self, q: &Requete) -> Issue {
        let oui = q.champ("d") == "oui";
        let vue = Vue::Abonne(q.champ("nom"));
        if oui {
            Self::sur(q, &vue)?;
        }
        let c = self.comptes.suspendre(&q.champ("nom"), oui).map_err(|e| (vue.clone(), e, 400))?;
        info!("compte {} {} par l'administrateur", c.nom, if oui { "suspendu" } else { "réactivé" });
        let t = if oui {
            format!("Compte {} suspendu : le central le déconnecte sous 2 secondes et refuse ses connexions. Son numéro reste réservé.", c.nom)
        } else {
            format!("Compte {} réactivé : il peut se reconnecter.", c.nom)
        };
        Ok((vue, t))
    }

    fn adm_numero(&self, q: &Requete) -> Issue {
        let vue = Vue::Abonne(q.champ("nom"));
        let c = self.compte_vise(q, vue.clone())?;
        let numero = numero_saisi(&q.champ("numero"));
        if numero == c.numero {
            return Err((vue, format!("C'est déjà le numéro de {}.", c.nom), 400));
        }
        let cfg = self.config().ok_or((vue.clone(), "Configuration du central illisible : impossible de vérifier le numéro.".to_string(), 503))?;
        self.numero_attribuable(&cfg, &numero).map_err(|e| (vue.clone(), e, 400))?;
        let (c, ancien) = self.comptes.changer_numero(&c.nom, &numero).map_err(|e| (vue.clone(), e, 400))?;
        info!("numéro de {} changé par l'administrateur : {ancien} → {}", c.nom, c.numero);
        self.notifier(Lettre {
            a: c.courriel.clone(),
            sujet: format!("{} : votre nouveau numéro", self.r.nom),
            corps: format!(
                "Bonjour {nom},\n\nL'opérateur du central vous a attribué un nouveau numéro : {num} (au lieu du {ancien}).\n\nVotre logiciel le reçoit tout seul à la prochaine connexion au central. Prévenez vos correspondants !\n\n{sig}",
                nom = c.nom,
                num = numero_lisible(&c.numero),
                ancien = numero_lisible(&ancien),
                sig = self.signature()
            ),
        });
        Ok((vue, format!("{} : nouveau numéro {} (au lieu du {}). Le central le déconnecte ; il recevra son nouveau numéro en se reconnectant. Un courrier le prévient.", c.nom, numero_lisible(&c.numero), numero_lisible(&ancien))))
    }

    fn adm_mdp_lien(&self, q: &Requete) -> Issue {
        let vue = Vue::Abonne(q.champ("nom"));
        let c = self.compte_vise(q, vue.clone())?;
        if !self.postier.actif() {
            return Err((vue, "L'envoi des e-mails n'est pas configuré (SMTP_HOST vide).".into(), 503));
        }
        if !self.autoriser("courriels", COURRIELS_PAR_HEURE, 3600) {
            return Err((vue, "Trop d'e-mails envoyés cette heure-ci : réessayez plus tard.".into(), 503));
        }
        self.lien_reinitialisation(&c, "L'opérateur du central vous invite").map_err(|e| (vue.clone(), e, 503))?;
        info!("lien de réinitialisation envoyé à {} par l'administrateur", c.nom);
        Ok((vue, format!("Lien de réinitialisation envoyé à {} (valable une heure).", c.courriel)))
    }

    fn adm_mdp_provisoire(&self, q: &Requete) -> Issue {
        let vue = Vue::Abonne(q.champ("nom"));
        Self::sur(q, &vue)?;
        let c = self.compte_vise(q, vue.clone())?;
        let mdp = mot_de_passe_provisoire();
        let hash = PasswordHash::new(&mdp, DEFAULT_ITERATIONS).to_string();
        self.comptes.changer_mot_de_passe(&c.nom, &hash).map_err(|e| (vue.clone(), e, 503))?;
        self.fermer_sessions(&c.nom, None);
        info!("mot de passe provisoire donné à {} par l'administrateur", c.nom);
        self.notifier(Lettre {
            a: c.courriel.clone(),
            sujet: format!("{} : mot de passe provisoire", self.r.nom),
            corps: format!(
                "Bonjour {nom},\n\nL'opérateur du central a remplacé le mot de passe de votre abonnement « {nom} » par un mot de passe provisoire, qu'il vous communiquera lui-même.\n\nDès que vous l'avez, changez-le dans votre espace abonné ({url}/compte), ou choisissez-en un nouveau avec « Mot de passe oublié » ({url}/oubli). Pensez aussi à le changer dans le logiciel si vous l'aviez mémorisé.\n\n{sig}",
                nom = c.nom,
                url = self.r.url,
                sig = self.signature()
            ),
        });
        Ok((vue, format!("Mot de passe provisoire de {} : {mdp} — affiché une seule fois, notez-le. Le central le prend en compte sous 2 secondes ; ses sessions sur le site sont fermées.", c.nom)))
    }

    fn adm_inscription_renvoyer(&self, q: &Requete) -> Issue {
        let vue = Vue::Inscriptions;
        if !self.postier.actif() {
            return Err((vue, "L'envoi des e-mails n'est pas configuré (SMTP_HOST vide).".into(), 503));
        }
        if !self.autoriser("courriels", COURRIELS_PAR_HEURE, 3600) {
            return Err((vue, "Trop d'e-mails envoyés cette heure-ci : réessayez plus tard.".into(), 503));
        }
        let (i, jeton) = self.comptes.renouveler_inscription(&q.champ("nom")).map_err(|e| (vue.clone(), e, 400))?;
        self.postier.poster(self.lettre_activation(&i.nom, &i.numero, &i.courriel, &jeton));
        info!("lien d'activation renvoyé à {} par l'administrateur", i.nom);
        Ok((vue, format!("Nouveau lien d'activation envoyé à {} pour {} (valable 48 heures).", i.courriel, i.nom)))
    }

    fn adm_inscription_supprimer(&self, q: &Requete) -> Issue {
        let vue = Vue::Inscriptions;
        Self::sur(q, &vue)?;
        let i = self.comptes.annuler_inscription(&q.champ("nom")).map_err(|e| (vue.clone(), e, 400))?;
        info!("inscription en attente abandonnée par l'administrateur : {} ({})", i.nom, i.numero);
        Ok((vue, format!("Inscription de {} supprimée : le nom, le numéro {} et l'adresse sont libérés.", i.nom, numero_lisible(&i.numero))))
    }

    // ───────────────────────── modération ─────────────────────────

    fn adm_article_supprimer(&self, q: &Requete) -> Issue {
        let nom = q.champ("nom");
        let vue = Vue::retour(q, Vue::Tinyblog(Some(nom.clone())));
        Self::sur(q, &vue)?;
        let store = self.tinyblogs().ok_or((vue.clone(), "Pas de dossier de données.".to_string(), 503))?;
        let id: Option<u32> = q.champ("id").parse().ok();
        let retire = store.modifier_existant(&nom, |b| {
            let pos = b.articles.iter().position(|a| Some(a.id) == id)?;
            let a = b.articles.remove(pos);
            Some((a.titre, a.image))
        });
        let Some(Some((titre, image))) = retire else {
            return Err((vue, "Article introuvable (déjà supprimé ?).".into(), 404));
        };
        if let Some(f) = image {
            store.supprimer_image(&f);
        }
        info!("tinyblog : article « {titre} » de {nom} supprimé par l'administrateur");
        Ok((vue, format!("Article « {titre} » du Tinyblog de {nom} supprimé.")))
    }

    fn adm_com_supprimer(&self, q: &Requete) -> Issue {
        let nom = q.champ("nom");
        let vue = Vue::retour(q, Vue::Tinyblog(Some(nom.clone())));
        let store = self.tinyblogs().ok_or((vue.clone(), "Pas de dossier de données.".to_string(), 503))?;
        let (id, k, e): (Option<u32>, Option<usize>, String) = (q.champ("id").parse().ok(), q.champ("k").parse().ok(), q.champ("e"));
        let retire = store.modifier_existant(&nom, |b| {
            let a = b.articles.iter_mut().find(|a| Some(a.id) == id)?;
            let k = k.filter(|k| *k < a.coms.len() && empreinte_com(&a.coms[*k]) == e)?;
            Some(a.coms.remove(k))
        });
        let Some(Some(c)) = retire else {
            return Err((vue, "Commentaire introuvable : la page n'était plus à jour, rechargez-la.".into(), 404));
        };
        info!("tinyblog : commentaire de {} chez {nom} supprimé par l'administrateur", c.de);
        Ok((vue, format!("Commentaire de {} supprimé du Tinyblog de {nom}.", c.de)))
    }

    fn adm_blog_vider(&self, q: &Requete) -> Issue {
        let nom = q.champ("nom");
        let vue = Vue::Tinyblog(Some(nom.clone()));
        let store = self.tinyblogs().ok_or((vue.clone(), "Pas de dossier de données.".to_string(), 503))?;
        store
            .modifier_existant(&nom, |b| {
                b.titre.clear();
                b.description.clear();
            })
            .ok_or((vue.clone(), "Tinyblog introuvable.".to_string(), 404))?;
        info!("tinyblog : titre et présentation de {nom} effacés par l'administrateur");
        Ok((vue, format!("Titre et présentation du Tinyblog de {nom} effacés.")))
    }

    fn adm_blog_supprimer(&self, q: &Requete) -> Issue {
        let nom = q.champ("nom");
        let vue = Vue::retour(q, Vue::Tinyblog(None));
        Self::sur(q, &vue)?;
        let b = self.tinyblogs().and_then(|s| s.supprimer_blog(&nom)).ok_or((vue.clone(), "Tinyblog introuvable.".to_string(), 404))?;
        info!("tinyblog de {nom} supprimé par l'administrateur ({} articles)", b.articles.len());
        let vue = if vue == Vue::Tinyblog(Some(nom.clone())) { Vue::Tinyblog(None) } else { vue };
        Ok((vue, format!("Tinyblog de {nom} supprimé ({} article(s), avec leurs images et leurs commentaires).", b.articles.len())))
    }

    fn adm_depot_supprimer(&self, q: &Requete) -> Issue {
        let vue = Vue::Caches;
        let d = self.donnees().ok_or((vue.clone(), "Pas de dossier de données.".to_string(), 503))?;
        let fichier = q.champ("fichier");
        let e = depots::supprimer(&d, &fichier, &q.champ("cle"), &q.champ("e")).map_err(|e| (vue.clone(), e, 400))?;
        let de = e.de.unwrap_or_else(|| "?".into());
        info!("numéros cachés : entrée de {de} retirée de {fichier} par l'administrateur");
        Ok((vue, format!("Entrée de {de} retirée ({fichier}).")))
    }

    fn adm_depots_de(&self, q: &Requete) -> Issue {
        let nom = q.champ("nom");
        let vue = Vue::Abonne(nom.clone());
        Self::sur(q, &vue)?;
        let d = self.donnees().ok_or((vue.clone(), "Pas de dossier de données.".to_string(), 503))?;
        let n = depots::supprimer_de(&d, &nom).map_err(|e| (vue.clone(), e, 503))?;
        info!("numéros cachés : {n} entrée(s) de {nom} effacée(s) par l'administrateur");
        Ok((vue, format!("{n} message(s) ou score(s) de {nom} effacé(s) des numéros cachés.")))
    }
}
