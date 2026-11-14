//! v34-server : central téléphonique + modems des numéros spéciaux
//! (voir deploy/README.md).

use std::path::PathBuf;
use std::process::ExitCode;

use v34_net::auth::DEFAULT_ITERATIONS;
use v34_server::admin;
use v34_server::config::{Config, DEFAULT_PATH};
use v34_server::{info, log, warn, Server};

const USAGE: &str = "\
v34-server — central téléphonique V.34 sur IP et modem « FAI »

USAGE :
  v34-server [--config CHEMIN]            démarre le central
  v34-server verifier [--config CHEMIN]   vérifie la configuration et quitte
  v34-server ajouter-abonne NOM NUMERO [--config CHEMIN]
        ajoute (ou met à jour) un abonné ; le mot de passe est demandé au
        clavier (ou lu sur l'entrée standard si ce n'est pas un terminal)
  v34-server retirer-abonne NOM [--config CHEMIN]
  v34-server liste-abonnes [--config CHEMIN]
  v34-server --example-config             affiche une configuration commentée
  v34-server --version

CHEMIN par défaut : /etc/v34-server/config.toml. Un central en marche
recharge l'annuaire tout seul quand le fichier change (sous 2 s).";

const EXAMPLE: &str = include_str!("../../../deploy/config.example.toml");

fn main() -> ExitCode {
    let mut positional = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut iterations = DEFAULT_ITERATIONS;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" | "aide" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("v34-server {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--example-config" => {
                print!("{EXAMPLE}");
                return ExitCode::SUCCESS;
            }
            "--config" | "-c" => match it.next() {
                Some(p) => path = Some(PathBuf::from(p)),
                None => return usage_error("--config attend un chemin"),
            },
            // option discrète, pour les tests
            "--iterations" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) => iterations = n,
                None => return usage_error("--iterations attend un nombre"),
            },
            "--check" => positional.insert(0, "verifier".to_string()),
            s if s.starts_with('-') => return usage_error(&format!("option inconnue : {s}")),
            s => positional.push(s.to_string()),
        }
    }
    // compatibilité : `v34-server CHEMIN` (premier argument = fichier .toml)
    if path.is_none() && positional.first().is_some_and(|p| p.ends_with(".toml")) {
        path = Some(PathBuf::from(positional.remove(0)));
    }
    let path = path.unwrap_or_else(|| PathBuf::from(DEFAULT_PATH));
    let args: Vec<&str> = positional.iter().map(String::as_str).collect();
    let res = match args.as_slice() {
        [] => return run(path),
        ["verifier"] | ["vérifier"] => check(&path),
        ["ajouter-abonne", nom, numero] => add(&path, nom, numero, iterations),
        ["retirer-abonne", nom] => admin::remove_subscriber(&path, nom).map(|()| {
            println!("Abonné {nom} retiré de {}.", path.display());
        }),
        ["liste-abonnes"] => list(&path),
        _ => return usage_error("commande inconnue ou arguments manquants"),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erreur : {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("{msg}\n\n{USAGE}");
    ExitCode::from(2)
}

fn check(path: &std::path::Path) -> Result<(), String> {
    let cfg = Config::load(path)?;
    for w in cfg.validate()? {
        println!("attention : {w}");
    }
    println!(
        "{} : configuration valide ({} abonné(s), numéros spéciaux : {})",
        path.display(),
        cfg.abonnes.len(),
        specials(&cfg)
    );
    Ok(())
}

fn specials(cfg: &Config) -> String {
    let s: Vec<String> = cfg.services().iter().map(|(n, k)| format!("{n} ({k})")).collect();
    if s.is_empty() {
        "aucun".into()
    } else {
        s.join(", ")
    }
}

fn add(path: &std::path::Path, nom: &str, numero: &str, iterations: u32) -> Result<(), String> {
    let pw = admin::read_password(&format!("Mot de passe de {nom} : "), true)?;
    if pw.chars().count() < 8 {
        return Err("mot de passe trop court (8 caractères au moins)".into());
    }
    let existed = admin::add_subscriber(path, nom, numero, &pw, iterations)?;
    println!(
        "Abonné {nom} {} au {numero} dans {}.",
        if existed { "mis à jour" } else { "ajouté" },
        path.display()
    );
    println!("Un central en marche recharge l'annuaire automatiquement.");
    Ok(())
}

fn list(path: &std::path::Path) -> Result<(), String> {
    let cfg = admin::list_subscribers(path)?;
    if cfg.abonnes.is_empty() {
        println!("Aucun abonné. (v34-server ajouter-abonne NOM NUMERO)");
    } else {
        println!("{:<32} NUMÉRO", "NOM");
        for a in &cfg.abonnes {
            println!("{:<32} {}", a.nom, a.numero);
        }
    }
    println!("Numéros spéciaux : {}", specials(&cfg));
    Ok(())
}

fn run(path: PathBuf) -> ExitCode {
    let cfg = match Config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration invalide : {e}");
            return ExitCode::from(2);
        }
    };
    if let Ok(level) = cfg.log_level() {
        log::set_level(level);
    }
    for w in cfg.validate().unwrap_or_default() {
        warn!("configuration : {w}");
    }
    let mut server = match Server::bind(cfg.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("impossible d'ouvrir {} : {e}", cfg.serveur.ecoute);
            return ExitCode::FAILURE;
        }
    };
    server.watch_config(path.clone());
    if !cfg.serveur.comptes.trim().is_empty() {
        let f = PathBuf::from(cfg.serveur.comptes.trim());
        info!("comptes du site d'inscription : {}", f.display());
        server = server.with_comptes_file(f);
    }
    if let Some(e) = cfg.serveur.etat_path() {
        info!("état publié pour le site dans {}", e.display());
        server = server.with_status_file(e);
    }
    info!(
        "v34-server {} à l'écoute sur UDP {} — {} abonné(s), numéros spéciaux : {}",
        env!("CARGO_PKG_VERSION"),
        server.local_addr().map(|a| a.to_string()).unwrap_or_default(),
        cfg.abonnes.len(),
        specials(&cfg)
    );
    server.run();
    ExitCode::SUCCESS
}
