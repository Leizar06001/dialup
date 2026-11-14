//! Espace abonné : la facture de Tinyverse Télécom (tickets du central),
//! les succès et le classement des plus grosses factures du mois.
//!
//! Tout vient des fichiers que le central écrit dans son dossier de données
//! ([`crate::facturation`]) ; le site ne fait que lire.

use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;

use v34_net::succes;
use v34_net::tarif::{self, Categorie};

use super::pages::{date_heure_fr, esc, numero_lisible};
use crate::facturation::{self, Ticket};

const MOIS: [&str; 12] = ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"];

/// 202611 → « novembre 2026 »
pub fn nom_mois(m: u32) -> String {
    let i = (m % 100).clamp(1, 12) as usize - 1;
    format!("{} {}", MOIS[i], m / 100)
}

/// « 1:02:03 » ou « 2:03 »
fn hms(s: u32) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// « 1 h 02 min », « 12 min », « 40 s »
fn duree(s: u64) -> String {
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h {:02} min", s / 3600, s % 3600 / 60),
    }
}

fn correspondant(t: &Ticket) -> String {
    match t.service.as_deref() {
        Some("fai") => "TINYVERSE (accès Internet)".into(),
        Some("oracle") => "3615 ORACLE".into(),
        Some("echo") => "Ligne de test".into(),
        Some(s) => s.to_string(),
        None => t.appele.clone(),
    }
}

/// Appels passés par `nom` ce mois-là.
fn appels(dir: &Path, nom: &str, mois: u32) -> Vec<Ticket> {
    facturation::lire_tickets(dir, mois).into_iter().filter(|t| t.appelant == nom).collect()
}

/// Mois valide (AAAAMM) ?
pub fn mois_valide(m: u32) -> bool {
    (190_001..=999_912).contains(&m) && (1..=12).contains(&(m % 100))
}

