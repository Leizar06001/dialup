//! Pages de l'administration : du HTML de 1998, sans JavaScript (la
//! politique de sécurité du site l'interdit). Chaque action est un petit
//! formulaire POST avec le jeton CSRF de la session ; ce qui efface ou
//! coupe demande de cocher « sûr ».

use std::collections::HashMap;
use std::fmt::Write;

use v34_net::succes;
use v34_net::tarif;

use crate::comptes::{Compte, Inscription};
use crate::facturation::{Carnet, Ticket};
use crate::tinyblog::store::{Article, Blog, Com};

use super::super::enc;
use super::super::etat::Etat;
use super::super::facture::{self, nom_mois};
use super::super::livre::Message;
use super::super::pages::{date_fr, date_heure_fr, duree_fr, esc, numero_lisible, pluriel, texte_multiligne, voyant, Nouvelle};
use super::depots::Depot;
use super::empreinte_com;

/// Onglet du menu de l'administration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Onglet {
    Tableau,
    Abonnes,
    Inscriptions,
    Factures,
    Tinyblog,
    Caches,
    Livre,
    Site,
}

/// Ce que montre le menu, sur toutes les pages.
pub struct Cadre<'a> {
    pub csrf: &'a str,
    pub admin: &'a str,
    pub onglet: Onglet,
    pub nb_abonnes: usize,
    pub nb_inscriptions: usize,
    pub nb_a_relire: usize,
    pub nb_a_decider: usize,
}

/// Petits formulaires de l'administration (jeton CSRF compris).
pub struct Formulaires<'a>(pub &'a str);

