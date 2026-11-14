//! Envoi d'e-mails par un serveur SMTP authentifié, en arrière-plan.
//!
//! Variables d'environnement (mêmes noms que les autres sites du VPS) :
//! `SMTP_HOST`, `SMTP_PORT` (465), `SMTP_USER`, `SMTP_PASSWORD`,
//! `SMTP_FROM` (« Nom <adresse> »), `SMTP_SSL` (vide : TLS direct si le port
//! est 465, sinon STARTTLS ; `true`/`false` pour forcer).
//!
//! `SMTP_HOST=journal` : rien n'est envoyé, les e-mails sont écrits dans le
//! journal (essais en local).

use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::Duration;

use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};

use crate::{error, info, warn};

/// Un e-mail à envoyer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lettre {
    pub a: String,
    pub sujet: String,
    pub corps: String,
}

#[derive(Clone)]
pub struct SmtpConfig {
    pub hote: String,
    pub port: u16,
    pub utilisateur: String,
    pub mot_de_passe: String,
    pub expediteur: String,
    /// TLS dès la connexion (465) plutôt que STARTTLS (587)
    pub tls_direct: bool,
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig").field("hote", &self.hote).field("port", &self.port).field("utilisateur", &self.utilisateur).finish_non_exhaustive()
    }
}

impl SmtpConfig {
    /// D'après l'environnement ; `None` si `SMTP_HOST` est vide.
    pub fn depuis_env(get: impl Fn(&str) -> Option<String>) -> Result<Option<SmtpConfig>, String> {
        let hote = get("SMTP_HOST").unwrap_or_default().trim().to_string();
        if hote.is_empty() {
            return Ok(None);
        }
        let port: u16 = match get("SMTP_PORT").filter(|p| !p.trim().is_empty()) {
            Some(p) => p.trim().parse().map_err(|_| format!("SMTP_PORT invalide : {p}"))?,
            None => 465,
        };
        let tls_direct = match get("SMTP_SSL").unwrap_or_default().trim().to_ascii_lowercase().as_str() {
            "" => port == 465,
            "1" | "true" | "oui" | "yes" => true,
            "0" | "false" | "non" | "no" => false,
            autre => return Err(format!("SMTP_SSL invalide : {autre}")),
        };
        let expediteur = get("SMTP_FROM").unwrap_or_default().trim().trim_matches('"').to_string();
        let c = SmtpConfig {
            hote,
            port,
            utilisateur: get("SMTP_USER").unwrap_or_default().trim().to_string(),
            mot_de_passe: get("SMTP_PASSWORD").unwrap_or_default(),
            expediteur,
            tls_direct,
        };
        if c.hote != "journal" {
            c.expediteur.parse::<Mailbox>().map_err(|e| format!("SMTP_FROM invalide « {} » : {e}", c.expediteur))?;
        }
        Ok(Some(c))
    }
}

/// File d'envoi : les e-mails partent d'un fil dédié (un serveur SMTP lent
/// ne bloque pas le site).
pub struct Postier {
    tx: Option<SyncSender<Lettre>>,
    /// Pour les tests : les lettres « envoyées » en mode journal
    #[cfg(test)]
    pub boite: std::sync::Arc<std::sync::Mutex<Vec<Lettre>>>,
}

impl Postier {
    pub fn new(cfg: Option<SmtpConfig>) -> Postier {
        #[cfg(test)]
        let boite = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let tx = cfg.map(|c| {
            let (tx, rx) = mpsc::sync_channel(100);
            #[cfg(test)]
            let b = boite.clone();
            std::thread::Builder::new()
                .name("courriel".into())
                .spawn(move || {
                    #[cfg(test)]
                    {
                        let _ = &c;
                        for l in rx {
                            b.lock().unwrap().push(l);
                        }
                    }
                    #[cfg(not(test))]
                    distribuer(c, rx)
                })
                .expect("fil d'envoi des e-mails");
            tx
        });
        Postier {
            tx,
            #[cfg(test)]
            boite,
        }
    }

