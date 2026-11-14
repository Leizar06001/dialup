//! Espace abonné (`/compte`) et mot de passe oublié (`/oubli`,
//! `/reinitialiser`).
//!
//! - Connexion avec le nom d'abonné et le mot de passe du logiciel : un
//!   abonné de la configuration du central (lecture seule, « géré par
//!   l'opérateur ») ou un compte créé par le site. Comme dans le central, la
//!   configuration a la priorité. Session par cookie `v34abo`, distincte de
//!   celle de l'administration.
//! - Les comptes du site peuvent changer de mot de passe et se résilier
//!   (mot de passe actuel exigé, formulaire avec jeton CSRF).
//! - Mot de passe oublié : un lien valable 1 h part par e-mail si un compte
//!   du site utilise l'adresse ; la page affichée est la même dans tous les
//!   cas (on ne révèle pas quelles adresses sont abonnées).

use v34_net::auth::{to_hex, PasswordHash, DEFAULT_ITERATIONS};
use v34_net::crypto::{ct_eq, random_bytes};

use crate::comptes::{maintenant, Compte};
use crate::{debug, info, warn};

use super::courriel::Lettre;
use super::pages::{self, numero_lisible};
use super::{courriel_valide, empreinte, Portail, Reponse, Requete, COURRIELS_PAR_HEURE, SESSION_S};

/// Essais de connexion (et de vérification du mot de passe actuel) par
/// adresse IP…
const ESSAIS_MAX: usize = 5;
/// …par fenêtre de (s)
const ESSAIS_FENETRE_S: u64 = 900;
/// Demandes « mot de passe oublié » par adresse IP et par heure
const OUBLIS_PAR_HEURE: usize = 3;
/// Liens de réinitialisation envoyés par compte et par heure (une boîte aux
/// lettres ne doit pas être inondée depuis plusieurs adresses)
const OUBLIS_PAR_COMPTE: usize = 3;

/// L'abonné d'une session.
pub(super) enum Abonne {
    /// Compte créé par le site (modifiable depuis l'espace abonné)
    Site(Compte),
    /// Abonné de la configuration du central (géré par l'opérateur)
    Config { nom: String, numero: String, fai: bool },
}

/// Le mot de passe correspond-il à l'empreinte ? (comparaison en temps
/// constant)
fn mot_de_passe_ok(h: &PasswordHash, mdp: &str) -> bool {
    ct_eq(&PasswordHash::with_salt(mdp, &h.salt, h.iterations).stored_key, &h.stored_key)
}

/// Contrôle d'un nouveau mot de passe saisi deux fois.
fn nouveau_mot_de_passe(mdp: &str, mdp2: &str) -> Result<(), String> {
    let n = mdp.chars().count();
    if !(8..=128).contains(&n) {
        Err("Nouveau mot de passe : 8 à 128 caractères.".into())
    } else if mdp != mdp2 {
        Err("Les deux nouveaux mots de passe sont différents.".into())
    } else {
        Ok(())
    }
}

impl Portail {
    /// Retrouve un abonné et l'empreinte de son mot de passe.
    fn abonne(&self, nom: &str) -> Option<(Abonne, PasswordHash)> {
        if let Some(a) = self.config().and_then(|c| c.annuaire().ok()) {
            if let Some((numero, h)) = a.get(nom) {
                return Some((Abonne::Config { nom: nom.to_string(), numero: numero.clone(), fai: a.fai_autorise(nom) }, h.clone()));
            }
        }
        let c = self.comptes.compte(nom)?;
        let h = PasswordHash::parse(&c.mot_de_passe_hash).ok()?;
        Some((Abonne::Site(c), h))
    }

    /// Session de l'espace abonné : (jeton, abonné). Une session dont le
    /// compte a disparu n'est plus valable.
    fn session_abonne(&self, q: &Requete) -> Option<(String, Abonne)> {
        let jeton = q.cookie("v34abo")?;
        let now = maintenant();
        let nom = {
            let mut s = self.sessions_abonnes.lock().unwrap();
            s.retain(|_, (_, exp)| *exp > now);
            s.get(&empreinte(&jeton))?.0.clone()
        };
        let (a, _) = self.abonne(&nom)?;
        Some((jeton, a))
    }

    fn csrf_abonne(&self, jeton: &str) -> String {
        self.signer(&["csrf-abonne", jeton])[..32].to_string()
    }

    /// Ferme les sessions d'un abonné (sauf, éventuellement, la session en
    /// cours).
    fn fermer_sessions(&self, nom: &str, sauf: Option<&str>) {
        let garder = sauf.map(empreinte);
        self.sessions_abonnes.lock().unwrap().retain(|e, (n, _)| n != nom || garder.as_deref() == Some(e.as_str()));
    }

