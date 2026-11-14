//! v34-portail : site d'inscription façon FAI 1998 (voir deploy/portail/).
//!
//! Réglages par variables d'environnement (deploy/portail/.env.example) ;
//! les comptes sont écrits dans le fichier partagé avec le central.

use std::process::ExitCode;

use v34_server::comptes::Comptes;
use v34_server::log::{self, Level};
use v34_server::portail::courriel::SmtpConfig;
use v34_server::portail::{Portail, Reglages};
use v34_server::{error, warn};

fn main() -> ExitCode {
    if std::env::args().any(|a| a == "--version") {
        println!("v34-portail {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let get = |k: &str| std::env::var(k).ok();
    if let Some(l) = get("PORTAIL_JOURNAL").and_then(|j| Level::parse(&j)) {
        log::set_level(l);
    }
    let r = match Reglages::depuis_env(get) {
        Ok(r) => r,
        Err(e) => {
            error!("réglages : {e}");
            return ExitCode::FAILURE;
        }
    };
    let smtp = match SmtpConfig::depuis_env(get) {
        Ok(s) => s,
        Err(e) => {
            error!("réglages SMTP : {e}");
            return ExitCode::FAILURE;
        }
    };
    if smtp.is_none() {
        warn!("SMTP_HOST vide : les inscriptions sont fermées (pas d'e-mail de validation possible)");
    }
    if r.admin.is_empty() || r.admin_courriel.is_empty() {
        warn!("PORTAIL_ADMIN ou PORTAIL_ADMIN_COURRIEL vide : personne ne recevra les demandes d'accès Internet");
    }
    let comptes = match Comptes::ouvrir(&r.comptes) {
        Ok(c) => c,
        Err(e) => {
            error!("fichier des comptes : {e}");
            return ExitCode::FAILURE;
        }
    };
    match Portail::new(r, comptes, smtp).servir() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("serveur HTTP : {e}");
            ExitCode::FAILURE
        }
    }
}