impl Formulaires<'_> {
    /// Un bouton qui envoie `champs` à `/admin/{action}` ; `sur` : case
    /// « sûr » à cocher.
    pub fn bouton(&self, action: &str, champs: &[(&str, &str)], texte: &str, sur: bool) -> String {
        let mut caches = String::new();
        for (n, v) in champs {
            let _ = write!(caches, r#"<input type="hidden" name="{}" value="{}">"#, esc(n), esc(v));
        }
        format!(
            r#"<form method="post" action="/admin/{action}" style="display:inline"><input type="hidden" name="csrf" value="{csrf}">{caches}{coche}<button type="submit">{texte}</button></form>"#,
            csrf = esc(self.0),
            coche = if sur { r#"<label class="aide"><input type="checkbox" name="sur" value="1" required>sûr</label> "# } else { "" }
        )
    }

    /// Début d'un formulaire plus grand (à fermer par `</form>`).
    pub fn debut(&self, action: &str) -> String {
        format!(r#"<form method="post" action="/admin/{action}" class="adm-form"><input type="hidden" name="csrf" value="{}">"#, esc(self.0))
    }
}

/// Lien vers la fiche d'un abonné.
fn lien_abonne(nom: &str) -> String {
    format!(r#"<a href="/admin/abonne?nom={}">{}</a>"#, enc(nom), esc(nom))
}

/// Le correspondant d'un appel en cours (champ `vers` de l'état du central).
fn vers_lisible(vers: &str) -> String {
    match vers {
        "fai" => "TINYVERSE (accès Internet)".into(),
        "oracle" => "3615 ORACLE".into(),
        "echo" => "la ligne de test".into(),
        s if s.starts_with("cache-") => match crate::caches::LISTE.iter().find(|n| n.service == s) {
            Some(n) => format!("le numéro caché « {} »", esc(n.nom)),
            None => esc(s),
        },
        s => lien_abonne(s),
    }
}

/// « 2 appels, 12 min »
fn appels_duree(n: usize, secondes: u64) -> String {
    format!("{}, {}", pluriel(n as u64, "appel", "appels"), facture::duree(secondes))
}

/// Toutes les pages : titre, menu, message.
pub fn cadre(c: &Cadre, info: Option<&str>, erreur: Option<&str>, contenu: &str) -> String {
    let mut h = format!(r#"<h2>Administration</h2><p class="aide">Connecté en tant que <b>{}</b>, opérateur du réseau.</p>"#, esc(c.admin));
    let lien = |o: Onglet, href: &str, texte: &str| format!(r#"<a href="{href}"{}>{texte}</a>"#, if c.onglet == o { r#" class="ici""# } else { "" });
    let compte = |n: usize, quoi: &str| if n > 0 { format!(" ({n}{quoi})") } else { String::new() };
    h.push_str(r#"<div class="adm-menu">"#);
    h.push_str(&lien(Onglet::Tableau, "/admin", &format!("Tableau de bord{}", compte(c.nb_a_decider, " à décider"))));
    h.push_str(&lien(Onglet::Abonnes, "/admin/abonnes", &format!("Abonnés ({})", c.nb_abonnes)));
    h.push_str(&lien(Onglet::Inscriptions, "/admin/inscriptions", &format!("Inscriptions{}", compte(c.nb_inscriptions, ""))));
    h.push_str(&lien(Onglet::Factures, "/admin/factures", "Factures"));
    h.push_str(&lien(Onglet::Tinyblog, "/admin/tinyblog", "Tinyblog"));
    h.push_str(&lien(Onglet::Caches, "/admin/caches", "Numéros cachés"));
    h.push_str(&lien(Onglet::Livre, "/admin/livre", &format!("Livre d'or{}", compte(c.nb_a_relire, " à relire"))));
    h.push_str(&lien(Onglet::Site, "/admin/site", "Le site"));
    let _ = write!(h, r#"<form method="post" action="/admin/deconnexion"><input type="hidden" name="csrf" value="{}"><button type="submit">Se déconnecter</button></form></div>"#, esc(c.csrf));
    if let Some(i) = info {
        let _ = write!(h, r#"<div class="ok">{}</div>"#, esc(i));
    }
    if let Some(e) = erreur {
        let _ = write!(h, r#"<div class="erreur">{}</div>"#, esc(e));
    }
    h.push_str(contenu);
    h
}

pub fn connexion(admin: &str, erreur: Option<&str>) -> String {
    let mut h = String::from("<h2>Administration</h2>");
    if let Some(e) = erreur {
        let _ = write!(h, r#"<div class="erreur">{}</div>"#, esc(e));
    }
    let _ = write!(
        h,
        r#"<form method="post" action="/admin/connexion">
<p><label>Abonné</label><br><input type="text" value="{a}" disabled></p>
<p><label for="mdp">Mot de passe</label><br><input type="password" id="mdp" name="mdp" required autofocus></p>
<p><button type="submit">Entrer</button></p></form>"#,
        a = esc(admin)
    );
    h
}

// ───────────────────────── abonnés ─────────────────────────

/// Un abonné, qu'il vienne du site ou de la configuration du central.
#[derive(Clone, Debug)]
pub struct LigneAbonne {
    pub nom: String,
    pub numero: String,
    /// Compte du site (`None` : abonné de config.toml)
    pub compte: Option<Compte>,
    /// Accès Internet (décision enregistrée, même si le compte est suspendu)
    pub fai: bool,
    pub en_ligne: bool,
    /// Compte du site ignoré par le central : nom ou numéro déjà dans la
    /// configuration
    pub ignore: bool,
    /// Facture du mois en cours
    pub centimes: u64,
}

impl LigneAbonne {
    fn internet(&self, f: &Formulaires, retour: &str) -> String {
        let etat = match &self.compte {
            _ if self.fai => r#"<span class="voyant v-vert"></span> <b>accordé</b>"#,
            Some(c) if c.fai_decide => r#"<span class="voyant v-rouge"></span> refusé"#,
            Some(_) => r#"<span class="voyant v-orange"></span> <span class="nouveau">à décider</span>"#,
            None => r#"<span class="voyant v-rouge"></span> non (config.toml)"#,
        };
        let Some(c) = &self.compte else { return etat.into() };
        let champs = |d: &'static str| [("nom", c.nom.as_str()), ("d", d), ("retour", retour)];
        let boutons = if c.fai {
            f.bouton("fai", &champs("non"), "Retirer", false)
        } else if c.fai_decide {
            f.bouton("fai", &champs("oui"), "Accorder", false)
        } else {
            f.bouton("fai", &champs("oui"), "Accorder", false) + " " + &f.bouton("fai", &champs("non"), "Refuser", false)
        };
        format!("{etat} {boutons}")
    }

    fn etat(&self) -> String {
        let mut v = Vec::new();
        if self.en_ligne {
            v.push(r#"<span class="voyant v-vert"></span> en ligne"#.to_string());
        }
        if self.compte.as_ref().is_some_and(|c| c.suspendu) {
            v.push(r#"<span class="voyant v-rouge"></span> <b>suspendu</b>"#.into());
        }
        if self.ignore {
            v.push(r#"<span class="voyant v-orange"></span> ignoré par le central (doublon de config.toml)"#.into());
        }
        v.join("<br>")
    }
}

pub fn abonnes(f: &Formulaires, lignes: &[LigneAbonne], filtre: &str, total: usize) -> String {
    let mut h = format!(
        r#"<form method="get" action="/admin/abonnes" class="adm-form"><label for="q">Chercher un nom, un numéro, une adresse</label><br><input type="text" id="q" name="q" value="{}"> <button type="submit">Chercher</button></form>"#,
        esc(filtre)
    );
    if filtre.is_empty() {
        let _ = write!(h, "<h3>Tous les abonnés ({total})</h3>");
    } else {
        let _ = write!(h, "<h3>{} sur {total} pour « {} »</h3>", pluriel(lignes.len() as u64, "abonné", "abonnés"), esc(filtre));
    }
    if lignes.is_empty() {
        h.push_str("<p>Personne.</p>");
        return h;
    }
    h.push_str(r#"<table class="grille"><tr><th>Abonné</th><th>Numéro</th><th>Courriel</th><th>Inscrit le</th><th>Internet</th><th>État</th><th>Ce mois-ci</th></tr>"#);
    for l in lignes {
        let (courriel, cree) = match &l.compte {
            Some(c) => (esc(&c.courriel), esc(&date_fr(c.cree))),
            None => (r#"<span class="aide">config.toml</span>"#.to_string(), String::new()),
        };
        let _ = write!(
            h,
            r#"<tr><td><b>{}</b></td><td>{}</td><td>{courriel}</td><td>{cree}</td><td>{}</td><td>{}</td><td class="n">{}</td></tr>"#,
            lien_abonne(&l.nom),
            esc(&numero_lisible(&l.numero)),
            l.internet(f, "abonnes"),
            l.etat(),
            tarif::francs(l.centimes)
        );
    }
    h.push_str("</table>");
    h
}

/// Ce que montre la fiche d'un abonné.
pub struct Fiche<'a> {
    pub ligne: &'a LigneAbonne,
    /// Appel en cours : (correspondant, décroché)
    pub appel: Option<(String, Option<u64>)>,
    /// Le central publie-t-il son état ?
    pub central: bool,
    pub maintenant: u64,
    pub mois: u32,
    pub tickets: &'a [Ticket],
    pub autres_mois: &'a [u32],
    pub carnet: Option<&'a Carnet>,
    pub blog: Option<&'a Blog>,
    pub depots: usize,
    pub suggestions: &'a [String],
    /// Les e-mails partent-ils ?
    pub postier: bool,
}

pub fn fiche(f: &Formulaires, v: &Fiche) -> String {
    let l = v.ligne;
    let nom = l.nom.as_str();
    let mut h = format!(r#"<h3>Fiche de l'abonné {}</h3><table class="grille fiche"><tr><th>Numéro</th><td><span class="num">{}</span></td></tr>"#, esc(nom), esc(&numero_lisible(&l.numero)));
    match &l.compte {
        Some(c) => {
            let _ = write!(
                h,
                r#"<tr><th>Adresse électronique</th><td><a href="mailto:{c}">{c}</a></td></tr><tr><th>Origine</th><td>inscrit par le site le {}</td></tr>"#,
                esc(&date_heure_fr(c.cree)),
                c = esc(&c.courriel)
            );
        }
        None => h.push_str(r#"<tr><th>Origine</th><td>déclaré dans <code>/etc/v34-server/config.toml</code> : géré par <code>v34-server ajouter-abonne</code> / <code>retirer-abonne</code> (le site ne modifie jamais ce fichier)</td></tr>"#),
    }
    let _ = write!(h, "<tr><th>Accès Internet</th><td>{}</td></tr>", l.internet(f, "abonne"));
    if let Some(c) = &l.compte {
        let compte = if c.suspendu {
            format!(r#"<span class="voyant v-rouge"></span> <b>suspendu</b> : le central refuse sa connexion. {}"#, f.bouton("suspendre", &[("nom", nom), ("d", "non")], "Réactiver", false))
        } else {
            format!(r#"<span class="voyant v-vert"></span> actif {}"#, f.bouton("suspendre", &[("nom", nom), ("d", "oui")], "Suspendre", true))
        };
        let _ = write!(h, "<tr><th>Compte</th><td>{compte}</td></tr>");
    }
    if l.ignore {
        h.push_str(r#"<tr><th>Attention</th><td>Ce compte du site est <b>ignoré par le central</b> : son nom ou son numéro est aussi dans config.toml, qui a la priorité.</td></tr>"#);
    }
    let maintenant = match (&v.appel, l.en_ligne, v.central) {
        (_, _, false) => r#"<span class="voyant v-eteint"></span> on ne sait pas : le central ne publie pas son état"#.to_string(),
        (Some((vers, Some(t))), _, _) => format!(r#"<span class="voyant v-orange clignote"></span> en communication avec {} depuis {}"#, vers_lisible(vers), esc(&duree_fr(v.maintenant.saturating_sub(*t)))),
        (Some((vers, None)), _, _) => format!(r#"<span class="voyant v-orange clignote"></span> ça sonne chez {}"#, vers_lisible(vers)),
        (None, true, _) => r#"<span class="voyant v-vert"></span> en ligne, ligne libre"#.into(),
        (None, false, _) => r#"<span class="voyant v-eteint"></span> hors ligne"#.into(),
    };
    let _ = write!(h, "<tr><th>En ce moment</th><td>{maintenant}</td></tr></table>");

    if l.compte.is_some() {
        // Numéro
        let mut sugg = String::new();
        for s in v.suggestions {
            let _ = write!(sugg, "<code>{}</code> ", esc(&numero_lisible(s)));
        }
        let _ = write!(
            h,
            r#"<h3>Changer de numéro</h3>{debut}<input type="hidden" name="nom" value="{nom}">
<p><label for="numero">Nouveau numéro</label><br><input type="text" id="numero" name="numero" required maxlength="14" inputmode="numeric" placeholder="01 23 45 67 89"> <button type="submit">Attribuer</button></p>
<p class="aide">10 chiffres, 01 à 07 ou 09, libre. L'abonné est déconnecté et reçoit son nouveau numéro en se reconnectant ; un courrier le prévient. Numéros libres au hasard : {sugg}</p></form>"#,
            debut = f.debut("numero"),
            nom = esc(nom)
        );
        // Mot de passe
        h.push_str("<h3>Mot de passe</h3><p>");
        if v.postier {
            h.push_str(&f.bouton("mdp/lien", &[("nom", nom)], "Envoyer un lien de réinitialisation", false));
            h.push(' ');
        }
        h.push_str(&f.bouton("mdp/provisoire", &[("nom", nom)], "Donner un mot de passe provisoire", true));
        h.push_str(r#"</p><p class="aide">Le lien (valable 1 h) part à l'adresse de l'abonné, comme « Mot de passe oublié ». Le mot de passe provisoire remplace l'actuel tout de suite : il s'affiche ici une seule fois, à vous de le lui transmettre.</p>"#);
    }

    // Facture et succès
    let total: u64 = v.tickets.iter().map(|t| t.centimes as u64).sum();
    let secondes: u64 = v.tickets.iter().map(|t| t.duree_s as u64).sum();
    let _ = write!(
        h,
        r#"<h3>Facture de {m}</h3><p>{n} : <b>{fr}</b> (soit {eu}). <a href="/admin/facture?nom={e}&amp;mois={mois}">Détail des communications</a></p>"#,
        m = nom_mois(v.mois),
        n = appels_duree(v.tickets.len(), secondes),
        fr = tarif::francs(total),
        eu = tarif::euros(total),
        e = enc(nom),
        mois = v.mois
    );
    if !v.autres_mois.is_empty() {
        let liens: Vec<String> = v.autres_mois.iter().map(|m| format!(r#"<a href="/admin/facture?nom={}&amp;mois={m}">{}</a>"#, enc(nom), nom_mois(*m))).collect();
        let _ = write!(h, "<p>Autres relevés : {}</p>", liens.join(" · "));
    }
    let eus = v.carnet.map(|c| c.succes.len()).unwrap_or(0);
    let _ = write!(h, "<h3>Succès ({eus}/{})</h3>", succes::LISTE.len());
    match v.carnet {
        Some(c) if !c.succes.is_empty() => {
            h.push_str(r#"<table class="grille succes">"#);
            let mut l: Vec<(&String, &u64)> = c.succes.iter().collect();
            l.sort_by_key(|(_, t)| **t);
            for (id, t) in l {
                let (titre, desc) = succes::trouver(id).map(|s| (s.titre, s.description)).unwrap_or((id.as_str(), ""));
                let _ = write!(h, r#"<tr class="eu"><td>★</td><td><b>{}</b></td><td>{}</td><td>{}</td></tr>"#, esc(titre), esc(desc), esc(&date_fr(*t)));
            }
            h.push_str("</table>");
            let _ = write!(h, r#"<p class="aide">{} de connexion, {} en ligne, {} appelé(s).</p>"#, pluriel(c.jours.len() as u64, "jour", "jours"), facture::duree(c.secondes), c.appeles.len());
        }
        _ => h.push_str("<p>Aucun pour l'instant.</p>"),
    }

    // Tinyblog et numéros cachés
    h.push_str("<h3>Tinyblog</h3>");
    match v.blog {
        Some(b) => {
            let coms: usize = b.articles.iter().map(|a| a.coms.len()).sum();
            let _ = write!(
                h,
                r#"<p>« {} » : {}, {} reçus, {} J'kiffe ; dernier article le {}. <a href="/admin/tinyblog?nom={}">Modérer</a></p><p>{}</p>"#,
                esc(if b.titre.is_empty() { "sans titre" } else { &b.titre }),
                pluriel(b.articles.len() as u64, "article", "articles"),
                pluriel(coms as u64, "com's", "com's"),
                b.kiffs(),
                esc(&date_fr(b.dernier())),
                enc(nom),
                f.bouton("tinyblog/supprimer", &[("nom", nom), ("retour", "abonne")], "Supprimer son Tinyblog", true)
            );
        }
        None => h.push_str("<p>Pas de Tinyblog.</p>"),
    }
    let _ = write!(h, r#"<h3>Numéros cachés</h3><p>{} laissé(s) sur les numéros cachés. <a href="/admin/caches">Voir</a></p>"#, pluriel(v.depots as u64, "message ou score", "messages ou scores"));
    if v.depots > 0 {
        let _ = write!(h, "<p>{}</p>", f.bouton("caches/supprimer-de", &[("nom", nom)], "Tout effacer", true));
    }

    // Suppression
    if l.compte.is_some() {
        let _ = write!(
            h,
            r#"<h3>Supprimer le compte</h3>{debut}<input type="hidden" name="nom" value="{nom}">
<p>Le compte est effacé du fichier des abonnés et le numéro <span class="num">{num}</span> libéré. Les tickets de communication et les succès restent (le central les tient).</p>
<p><label><input type="checkbox" name="tout" value="1"> Effacer aussi son Tinyblog et ce qu'il a laissé sur les numéros cachés</label><br>
<label><input type="checkbox" name="sur" value="1" required> sûr</label></p>
<p><button type="submit">Supprimer le compte</button></p></form>"#,
            debut = f.debut("supprimer"),
            nom = esc(nom),
            num = esc(&numero_lisible(&l.numero))
        );
    }
    h
}

pub fn inscriptions(f: &Formulaires, attente: &[Inscription], postier: bool) -> String {
    let mut h = format!(
        r#"<h3>Inscriptions en attente de confirmation de l'e-mail ({})</h3><p class="aide">Le nom, le numéro et l'adresse sont réservés 48 heures, le temps que le futur abonné clique sur le lien reçu. « Renvoyer » envoie un nouveau lien (l'ancien ne marche plus) et repousse l'échéance.</p>"#,
        attente.len()
    );
    if attente.is_empty() {
        h.push_str("<p>Aucune.</p>");
        return h;
    }
    h.push_str(r#"<table class="grille"><tr><th>Abonné</th><th>Numéro</th><th>Courriel</th><th>Expire le</th><th></th></tr>"#);
    for i in attente {
        let renvoyer = if postier { f.bouton("inscription/renvoyer", &[("nom", &i.nom)], "Renvoyer le lien", false) + " " } else { String::new() };
        let _ = write!(
            h,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{renvoyer}{}</td></tr>",
            esc(&i.nom),
            esc(&numero_lisible(&i.numero)),
            esc(&i.courriel),
            esc(&date_heure_fr(i.expire)),
            f.bouton("inscription/supprimer", &[("nom", &i.nom)], "Supprimer", true)
        );
    }
    h.push_str("</table>");
    h
}

// ───────────────────────── tableau de bord ─────────────────────────

pub struct Tableau<'a> {
    pub etat: Option<&'a Etat>,
    pub maintenant: u64,
    pub lignes: &'a [LigneAbonne],
    pub nb_inscriptions: usize,
    pub nb_a_relire: usize,
    /// (blogs, articles, commentaires)
    pub blogs: (usize, usize, usize),
    pub mois: u32,
    pub tickets: &'a [Ticket],
    /// Les inscriptions sont ouvertes ? Sinon, pourquoi.
    pub inscriptions: Result<(), String>,
}

/// Le pupitre du central : en service, qui est en ligne, qui appelle qui.
fn pupitre(etat: Option<&Etat>, maintenant: u64) -> String {
    let mut h = String::from(r#"<table class="pupitre"><tr><th colspan="3">PUPITRE DU CENTRAL</th></tr>"#);
    let ligne = |h: &mut String, voyants: &str, quoi: &str, valeur: &str| {
        let _ = write!(h, r#"<tr><td class="v">{voyants}</td><td>{quoi}</td><td>{valeur}</td></tr>"#);
    };
    let Some(e) = etat else {
        ligne(&mut h, &voyant("v-rouge clignote"), "Central", r#"<span class="hs">HORS SERVICE ou muet</span> : son fichier d'état est absent ou a plus de 30 s"#);
        h.push_str("</table>");
        return h;
    };
    ligne(&mut h, &voyant("v-vert"), "Central", &format!("<b>EN SERVICE</b> depuis le {} ({}) — version {}", esc(&date_heure_fr(e.depuis)), esc(&duree_fr(maintenant.saturating_sub(e.depuis))), esc(&e.version)));
    let noms = if e.en_ligne.is_empty() && e.connectes > 0 {
        " (noms inconnus : central trop ancien)".to_string()
    } else if e.en_ligne.is_empty() {
        String::new()
    } else {
        format!(" : {}", e.en_ligne.iter().map(|n| lien_abonne(n)).collect::<Vec<_>>().join(", "))
    };
    ligne(&mut h, &voyant(if e.connectes > 0 { "v-vert" } else { "v-eteint" }), "En ligne", &format!("<b>{}</b> sur {}{noms}", e.connectes, pluriel(e.abonnes, "abonné", "abonnés")));
    let appels = if e.appels == 0 {
        "aucun".to_string()
    } else if e.communications.is_empty() {
        format!("<b>{}</b>", pluriel(e.appels, "communication", "communications"))
    } else {
        e.communications
            .iter()
            .map(|c| {
                let quand = match c.decroche {
                    Some(t) => format!("depuis {}", esc(&duree_fr(maintenant.saturating_sub(t)))),
                    None => "ça sonne".into(),
                };
                format!("{} → {} ({}), {quand}", lien_abonne(&c.de), vers_lisible(&c.vers), esc(&numero_lisible(&c.numero)))
            })
            .collect::<Vec<_>>()
            .join("<br>")
    };
    ligne(&mut h, &voyant(if e.appels > 0 { "v-orange clignote" } else { "v-eteint" }), "Appels en cours", &appels);
    ligne(&mut h, &voyant(if e.modems.occupes < e.modems.max { "v-vert" } else { "v-rouge" }), "Modems du central", &format!("{} occupé(s) sur {}", e.modems.occupes, e.modems.max));
    ligne(&mut h, &voyant(if e.salon > 0 { "v-vert" } else { "v-eteint" }), "Salon du FAI", &pluriel(e.salon, "présent", "présents"));
    h.push_str("</table>");
    h
}

pub fn tableau(f: &Formulaires, t: &Tableau) -> String {
    let mut h = pupitre(t.etat, t.maintenant);
    let site: Vec<&LigneAbonne> = t.lignes.iter().filter(|l| l.compte.is_some()).collect();
    let config = t.lignes.len() - site.len();
    let fai = t.lignes.iter().filter(|l| l.fai).count();
    let a_decider: Vec<&LigneAbonne> = site.iter().copied().filter(|l| l.compte.as_ref().is_some_and(|c| !c.fai_decide)).collect();
    let suspendus = site.iter().filter(|l| l.compte.as_ref().is_some_and(|c| c.suspendu)).count();
    let inscriptions = match &t.inscriptions {
        Ok(()) => r#"<span class="voyant v-vert"></span> ouvertes"#.to_string(),
        Err(pourquoi) => format!(r#"<span class="voyant v-rouge"></span> <b>fermées</b> ({})"#, esc(pourquoi)),
    };
    let (blogs, articles, coms) = t.blogs;
    let _ = write!(
        h,
        r#"<h3>En chiffres</h3><table class="grille fiche">
<tr><th>Abonnés</th><td><a href="/admin/abonnes"><span class="adm-chiffre">{n}</span></a> : {site} inscrit(s) par le site, {config} dans config.toml</td></tr>
<tr><th>Accès Internet</th><td>{fai} accordé(s){decider}</td></tr>
<tr><th>Comptes suspendus</th><td>{suspendus}</td></tr>
<tr><th>Inscriptions en attente</th><td><a href="/admin/inscriptions">{ins}</a></td></tr>
<tr><th>Livre d'or</th><td><a href="/admin/livre">{relire} à relire</a></td></tr>
<tr><th>Tinyblog</th><td><a href="/admin/tinyblog">{blogs}</a>, {articles}, {coms}</td></tr>
<tr><th>Inscriptions</th><td>{inscriptions} — <a href="/admin/site">changer</a></td></tr></table>"#,
        n = t.lignes.len(),
        site = site.len(),
        decider = if a_decider.is_empty() { String::new() } else { format!(r#", <span class="nouveau">{} à décider</span>"#, a_decider.len()) },
        ins = t.nb_inscriptions,
        relire = t.nb_a_relire,
        blogs = pluriel(blogs as u64, "blog", "blogs"),
        articles = pluriel(articles as u64, "article", "articles"),
        coms = pluriel(coms as u64, "com's", "com's"),
    );

    // Le mois
    let total: u64 = t.tickets.iter().map(|x| x.centimes as u64).sum();
    let secondes: u64 = t.tickets.iter().map(|x| x.duree_s as u64).sum();
    let _ = write!(
        h,
        r#"<h3>{m}</h3><p>{n} passés sur le réseau, pour <b>{fr}</b> (soit {eu}) de factures. <a href="/admin/factures">Toutes les factures</a></p>"#,
        m = nom_mois(t.mois),
        n = pluriel(t.tickets.len() as u64, "appel", "appels") + &format!(" ({})", facture::duree(secondes)),
        fr = tarif::francs(total),
        eu = tarif::euros(total)
    );
    let top = classement(t.tickets);
    if !top.is_empty() {
        h.push_str(r#"<table class="grille classement">"#);
        for (i, (nom, (n, _, c))) in top.iter().take(5).enumerate() {
            let _ = write!(h, r#"<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>"#, i + 1, lien_abonne(nom), pluriel(*n as u64, "appel", "appels"), tarif::francs(*c));
        }
        h.push_str("</table>");
    }

    // À décider, puis les derniers inscrits
    if !a_decider.is_empty() {
        let _ = write!(h, r#"<h3>Accès Internet à décider ({})</h3><table class="grille"><tr><th>Abonné</th><th>Numéro</th><th>Courriel</th><th>Inscrit le</th><th>Internet</th></tr>"#, a_decider.len());
        for l in &a_decider {
            let c = l.compte.as_ref().expect("compte du site");
            let _ = write!(h, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", lien_abonne(&l.nom), esc(&numero_lisible(&l.numero)), esc(&c.courriel), esc(&date_fr(c.cree)), l.internet(f, "tableau"));
        }
        h.push_str("</table>");
    }
    let mut derniers = site.clone();
    derniers.sort_by_key(|l| std::cmp::Reverse(l.compte.as_ref().map_or(0, |c| c.cree)));
    if !derniers.is_empty() {
        h.push_str(r#"<h3>Derniers inscrits</h3><table class="grille"><tr><th>Abonné</th><th>Numéro</th><th>Inscrit le</th><th>Internet</th><th>État</th></tr>"#);
        for l in derniers.iter().take(5) {
            let c = l.compte.as_ref().expect("compte du site");
            let _ = write!(h, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>", lien_abonne(&l.nom), esc(&numero_lisible(&l.numero)), esc(&date_fr(c.cree)), l.internet(f, "tableau"), l.etat());
        }
        h.push_str("</table>");
    }
    h
}

/// Appelant → (appels, secondes, centimes), du plus gros au plus petit.
pub fn classement(tickets: &[Ticket]) -> Vec<(String, (usize, u64, u64))> {
    let mut m: HashMap<&str, (usize, u64, u64)> = HashMap::new();
    for t in tickets {
        let e = m.entry(t.appelant.as_str()).or_default();
        e.0 += 1;
        e.1 += t.duree_s as u64;
        e.2 += t.centimes as u64;
    }
    let mut v: Vec<(String, (usize, u64, u64))> = m.into_iter().map(|(n, x)| (n.to_string(), x)).collect();
    v.sort_by(|a, b| b.1 .2.cmp(&a.1 .2).then(b.1 .1.cmp(&a.1 .1)).then(a.0.cmp(&b.0)));
    v
}

// ───────────────────────── factures ─────────────────────────

pub fn factures(mois: u32, autres: &[u32], tickets: &[Ticket]) -> String {
    let total: u64 = tickets.iter().map(|t| t.centimes as u64).sum();
    let secondes: u64 = tickets.iter().map(|t| t.duree_s as u64).sum();
    let mut h = format!(r#"<h3>Factures de {}</h3>"#, nom_mois(mois));
    if !autres.is_empty() {
        let liens: Vec<String> = autres.iter().map(|m| format!(r#"<a href="/admin/factures?mois={m}">{}</a>"#, nom_mois(*m))).collect();
        let _ = write!(h, "<p>Autres mois : {}</p>", liens.join(" · "));
    }
    let _ = write!(
        h,
        r#"<table class="grille fiche"><tr><th>Communications</th><td>{}</td></tr><tr><th>Total facturé</th><td><b>{}</b> (soit {}) — jamais réclamé</td></tr></table>"#,
        appels_duree(tickets.len(), secondes),
        tarif::francs(total),
        tarif::euros(total)
    );
    let c = classement(tickets);
    if c.is_empty() {
        h.push_str("<p>Aucune communication ce mois-là.</p>");
        return h;
    }
    h.push_str(r#"<table class="grille classement"><tr><th>#</th><th>Abonné</th><th>Appels</th><th>Durée</th><th>Montant</th></tr>"#);
    for (i, (nom, (n, s, cts))) in c.iter().enumerate() {
        let _ = write!(
            h,
            r#"<tr><td>{}</td><td><a href="/admin/facture?nom={}&amp;mois={mois}">{}</a></td><td class="n">{n}</td><td class="n">{}</td><td>{}</td></tr>"#,
            i + 1,
            enc(nom),
            esc(nom),
            facture::duree(*s),
            tarif::francs(*cts)
        );
    }
    h.push_str("</table>");
    let mut journal: Vec<&Ticket> = tickets.iter().collect();
    journal.sort_by_key(|t| std::cmp::Reverse(t.debut));
    let _ = write!(h, "<h3>Journal des communications ({} dernières)</h3>", journal.len().min(100));
    h.push_str(r#"<table class="grille detail"><tr><th>Décroché le</th><th>Appelant</th><th>Numéro composé</th><th>Correspondant</th><th>Durée</th><th>Montant</th></tr>"#);
    for t in journal.iter().take(100) {
        let _ = write!(
            h,
            r#"<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class="montant">{}</td></tr>"#,
            esc(&date_heure_fr(t.debut)),
            lien_abonne(&t.appelant),
            esc(&numero_lisible(&t.numero)),
            esc(&facture::correspondant(t)),
            facture::hms(t.duree_s),
            tarif::francs(t.centimes as u64)
        );
    }
    h.push_str("</table>");
    h
}

// ───────────────────────── Tinyblog ─────────────────────────

fn extrait(t: &str, n: usize) -> String {
    let mut e: String = t.chars().take(n).collect();
    if t.chars().count() > n {
        e.push('…');
    }
    e
}

fn image(a: &Article, largeur: u32) -> String {
    match &a.image {
        Some(f) => format!(r#"<img src="/admin/tinyblog/img?f={}" alt="image de l'article" style="max-width:{largeur}px;width:100%;height:auto;image-rendering:pixelated">"#, enc(f)),
        None => String::new(),
    }
}

/// Une ligne de commentaire ; `avec_article` : sur la page de tous les
/// blogs (on dit sur quel article, et on y revient après la suppression).
fn ligne_com(f: &Formulaires, h: &mut String, blog: &str, a: &Article, k: usize, c: &Com, avec_article: bool) {
    let (sur, retour) = if avec_article {
        (format!(r#"<td><a href="/admin/tinyblog?nom={}">{}</a> : {}</td>"#, enc(blog), esc(blog), esc(&extrait(&a.titre, 40))), "tinyblog")
    } else {
        (String::new(), "blog")
    };
    let id = a.id.to_string();
    let k = k.to_string();
    let _ = write!(
        h,
        "<tr><td>{}</td><td>{}</td>{sur}<td>{}</td><td>{}</td></tr>",
        esc(&date_heure_fr(c.date)),
        lien_abonne(&c.de),
        texte_multiligne(&c.texte),
        f.bouton("tinyblog/com/supprimer", &[("nom", blog), ("id", &id), ("k", &k), ("e", &empreinte_com(c)), ("retour", retour)], "Supprimer", false)
    );
}

/// Tous les blogs, les derniers articles et les derniers commentaires.
pub fn tinyblogs(f: &Formulaires, blogs: &[(String, Blog)]) -> String {
    let mut h = format!(
        r#"<p class="aide">Les blogs des abonnés, sur <code>http://tinyblog/</code> par le modem. Ils ne sont pas relus avant publication : vous pouvez retirer un commentaire, un article (et son image), ou tout un blog.</p><h3>Les Tinyblogs ({})</h3>"#,
        blogs.len()
    );
    if blogs.is_empty() {
        h.push_str("<p>Aucun pour l'instant.</p>");
        return h;
    }
    h.push_str(r#"<table class="grille"><tr><th>Blog</th><th>Titre</th><th>Articles</th><th>Com's</th><th>J'kiffe</th><th>Dernier article</th></tr>"#);
    for (nom, b) in blogs {
        let coms: usize = b.articles.iter().map(|a| a.coms.len()).sum();
        let _ = write!(
            h,
            r#"<tr><td><a href="/admin/tinyblog?nom={}">{}</a></td><td>{}</td><td class="n">{}</td><td class="n">{coms}</td><td class="n">{}</td><td>{}</td></tr>"#,
            enc(nom),
            esc(nom),
            esc(&extrait(&b.titre, 60)),
            b.articles.len(),
            b.kiffs(),
            esc(&date_fr(b.dernier()))
        );
    }
    h.push_str("</table>");
    let mut articles: Vec<(&str, &Article)> = blogs.iter().flat_map(|(n, b)| b.articles.iter().map(move |a| (n.as_str(), a))).collect();
    articles.sort_by_key(|(_, a)| std::cmp::Reverse(a.date));
    if !articles.is_empty() {
        h.push_str(r#"<h3>Derniers articles</h3><table class="grille"><tr><th>Publié le</th><th>Blog</th><th>Article</th><th></th></tr>"#);
        for (nom, a) in articles.iter().take(20) {
            let id = a.id.to_string();
            let _ = write!(
                h,
                r#"<tr><td>{}</td><td><a href="/admin/tinyblog?nom={}">{}</a></td><td><b>{}</b><br>{}<br>{}</td><td>{}</td></tr>"#,
                esc(&date_heure_fr(a.date)),
                enc(nom),
                esc(nom),
                esc(&a.titre),
                texte_multiligne(&extrait(&a.texte, 300)),
                image(a, 160),
                f.bouton("tinyblog/article/supprimer", &[("nom", nom), ("id", &id), ("retour", "tinyblog")], "Supprimer", true)
            );
        }
        h.push_str("</table>");
    }
    let mut coms: Vec<(&str, &Article, usize, &Com)> = blogs.iter().flat_map(|(n, b)| b.articles.iter().flat_map(move |a| a.coms.iter().enumerate().map(move |(k, c)| (n.as_str(), a, k, c)))).collect();
    coms.sort_by_key(|(_, _, _, c)| std::cmp::Reverse(c.date));
    if !coms.is_empty() {
        h.push_str(r#"<h3>Derniers commentaires</h3><table class="grille"><tr><th>Laissé le</th><th>Par</th><th>Sur</th><th>Commentaire</th><th></th></tr>"#);
        for (nom, a, k, c) in coms.iter().take(30) {
            ligne_com(f, &mut h, nom, a, *k, c, true);
        }
        h.push_str("</table>");
    }
    h
}

/// Un blog en entier, pour la modération.
pub fn tinyblog(f: &Formulaires, nom: &str, b: &Blog) -> String {
    let mut h = format!(
        r#"<p><a href="/admin/tinyblog">« Tous les Tinyblogs</a></p><h3>Tinyblog de {}</h3><table class="grille fiche"><tr><th>Titre</th><td>{}</td></tr><tr><th>Présentation</th><td>{}</td></tr><tr><th>Thème</th><td>{}</td></tr><tr><th>Créé le</th><td>{}</td></tr><tr><th>Abonné</th><td>{}</td></tr></table><p>{} {}</p>"#,
        esc(nom),
        esc(&b.titre),
        texte_multiligne(&b.description),
        esc(&b.theme),
        esc(&date_fr(b.cree)),
        lien_abonne(nom),
        f.bouton("tinyblog/vider", &[("nom", nom)], "Effacer le titre et la présentation", false),
        f.bouton("tinyblog/supprimer", &[("nom", nom), ("retour", "tinyblog")], "Supprimer tout le blog", true)
    );
    if b.articles.is_empty() {
        h.push_str("<p>Aucun article.</p>");
        return h;
    }
    let mut articles: Vec<&Article> = b.articles.iter().collect();
    articles.sort_by_key(|a| std::cmp::Reverse(a.date));
    for a in articles {
        let id = a.id.to_string();
        let _ = write!(
            h,
            r#"<div class="msg"><div class="msg-t">Article n° {id}, le {} — {} J'kiffe — {}</div><b>{}</b><br>{}<br>{}</div>"#,
            esc(&date_heure_fr(a.date)),
            a.kiffs.len(),
            f.bouton("tinyblog/article/supprimer", &[("nom", nom), ("id", &id), ("retour", "blog")], "Supprimer l'article", true),
            esc(&a.titre),
            texte_multiligne(&a.texte),
            image(a, 320)
        );
        if !a.coms.is_empty() {
            h.push_str(r#"<table class="grille"><tr><th>Laissé le</th><th>Par</th><th>Commentaire</th><th></th></tr>"#);
            for (k, c) in a.coms.iter().enumerate() {
                ligne_com(f, &mut h, nom, a, k, c, false);
            }
            h.push_str("</table>");
        }
    }
    h
}

// ───────────────────────── numéros cachés ─────────────────────────

pub fn caches(f: &Formulaires, depots: &[Depot]) -> String {
    let mut h = String::from(r#"<p class="aide">Ce que les abonnés laissent sur les numéros cachés du central (messages, meilleurs scores), lu dans son dossier de données. Le central n'en garde que les derniers ; vous pouvez retirer ce qui ne va pas.</p>"#);
    for d in depots {
        let _ = write!(h, r#"<h3>{} ({})</h3>"#, esc(&d.libelle), d.entrees.len());
        if d.entrees.is_empty() {
            h.push_str("<p>Rien pour l'instant.</p>");
            continue;
        }
        h.push_str(r#"<table class="grille"><tr><th>Date</th><th>Abonné</th><th>Message ou score</th><th></th></tr>"#);
        // les plus récents d'abord
        for e in d.entrees.iter().rev() {
            let _ = write!(
                h,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                e.quand.map(|t| esc(&date_heure_fr(t))).unwrap_or_default(),
                e.de.as_deref().map(lien_abonne).unwrap_or_default(),
                esc(&e.texte),
                f.bouton("caches/supprimer", &[("fichier", &d.fichier), ("cle", &e.cle), ("e", &e.empreinte)], "Supprimer", false)
            );
        }
        h.push_str("</table>");
    }
    h
}

// ───────────────────────── livre d'or ─────────────────────────

pub fn livre(f: &Formulaires, a_relire: &[Message], publies: &[Message]) -> String {
    let mut h = String::new();
    let ligne = |h: &mut String, m: &Message, publier: bool| {
        let publier = if publier { f.bouton("livre/publier", &[("id", &m.id)], "Publier", false) + " " } else { String::new() };
        let _ = write!(
            h,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{publier}{}</td></tr>",
            esc(&date_heure_fr(m.date)),
            esc(&m.pseudo),
            texte_multiligne(&m.texte),
            f.bouton("livre/supprimer", &[("id", &m.id)], "Supprimer", false)
        );
    };
    let _ = write!(h, "<h3>Livre d'or : messages à relire ({})</h3>", a_relire.len());
    if a_relire.is_empty() {
        h.push_str("<p>Rien à relire.</p>");
    } else {
        h.push_str("<table class=\"grille\"><tr><th>Déposé le</th><th>Pseudo</th><th>Message</th><th></th></tr>");
        for m in a_relire {
            ligne(&mut h, m, true);
        }
        h.push_str("</table>");
    }
    if !publies.is_empty() {
        let _ = write!(h, "<h3>Livre d'or : derniers messages publiés ({})</h3><table class=\"grille\"><tr><th>Déposé le</th><th>Pseudo</th><th>Message</th><th></th></tr>", publies.len());
        for m in publies {
            ligne(&mut h, m, false);
        }
        h.push_str("</table>");
    }
    h
}

// ───────────────────────── le site ─────────────────────────

/// Une nouvelle de « Quoi de neuf ? » et d'où elle vient.
pub enum Origine {
    GitHub,
    /// Ajoutée depuis l'administration (son identifiant)
    Ajoutee(String),
    /// Écrite dans le code du site
    Code,
}

pub struct VueSite<'a> {
    /// Les inscriptions sont fermées par l'administrateur
    pub fermees: bool,
    /// Les e-mails partent-ils ?
    pub postier: bool,
    /// La configuration du central est-elle lisible ?
    pub config: bool,
    pub nouvelles: &'a [(Nouvelle, Origine)],
    /// Aujourd'hui (AAAA-MM-JJ, heure de Paris)
    pub aujourdhui: &'a str,
}

pub fn site(f: &Formulaires, v: &VueSite) -> String {
    let mut h = String::from("<h3>Inscriptions</h3>");
    let mut obstacles = Vec::new();
    if !v.postier {
        obstacles.push("l'envoi des e-mails n'est pas configuré (SMTP_HOST vide)");
    }
    if !v.config {
        obstacles.push("la configuration du central est illisible");
    }
    if v.fermees {
        let _ = write!(h, r#"<p><span class="voyant v-rouge"></span> <b>Fermées</b> par vous. {}</p>"#, f.bouton("site/inscriptions", &[("d", "ouvrir")], "Rouvrir les inscriptions", false));
    } else {
        let _ = write!(h, r#"<p><span class="voyant v-vert"></span> Ouvertes de votre côté. {}</p>"#, f.bouton("site/inscriptions", &[("d", "fermer")], "Fermer les inscriptions", false));
    }
    if !obstacles.is_empty() {
        let _ = write!(h, r#"<div class="erreur">Les inscriptions restent fermées tant que {}.</div>"#, obstacles.join(", et que "));
    }
    h.push_str(r#"<p class="aide">Fermées, la page d'inscription l'annonce et le bouton de l'accueil disparaît ; les inscriptions déjà en attente peuvent toujours être confirmées.</p>"#);

    let _ = write!(h, r#"<h3>« Quoi de neuf ? » sur l'accueil</h3><table class="grille"><tr><th>Date</th><th>Nouvelle</th><th>Origine</th></tr>"#);
    for (n, o) in v.nouvelles {
        let origine = match o {
            Origine::GitHub => "version publiée sur GitHub".to_string(),
            Origine::Code => "écrite dans le code du site".to_string(),
            Origine::Ajoutee(id) => format!("ajoutée ici {}", f.bouton("site/nouvelle/supprimer", &[("id", id)], "Supprimer", false)),
        };
        let date = n.date.get(8..10).zip(n.date.get(5..7)).zip(n.date.get(0..4)).map(|((j, m), a)| format!("{j}/{m}/{a}")).unwrap_or_else(|| n.date.clone());
        let _ = write!(h, "<tr><td>{}</td><td>{}</td><td>{origine}</td></tr>", esc(&date), esc(&n.titre));
    }
    let _ = write!(
        h,
        r#"</table>{debut}<p><label for="date">Date</label><br><input type="text" id="date" name="date" value="{d}" required maxlength="10" pattern="[0-9]{{4}}-[0-9]{{2}}-[0-9]{{2}}" style="max-width:9em"></p>
<p><label for="titre">Nouvelle</label><br><input type="text" id="titre" name="titre" required maxlength="{max}" style="max-width:100%"></p>
<p><button type="submit">Ajouter à « Quoi de neuf ? »</button> <span class="aide">Une ligne, sans HTML. La plus récente s'affiche en premier, avec « NOUVEAU ! ».</span></p></form>"#,
        debut = f.debut("site/nouvelle"),
        d = esc(v.aujourdhui),
        max = super::site::TITRE_MAX
    );
    h
}