    fn cookie_abonne(&self, valeur: &str, max_age: u64) -> (&'static str, String) {
        let secure = if self.r.url.starts_with("https://") { "; Secure" } else { "" };
        ("Set-Cookie", format!("v34abo={valeur}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"))
    }

    /// E-mail de notification au titulaire, dans la limite du plafond global
    /// (au-delà, il est abandonné et noté dans le journal).
    fn notifier(&self, l: Lettre) {
        if self.autoriser("courriels", COURRIELS_PAR_HEURE, 3600) {
            self.postier.poster(l);
        } else {
            warn!("plafond des e-mails atteint : « {} » non envoyé à {}", l.sujet, l.a);
        }
    }

    fn signature(&self) -> String {
        format!("-- \n{}\n{}\n", self.r.nom, self.r.url)
    }

    // ───────────────────────── espace abonné ─────────────────────────

    /// GET /compte : formulaire de connexion, ou la page de l'abonné.
    pub(super) fn compte(&self, q: &Requete) -> Reponse {
        match self.session_abonne(q) {
            Some((jeton, a)) => self.compte_vue(&jeton, &a, None, None, 200),
            None => self.compte_connexion_page(None, "", 200),
        }
    }

    fn compte_connexion_page(&self, erreur: Option<&str>, nom: &str, statut: u16) -> Reponse {
        let mut r = self.page(statut, "Mon compte", &pages::compte_connexion(erreur, nom));
        r.entetes.push(("Cache-Control", "no-store".into()));
        r
    }

    fn compte_vue(&self, jeton: &str, a: &Abonne, info: Option<&str>, erreur: Option<&str>, statut: u16) -> Reponse {
        let v = match a {
            Abonne::Site(c) => pages::VueCompte {
                nom: &c.nom,
                numero: &c.numero,
                courriel: Some(&c.courriel),
                cree: Some(pages::date_fr(c.cree)),
                acces: if c.fai {
                    pages::Acces::Accorde
                } else if c.fai_decide {
                    pages::Acces::Refuse
                } else {
                    pages::Acces::EnAttente
                },
            },
            Abonne::Config { nom, numero, fai } => pages::VueCompte {
                nom,
                numero,
                courriel: None,
                cree: None,
                acces: if *fai { pages::Acces::Accorde } else { pages::Acces::Refuse },
            },
        };
        let mut r = self.page(statut, "Mon compte", &pages::compte(&v, &self.r.central, &self.csrf_abonne(jeton), info, erreur));
        r.entetes.push(("Cache-Control", "no-store".into()));
        r
    }

    /// POST /compte/connexion
    pub(super) fn compte_connexion(&self, q: &Requete) -> Reponse {
        // (borné : le nom saisi peut finir dans le journal)
        let nom: String = q.champ("nom").trim().chars().take(64).collect();
        if !self.autoriser(&format!("abonne:{}", q.ip), ESSAIS_MAX, ESSAIS_FENETRE_S) {
            return self.compte_connexion_page(Some("Trop d'essais : attendez un quart d'heure."), &nom, 429);
        }
        let mdp = q.champ("mdp");
        let ok = match self.abonne(&nom) {
            Some((_, h)) => mot_de_passe_ok(&h, &mdp),
            None => {
                // Même travail qu'un vrai essai : la durée ne dit pas si le nom existe.
                let _ = PasswordHash::with_salt(&mdp, &[0; 16], DEFAULT_ITERATIONS);
                false
            }
        };
        if !ok {
            warn!("connexion à l'espace abonné refusée pour {nom:?} depuis {}", q.ip);
            return self.compte_connexion_page(Some("Nom d'abonné ou mot de passe incorrect."), &nom, 403);
        }
        let jeton = to_hex(&random_bytes::<32>());
        self.sessions_abonnes.lock().unwrap().insert(empreinte(&jeton), (nom.clone(), maintenant() + SESSION_S));
        info!("{nom} se connecte à son espace abonné depuis {}", q.ip);
        let mut r = Reponse::redirection("/compte");
        r.entetes.push(self.cookie_abonne(&jeton, SESSION_S));
        r
    }

