//! Pages du site, façon fournisseur d'accès de 1998 : tableaux, fond gris,
//! Times, GIF « en construction », compteur de visites, bandeau défilant…
//! (mais lisible sur un téléphone de 2026).

use std::fmt::Write;

/// Échappe un texte pour le HTML (contenu et attributs).
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

/// « 0142424242 » → « 01 42 42 42 42 »
pub fn numero_lisible(n: &str) -> String {
    let mut o = String::new();
    for (i, c) in n.chars().enumerate() {
        if i > 0 && i % 2 == 0 {
            o.push(' ');
        }
        o.push(c);
    }
    o
}

const STYLE: &str = r#"
body{margin:0;background:#c0c0c0 url("data:image/gif;base64,R0lGODlhBAAEAIAAAKysrMDAwCH5BAAAAAAALAAAAAAEAAQAAAIFhI8Wm1kAOw==");color:#000;font:16px/1.35 "Times New Roman",Times,serif}
a{color:#0000ee}a:visited{color:#551a8b}
.page{max-width:780px;margin:0 auto;background:#fff;border-left:2px solid #808080;border-right:2px solid #808080;min-height:100vh}
.tete{background:#000080;color:#fff;padding:10px 14px;border-bottom:4px ridge #c0c0c0}
.tete h1{margin:0;font:bold 30px Georgia,"Times New Roman",serif;letter-spacing:1px}
.tete h1 span{color:#ffff00}
.tete p{margin:2px 0 0;font:italic 14px Georgia,serif;color:#c0c0ff}
.menu{background:#c0c0c0;border-bottom:2px solid #808080;padding:4px 8px;font:bold 13px Verdana,Arial,sans-serif}
.menu a{color:#000080;margin-right:12px;text-decoration:none}.menu a:hover{text-decoration:underline}
.defile{background:#000;color:#00ff00;font:14px "Courier New",monospace;white-space:nowrap;overflow:hidden;padding:3px 0}
.defile span{display:inline-block;padding-left:100%;animation:defile 28s linear infinite}
@keyframes defile{to{transform:translateX(-100%)}}
.corps{padding:12px 18px 20px}
h2{font:bold 22px Georgia,serif;color:#800000;border-bottom:1px solid #808080;margin:18px 0 8px}
h3{font:bold 17px Georgia,serif;color:#000080;margin:14px 0 4px}
.nouveau{color:#ff0000;font:bold 12px Verdana,sans-serif;animation:clignote 1s steps(1) infinite}
@keyframes clignote{50%{visibility:hidden}}
.boite{border:3px outset #c0c0c0;background:#ffffcc;padding:8px 12px;margin:10px 0}
.erreur{border:3px outset #c0c0c0;background:#ffdddd;padding:8px 12px;margin:10px 0;color:#800000}
.ok{border:3px outset #c0c0c0;background:#ddffdd;padding:8px 12px;margin:10px 0}
table.grille{border-collapse:collapse;width:100%;margin:8px 0;background:#fff}
table.grille td,table.grille th{border:1px solid #808080;padding:4px 6px;text-align:left;vertical-align:top}
table.grille th{background:#000080;color:#fff;font:bold 13px Verdana,sans-serif}
.bouton,button{display:inline-block;font:bold 14px Verdana,Arial,sans-serif;color:#000;background:#c0c0c0;border:3px outset #fff;padding:4px 14px;text-decoration:none;cursor:pointer}
.bouton:active,button:active{border-style:inset}
.gros{font-size:18px;padding:8px 22px;background:#ffcc00}
input[type=text],input[type=email],input[type=password]{font:16px "Courier New",monospace;border:2px inset #c0c0c0;padding:3px 5px;width:100%;max-width:320px;box-sizing:border-box}
label{font:bold 14px Verdana,sans-serif}
.aide{font:12px Verdana,sans-serif;color:#555}
.num{font:bold 20px "Courier New",monospace;letter-spacing:1px}
.pied{border-top:2px solid #808080;padding:10px 18px;font:12px Verdana,sans-serif;color:#444;text-align:center;background:#eee}
.cache{position:absolute;left:-9999px;width:1px;height:1px;overflow:hidden}
code{font:15px "Courier New",monospace;background:#eee;padding:0 3px}
@media(max-width:600px){.tete h1{font-size:22px}.corps{padding:10px}}
"#;

/// Gabarit commun.
pub fn gabarit(nom_fai: &str, titre: &str, visites: u64, contenu: &str) -> String {
    let nom = esc(nom_fai);
    let (debut, fin) = match nom_fai.split_once(' ') {
        Some((a, b)) => (esc(a), esc(b)),
        None => (nom.clone(), String::new()),
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="fr">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{titre} — {nom}</title>
<style>{STYLE}</style>
</head>
<body>
<div class="page">
<div class="tete"><h1>{debut} <span>{fin}</span></h1><p>Votre accès à l'autoroute de l'information… à 28 800 bit/s</p></div>
<div class="menu"><a href="/">Accueil</a><a href="/inscription">Inscription</a><a href="/telechargement">Logiciel</a><a href="/aide">Aide</a></div>
<div class="defile"><span>*** NOUVEAU : modems V.34 jusqu'à 33 600 bit/s ! *** Inscription GRATUITE, sans frais de mise en service *** Le salon de discussion du FAI est ouvert 24 h/24 *** Mode « web 1998 » : Internet comme avant, en plus léger *** Choisissez votre numéro de téléphone ! ***</span></div>
<div class="corps">
{contenu}
</div>
<div class="pied">
<img src="/compteur.gif?n={visites}" alt="{visites} visites" width="88" height="20"><br>
Vous êtes le visiteur n° {visites} depuis le dernier redémarrage du serveur.<br>
Site optimisé pour Netscape Navigator 4.0 en 800×600, 256 couleurs.<br>
{nom} est un logiciel libre : vous pouvez <a href="/aide#heberger">héberger votre propre central</a>.
</div>
</div>
</body>
</html>"#,
        titre = esc(titre),
    )
}

pub fn accueil(nom_fai: &str, central: &str, inscriptions_ouvertes: bool) -> String {
    let mut h = String::new();
    let _ = write!(
        h,
        r#"<p><img src="/construction.gif" alt="En construction" width="88" height="31" style="float:right;margin:0 0 8px 8px">
<b>Bienvenue chez {nom} !</b> Ici, pas de fibre ni de 5G : votre ordinateur se transforme en
<b>modem V.34</b>, le vrai, celui qui chante. Vous recevez <b>votre propre numéro de téléphone</b>,
vous appelez les autres abonnés pour discuter et échanger des fichiers, et vous composez le
<b>08 36 65 65 65</b> pour surfer sur Internet à travers votre modem.</p>
<h2>Ce que comprend votre abonnement <span class="nouveau">NOUVEAU !</span></h2>
<table class="grille">
<tr><th>Service</th><th>Description</th></tr>
<tr><td>☎ Votre numéro</td><td>Choisissez-le vous-même à l'inscription (10 chiffres, comme en 1998).</td></tr>
<tr><td>💬 Chat &amp; fichiers</td><td>Appelez un autre abonné : négociation V.8/V.34 audible, puis discussion et transfert de fichiers.</td></tr>
<tr><td>🌐 Accès Internet</td><td>Le modem du FAI décroche et vous donne accès au Web, en mode <i>moderne</i> ou <i>web 1998</i> (pages allégées, images en 256 couleurs). Sur validation de l'opérateur.</td></tr>
<tr><td>🗨 Salon du FAI</td><td>Tous les abonnés connectés au FAI discutent ensemble.</td></tr>
<tr><td>📞 Ligne de test</td><td>Le 08 00 00 00 00 vous renvoie tout ce que vous lui envoyez.</td></tr>
</table>
<h2>Tarifs</h2>
<table class="grille">
<tr><td>Abonnement mensuel</td><td><b>0 F</b> (0,00 €)</td></tr>
<tr><td>Communication, heures pleines</td><td><b>0 F</b> la minute</td></tr>
<tr><td>Communication, heures creuses</td><td><b>0 F</b> la minute, mais avec la satisfaction d'avoir attendu</td></tr>
<tr><td>Frais de résiliation</td><td>Un pincement au cœur</td></tr>
</table>
<p style="text-align:center;margin:22px 0">"#,
        nom = esc(nom_fai)
    );
    if inscriptions_ouvertes {
        h.push_str(r#"<a class="bouton gros" href="/inscription">» Je m'inscris gratuitement «</a> &nbsp; "#);
    }
    let _ = write!(
        h,
        r#"<a class="bouton gros" href="/telechargement">Télécharger le logiciel</a></p>
<div class="boite"><b>Adresse du central :</b> <code>{central}</code> — c'est elle que vous saisirez dans le logiciel, avec votre nom d'abonné et votre mot de passe.</div>"#,
        central = esc(central)
    );
    if !inscriptions_ouvertes {
        h.push_str(r#"<div class="erreur">Les inscriptions sont momentanément fermées (le facteur électronique est en grève). Revenez plus tard !</div>"#);
    }
    h
}

/// Valeurs du formulaire d'inscription (pour le réafficher après une erreur).
#[derive(Default)]
pub struct Formulaire {
    pub nom: String,
    pub courriel: String,
    pub numero: String,
}

pub fn inscription(f: &Formulaire, erreurs: &[String], suggestions: &[String], jeton_temps: &str) -> String {
    let mut h = String::from("<h2>Inscription</h2>");
    if !erreurs.is_empty() {
        h.push_str("<div class=\"erreur\"><b>Votre formulaire contient des erreurs :</b><ul>");
        for e in erreurs {
            let _ = write!(h, "<li>{}</li>", esc(e));
        }
        h.push_str("</ul></div>");
    }
    let mut sugg = String::new();
    for s in suggestions {
        let _ = write!(sugg, "<code>{}</code> ", esc(&numero_lisible(s)));
    }
    let _ = write!(
        h,
        r#"<p>Remplissez ce formulaire : vous recevrez un <b>courrier électronique</b> pour confirmer votre adresse.
Votre numéro est réservé dès l'envoi, et définitivement attribué à la confirmation.</p>
<form method="post" action="/inscription" autocomplete="off">
<input type="hidden" name="t" value="{t}">
<div class="cache"><label>Ne pas remplir <input type="text" name="site" tabindex="-1"></label></div>
<p><label for="nom">Nom d'abonné</label><br>
<input type="text" id="nom" name="nom" value="{nom}" required maxlength="32" pattern="[A-Za-z0-9._\-]+" autocapitalize="none" spellcheck="false"><br>
<span class="aide">1 à 32 caractères : lettres sans accent, chiffres, point, tiret, souligné. C'est votre identifiant de connexion.</span></p>
<p><label for="numero">Numéro de téléphone souhaité</label><br>
<input type="text" id="numero" name="numero" value="{numero}" required maxlength="14" inputmode="numeric" placeholder="01 23 45 67 89"><br>
<span class="aide">10 chiffres commençant par 01 à 07 ou 09 (les 08 sont réservés aux services). Numéros libres au hasard : {sugg}</span></p>
<p><label for="courriel">Adresse électronique</label><br>
<input type="email" id="courriel" name="courriel" value="{courriel}" required maxlength="254"><br>
<span class="aide">Pour confirmer l'inscription et vous prévenir quand l'accès Internet est activé. Jamais affichée ni revendue (on n'est pas en 2026).</span></p>
<p><label for="mdp">Mot de passe</label><br>
<input type="password" id="mdp" name="mdp" required minlength="8" maxlength="128"><br>
<label for="mdp2">Mot de passe (encore)</label><br>
<input type="password" id="mdp2" name="mdp2" required minlength="8" maxlength="128"><br>
<span class="aide">8 caractères au moins. Il ne circule jamais en clair : le central vérifie une preuve, pas le mot de passe.</span></p>
<p><label><input type="checkbox" name="cgu" value="1" required> J'accepte les conditions générales : je n'utiliserai pas cet accès pour des activités illégales, et je ne monopoliserai pas la ligne pendant que maman attend un coup de fil.</label></p>
<p><button type="submit" class="gros">Envoyer mon inscription</button></p>
</form>"#,
        t = esc(jeton_temps),
        nom = esc(&f.nom),
        numero = esc(&f.numero),
        courriel = esc(&f.courriel),
    );
    h
}

pub fn inscription_envoyee(courriel: &str, numero: &str) -> String {
    format!(
        r#"<h2>Vérifiez votre boîte aux lettres électronique !</h2>
<div class="ok">Un courrier vient de partir vers <b>{c}</b>. Cliquez sur le lien qu'il contient
<b>dans les 48 heures</b> pour activer votre abonnement et obtenir le numéro <span class="num">{n}</span>.</div>
<p>Rien reçu ? Regardez dans les indésirables : nos modems sont parfois pris pour des spammeurs.</p>"#,
        c = esc(courriel),
        n = esc(&numero_lisible(numero))
    )
}

pub fn confirmer_inscription(jeton: &str, nom: &str, numero: &str) -> String {
    format!(
        r#"<h2>Activer votre abonnement</h2>
<p>Abonné <b>{nom}</b>, numéro <span class="num">{n}</span>.</p>
<form method="post" action="/valider"><input type="hidden" name="t" value="{t}">
<p><button type="submit" class="gros">Activer mon abonnement</button></p></form>"#,
        nom = esc(nom),
        n = esc(&numero_lisible(numero)),
        t = esc(jeton)
    )
}

pub fn bienvenue(nom: &str, numero: &str, central: &str, acces_en_attente: bool) -> String {
    let mut h = format!(
        r#"<h2>Bienvenue, {nom} !</h2>
<div class="ok">Votre abonnement est activé. Votre numéro : <span class="num">{n}</span></div>
<h3>Pour vous connecter</h3>
<ol>
<li><a href="/telechargement">Téléchargez le logiciel</a> et lancez-le.</li>
<li>Cliquez sur le badge <b>BOUCLE LOCALE</b> en haut à droite.</li>
<li>Serveur : <code>{c}</code> — Abonné : <code>{nom}</code> — votre mot de passe. Cochez « mémoriser » pour vous connecter tout seul au démarrage.</li>
<li>Le badge affiche <b>RÉSEAU · N° {n}</b> : vous êtes en ligne. Appelez un abonné de l'<b>ANNUAIRE</b>, ou la ligne de test <b>08 00 00 00 00</b>.</li>
</ol>"#,
        nom = esc(nom),
        n = esc(&numero_lisible(numero)),
        c = esc(central)
    );
    if acces_en_attente {
        h.push_str(r#"<div class="boite"><b>Accès Internet :</b> en attente de validation par l'opérateur. Vous recevrez un courrier dès qu'il sera activé ; en attendant, le 08 36 65 65 65 sonne occupé.</div>"#);
    }
    h
}

pub fn confirmer_decision(nom: &str, numero: &str, courriel: &str, oui: bool, champs: &str) -> String {
    format!(
        r#"<h2>Accès Internet de {nom}</h2>
<p>Abonné <b>{nom}</b> ({c}), numéro <span class="num">{n}</span>.</p>
<form method="post" action="/decision">{champs}
<p><button type="submit" class="gros">{action}</button></p></form>"#,
        nom = esc(nom),
        c = esc(courriel),
        n = esc(&numero_lisible(numero)),
        action = if oui { "Accorder l'accès Internet" } else { "Refuser l'accès Internet" }
    )
}

pub fn message(titre: &str, texte: &str, ok: bool) -> String {
    format!(r#"<h2>{}</h2><div class="{}">{}</div><p><a href="/">Retour à l'accueil</a></p>"#, esc(titre), if ok { "ok" } else { "erreur" }, esc(texte))
}

/// Une ligne de la page de téléchargement.
pub struct Fichier {
    pub systeme: &'static str,
    pub nom: String,
    pub url: String,
    pub taille: u64,
}

pub fn telechargement(version: Option<(&str, &str)>, fichiers: &[Fichier], depot: &str) -> String {
    let mut h = String::from("<h2>Télécharger le logiciel</h2>");
    match version {
        Some((v, page)) if !fichiers.is_empty() => {
            let _ = write!(h, r#"<p>Version <b>{}</b> — <a href="{}">notes de version</a></p><table class="grille"><tr><th>Système</th><th>Fichier</th><th>Taille</th><th>À 28 800 bit/s</th></tr>"#, esc(v), esc(page));
            for f in fichiers {
                let secondes = f.taille as f64 * 8.0 / 28_800.0;
                let duree = if secondes >= 3600.0 { format!("{:.0} h {:02.0} min", (secondes / 3600.0).floor(), (secondes % 3600.0) / 60.0) } else { format!("{:.0} min", (secondes / 60.0).ceil()) };
                let _ = write!(
                    h,
                    r#"<tr><td><b>{}</b></td><td><a href="{}">{}</a></td><td>{:.1} Mo</td><td>{}</td></tr>"#,
                    esc(f.systeme),
                    esc(&f.url),
                    esc(&f.nom),
                    f.taille as f64 / 1_048_576.0,
                    duree
                );
            }
            h.push_str("</table><p class=\"aide\">(La dernière colonne est donnée à titre nostalgique : le téléchargement, lui, passe par votre vraie connexion.)</p>");
        }
        _ => h.push_str(r#"<div class="boite">Le logiciel est en cours de pressage sur CD-ROM… Revenez bientôt !</div>"#),
    }
    h.push_str(
        r#"<h3>Macintosh</h3><p>Le logiciel n'est pas signé par Apple : au premier lancement, faites <b>clic droit &gt; Ouvrir</b>,
ou dans le Terminal : <code>xattr -dr com.apple.quarantine "/Applications/V34 Modem.app"</code></p>
<h3>Windows</h3><p>Windows SmartScreen peut afficher « Windows a protégé votre ordinateur » : cliquez sur <b>Informations complémentaires &gt; Exécuter quand même</b>.</p>
<h3>Linux</h3><p>Rendez le fichier .AppImage exécutable (<code>chmod +x</code>), ou installez le .deb.</p>"#,
    );
    if !depot.is_empty() {
        let _ = write!(h, r#"<p>Code source et anciennes versions : <a href="https://github.com/{d}">github.com/{d}</a></p>"#, d = esc(depot));
    }
    h
}

pub fn aide(central: &str, depot: &str) -> String {
    let src = if depot.is_empty() { String::new() } else { format!(r#" Le code est sur <a href="https://github.com/{d}">github.com/{d}</a>."#, d = esc(depot)) };
    format!(
        r#"<h2>Aide</h2>
<h3>Comment ça marche ?</h3>
<p>Le logiciel contient un vrai modem V.34 : il fabrique le son d'un modem de 1998, et ce son est transporté par Internet
jusqu'au central (<code>{c}</code>), qui le relaie vers la personne que vous appelez, ou vers le modem du fournisseur d'accès.
Tout ce que vous voyez passer est réellement modulé et démodulé.</p>
<h3>Je n'arrive pas à me connecter</h3>
<ul><li>Vérifiez le nom d'abonné (sensible aux majuscules) et le mot de passe.</li>
<li>Le central utilise le port <b>UDP 3434</b> : certains réseaux d'entreprise le bloquent.</li></ul>
<h3>Le 08 36 65 65 65 sonne occupé</h3>
<p>Votre accès Internet n'est pas encore activé par l'opérateur, ou tous les modems du FAI sont occupés.</p>
<h3 id="heberger">Héberger votre propre central</h3>
<p>Le central, le FAI et ce site sont des logiciels libres (licence MIT).{src}
Suivez <code>deploy/README.md</code> : un VPS sous Linux suffit.</p>"#,
        c = esc(central)
    )
}

pub fn admin_connexion(admin: &str, erreur: Option<&str>) -> String {
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

/// Ligne du tableau d'administration.
pub struct LigneAdmin {
    pub nom: String,
    pub numero: String,
    pub courriel: String,
    pub cree: String,
    pub fai: bool,
    pub decide: bool,
}

pub fn admin_tableau(csrf: &str, comptes: &[LigneAdmin], attente: &[(String, String, String)], config: &[(String, String)], info: Option<&str>) -> String {
    let mut h = String::from("<h2>Administration</h2>");
    if let Some(i) = info {
        let _ = write!(h, r#"<div class="ok">{}</div>"#, esc(i));
    }
    // Pas de JavaScript (la politique de sécurité du site l'interdit) : la
    // suppression demande de cocher une case.
    let bouton = |action: &str, nom: &str, champ: &str, texte: &str, confirm: bool| {
        format!(
            r#"<form method="post" action="/admin/{action}" style="display:inline"><input type="hidden" name="csrf" value="{csrf}"><input type="hidden" name="nom" value="{nom}">{champ}{coche}<button type="submit">{texte}</button></form>"#,
            csrf = esc(csrf),
            nom = esc(nom),
            coche = if confirm { r#"<label class="aide"><input type="checkbox" name="sur" value="1" required>sûr</label> "# } else { "" }
        )
    };
    let _ = write!(h, "<h3>Abonnés inscrits par le site ({})</h3>", comptes.len());
    if comptes.is_empty() {
        h.push_str("<p>Aucun pour l'instant.</p>");
    } else {
        h.push_str("<table class=\"grille\"><tr><th>Abonné</th><th>Numéro</th><th>Courriel</th><th>Inscrit le</th><th>Internet</th><th></th></tr>");
        for c in comptes {
            let etat = if c.fai { "<b style=\"color:green\">oui</b>" } else if c.decide { "refusé" } else { "<span class=\"nouveau\">à décider</span>" };
            let action = if c.fai {
                bouton("fai", &c.nom, r#"<input type="hidden" name="d" value="non">"#, "Retirer", false)
            } else {
                bouton("fai", &c.nom, r#"<input type="hidden" name="d" value="oui">"#, "Accorder", false)
            };
            let _ = write!(
                h,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{etat}</td><td>{action} {}</td></tr>",
                esc(&c.nom),
                esc(&numero_lisible(&c.numero)),
                esc(&c.courriel),
                esc(&c.cree),
                bouton("supprimer", &c.nom, "", "Supprimer", true)
            );
        }
        h.push_str("</table>");
    }
    if !attente.is_empty() {
        let _ = write!(h, "<h3>Inscriptions en attente de confirmation de l'e-mail ({})</h3><table class=\"grille\"><tr><th>Abonné</th><th>Numéro</th><th>Courriel</th></tr>", attente.len());
        for (n, num, c) in attente {
            let _ = write!(h, "<tr><td>{}</td><td>{}</td><td>{}</td></tr>", esc(n), esc(&numero_lisible(num)), esc(c));
        }
        h.push_str("</table>");
    }
    let _ = write!(h, "<h3>Abonnés de la configuration du serveur ({})</h3><p class=\"aide\">Gérés avec <code>v34-server ajouter-abonne</code> / <code>retirer-abonne</code>.</p><table class=\"grille\"><tr><th>Abonné</th><th>Numéro</th></tr>", config.len());
    for (n, num) in config {
        let _ = write!(h, "<tr><td>{}</td><td>{}</td></tr>", esc(n), esc(&numero_lisible(num)));
    }
    let _ = write!(
        h,
        r#"</table><form method="post" action="/admin/deconnexion"><input type="hidden" name="csrf" value="{}"><p><button type="submit">Se déconnecter</button></p></form>"#,
        esc(csrf)
    );
    h
}
