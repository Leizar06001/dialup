//! Pages du site, façon fournisseur d'accès de 1998 : tableaux, fond gris,
//! Times, GIF « en construction », compteur de visites, bandeau défilant…
//! (mais lisible sur un téléphone de 2026).

use std::fmt::Write;

use super::etat::Etat;
use super::livre::Message;

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
.defile span{display:inline-block;padding-left:100%;animation:defile 45s linear infinite}
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
textarea{font:15px "Courier New",monospace;border:2px inset #c0c0c0;padding:3px 5px;width:100%;max-width:520px;box-sizing:border-box}
table.fiche th{width:38%}
.releve{border:2px dashed #808080;background:#fdfdf5;padding:8px 12px;margin:10px 0}
.releve-tete{font:13px "Courier New",monospace;border-bottom:1px dotted #808080;padding-bottom:6px;margin-bottom:6px}
table.detail{font-size:13px}
table.detail td.montant,table.classement td:last-child{text-align:right;white-space:nowrap}
table.succes td:first-child,table.classement td:first-child{width:1.5em;text-align:center}
table.succes tr.eu td:first-child{color:#cc9900}
table.succes tr.pas-eu{color:#808080}
table.classement tr.moi{background:#ffffcc;font-weight:bold}
.adm-menu{background:#000080;border:3px outset #c0c0c0;padding:4px 6px;margin:8px 0;font:bold 13px Verdana,Arial,sans-serif;line-height:1.9}
.adm-menu a{color:#fff;text-decoration:none;margin-right:10px;white-space:nowrap}.adm-menu a:hover{text-decoration:underline}
.adm-menu a.ici{color:#ffff00}.adm-menu form{display:inline}.adm-menu button{font-size:11px;padding:1px 6px}
.adm-chiffre{font:bold 22px "Courier New",monospace;color:#800000}
table.grille td.n{text-align:right;white-space:nowrap}
.adm-form{border:2px groove #c0c0c0;background:#f4f4f4;padding:6px 10px;margin:8px 0}
.adm-form p{margin:4px 0}
.msg{border:2px groove #c0c0c0;background:#fffff0;padding:6px 10px;margin:8px 0;overflow-wrap:anywhere}
.msg-t{font:12px Verdana,sans-serif;color:#555;border-bottom:1px dotted #c0c0c0;margin-bottom:4px}
.pied p{margin:6px 0}
.webring{display:inline-block;background:#000033;color:#ffff99;border:4px ridge #c0c0c0;padding:5px 12px;margin:8px 0;font:bold 12px Verdana,sans-serif}
.webring a{color:#00ffff}.webring a:visited{color:#ff99ff}
.wr-titre{font:bold 13px Georgia,serif;color:#ff9900;letter-spacing:1px}
.badges{margin:8px 0 4px;line-height:0}
.badge{display:inline-block;position:relative;width:88px;height:31px;box-sizing:border-box;margin:2px;overflow:hidden;vertical-align:middle;text-decoration:none;text-align:left;border:1px solid #000;line-height:normal;white-space:nowrap}
.badge span{position:absolute;display:block}
.b-ns{background:linear-gradient(#000,#00003a)}
.b-ns .i{left:3px;top:3px;width:23px;height:23px;background:linear-gradient(135deg,#7f9cff,#0a1a7a);color:#fff;font:bold 20px/23px Georgia,serif;text-align:center;border:1px solid #9ab;box-sizing:border-box}
.b-ns .e{left:19px;top:5px;width:4px;height:4px;background:#fff;border-radius:50%;box-shadow:0 0 3px 2px #fff}
.b-ns .h{left:31px;top:2px;font:bold 9px Verdana,sans-serif;color:#ccc}
.b-ns .b{left:30px;top:11px;font:italic bold 15px Georgia,serif;color:#ffff00}
.b-bn{background:#fff}
.b-bn .i{left:5px;top:4px;width:15px;height:21px;border:1px solid #000;background:repeating-linear-gradient(#fff 0 3px,#99a 3px 4px);box-sizing:border-box;border-top:4px solid #000080}
.b-bn .h{left:25px;top:3px;font:9px Verdana,sans-serif;color:#000}
.b-bn .b{left:25px;top:14px;font:bold 10px Verdana,sans-serif;color:#000080}
.b-res{background:#008080}
.b-res .i{left:4px;top:4px;width:22px;height:16px;background:#000080;border:2px solid #c0c0c0;box-sizing:border-box}
.b-res .e{left:11px;top:20px;width:8px;height:5px;background:#c0c0c0}
.b-res .h{left:31px;top:2px;font:9px Verdana,sans-serif;color:#fff}
.b-res .b{left:30px;top:12px;font:bold 13px "Courier New",monospace;color:#ffff00}
.b-v34{background:#fff}
.b-v34 .i{left:3px;top:3px;width:78px;height:21px;border:2px solid #0060a8;border-right-color:transparent;border-radius:50%;transform:rotate(-7deg)}
.b-v34 .h{left:14px;top:4px;font:9px Verdana,sans-serif;color:#333}
.b-v34 .b{left:30px;top:12px;font:italic bold 14px Georgia,serif;color:#0060a8}
kbd{font:bold 11px Verdana,sans-serif;border:2px outset #fff;background:#c0c0c0;padding:0 3px}
.voyant{display:inline-block;width:12px;height:12px;border-radius:50%;border:1px solid #111;vertical-align:-1px;margin:0 2px}
.v-vert{background:radial-gradient(circle at 35% 35%,#dfd,#2c2 45%,#060);box-shadow:0 0 5px #0f0}
.v-rouge{background:radial-gradient(circle at 35% 35%,#fdd,#e22 45%,#600);box-shadow:0 0 5px #f00}
.v-orange{background:radial-gradient(circle at 35% 35%,#ffd,#fa0 45%,#740);box-shadow:0 0 5px #fa0}
.v-eteint{background:radial-gradient(circle at 35% 35%,#888,#333 60%)}
.clignote{animation:clignote 1s steps(1) infinite}
table.pupitre{border-collapse:collapse;width:100%;margin:8px 0;background:#1c1c1c;color:#33ff33;font:14px "Courier New",monospace;border:4px ridge #a0a0a0}
table.pupitre td,table.pupitre th{border:1px solid #444;padding:5px 8px;text-align:left;vertical-align:middle}
table.pupitre th{background:#3a3a3a;color:#ffcc00;font:bold 13px Verdana,sans-serif;letter-spacing:1px}
table.pupitre td.v{width:76px;line-height:16px}
table.pupitre .hs{color:#ff4444;font-weight:bold}
table.pupitre a{color:#00ffff}
.quoi{margin:4px 0 0;padding-left:22px}
@media(max-width:600px){.tete h1{font-size:22px}.corps{padding:10px}table.pupitre{font-size:13px}}
"#;

/// Gabarit commun.
pub fn gabarit(nom_fai: &str, titre: &str, visites: u64, contenu: &str) -> String {
    let nom = esc(nom_fai);
    let (debut, fin) = match nom_fai.split_once(' ') {
        Some((a, b)) => (esc(a), esc(b)),
        None => (nom.clone(), String::new()),
    };
    let compteur = format!("{visites:06}");
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
<div class="tete"><h1>{debut} <span>{fin}</span></h1><p>Votre accès à l'autoroute de l'information… jusqu'à 56 000 bit/s</p></div>
<div class="menu"><a href="/">Accueil</a><a href="/inscription">Inscription</a><a href="/compte">Mon compte</a><a href="/telechargement">Logiciel</a><a href="/etat">État du réseau</a><a href="/livre">Livre d'or</a><a href="/aide">Aide</a></div>
<div class="defile"><span>*** NOUVEAU : le 56K (V.90) est arrivé ! Jusqu'à 56 000 bit/s en réception au 08 36 15 42 86 *** NOUVEAU : 3615 ORACLE, l'avenir en direct au 08 36 67 22 53 *** NOUVEAU : le visiophone V.Hallucination entre abonnés *** Inscription GRATUITE, sans frais de mise en service *** Le salon de discussion du FAI est ouvert 24 h/24 *** Mode « web 1998 » : Internet comme avant, en plus léger *** Choisissez votre numéro de téléphone ! ***</span></div>
<div class="corps">
{contenu}
</div>
<div class="pied">
<p><a href="/mentions">Mentions légales</a> · <a href="/conditions">Conditions générales</a> · <a href="/confidentialite">Confidentialité</a> · <a href="/etat">État du réseau</a></p>
{webring}
{BADGES}
<p>★ Ajoutez-nous à vos favoris ! (<kbd>Ctrl</kbd>+<kbd>D</kbd>, ou <kbd>⌘</kbd>+<kbd>D</kbd> sur Macintosh) ★</p>
<p><img src="/compteur.gif?n={compteur}" alt="{visites} visites" width="{largeur}" height="20"><br>
Vous êtes le visiteur n° {visites} depuis le dernier redémarrage du serveur.<br>
Site optimisé pour Netscape Navigator 4.0 en 800×600, 256 couleurs.<br>
{nom} est un logiciel libre : vous pouvez <a href="/aide#heberger">héberger votre propre central</a>.</p>
</div>
</div>
</body>
</html>"#,
        titre = esc(titre),
        webring = webring_encart(),
        // compteur façon odomètre : 6 chiffres au moins, à sa taille réelle (13 px par chiffre)
        largeur = compteur.len() * 13 + 1,
    )
}

/// Badges 88×31 « dessinés » en CSS (aucune image externe).
const BADGES: &str = r#"<div class="badges"><a class="badge b-ns" href="/telechargement" title="Netscape Now! (enfin, notre logiciel)" aria-label="Netscape Now!"><span class="i">N</span><span class="e"></span><span class="h">Netscape</span><span class="b">Now!</span></a><span class="badge b-bn" role="img" aria-label="Fait avec le Bloc-notes" title="Fait avec le Bloc-notes"><span class="i"></span><span class="h">Fait avec le</span><span class="b">Bloc-notes</span></span><span class="badge b-res" role="img" aria-label="Optimisé pour 800×600" title="Optimisé pour 800×600"><span class="i"></span><span class="e"></span><span class="h">optimisé</span><span class="b">800×600</span></span><span class="badge b-v34" role="img" aria-label="Modem V.34 inside" title="Modem V.34 inside"><span class="i"></span><span class="h">modem <b>V.34</b></span><span class="b">inside</span></span></div>"#;

/// Membres du « Webring du Web rétro » : (adresse, nom, description). Le
/// site se place entre le dernier et le premier.
pub const WEBRING: &[(&str, &str, &str)] = &[
    ("https://wiby.me/", "Wiby", "le moteur de recherche du Web à l'ancienne : pages perso, sites légers, zéro publicité."),
    ("https://frogfind.com/", "FrogFind!", "le moteur de recherche pour vieux ordinateurs : tout le Web converti en HTML tout simple."),
    ("https://theoldnet.com/", "TheOldNet", "surfez sur le Web de 1998, grâce aux archives de la Wayback Machine."),
    ("https://lite.cnn.com/", "CNN Lite", "les actualités en texte brut, légères comme une plume (en anglais)."),
    ("https://neocities.org/", "Neocities", "hébergement gratuit de pages perso, digne héritier de GeoCities."),
];

fn webring_encart() -> String {
    let (prec, suiv) = (WEBRING[WEBRING.len() - 1].0, WEBRING[0].0);
    format!(
        r#"<div class="webring"><div class="wr-titre">✦ Webring du Web rétro ✦</div><a href="{prec}">← précédent</a> | <a href="/webring/hasard">au hasard</a> | <a href="/webring">la liste</a> | <a href="{suiv}">suivant →</a></div>"#
    )
}

pub fn webring() -> String {
    let mut h = String::from(
        r#"<h2>Webring du Web rétro</h2>
<p>Un <b>webring</b>, c'est une ronde de sites qui se tiennent la main : en bas de chaque page, « précédent » et « suivant » mènent au voisin, et on fait le tour du Web de proche en proche, sans moteur de recherche. Voici les membres de notre anneau :</p><ul>"#,
    );
    for (url, nom, desc) in WEBRING {
        let _ = write!(h, r#"<li><a href="{}"><b>{}</b></a> — {}</li>"#, esc(url), esc(nom), esc(desc));
    }
    h.push_str(r#"</ul><p><a class="bouton" href="/webring/hasard">Emmenez-moi au hasard !</a></p>"#);
    h
}

/// Une entrée de la rubrique « Quoi de neuf ? ».
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Nouvelle {
    pub titre: String,
    /// AAAA-MM-JJ
    pub date: String,
    /// Page de la version (GitHub), ou vide
    pub lien: String,
}

/// « AAAA-MM-JJ… » → « JJ/MM/AAAA »
fn date_iso_fr(d: &str) -> String {
    match (d.get(0..4), d.get(5..7), d.get(8..10)) {
        (Some(a), Some(m), Some(j)) => format!("{j}/{m}/{a}"),
        _ => d.to_string(),
    }
}

pub fn accueil(nom_fai: &str, central: &str, inscriptions_ouvertes: bool, nouvelles: &[Nouvelle]) -> String {
    let mut h = String::new();
    let _ = write!(
        h,
        r#"<p><img src="/construction.gif" alt="En construction" width="88" height="31" style="float:right;margin:0 0 8px 8px">
<b>Bienvenue chez {nom} !</b> Ici, pas de fibre ni de 5G : votre ordinateur se transforme en
<b>modem V.34 et 56K</b>, le vrai, celui qui chante. Vous recevez <b>votre propre numéro de téléphone</b>,
vous appelez les autres abonnés pour discuter, échanger des fichiers et des télécopies, et vous voir au visiophone, et vous composez le
<b>08 36 15 42 86</b> pour surfer sur Internet à travers votre modem, jusqu'à 56 000 bit/s, et tenir votre Tinyblog.
Chaque minute est taxée en francs, comme en 1998 : la facture est pour de rire, les succès pour la gloire.</p>
{quoi}
<h2>Ce que comprend votre abonnement <span class="nouveau">NOUVEAU !</span></h2>
<table class="grille">
<tr><th>Service</th><th>Description</th></tr>
<tr><td>☎ Votre numéro</td><td>Choisissez-le vous-même à l'inscription (10 chiffres, comme en 1998).</td></tr>
<tr><td>💬 Messagerie &amp; fichiers</td><td>Appelez un autre abonné : négociation V.8/V.34 audible (jusqu'à 33 600 bit/s), puis la messagerie s'ouvre : discussion et transfert de fichiers.</td></tr>
<tr><td>📠 Télécopie <span class="nouveau">NOUVEAU !</span></td><td>Un vrai télécopieur du groupe 3 : écrivez, dessinez ou importez une image, composez le numéro d'un abonné, et son modem imprime la page sur papier thermique, ligne par ligne. Négociation T.30 et page en V.29 à 9 600 bit/s (7 200 ou 4 800 si la ligne crachote).</td></tr>
<tr><td>📹 Visiophone</td><td>Pendant un appel entre abonnés, votre webcam redessinée en une quarantaine de formes géométriques, 2 à 3 images par seconde (« V.Hallucination » : ressemblant comme un rêve ressemble à la veille).</td></tr>
<tr><td>🌐 Accès Internet</td><td>Appelez TINYVERSE, en tête de l'annuaire du logiciel : le modem du FAI décroche et vous donne accès au Web, en mode <i>moderne</i> ou <i>web 1998</i> (pages allégées, images en 256 couleurs ou « V.Hallucination »). Sur validation de l'opérateur.</td></tr>
<tr><td>🚀 56K (V.90)</td><td>Le modem du FAI est raccordé en numérique : jusqu'à 56 000 bit/s en réception sur une ligne parfaite, environ 40 000 sur une ligne urbaine (l'envoi reste en V.34). Avec la vraie poignée de main de 1999, ses sifflements et son bourdonnement.</td></tr>
<tr><td>🗨 Salon du FAI</td><td>Tous les abonnés connectés au FAI discutent ensemble.</td></tr>
<tr><td>📝 Tinyblog <span class="nouveau">NOUVEAU !</span></td><td>Votre blog, à l'adresse <code>tinyblog</code>, lisible seulement par le modem : articles avec une image en 256 couleurs, texte en [b]gras[/b], en couleur ou qui clignote, six thèmes, des « J'kiffe » et des com's. Avec l'accès Internet.</td></tr>
<tr><td>🧾 Facture &amp; succès <span class="nouveau">NOUVEAU !</span></td><td>Le compteur TAXES tourne pendant vos appels. Dans <a href="/compte">Mon compte</a> : votre facture détaillée du mois (jamais réclamée), le classement des plus grosses factures du réseau, et 38 succès à décrocher, dont quelques-uns bien cachés.</td></tr>
<tr><td>🕵 Numéros cachés <span class="nouveau">NOUVEAU !</span></td><td>Certains numéros ne figurent dans aucun annuaire. L'oracle en connaît au moins un, et le modem sait balayer une plage de numéros, comme les « wardialers » de l'époque. Ce sont de vrais services Minitel, chacun avec sa page. À vous de chercher.</td></tr>
<tr><td>🔮 3615 ORACLE <span class="nouveau">MINITEL !</span></td><td>Au 08 36 67 22 53 : une intelligence artificielle persuadée qu'on est en 1998, sur un vrai Minitel. Le sifflement à 1300 Hz, CONNECT 1200/75, et l'écran Vidéotex dans la messagerie : la boule de cristal en mosaïques, les touches ENVOI, SUITE, GUIDE, CONNEXION/FIN. Au-delà de 1998, elle ne voit que des brumes. 2,23 F la minute (pour de rire, comme toute la facture). Avec l'accès Internet, une trentaine de questions par jour.</td></tr>
<tr><td>📞 Ligne de test</td><td>Le 08 00 00 00 00 vous renvoie tout ce que vous lui envoyez.</td></tr>
</table>
<h2>Tarifs</h2>
<table class="grille">
<tr><td>Abonnement mensuel</td><td><b>0 F</b> (0,00 €)</td></tr>
<tr><td>Communication locale (abonnés, accès Internet)</td><td><b>0,74 F</b> les 3 premières minutes, puis <b>0,28 F</b> la minute</td></tr>
<tr><td>Heures creuses (22 h – 8 h, samedi et dimanche)</td><td><b>0,14 F</b> la minute, et la satisfaction d'avoir attendu</td></tr>
<tr><td>Numéros verts (08 00)</td><td>Gratuit</td></tr>
<tr><td>Kiosque (08 36), dont 3615 ORACLE</td><td><b>2,23 F</b> la minute</td></tr>
<tr><td>Somme réellement prélevée</td><td><b>0 F</b> : Tinyverse Télécom ne vous réclamera jamais rien. Seul l'appelant « paie ».</td></tr>
<tr><td>Frais de résiliation</td><td>Un pincement au cœur</td></tr>
</table>
<p style="text-align:center;margin:22px 0">"#,
        nom = esc(nom_fai),
        quoi = quoi_de_neuf(nouvelles),
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

fn quoi_de_neuf(nouvelles: &[Nouvelle]) -> String {
    if nouvelles.is_empty() {
        return String::new();
    }
    let mut h = String::from(r#"<div class="boite"><b>Quoi de neuf ?</b><ul class="quoi">"#);
    for (i, n) in nouvelles.iter().enumerate() {
        let titre = if n.lien.is_empty() { esc(&n.titre) } else { format!(r#"<a href="{}">{}</a>"#, esc(&n.lien), esc(&n.titre)) };
        let neuf = if i == 0 { r#" <span class="nouveau">NOUVEAU !</span>"# } else { "" };
        let _ = write!(h, "<li><b>{}</b> — {titre}{neuf}</li>", esc(&date_iso_fr(&n.date)));
    }
    h.push_str("</ul></div>");
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
<p><label><input type="checkbox" name="cgu" value="1" required> J'accepte les <a href="/conditions" target="_blank" rel="noopener">conditions générales</a> : je n'utiliserai pas cet accès pour des activités illégales, et je ne monopoliserai pas la ligne pendant que maman attend un coup de fil.</label><br>
<span class="aide">Ce que nous faisons de vos données : <a href="/confidentialite" target="_blank" rel="noopener">politique de confidentialité</a> (en bref : le strict nécessaire).</span></p>
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
<li>Pour Internet : en tête de l'<b>ANNUAIRE</b>, <b>TINYVERSE</b> ☎ APPELER (le 08 36 15 42 86), puis le bouton <b>NAVIGATEUR</b>.</li>
</ol>
<p>Tout le reste (56K, visiophone, commandes AT…) est dans la notice du logiciel : bouton <kbd>? AIDE</kbd> ou touche <kbd>F1</kbd>.</p>"#,
        nom = esc(nom),
        n = esc(&numero_lisible(numero)),
        c = esc(central)
    );
    if acces_en_attente {
        h.push_str(r#"<div class="boite"><b>Accès Internet :</b> en attente de validation par l'opérateur. Vous recevrez un courrier dès qu'il sera activé ; en attendant, le 08 36 15 42 86 sonne occupé.</div>"#);
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
        r#"<h3>Macintosh</h3><p>Le logiciel n'est pas signé par Apple : au premier lancement, faites <b>clic droit &gt; Ouvrir</b>
(depuis macOS 15 : <b>Réglages Système &gt; Confidentialité et sécurité &gt; Ouvrir quand même</b>),
ou dans le Terminal : <code>xattr -dr com.apple.quarantine "/Applications/V34 Modem.app"</code></p>
<p>Le navigateur intégré demande <b>macOS 14 (Sonoma)</b> ou plus récent pour le Web ; la messagerie, les fichiers et le visiophone marchent partout.
Pour le visiophone, macOS demande l'accès à la caméra la première fois.</p>
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
<p class="aide">Le logiciel a aussi sa notice complète : bouton <kbd>? AIDE</kbd> en haut à droite, ou touche <kbd>F1</kbd>. Les voyants, les étapes de la négociation et la face arrière ont leur infobulle.</p>
<h3>Comment ça marche ?</h3>
<p>Le logiciel contient un vrai modem V.34 et 56K (V.90) : il fabrique le son d'un modem de 1998, et ce son est transporté par Internet
jusqu'au central (<code>{c}</code>), qui le relaie vers la personne que vous appelez, ou vers le modem du fournisseur d'accès.
Tout ce que vous voyez passer est réellement modulé et démodulé, et le modem commente lui-même sa négociation (touche COMMENTAIRES).</p>
<h3>Surfer sur Internet</h3>
<ol><li>Connectez le logiciel au central (badge en haut à droite).</li>
<li>En tête de l'<b>ANNUAIRE</b>, <b>TINYVERSE</b> ☎ APPELER : le logiciel compose le <b>08 36 15 42 86</b>, le modem du FAI décroche, et ça chante.</li>
<li>Après CONNECT, comme sous Windows 98, la fenêtre « Connexion à TINYVERSE » vérifie votre nom d'utilisateur et votre mot de passe (pour la forme : le central vous connaît déjà), puis la LED verte du bouton NAVIGATEUR s'allume.</li>
<li>Bouton <b>NAVIGATEUR</b> : en <b>WEB 1998</b>, le FAI vous sert les pages allégées (HTML simple, images GIF en 256 couleurs, ou « V.Hallucination » redessinées en une cinquantaine de formes) ; en <b>MODERNE</b>, le vrai Web, tel quel, à la vitesse du modem. Prévoyez un café.</li></ol>
<h3>Le 56K (V.90)</h3>
<p>Le modem du FAI est raccordé en numérique au central, comme chez les fournisseurs d'accès de 1998 : votre modem peut recevoir jusqu'à <b>56 000 bit/s</b>.</p>
<ul><li>C'est l'interrupteur <b>DIP 1 « 56K V.90 »</b> du panneau <b>CONFIG</b> (la face arrière du modem), sur ON d'usine. Il faut le logiciel en version 0.1.2 ou plus récente.</li>
<li>En réception : jusqu'à 56 000 bit/s sur une ligne parfaite, environ 40 000 sur la ligne « Urbaine ». L'envoi reste en V.34 : de l'ordre de 14 400 à 19 200 bit/s.</li>
<li>Seulement avec le FAI : entre deux abonnés, il n'y a pas de bout numérique, c'est donc du V.34 (33 600 bit/s au plus).</li>
<li>Ligne trop bruitée, ou qui décale les fréquences (« Longue distance ») : le modem se rabat tout seul en V.34. La molette <b>DÉBIT MAX</b> plafonne le débit ; sous 28 000, le 56K se coupe.</li>
<li>La poignée de main est celle de 1999 : une vingtaine de secondes de sifflements et de bourdonnement (le fameux DIL) avant CONNECT. Pressé ? <b>DIP 3 « 56K EXPRESS »</b> sur ON : une dizaine de secondes.</li></ul>
<h3>Messagerie, fichiers et visiophone</h3>
<p>Appelez un abonné de l'ANNUAIRE : dès CONNECT, la <b>MESSAGERIE</b> s'ouvre (bouton du même nom sur la façade). On y discute, on s'envoie des fichiers (8 Mo au plus : c'est un modem, pas la fibre),
et on se voit au <b>visiophone V.Hallucination</b> : votre webcam, ou la mire, redessinée en une quarantaine de formes géométriques (environ 250 octets l'image), 2 à 3 images par seconde.
La caméra reste éteinte tant que vous n'avez pas choisi CAMÉRA. Pendant une session avec le FAI, la messagerie devient le salon de discussion de tous les abonnés connectés au FAI.</p>
<p>Pour qu'un abonné puisse vous appeler, laissez le logiciel connecté au central : avec l'interrupteur <b>DIP 2 « RÉP. AUTO »</b> sur ON, le modem décroche tout seul après deux sonneries.</p>
<h3>3615 ORACLE</h3>
<p>Une intelligence artificielle qu'on ne joint qu'au modem, et qui est persuadée qu'on est en 1998 : le Minitel, le franc, Windows 98, la Coupe du monde…
Ce qui vient après, elle ne fait que l'entrevoir, comme une voyante dans le brouillard.
C'est un vrai service <b>Minitel</b>.</p>
<ol><li>En bas de l'<b>ANNUAIRE</b>, dans les numéros spéciaux : <b>3615 ORACLE</b> ☎ APPELER (le <b>08 36 67 22 53</b>, « ORACLE » sur le clavier du téléphone).</li>
<li>Le serveur ne répond pas comme un modem ordinaire : un 2100 Hz tout plat (l'ANS), puis le célèbre sifflement du Minitel, à 1300 Hz. Votre modem le reconnaît, répond par sa porteuse à 390 Hz et passe en <b>V.23</b> : CONNECT 1200/75 en trois secondes et demie, 1200 bit/s vers l'écran, 75 bit/s depuis le clavier.</li>
<li>La <b>MESSAGERIE</b> ne s'ouvre pas toute seule : sa LED clignote. Ouvrez-la : la conversation devient l'écran d'un Minitel 1 (40 colonnes, 25 rangées, en niveaux de gris), avec ses touches de fonction en dessous. Cliquez sur l'écran, tapez votre question, puis <b>ENVOI</b> (Entrée). Les lettres partent à 75 bit/s et c'est le serveur qui les renvoie à l'écran, comme sur Télétel ; l'oracle répond en quelques phrases et se souvient de la conversation jusqu'à ce que vous raccrochiez.</li>
<li>Les touches : <b>CORRECTION</b> (⌫), <b>ANNULATION</b> (Échap), <b>SUITE</b> (page suivante d'un long texte), <b>GUIDE</b> (F1), <b>SOMMAIRE</b>, <b>RÉPÉTITION</b> (la dernière réponse), <b>CONNEXION/FIN</b> (F10 : le service dit au revoir et raccroche). Les anciennes commandes <code>/aide</code> et <code>/fin</code> marchent toujours. Le compteur TAXES affiche 2,23 F la minute, mais rien n'est jamais réclamé.</li></ol>
<p>Comme sur un vrai Minitel, il n'y a pas de correction d'erreurs : sur une ligne qui crépite, un caractère abîmé s'affiche en pavé.
<p>Réservé aux abonnés qui ont l'accès Internet, une trentaine de questions par jour. Si l'IA est injoignable, l'oracle tire les cartes à la place, et le dit.
Vos questions sont transmises à OpenRouter et au fournisseur du modèle pour fabriquer la réponse (voir la <a href="/confidentialite">politique de confidentialité</a>) : n'y mettez rien de personnel.</p>
<h3>Télécopie</h3>
<p>Un vrai télécopieur du groupe 3, comme ceux des bureaux de 1998 (logiciel 0.1.7 ou plus récent des deux côtés).</p>
<ol><li>Bouton <b>FAX</b> de la façade (sa LED est verte quand la ligne est libre : un fax ne part que hors appel), ou bouton <b>TÉLÉCOPIE</b> de la fenêtre du navigateur, à côté de MESSAGERIE.</li>
<li>Écrivez, dessinez au crayon ou importez une image, tapez le numéro d'un abonné (ou cliquez-le dans la liste), puis <b>Envoyer</b>.</li>
<li>Le correspondant doit laisser son modem décrocher tout seul (<b>DIP 2 « RÉP. AUTO »</b> sur ON) : son modem reconnaît le sifflement du fax, négocie en T.30, reçoit la page en V.29 (9 600 bit/s, 7 200 ou 4 800 si la ligne est mauvaise) et l'imprime ligne par ligne sur papier thermique. Elle est aussi enregistrée en image dans le dossier <b>Téléchargements/V34 reçus</b>.</li></ol>
<h3>Tinyblog</h3>
<p>Les blogs des abonnés, comme en 2002, mais au modem. Avec l'accès Internet, ouvrez le <b>NAVIGATEUR</b> et cliquez sur <b>Tinyblog</b> (ou tapez l'adresse <code>tinyblog</code>).
Pas de mot de passe à retenir : le FAI sait déjà qui vous êtes.</p>
<ul><li><b>Gérer mon blog</b> : titre, présentation, thème (nuit étoilée, rose bonbon, flammes, océan, matrice, papier quadrillé) et nouveaux articles.</li>
<li>Un article, c'est un titre, du texte et une image, réduite en GIF 256 couleurs. Une photo met une bonne minute à partir à 33 600 bit/s : c'était ça, 1998.</li>
<li>Mise en forme : <code>[b]gras[/b]</code>, <code>[i]italique[/i]</code>, <code>[u]souligné[/u]</code>, <code>[grand]…[/grand]</code>, <code>[centre]…[/centre]</code>, <code>[clignote]…[/clignote]</code>, <code>[couleur=rose]…[/couleur]</code>, et les smileys <code>:)</code> <code>&lt;3</code>.</li>
<li>Chez les autres : « J'kiffe » et « Lâche ton com's ». L'auteur d'un blog peut supprimer les commentaires qu'on y laisse.</li></ul>
<p>Vous êtes responsable de ce que vous publiez (voir les <a href="/conditions">conditions</a>).</p>
<h3>La facture et les succès</h3>
<p>Le compteur <b>TAXES</b>, sous DURÉE, tourne pendant vos appels, au tarif de France Télécom en 1998 : 0,74 F les 3 premières minutes, puis 0,28 F la minute (0,14 F en heures creuses, de 22 h à 8 h et le week-end) ; 2,23 F la minute pour les 08 36, gratuit pour les 08 00. Seul l'appelant paie, et personne ne paie vraiment.</p>
<ul><li>Onglet <b>SUCCÈS</b>, à côté de l'ANNUAIRE : votre facture du mois et vos succès (★ obtenus, ☆ à gagner, ??? cachés). Le commentateur annonce chaque nouveau succès.</li>
<li>Sur ce site, <a href="/compte">Mon compte</a> : le détail de vos communications, mois par mois, et le classement des plus grosses factures du réseau.</li></ul>
<h3>Numéros cachés</h3>
<p>Il paraît que certains numéros ne figurent dans aucun annuaire. L'oracle en connaîtrait au moins un… Chaque numéro trouvé rapporte un succès, et parfois une belle facture.
Ce sont, eux aussi, de vrais services Minitel : chacun sa page, son bandeau et ses touches (RÉPÉTITION, SUITE, GUIDE…).
Les plus patients balaieront une plage entière depuis le TERMINAL, avec la commande <code>AT*B</code> (voir l'aide du logiciel). Attention : chaque porteuse trouvée est taxée.</p>
<h3>Le morse</h3>
<p>Un mode « maison », comme les modems bricolés des années 90 : avec l'interrupteur <b>DIP 4 « MORSE »</b> du panneau <b>CONFIG</b> sur ON, votre modem le propose pendant la négociation V.8, dans un « service non normalisé » (code pays de la France, fournisseur TINYVERSE).
Si le modem d'en face sait faire et accepte, plus de V.34 : <b>CONNECT 20 MPM MORSE</b>, et du vrai code morse international, 20 mots par minute (environ deux lettres par seconde), lettres accentuées comprises.
Les deux parlent en même temps : 800 Hz pour l'appelant, 1600 Hz pour l'appelé. Vos messages partent entre KA et AR, la réponse s'écrit lettre par lettre dans la MESSAGERIE ; texte seulement, ni fichiers ni visiophone.
Sinon, rien ne change : V.34 ou 56K comme d'habitude (le FAI et 3615 ORACLE ne parlent pas morse). Pour vous entraîner seul, appelez la ligne de test, le <b>08 00 00 00 00</b> : elle répond en morse et vous renvoie ce qu'elle entend.</p>
<h3>Je n'arrive pas à me connecter</h3>
<ul><li>Vérifiez le nom d'abonné (sensible aux majuscules) et le mot de passe. Changé sur ce site ? Changez-le aussi dans le logiciel s'il était mémorisé.</li>
<li>Le central utilise le port <b>UDP 3434</b> : certains réseaux d'entreprise le bloquent.</li></ul>
<h3>Ça sonne occupé</h3>
<p>Au 08 36 15 42 86 ou au 08 36 67 22 53 : votre accès Internet n'est pas encore activé par l'opérateur, ou tous les modems du central sont occupés.
Chez un abonné : il est déjà en ligne. Et trois notes montantes, c'est que le numéro n'est attribué à personne, ou que son abonné n'est pas connecté.</p>
<h3 id="heberger">Héberger votre propre central</h3>
<p>Le central, le FAI et ce site sont des logiciels libres (licence MIT).{src}
Suivez <code>deploy/README.md</code> : un VPS sous Linux suffit.</p>"#,
        c = esc(central)
    )
}

/// Texte de plusieurs lignes, échappé (retours à la ligne gardés).
pub fn texte_multiligne(t: &str) -> String {
    esc(t).replace('\n', "<br>")
}

// ───────────────────────── dates ─────────────────────────

/// Date civile (année, mois, jour) d'un nombre de jours depuis le
/// 1er janvier 1970 (algorithme de Howard Hinnant).
fn civil(jours: i64) -> (i64, u32, u32) {
    let z = jours + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m as u32, d as u32)
}

/// Inverse de [`civil`].
fn jours_depuis_civil(a: i64, m: u32, j: u32) -> i64 {
    let a = if m <= 2 { a - 1 } else { a };
    let era = a.div_euclid(400);
    let yoe = a - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(j) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Heure de Paris : UTC+1, UTC+2 du dernier dimanche de mars au dernier
/// dimanche d'octobre (changements à 1 h UTC).
fn heure_de_paris(t: u64) -> i64 {
    let t = t as i64;
    let (a, _, _) = civil(t.div_euclid(86_400));
    let dernier_dimanche = |m: u32| {
        let j = jours_depuis_civil(a, m, 31);
        j - (j + 4).rem_euclid(7) // le 1er janvier 1970 était un jeudi
    };
    let ete = (dernier_dimanche(3) * 86_400 + 3600..dernier_dimanche(10) * 86_400 + 3600).contains(&t);
    t + if ete { 7200 } else { 3600 }
}

/// « 14/11/2026 » (heure de Paris)
pub fn date_fr(t: u64) -> String {
    let (a, m, j) = civil(heure_de_paris(t).div_euclid(86_400));
    format!("{j:02}/{m:02}/{a}")
}

/// « 14/11/2026 à 10 h 32 » (heure de Paris)
pub fn date_heure_fr(t: u64) -> String {
    let s = heure_de_paris(t).rem_euclid(86_400);
    format!("{} à {} h {:02}", date_fr(t), s / 3600, s % 3600 / 60)
}

/// « 2 j 03 h 12 min »
pub fn duree_fr(s: u64) -> String {
    let (j, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if j > 0 {
        format!("{j} j {h:02} h {m:02} min")
    } else if h > 0 {
        format!("{h} h {m:02} min")
    } else if m > 0 {
        format!("{m} min")
    } else {
        "moins d'une minute".into()
    }
}

pub fn pluriel(n: u64, singulier: &str, pluriel: &str) -> String {
    format!("{n} {}", if n > 1 { pluriel } else { singulier })
}

// ───────────────────────── espace abonné ─────────────────────────

pub fn compte_connexion(erreur: Option<&str>, nom: &str) -> String {
    let mut h = String::from("<h2>Mon compte</h2>");
    if let Some(e) = erreur {
        let _ = write!(h, r#"<div class="erreur">{}</div>"#, esc(e));
    }
    let _ = write!(
        h,
        r#"<p>Connectez-vous avec votre <b>nom d'abonné</b> et votre <b>mot de passe</b> : les mêmes que dans le logiciel.</p>
<form method="post" action="/compte/connexion">
<p><label for="nom">Nom d'abonné</label><br><input type="text" id="nom" name="nom" value="{nom}" required maxlength="32" autocomplete="username" autocapitalize="none" spellcheck="false"></p>
<p><label for="mdp">Mot de passe</label><br><input type="password" id="mdp" name="mdp" required maxlength="128" autocomplete="current-password"></p>
<p><button type="submit">Entrer</button></p></form>
<p>» <a href="/oubli">Mot de passe oublié ?</a><br>» Pas encore abonné ? <a href="/inscription">Inscrivez-vous gratuitement</a> !</p>"#,
        nom = esc(nom)
    );
    h
}

/// État de l'accès Internet d'un abonné.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acces {
    Accorde,
    EnAttente,
    Refuse,
}

/// Ce que l'espace abonné affiche.
pub struct VueCompte<'a> {
    pub nom: &'a str,
    pub numero: &'a str,
    /// Compte du site : son adresse (les abonnés de la configuration n'en ont pas)
    pub courriel: Option<&'a str>,
    pub cree: Option<String>,
    pub acces: Acces,
    /// Abonnement suspendu par l'opérateur
    pub suspendu: bool,
}

pub fn compte(v: &VueCompte, central: &str, csrf: &str, info: Option<&str>, erreur: Option<&str>, suite: &str) -> String {
    let mut h = String::from("<h2>Mon compte</h2>");
    if let Some(i) = info {
        let _ = write!(h, r#"<div class="ok">{}</div>"#, esc(i));
    }
    if let Some(e) = erreur {
        let _ = write!(h, r#"<div class="erreur">{}</div>"#, esc(e));
    }
    if v.suspendu {
        h.push_str(r#"<div class="erreur"><b>Abonnement suspendu</b> par l'opérateur : le central refuse la connexion de votre logiciel. Pour en savoir plus, écrivez-lui (voir les <a href="/mentions">mentions légales</a>).</div>"#);
    }
    let acces = match v.acces {
        Acces::Accorde => r#"<span class="voyant v-vert"></span> <b>accordé</b> : appelez TINYVERSE (08 36 15 42 86), en tête de l'annuaire du logiciel, et surfez !"#,
        Acces::EnAttente => r#"<span class="voyant v-orange"></span> <b>en attente de décision</b> de l'opérateur : un courrier vous préviendra."#,
        Acces::Refuse => r#"<span class="voyant v-rouge"></span> <b>refusé</b> : vous pouvez toujours appeler les autres abonnés et la ligne de test."#,
    };
    let _ = write!(
        h,
        r#"<table class="grille fiche"><tr><th>Abonné</th><td><b>{nom}</b></td></tr><tr><th>Numéro</th><td><span class="num">{num}</span></td></tr>"#,
        nom = esc(v.nom),
        num = esc(&numero_lisible(v.numero))
    );
    if let Some(c) = v.courriel {
        let _ = write!(h, "<tr><th>Adresse électronique</th><td>{}</td></tr>", esc(c));
    }
    if let Some(c) = &v.cree {
        let _ = write!(h, "<tr><th>Abonné depuis le</th><td>{}</td></tr>", esc(c));
    }
    let _ = write!(h, r#"<tr><th>Accès Internet</th><td>{acces}</td></tr><tr><th>Adresse du central</th><td><code>{}</code></td></tr></table>"#, esc(central));
    // Facture du mois, succès, classement (voir `facture`)
    h.push_str(suite);
    let csrf = format!(r#"<input type="hidden" name="csrf" value="{}">"#, esc(csrf));
    if v.courriel.is_some() {
        let _ = write!(
            h,
            r#"<h3>Changer de mot de passe</h3>
<form method="post" action="/compte/mdp">{csrf}
<p><label for="mdp-m">Mot de passe actuel</label><br><input type="password" id="mdp-m" name="mdp" required maxlength="128" autocomplete="current-password"></p>
<p><label for="nouveau">Nouveau mot de passe</label><br><input type="password" id="nouveau" name="nouveau" required minlength="8" maxlength="128" autocomplete="new-password"><br>
<label for="nouveau2">Nouveau mot de passe (encore)</label><br><input type="password" id="nouveau2" name="nouveau2" required minlength="8" maxlength="128" autocomplete="new-password"><br>
<span class="aide">8 à 128 caractères. Un courrier vous confirmera le changement.</span></p>
<p><button type="submit">Changer mon mot de passe</button></p></form>
<h3>Résilier mon abonnement</h3>
<form method="post" action="/compte/supprimer">{csrf}
<p>Votre compte sera supprimé et le numéro <span class="num">{num}</span> libéré : quelqu'un d'autre pourra le choisir. C'est définitif !</p>
<p><label for="mdp-s">Mot de passe actuel</label><br><input type="password" id="mdp-s" name="mdp" required maxlength="128" autocomplete="current-password"></p>
<p><label><input type="checkbox" name="sur" value="1" required> Oui, je résilie mon abonnement et je libère mon numéro.</label></p>
<p><button type="submit">Résilier mon abonnement</button></p></form>"#,
            num = esc(&numero_lisible(v.numero))
        );
    } else {
        h.push_str(r#"<div class="boite">Votre abonnement est <b>géré par l'opérateur</b> du central (il est déclaré dans sa configuration) : pour changer de mot de passe ou résilier, adressez-vous à lui.</div>"#);
    }
    let _ = write!(h, r#"<form method="post" action="/compte/deconnexion">{csrf}<p><button type="submit">Se déconnecter</button></p></form>"#);
    h
}

// ───────────────────────── mot de passe oublié ─────────────────────────

pub fn oubli(courriel: &str, erreur: Option<&str>, jeton_temps: &str) -> String {
    let mut h = String::from("<h2>Mot de passe oublié ?</h2>");
    if let Some(e) = erreur {
        let _ = write!(h, r#"<div class="erreur">{}</div>"#, esc(e));
    }
    let _ = write!(
        h,
        r#"<p>Ça arrive aux meilleurs, surtout après une nuit blanche sur IRC. Indiquez l'adresse électronique de votre abonnement : nous vous enverrons un lien pour choisir un nouveau mot de passe, valable <b>une heure</b>.</p>
<form method="post" action="/oubli" autocomplete="off">
<input type="hidden" name="t" value="{t}">
<div class="cache"><label>Ne pas remplir <input type="text" name="site" tabindex="-1"></label></div>
<p><label for="courriel">Adresse électronique</label><br><input type="email" id="courriel" name="courriel" value="{c}" required maxlength="254"></p>
<p><button type="submit">Envoyer le lien</button></p></form>
<p class="aide">Abonné déclaré directement par l'opérateur du central (sans inscription sur ce site) ? C'est lui qui change les mots de passe.</p>"#,
        t = esc(jeton_temps),
        c = esc(courriel)
    );
    h
}

pub fn oubli_envoye(courriel: &str) -> String {
    format!(
        r#"<h2>Vérifiez votre boîte aux lettres électronique</h2>
<div class="ok">Si un abonnement de ce site utilise l'adresse <b>{}</b>, un courrier vient de partir avec un lien pour choisir un nouveau mot de passe. Il est valable <b>une heure</b>.</div>
<p>Rien reçu au bout de quelques minutes ? Regardez dans les indésirables, et vérifiez l'adresse : c'est celle que vous avez donnée à l'inscription.</p>
<p><a href="/compte">Retour à l'espace abonné</a></p>"#,
        esc(courriel)
    )
}

pub fn reinitialiser(jeton: &str, nom: &str, erreur: Option<&str>) -> String {
    let mut h = String::from("<h2>Nouveau mot de passe</h2>");
    if let Some(e) = erreur {
        let _ = write!(h, r#"<div class="erreur">{}</div>"#, esc(e));
    }
    let _ = write!(
        h,
        r#"<p>Abonné <b>{nom}</b> : choisissez votre nouveau mot de passe.</p>
<form method="post" action="/reinitialiser"><input type="hidden" name="t" value="{t}">
<p><label for="mdp">Nouveau mot de passe</label><br><input type="password" id="mdp" name="mdp" required minlength="8" maxlength="128" autocomplete="new-password"><br>
<label for="mdp2">Nouveau mot de passe (encore)</label><br><input type="password" id="mdp2" name="mdp2" required minlength="8" maxlength="128" autocomplete="new-password"><br>
<span class="aide">8 à 128 caractères. Ce lien ne sert qu'une fois.</span></p>
<p><button type="submit" class="gros">Enregistrer</button></p></form>"#,
        nom = esc(nom),
        t = esc(jeton)
    );
    h
}

pub fn reinitialiser_invalide() -> String {
    r#"<h2>Lien invalide</h2><div class="erreur">Ce lien est invalide, déjà utilisé ou expiré (il n'est valable qu'une heure).</div><p><a href="/oubli">Refaire une demande</a> — <a href="/">Retour à l'accueil</a></p>"#.into()
}

pub fn reinitialise(nom: &str) -> String {
    format!(
        r#"<h2>Mot de passe changé</h2><div class="ok">C'est fait, <b>{}</b> : votre nouveau mot de passe est enregistré (le central le prend en compte sous 2 secondes). Un courrier vous le confirme.</div><p>Pensez à le changer aussi dans le logiciel si vous l'aviez mémorisé. <a href="/compte">Aller à mon compte</a></p>"#,
        esc(nom)
    )
}

// ───────────────────────── livre d'or ─────────────────────────

/// Saisies du formulaire du livre d'or (pour le réafficher après une erreur).
pub struct FormulaireLivre<'a> {
    pub pseudo: &'a str,
    pub texte: &'a str,
    pub erreurs: &'a [String],
    pub jeton: &'a str,
}

pub fn livre(f: &FormulaireLivre, messages: &[Message], total: usize, page: usize, pages_max: usize) -> String {
    let mut h = String::from(
        r#"<h2>Livre d'or</h2>
<p><img src="/construction.gif" alt="" width="88" height="31" style="float:right;margin:0 0 8px 8px">Vous aimez notre site ? Vous le trouvez trop lent ? Vous passiez juste dire bonjour ? <b>Signez notre livre d'or !</b> Les messages sont publiés après relecture par le webmestre (qui lit tout, même les fautes d'orthographe).</p>"#,
    );
    if !f.erreurs.is_empty() {
        h.push_str("<div class=\"erreur\"><ul>");
        for e in f.erreurs {
            let _ = write!(h, "<li>{}</li>", esc(e));
        }
        h.push_str("</ul></div>");
    }
    let _ = write!(
        h,
        r#"<form method="post" action="/livre" autocomplete="off">
<input type="hidden" name="t" value="{t}">
<div class="cache"><label>Ne pas remplir <input type="text" name="site" tabindex="-1"></label></div>
<p><label for="pseudo">Votre pseudo</label><br><input type="text" id="pseudo" name="pseudo" value="{p}" required maxlength="32"></p>
<p><label for="message">Votre message</label><br><textarea id="message" name="message" rows="5" cols="50" required maxlength="500">{m}</textarea><br>
<span class="aide">500 caractères au plus. Pas de HTML ni de liens cliquables : on est entre gens bien élevés.</span></p>
<p><button type="submit">Signer le livre d'or</button></p></form>"#,
        t = esc(f.jeton),
        p = esc(f.pseudo),
        m = esc(f.texte)
    );
    if total == 0 {
        h.push_str("<h3>Le livre est encore vierge</h3><p>Soyez le premier à le signer !</p>");
        return h;
    }
    let _ = write!(h, "<h3>{} dans le livre</h3>", pluriel(total as u64, "message", "messages"));
    for m in messages {
        let _ = write!(
            h,
            r#"<div class="msg"><div class="msg-t"><b>{}</b> a écrit le {} :</div>{}</div>"#,
            esc(&m.pseudo),
            esc(&date_heure_fr(m.date)),
            texte_multiligne(&m.texte)
        );
    }
    if pages_max > 1 {
        h.push_str(r#"<p style="text-align:center">"#);
        if page > 1 {
            let _ = write!(h, r#"<a href="/livre?p={}">« plus récents</a> | "#, page - 1);
        }
        let _ = write!(h, "page {page} sur {pages_max}");
        if page < pages_max {
            let _ = write!(h, r#" | <a href="/livre?p={}">plus anciens »</a>"#, page + 1);
        }
        h.push_str("</p>");
    }
    h
}

pub fn livre_merci(pseudo: &str) -> String {
    format!(
        r#"<h2>Merci, {} !</h2><div class="ok">Votre message est bien arrivé. Il sera publié dès que le webmestre l'aura relu (il n'est pas toujours devant son écran : parfois, il attend que la ligne se libère).</div><p><a href="/livre">Retour au livre d'or</a></p>"#,
        esc(pseudo)
    )
}

// ───────────────────────── état du réseau ─────────────────────────

pub fn voyant(classe: &str) -> String {
    format!(r#"<span class="voyant {classe}"></span>"#)
}

pub fn etat(nom_fai: &str, etat: Option<&Etat>, version_appli: Option<&str>, maintenant: u64) -> String {
    let mut h = format!(
        r#"<h2>État du réseau</h2>
<p class="aide">Relevé du {} (heure de Paris). Cette page se met à jour toute seule toutes les 30 secondes.</p>
<table class="pupitre"><tr><th colspan="3">PUPITRE DE SUPERVISION — {}</th></tr>"#,
        esc(&date_heure_fr(maintenant)),
        esc(&nom_fai.to_uppercase())
    );
    let ligne = |h: &mut String, voyants: &str, quoi: &str, valeur: &str| {
        let _ = write!(h, r#"<tr><td class="v">{voyants}</td><td>{quoi}</td><td>{valeur}</td></tr>"#);
    };
    let logiciel = match version_appli {
        Some(v) => format!(r#"dernière version : <b>{}</b> — <a href="/telechargement">télécharger</a>"#, esc(v)),
        None => r#"<a href="/telechargement">page de téléchargement</a>"#.to_string(),
    };
    let Some(e) = etat else {
        ligne(&mut h, &voyant("v-rouge clignote"), "Central téléphonique", r#"<span class="hs">HORS SERVICE ou injoignable</span>"#);
        ligne(&mut h, &voyant("v-eteint"), "Logiciel", &logiciel);
        h.push_str(
            r#"</table><div class="erreur">Le central ne donne plus de nouvelles depuis plus de 30 secondes : il est arrêté, en maintenance, ou la ligne est coupée. Nos techniciens sont sans doute déjà sur le poteau : réessayez dans quelques minutes.</div>"#,
        );
        return h + LEGENDE;
    };
    ligne(
        &mut h,
        &voyant("v-vert"),
        "Central téléphonique",
        &format!("<b>EN SERVICE</b> depuis le {} ({}) — version {}", esc(&date_heure_fr(e.depuis)), esc(&duree_fr(maintenant.saturating_sub(e.depuis))), esc(&e.version)),
    );
    ligne(
        &mut h,
        &voyant(if e.connectes > 0 { "v-vert" } else { "v-eteint" }),
        "Abonnés connectés",
        &format!("<b>{}</b> sur {}", e.connectes, pluriel(e.abonnes, "abonné", "abonnés")),
    );
    ligne(
        &mut h,
        &voyant(if e.appels > 0 { "v-orange clignote" } else { "v-eteint" }),
        "Appels en cours",
        &if e.appels > 0 { format!("<b>{}</b>", pluriel(e.appels, "communication", "communications")) } else { "aucun : les lignes sont libres".into() },
    );
    let occupes = e.modems.occupes.min(e.modems.max);
    let libres = e.modems.max - occupes;
    let voyants: String = if e.modems.max <= 32 {
        (0..e.modems.max).map(|i| voyant(if i < occupes { "v-rouge" } else { "v-vert" })).collect()
    } else {
        voyant(if libres > 0 { "v-vert" } else { "v-rouge" })
    };
    let modems = if e.modems.max == 0 {
        "aucun modem en service".to_string()
    } else if libres == 0 {
        format!("<b>tous occupés</b> ({}) : ça sonne occupé, réessayez dans un moment !", e.modems.max)
    } else {
        format!("<b>{}</b>, {}", pluriel(libres, "libre", "libres"), pluriel(occupes, "occupé", "occupés"))
    };
    ligne(&mut h, &voyants, "Modems du FAI", &modems);
    ligne(
        &mut h,
        &voyant(if e.salon > 0 { "v-vert" } else { "v-eteint" }),
        "Salon du FAI",
        &if e.salon > 0 { format!("<b>{}</b>", pluriel(e.salon, "présent", "présents")) } else { "personne pour l'instant".into() },
    );
    ligne(&mut h, &voyant(if version_appli.is_some() { "v-vert" } else { "v-eteint" }), "Logiciel", &logiciel);
    h.push_str("</table>");
    h + LEGENDE
}

const LEGENDE: &str = r#"<p class="aide"><span class="voyant v-vert"></span> en service, libre &nbsp; <span class="voyant v-orange"></span> en communication &nbsp; <span class="voyant v-rouge"></span> occupé ou hors service &nbsp; <span class="voyant v-eteint"></span> au repos</p>"#;