    /// POST /compte/mdp, /compte/supprimer, /compte/deconnexion
    pub(super) fn compte_action(&self, q: &Requete) -> Reponse {
        let Some((jeton, a)) = self.session_abonne(q) else { return Reponse::redirection("/compte") };
        if !ct_eq(q.champ("csrf").as_bytes(), self.csrf_abonne(&jeton).as_bytes()) {
            return self.message(403, "Refusé", "Jeton de formulaire invalide : rechargez la page de votre compte.", false);
        }
        if q.chemin == "/compte/deconnexion" {
            self.sessions_abonnes.lock().unwrap().remove(&empreinte(&jeton));
            let mut r = Reponse::redirection("/");
            r.entetes.push(self.cookie_abonne("", 0));
            return r;
        }
        let Abonne::Site(c) = &a else {
            return self.compte_vue(&jeton, &a, None, Some("Votre abonnement est géré par l'opérateur du central : adressez-vous à lui."), 403);
        };
        let erreur = |e: &str, statut| self.compte_vue(&jeton, &a, None, Some(e), statut);
        if !self.autoriser(&format!("abonne-verif:{}", q.ip), ESSAIS_MAX, ESSAIS_FENETRE_S) {
            return erreur("Trop d'essais : attendez un quart d'heure.", 429);
        }
        let actuel = PasswordHash::parse(&c.mot_de_passe_hash).is_ok_and(|h| mot_de_passe_ok(&h, &q.champ("mdp")));
        if !actuel {
            warn!("{} : mot de passe actuel incorrect ({}) depuis {}", c.nom, q.chemin, q.ip);
            return erreur("Mot de passe actuel incorrect.", 403);
        }
        if q.chemin == "/compte/supprimer" {
            if q.champ("sur") != "1" {
                return erreur("Cochez la case pour confirmer la résiliation.", 400);
            }
            return match self.comptes.supprimer(&c.nom) {
                Ok(c) => {
                    self.fermer_sessions(&c.nom, None);
                    info!("compte résilié par son titulaire : {} ({}) depuis {}", c.nom, c.numero, q.ip);
                    self.notifier(Lettre {
                        a: c.courriel.clone(),
                        sujet: format!("{} : abonnement résilié", self.r.nom),
                        corps: format!(
                            "Bonjour {nom},\n\nVotre abonnement est résilié et le numéro {num} est libéré. Votre nom d'abonné, votre adresse électronique, votre numéro et l'empreinte de votre mot de passe ont été effacés du fichier des abonnés.\n\nFrais de résiliation : un pincement au cœur. Revenez quand vous voulez !\n\n{sig}",
                            nom = c.nom,
                            num = numero_lisible(&c.numero),
                            sig = self.signature()
                        ),
                    });
                    let mut r = self.message(200, "Abonnement résilié", &format!("Votre compte {} est supprimé et le numéro {} est libéré. Au revoir, et merci d'avoir fait chanter nos modems !", c.nom, numero_lisible(&c.numero)), true);
                    r.entetes.push(self.cookie_abonne("", 0));
                    r
                }
                Err(e) => erreur(&e, 400),
            };
        }
        // Changement de mot de passe
        if let Err(e) = nouveau_mot_de_passe(&q.champ("nouveau"), &q.champ("nouveau2")) {
            return erreur(&e, 400);
        }
        let hash = PasswordHash::new(&q.champ("nouveau"), DEFAULT_ITERATIONS).to_string();
        if let Err(e) = self.comptes.changer_mot_de_passe(&c.nom, &hash) {
            return erreur(&e, 503);
        }
        // Les autres sessions (un autre ordinateur, peut-être pas le vôtre) sont fermées.
        self.fermer_sessions(&c.nom, Some(&jeton));
        info!("{} change son mot de passe depuis {}", c.nom, q.ip);
        self.notifier(self.lettre_mot_de_passe_change(c, "depuis votre espace abonné"));
        let a = self.abonne(&c.nom).map(|(a, _)| a).unwrap_or(a);
        self.compte_vue(&jeton, &a, Some("Mot de passe changé. Pensez à le changer aussi dans le logiciel si vous l'aviez mémorisé."), None, 200)
    }

    fn lettre_mot_de_passe_change(&self, c: &Compte, comment: &str) -> Lettre {
        let contact = if self.r.contact.is_empty() { String::new() } else { format!(", et prévenez l'opérateur ({})", self.r.contact) };
        Lettre {
            a: c.courriel.clone(),
            sujet: format!("{} : mot de passe changé", self.r.nom),
            corps: format!(
                "Bonjour {nom},\n\nLe mot de passe de votre abonnement « {nom} » vient d'être changé {comment}.\n\nC'est bien vous ? Rien à faire : pensez seulement à le mettre à jour dans le logiciel si vous l'aviez mémorisé.\n\nCe n'est pas vous ? Choisissez-en un nouveau tout de suite avec « Mot de passe oublié » ({url}/oubli){contact}.\n\n{sig}",
                nom = c.nom,
                url = self.r.url,
                sig = self.signature()
            ),
        }
    }

    // ───────────────────────── mot de passe oublié ─────────────────────────

    fn oubli_formulaire(&self, courriel: &str, erreur: Option<&str>, statut: u16) -> Reponse {
        if !self.postier.actif() {
            return self.message(503, "Fonction indisponible", "Le facteur électronique est en grève : impossible d'envoyer un lien pour l'instant. Écrivez à l'opérateur.", false);
        }
        self.page(statut, "Mot de passe oublié", &pages::oubli(courriel, erreur, &self.jeton_temps("oubli")))
    }