/// Ce qui s'ajoute à la page « Mon compte » : la facture du mois, les
/// succès, le classement.
pub fn resume(dir: &Path, nom: &str, maintenant: u64) -> String {
    let mois = tarif::mois(maintenant);
    let tous = facturation::lire_tickets(dir, mois);
    let miens: Vec<&Ticket> = tous.iter().filter(|t| t.appelant == nom).collect();
    let total: u64 = miens.iter().map(|t| t.centimes as u64).sum();
    let secondes: u64 = miens.iter().map(|t| t.duree_s as u64).sum();
    let mut h = String::new();
    let _ = write!(
        h,
        r#"<h3>Ma facture de {m}</h3>
<table class="grille fiche"><tr><th>Communications passées</th><td>{n} appel{s}, {d}</td></tr>
<tr><th>Montant</th><td><b>{f}</b> (soit {e})</td></tr></table>
<p><a href="/compte/facture?mois={mois}">Détail des communications</a> · <span class="aide">Tinyverse Télécom ne vous la réclamera jamais : les francs sont pour de rire.</span></p>"#,
        m = nom_mois(mois),
        n = miens.len(),
        s = if miens.len() > 1 { "s" } else { "" },
        d = duree(secondes),
        f = tarif::francs(total),
        e = tarif::euros(total),
    );

    // Succès : obtenus d'abord (avec la date), puis à gagner, cachés en « ??? »
    let carnets = facturation::lire_carnets(dir);
    let eus = carnets.get(nom).map(|c| c.succes.clone()).unwrap_or_default();
    let _ = write!(h, r#"<h3>Mes succès ({}/{})</h3><table class="grille succes">"#, eus.len(), succes::LISTE.len());
    let mut liste: Vec<&succes::Succes> = succes::LISTE.iter().collect();
    liste.sort_by_key(|s| !eus.contains_key(s.id));
    for s in liste {
        match eus.get(s.id) {
            Some(&quand) => {
                let _ = write!(h, r#"<tr class="eu"><td>★</td><td><b>{}</b></td><td>{}</td><td>{}</td></tr>"#, esc(s.titre), esc(s.description), esc(&super::pages::date_fr(quand)));
            }
            None if s.cache => h.push_str(r#"<tr class="pas-eu"><td>☆</td><td>???</td><td>Succès caché : à vous de le trouver.</td><td></td></tr>"#),
            None => {
                let _ = write!(h, r#"<tr class="pas-eu"><td>☆</td><td>{}</td><td>{}</td><td></td></tr>"#, esc(s.titre), esc(s.description));
            }
        }
    }
    h.push_str("</table>");

    // Classement du mois (abonnés seulement : cette page est derrière la connexion)
    let mut totaux: HashMap<&str, u64> = HashMap::new();
    for t in &tous {
        *totaux.entry(t.appelant.as_str()).or_default() += t.centimes as u64;
    }
    let mut classement: Vec<(&str, u64)> = totaux.into_iter().filter(|(_, c)| *c > 0).collect();
    classement.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    if !classement.is_empty() {
        let _ = write!(h, r#"<h3>Les plus grosses factures de {}</h3><table class="grille classement">"#, nom_mois(mois));
        for (i, (n, c)) in classement.iter().take(10).enumerate() {
            let moi = if *n == nom { r#" class="moi""# } else { "" };
            let _ = write!(h, "<tr{moi}><td>{}</td><td>{}</td><td>{}</td></tr>", i + 1, esc(n), tarif::francs(*c));
        }
        h.push_str("</table>");
    }
    h
}

/// Le relevé détaillé d'un mois.
pub fn detail(dir: &Path, nom: &str, numero: &str, mois: u32, maintenant: u64) -> String {
    let tickets = appels(dir, nom, mois);
    let total: u64 = tickets.iter().map(|t| t.centimes as u64).sum();
    let mut h = String::new();
    let _ = write!(
        h,
        r#"<h2>Relevé de communications</h2>
<div class="releve"><div class="releve-tete"><b>TINYVERSE TÉLÉCOM</b><br>Service des abonnés · 3615 PAS-DE-RÉCLAMATION</div>
<table class="grille fiche"><tr><th>Abonné</th><td><b>{nom}</b> · <span class="num">{num}</span></td></tr>
<tr><th>Période</th><td>{m}</td></tr></table>"#,
        nom = esc(nom),
        num = esc(&numero_lisible(numero)),
        m = nom_mois(mois),
    );
    if tickets.is_empty() {
        h.push_str("<p>Aucune communication passée ce mois-ci. Le combiné prend la poussière.</p>");
    } else {
        h.push_str(r#"<table class="grille detail"><tr><th>Date et heure</th><th>Numéro appelé</th><th>Correspondant</th><th>Durée</th><th>Tarif</th><th>Montant</th></tr>"#);
        for t in &tickets {
            let cat = Categorie::depuis_code(&t.categorie).unwrap_or(Categorie::Local);
            let tarif = match cat {
                Categorie::Local if tarif::heures_creuses(t.debut) => "Local, heures creuses",
                c => c.libelle(),
            };
            let _ = write!(
                h,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"montant\">{}</td></tr>",
                esc(&date_heure_fr(t.debut)),
                esc(&numero_lisible(&t.numero)),
                esc(&correspondant(t)),
                hms(t.duree_s),
                tarif,
                tarif::francs(t.centimes as u64)
            );
        }
        h.push_str("</table>");
    }
    let _ = write!(
        h,
        r#"<table class="grille fiche"><tr><th>Abonnement mensuel</th><td>0,00 F (offert)</td></tr>
<tr><th>Total des communications</th><td><b>{f}</b></td></tr>
<tr><th>Soit, en euros</th><td>{e} (1 € = 6,55957 F)</td></tr>
<tr><th>À régler avant le</th><td>jamais</td></tr></table></div>
<p class="aide">Tarifs : communication locale (abonnés, FAI) 0,74 F les 3 premières minutes, puis 0,28 F la minute en heures pleines et 0,14 F en heures creuses (22 h – 8 h, samedi et dimanche) ; 3615 ORACLE 2,23 F la minute ; ligne de test (numéro vert) gratuite. Seul l'appelant paie.</p>"#,
        f = tarif::francs(total),
        e = tarif::euros(total),
    );
    // Les autres mois facturés
    let courant = tarif::mois(maintenant);
    let mut autres: Vec<u32> = facturation::mois_factures(dir).into_iter().filter(|m| *m != mois).collect();
    if mois != courant && !autres.contains(&courant) {
        autres.insert(0, courant);
    }
    if !autres.is_empty() {
        h.push_str("<p>Autres relevés : ");
        let liens: Vec<String> = autres.iter().map(|m| format!(r#"<a href="/compte/facture?mois={m}">{}</a>"#, nom_mois(*m))).collect();
        h.push_str(&liens.join(" · "));
        h.push_str("</p>");
    }
    h.push_str(r#"<p><a href="/compte">Retour à mon compte</a></p>"#);
    h
}