    /// L'envoi est-il configuré ?
    pub fn actif(&self) -> bool {
        self.tx.is_some()
    }

    /// Met un e-mail dans la file. Faux si l'envoi est désactivé ou la file
    /// pleine.
    pub fn poster(&self, l: Lettre) -> bool {
        match &self.tx {
            Some(tx) => match tx.try_send(l) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) => {
                    warn!("file d'envoi des e-mails pleine");
                    false
                }
                Err(TrySendError::Disconnected(_)) => false,
            },
            None => false,
        }
    }
}

#[cfg_attr(test, allow(dead_code))]
fn distribuer(c: SmtpConfig, rx: Receiver<Lettre>) {
    if c.hote == "journal" {
        for l in rx {
            info!("e-mail (non envoyé, SMTP_HOST=journal) à {} : {}\n{}", l.a, l.sujet, l.corps);
        }
        return;
    }
    let de: Mailbox = c.expediteur.parse().expect("expéditeur vérifié au démarrage");
    let relais = if c.tls_direct { SmtpTransport::relay(&c.hote) } else { SmtpTransport::starttls_relay(&c.hote) };
    let transport = match relais {
        Ok(b) => b.port(c.port).credentials(Credentials::new(c.utilisateur.clone(), c.mot_de_passe.clone())).timeout(Some(Duration::from_secs(30))).build(),
        Err(e) => {
            error!("serveur SMTP {} inutilisable : {e}", c.hote);
            return;
        }
    };
    for l in rx {
        let a: Mailbox = match l.a.parse() {
            Ok(a) => a,
            Err(e) => {
                warn!("adresse invalide {} : {e}", l.a);
                continue;
            }
        };
        let msg = match Message::builder().from(de.clone()).to(a).subject(l.sujet.clone()).header(ContentType::TEXT_PLAIN).body(l.corps.clone()) {
            Ok(m) => m,
            Err(e) => {
                warn!("e-mail impossible à composer : {e}");
                continue;
            }
        };
        let mut essai = 0;
        loop {
            essai += 1;
            match transport.send(&msg) {
                Ok(_) => {
                    info!("e-mail envoyé à {} : {}", l.a, l.sujet);
                    break;
                }
                Err(e) if essai < 3 => {
                    warn!("envoi à {} : {e} (nouvel essai)", l.a);
                    std::thread::sleep(Duration::from_secs(5 * essai));
                }
                Err(e) => {
                    error!("envoi à {} abandonné : {e}", l.a);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(v: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| v.iter().find(|(n, _)| *n == k).map(|(_, x)| x.to_string())
    }

    #[test]
    fn reglages_comme_les_autres_sites() {
        assert!(SmtpConfig::depuis_env(env(&[])).unwrap().is_none());
        let c = SmtpConfig::depuis_env(env(&[
            ("SMTP_HOST", "smtp.mail.ovh.net"),
            ("SMTP_PORT", "465"),
            ("SMTP_USER", "contact@example.org"),
            ("SMTP_PASSWORD", "secret"),
            ("SMTP_FROM", "\"Tinyverse Dialup <contact@example.org>\""),
            ("SMTP_STARTTLS", "true"),
            ("SMTP_SSL", ""),
        ]))
        .unwrap()
        .unwrap();
        assert!(c.tls_direct, "465 : TLS direct");
        assert_eq!(c.expediteur, "Tinyverse Dialup <contact@example.org>");
        assert!(!format!("{c:?}").contains("secret"), "pas de mot de passe dans les journaux");
        let c = SmtpConfig::depuis_env(env(&[("SMTP_HOST", "h"), ("SMTP_PORT", "587"), ("SMTP_FROM", "a@b.fr")])).unwrap().unwrap();
        assert!(!c.tls_direct);
        assert!(SmtpConfig::depuis_env(env(&[("SMTP_HOST", "h"), ("SMTP_FROM", "pas une adresse")])).is_err());
    }
}