    /// GET /oubli
    pub(super) fn oubli_page(&self) -> Reponse {
        self.oubli_formulaire("", None, 200)
    }

    /// POST /oubli : la réponse est la même que l'adresse soit abonnée ou
    /// non.
    pub(super) fn oubli(&self, q: &Requete) -> Reponse {
        let courriel = q.champ("courriel").trim().to_string();
        if !self.postier.actif() {
            return self.oubli_formulaire(&courriel, None, 503);
        }
        if let Err(e) = self.controle_robot(q, "oubli") {
            return self.oubli_formulaire(&courriel, Some(&e), 400);
        }
        if !courriel_valide(&courriel) {
            return self.oubli_formulaire(&courriel, Some("Adresse électronique invalide."), 400);
        }
        if !self.autoriser(&format!("oubli:{}", q.ip), OUBLIS_PAR_HEURE, 3600) {
            return self.oubli_formulaire(&courriel, Some("Trop de demandes depuis votre adresse : réessayez dans une heure."), 429);
        }
        match self.comptes.compte_par_courriel(&courriel) {
            Some(c) if !self.autoriser(&format!("oubli-compte:{}", c.nom), OUBLIS_PAR_COMPTE, 3600) => {
                warn!("mot de passe oublié : trop de demandes pour {} (depuis {})", c.nom, q.ip);
            }
            Some(c) if !self.autoriser("courriels", COURRIELS_PAR_HEURE, 3600) => {
                warn!("plafond des e-mails atteint : lien de réinitialisation pour {} non envoyé", c.nom);
            }
            Some(c) => match self.comptes.demander_reinitialisation(&c.nom) {
                Ok(jeton) => {
                    info!("mot de passe oublié : lien envoyé à {} depuis {}", c.nom, q.ip);
                    self.postier.poster(Lettre {
                        a: c.courriel.clone(),
                        sujet: format!("{} : choisir un nouveau mot de passe", self.r.nom),
                        corps: format!(
                            "Bonjour {nom},\n\nQuelqu'un (vous, espérons-le) a demandé à changer le mot de passe de l'abonnement « {nom} » (numéro {num}).\n\nPour choisir un nouveau mot de passe, ouvrez ce lien dans l'heure :\n\n{url}/reinitialiser?t={jeton}\n\nSi vous n'avez rien demandé, ignorez ce message : votre mot de passe actuel reste valable.\n\n{sig}",
                            nom = c.nom,
                            num = numero_lisible(&c.numero),
                            url = self.r.url,
                            sig = self.signature()
                        ),
                    });
                }
                Err(e) => warn!("mot de passe oublié pour {} : {e}", c.nom),
            },
            None => debug!("mot de passe oublié : aucune inscription pour l'adresse saisie (depuis {})", q.ip),
        }
        self.page(200, "Mot de passe oublié", &pages::oubli_envoye(&courriel))
    }

    /// GET /reinitialiser?t=… : formulaire du nouveau mot de passe.
    pub(super) fn reinitialiser_page(&self, q: &Requete) -> Reponse {
        let t = q.param("t");
        match self.comptes.reinitialisation(&t) {
            Some(c) => self.page(200, "Nouveau mot de passe", &pages::reinitialiser(&t, &c.nom, None)),
            None => self.page(404, "Lien invalide", &pages::reinitialiser_invalide()),
        }
    }

    /// POST /reinitialiser
    pub(super) fn reinitialiser(&self, q: &Requete) -> Reponse {
        let t = q.champ("t");
        let Some(c) = self.comptes.reinitialisation(&t) else {
            return self.page(404, "Lien invalide", &pages::reinitialiser_invalide());
        };
        if let Err(e) = nouveau_mot_de_passe(&q.champ("mdp"), &q.champ("mdp2")) {
            return self.page(400, "Nouveau mot de passe", &pages::reinitialiser(&t, &c.nom, Some(&e)));
        }
        let hash = PasswordHash::new(&q.champ("mdp"), DEFAULT_ITERATIONS).to_string();
        match self.comptes.reinitialiser(&t, &hash) {
            Ok(c) => {
                self.fermer_sessions(&c.nom, None);
                info!("{} a choisi un nouveau mot de passe (lien de réinitialisation) depuis {}", c.nom, q.ip);
                self.notifier(self.lettre_mot_de_passe_change(&c, "avec le lien « mot de passe oublié »"));
                self.page(200, "Mot de passe changé", &pages::reinitialise(&c.nom))
            }
            Err(e) => self.message(400, "Impossible", &e, false),
        }
    }
}
