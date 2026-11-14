//! Pages de Tinyblog : HTML 3.2 et un peu de CSS (lisibles en mode
//! « web 1998 » comme en moderne), façon blogs des années 2000 : thèmes
//! chargés, texte qui clignote, « j'kiffe » et « lâche ton com's ».
//!
//! Le texte des abonnés n'est jamais pris tel quel : il est échappé, puis
//! mis en forme par quelques codes ([b], [couleur=rose], [clignote]…).

use std::fmt::Write;

use super::store::{Article, Blog};

pub struct Theme {
    pub id: &'static str,
    pub nom: &'static str,
    bg: &'static str,
    texte: &'static str,
    lien: &'static str,
    carte: &'static str,
    fond: &'static str,
}

pub const THEMES: &[Theme] = &[
    Theme {
        id: "nuit",
        nom: "Nuit étoilée",
        bg: "#000022",
        texte: "#ffffff",
        lien: "#66ccff",
        carte: "rgba(20,20,70,0.85)",
        fond: "radial-gradient(#fff 1px,transparent 1.6px) 0 0/40px 40px,radial-gradient(#ffd 1px,transparent 1.6px) 20px 25px/55px 55px,linear-gradient(#000022,#110033)",
    },
    Theme {
        id: "rose",
        nom: "Rose bonbon",
        bg: "#ffccee",
        texte: "#550033",
        lien: "#cc0077",
        carte: "rgba(255,240,250,0.92)",
        fond: "repeating-linear-gradient(45deg,#ffd6f0 0 14px,#ffc2e8 14px 28px)",
    },
    Theme {
        id: "flammes",
        nom: "Flammes",
        bg: "#330000",
        texte: "#ffee88",
        lien: "#ffaa00",
        carte: "rgba(40,0,0,0.85)",
        fond: "linear-gradient(0deg,#ff5500,#aa0000 35%,#220000 75%)",
    },
    Theme {
        id: "ocean",
        nom: "Océan",
        bg: "#003366",
        texte: "#e0f4ff",
        lien: "#7fdcff",
        carte: "rgba(0,30,60,0.85)",
        fond: "linear-gradient(#0077bb,#001a33)",
    },
    Theme {
        id: "matrice",
        nom: "Matrice",
        bg: "#000000",
        texte: "#33ff33",
        lien: "#99ff99",
        carte: "rgba(0,20,0,0.9)",
        fond: "repeating-linear-gradient(90deg,#001a00 0 2px,#000 2px 7px)",
    },
    Theme {
        id: "papier",
        nom: "Papier quadrillé",
        bg: "#ffffff",
        texte: "#000033",
        lien: "#0000cc",
        carte: "rgba(255,255,255,0.95)",
        fond: "linear-gradient(#ccd 1px,transparent 1px) 0 0/100% 22px,#fffef8",
    },
];

pub fn theme(id: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.id == id).unwrap_or(&THEMES[0])
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const COULEURS: &[(&str, &str)] = &[
    ("rouge", "#ff3030"),
    ("rose", "#ff66cc"),
    ("orange", "#ff9900"),
    ("jaune", "#ffee00"),
    ("vert", "#33cc33"),
    ("bleu", "#3399ff"),
    ("violet", "#aa55ff"),
    ("blanc", "#ffffff"),
    ("noir", "#000000"),
    ("gris", "#999999"),
];

/// Le texte d'un abonné, échappé puis mis en forme : [b] [i] [u] [centre]
/// [clignote] [grand] [petit] [couleur=rose], retours à la ligne, et
/// quelques frimousses. Les balises restées ouvertes sont refermées.
pub fn mise_en_forme(texte: &str) -> String {
    let t = esc(texte);
    let mut out = String::with_capacity(t.len() + 32);
    let mut pile: Vec<&'static str> = Vec::new();
    let mut i = 0;
    let b = t.as_bytes();
    while i < t.len() {
        if b[i] == b'[' {
            if let Some(fin) = t[i..].find(']') {
                let code = t[i + 1..i + fin].to_lowercase();
                if let Some((ouvre, ferme)) = balise(&code) {
                    out.push_str(&ouvre);
                    pile.push(ferme);
                    i += fin + 1;
                    continue;
                }
                if let Some(nom) = code.strip_prefix('/') {
                    let ferme = match nom {
                        "b" => Some("</b>"),
                        "i" => Some("</i>"),
                        "u" => Some("</u>"),
                        "centre" => Some("</div>"),
                        "clignote" => Some("</span>"),
                        "grand" | "petit" | "couleur" => Some("</font>"),
                        _ => None,
                    };
                    if let Some(f) = ferme {
                        if let Some(pos) = pile.iter().rposition(|x| *x == f) {
                            // referme aussi ce qui a été ouvert à l'intérieur
                            while pile.len() > pos {
                                out.push_str(pile.pop().unwrap_or(""));
                            }
                        }
                        i += fin + 1;
                        continue;
                    }
                }
            }
        }
        let c = t[i..].chars().next().unwrap_or(' ');
        if c == '\n' {
            out.push_str("<br>\n");
        } else if c != '\r' {
            out.push(c);
        }
        i += c.len_utf8();
    }
    while let Some(f) = pile.pop() {
        out.push_str(f);
    }
    out.replace(":-)", "☺").replace(":)", "☺").replace(":-(", "☹").replace(":(", "☹").replace("&lt;3", "<font color=\"#ff3366\">♥</font>")
}

fn balise(code: &str) -> Option<(String, &'static str)> {
    Some(match code {
        "b" => ("<b>".into(), "</b>"),
        "i" => ("<i>".into(), "</i>"),
        "u" => ("<u>".into(), "</u>"),
        "centre" => ("<div align=\"center\">".into(), "</div>"),
        "clignote" => ("<span class=\"clignote\">".into(), "</span>"),
        "grand" => ("<font size=\"5\">".into(), "</font>"),
        "petit" => ("<font size=\"2\">".into(), "</font>"),
        _ => {
            let c = code.strip_prefix("couleur=")?;
            let hex = COULEURS.iter().find(|(n, _)| *n == c)?.1;
            (format!("<font color=\"{hex}\">"), "</font>")
        }
    })
}

/// « 14/11/1998 à 23 h 05 » (heure de Paris)
fn date(t: u64) -> String {
    let h = v34_net::tarif::paris(t);
    format!("{:02}/{:02}/{} à {} h {:02}", h.jour, h.mois, h.annee, h.heure, h.minute)
}

fn debut(titre: &str, th: &Theme) -> String {
    format!(
        "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 3.2 Final//EN\">\n<html><head><meta charset=\"utf-8\"><title>{}</title>\n\
<style>body{{background:{};margin:0;padding:12px;font-family:\"Comic Sans MS\",\"Trebuchet MS\",Verdana,sans-serif;font-size:15px}}\
.page{{max-width:720px;margin:0 auto}}.carte{{background:{};border:2px solid {};border-radius:10px;padding:10px 14px;margin:12px 0}}\
.logo{{font:bold 34px Verdana,sans-serif;letter-spacing:2px;text-align:center;margin:6px 0;color:{}}}\
.logo span{{color:#ff66cc}}.clignote{{animation:cl 1s steps(2,jump-none) infinite}}@keyframes cl{{50%{{opacity:0.2}}}}\
.coms{{font-size:13px;border-top:1px dashed {};margin-top:8px;padding-top:6px}}.com{{margin:4px 0}}\
img.photo{{max-width:100%;border:3px double {};border-radius:6px}}input,textarea,select{{font:inherit}}\
.kiffe{{font-weight:bold;background:#ff66cc;color:#fff;border:2px outset #ffaadd;border-radius:12px;padding:1px 10px;cursor:pointer}}\
.menu{{text-align:center;font-size:14px}}</style></head>\n\
<body bgcolor=\"{}\" text=\"{}\" link=\"{}\" vlink=\"{}\"><div class=\"page\">\n",
        esc(titre),
        th.fond,
        th.carte,
        th.lien,
        th.lien,
        th.lien,
        th.lien,
        th.bg,
        th.texte,
        th.lien,
        th.lien
    )
}

const FIN: &str = "<p class=\"menu\"><font size=\"2\">Tinyblog &mdash; les blogs des abonnés Tinyverse, lisibles seulement par modem.<br>\
Meilleur affichage en 800x600. &copy; 1998 (ou à peu près).</font></p>\n</div></body></html>\n";

fn logo() -> &'static str {
    "<div class=\"logo\"><a href=\"/\" style=\"text-decoration:none;color:inherit\">TINY<span>BLOG</span></a></div>\n"
}

fn menu(visiteur: &str) -> String {
    format!(
        "<p class=\"menu\"><a href=\"/\">Accueil</a> &middot; <a href=\"/{v}\">Mon Tinyblog</a> &middot; <a href=\"/gerer\">Écrire un article</a> &middot; connecté en tant que <b>{v}</b></p>\n",
        v = esc(visiteur)
    )
}

/// Petite vignette d'article pour l'accueil.
fn vignette(s: &mut String, nom: &str, a: &Article) {
    let img = a.image.as_ref().map(|f| format!("<img src=\"/img/{}\" width=\"64\" align=\"left\" hspace=\"6\" alt=\"\">", esc(f))).unwrap_or_default();
    let _ = write!(
        s,
        "<div class=\"carte\">{img}<a href=\"/{n}#a{id}\"><b>{t}</b></a><br><font size=\"2\">par <a href=\"/{n}\">{n}</a>, le {d} &middot; ♥ {k} &middot; {c} com's</font><br clear=\"all\"></div>\n",
        n = esc(nom),
        id = a.id,
        t = esc(&a.titre),
        d = date(a.date),
        k = a.kiffs.len(),
        c = a.coms.len()
    );
}

pub fn accueil(visiteur: &str, blogs: &[(String, Blog)]) -> String {
    let th = theme("nuit");
    let mut s = debut("Tinyblog - les blogs des abonnés", th);
    s.push_str(logo());
    s.push_str("<p align=\"center\"><i>Les blogs des abonnés Tinyverse. Lâchez vos com's !</i></p>\n");
    s.push_str(&menu(visiteur));
    // les derniers articles, tous blogs confondus
    let mut tous: Vec<(&str, &Article)> = blogs.iter().flat_map(|(n, b)| b.articles.iter().map(move |a| (n.as_str(), a))).collect();
    tous.sort_by(|a, b| b.1.date.cmp(&a.1.date));
    s.push_str("<h3>Les derniers articles</h3>\n");
    if tous.is_empty() {
        s.push_str("<div class=\"carte\">Pas encore d'article. Sois le premier : <a href=\"/gerer\">écris le tien</a> !</div>\n");
    }
    for (n, a) in tous.iter().take(8) {
        vignette(&mut s, n, a);
    }
    // les plus kiffés
    let mut kiffes: Vec<&(&str, &Article)> = tous.iter().filter(|(_, a)| !a.kiffs.is_empty()).collect();
    kiffes.sort_by(|a, b| b.1.kiffs.len().cmp(&a.1.kiffs.len()));
    if !kiffes.is_empty() {
        s.push_str("<h3>Les plus kiffés</h3>\n<ol>\n");
        for (n, a) in kiffes.iter().take(5) {
            let _ = writeln!(s, "<li><a href=\"/{n}#a{}\">{}</a> par {n} &middot; ♥ {}</li>", a.id, esc(&a.titre), a.kiffs.len(), n = esc(n));
        }
        s.push_str("</ol>\n");
    }
    // tous les blogs, les plus récents d'abord
    let mut liste: Vec<&(String, Blog)> = blogs.iter().filter(|(_, b)| !b.articles.is_empty()).collect();
    liste.sort_by(|a, b| b.1.dernier().cmp(&a.1.dernier()));
    if !liste.is_empty() {
        s.push_str("<h3>Tous les Tinyblogs</h3>\n<ul>\n");
        for (n, b) in liste {
            let titre = if b.titre.is_empty() { format!("Le blog de {n}") } else { b.titre.clone() };
            let _ = writeln!(s, "<li><a href=\"/{}\"><b>{}</b></a> &mdash; {} article(s), ♥ {}</li>", esc(n), esc(&titre), b.articles.len(), b.kiffs());
        }
        s.push_str("</ul>\n");
    }
    s.push_str(FIN);
    s
}

pub const PAR_PAGE: usize = 5;

pub fn blog(visiteur: &str, nom: &str, b: &Blog, page: usize) -> String {
    let th = theme(&b.theme);
    let titre = if b.titre.is_empty() { format!("Le blog de {nom}") } else { b.titre.clone() };
    let mut s = debut(&titre, th);
    s.push_str(logo());
    s.push_str(&menu(visiteur));
    let _ = write!(s, "<div class=\"carte\" align=\"center\"><font size=\"6\"><b>{}</b></font>", esc(&titre));
    if !b.description.is_empty() {
        let _ = write!(s, "<br><i>{}</i>", mise_en_forme(&b.description));
    }
    let _ = write!(s, "<br><font size=\"2\">{} article(s) &middot; ♥ {} &middot; le Tinyblog de <b>{}</b></font></div>\n", b.articles.len(), b.kiffs(), esc(nom));
    let mut arts: Vec<&Article> = b.articles.iter().collect();
    arts.sort_by(|a, b| b.date.cmp(&a.date));
    let pages = arts.len().div_ceil(PAR_PAGE).max(1);
    let page = page.min(pages - 1);
    if arts.is_empty() {
        let _ = writeln!(
            s,
            "<div class=\"carte\">Ce Tinyblog est encore vide.{}</div>",
            if visiteur == nom { " <a href=\"/gerer\">Écris ton premier article !</a>" } else { "" }
        );
    }
    for a in arts.iter().skip(page * PAR_PAGE).take(PAR_PAGE) {
        let _ = write!(s, "<div class=\"carte\"><a name=\"a{}\"></a><font size=\"5\"><b>{}</b></font><br><font size=\"2\">le {}</font><br>\n", a.id, esc(&a.titre), date(a.date));
        if let Some(f) = &a.image {
            let _ = write!(s, "<p align=\"center\"><img class=\"photo\" src=\"/img/{}\" alt=\"[image]\"></p>\n", esc(f));
        }
        let _ = write!(s, "<p>{}</p>\n", mise_en_forme(&a.texte));
        let kiffe = a.kiffs.iter().any(|k| k == visiteur);
        let _ = write!(
            s,
            "<form method=\"post\" action=\"/{n}/kiffe?id={id}\"><input type=\"submit\" class=\"kiffe\" value=\"{} ♥ {}\"></form>\n",
            if kiffe { "Je kiffe plus" } else { "J'kiffe" },
            a.kiffs.len(),
            n = esc(nom),
            id = a.id
        );
        s.push_str("<div class=\"coms\">");
        if a.coms.is_empty() {
            s.push_str("<i>Pas encore de com's.</i>");
        }
        for (k, c) in a.coms.iter().enumerate() {
            let supprimer = if visiteur == nom || visiteur == c.de {
                format!(" <form method=\"post\" action=\"/{}/com/supprimer?id={}&amp;k={k}\" style=\"display:inline\"><input type=\"submit\" value=\"x\" title=\"Supprimer\"></form>", esc(nom), a.id)
            } else {
                String::new()
            };
            let _ = write!(s, "<div class=\"com\"><b>{}</b> <font size=\"1\">({})</font> : {}{supprimer}</div>", esc(&c.de), date(c.date), mise_en_forme(&c.texte));
        }
        let _ = write!(
            s,
            "<form method=\"post\" action=\"/{n}/com?id={id}\"><b>Lâche ton com's :</b> <input type=\"text\" name=\"texte\" size=\"40\" maxlength=\"300\"> <input type=\"submit\" value=\"Envoyer\"></form></div></div>\n",
            n = esc(nom),
            id = a.id
        );
    }
    if pages > 1 {
        s.push_str("<p class=\"menu\">Pages : ");
        for p in 0..pages {
            if p == page {
                let _ = write!(s, "<b>[{}]</b> ", p + 1);
            } else {
                let _ = write!(s, "<a href=\"/{}?page={}\">{}</a> ", esc(nom), p + 1, p + 1);
            }
        }
        s.push_str("</p>\n");
    }
    s.push_str(FIN);
    s
}

const AIDE_CODES: &str = "<font size=\"2\">Mise en forme : [b]gras[/b], [i]italique[/i], [u]souligné[/u], [grand]…[/grand], [petit]…[/petit], [centre]…[/centre], \
[clignote]…[/clignote], [couleur=rose]…[/couleur] (rouge, rose, orange, jaune, vert, bleu, violet, blanc, noir, gris). Et :) :( &lt;3</font>";

pub fn gerer(visiteur: &str, b: &Blog, message: Option<&str>) -> String {
    let th = theme(&b.theme);
    let mut s = debut("Gérer mon Tinyblog", th);
    s.push_str(logo());
    s.push_str(&menu(visiteur));
    if let Some(m) = message {
        let _ = writeln!(s, "<div class=\"carte\"><b>{}</b></div>", esc(m));
    }
    s.push_str(
        "<div class=\"carte\"><h3>Nouvel article</h3>\n\
<form method=\"post\" action=\"/gerer/article\" enctype=\"multipart/form-data\">\n\
<b>Titre :</b><br><input type=\"text\" name=\"titre\" size=\"50\" maxlength=\"80\"><br>\n\
<b>Image</b> (facultative ; elle part par le modem, puis elle est réduite en GIF 256 couleurs) :<br><input type=\"file\" name=\"image\" accept=\"image/*\"><br>\n\
<b>Texte :</b><br><textarea name=\"texte\" rows=\"8\" cols=\"60\" maxlength=\"4000\"></textarea><br>\n",
    );
    s.push_str(AIDE_CODES);
    s.push_str("<br><input type=\"submit\" value=\"Publier !\"></form></div>\n");
    let _ = write!(
        s,
        "<div class=\"carte\"><h3>Mon Tinyblog</h3>\n<form method=\"post\" action=\"/gerer/reglages\">\n\
<b>Titre du blog :</b><br><input type=\"text\" name=\"titre\" size=\"50\" maxlength=\"80\" value=\"{}\"><br>\n\
<b>Présentation :</b><br><textarea name=\"description\" rows=\"3\" cols=\"60\" maxlength=\"300\">{}</textarea><br>\n<b>Thème :</b> <select name=\"theme\">",
        esc(&b.titre),
        esc(&b.description)
    );
    for t in THEMES {
        let _ = write!(s, "<option value=\"{}\"{}>{}</option>", t.id, if t.id == b.theme || (b.theme.is_empty() && t.id == "nuit") { " selected" } else { "" }, t.nom);
    }
    s.push_str("</select> <input type=\"submit\" value=\"Enregistrer\"></form></div>\n");
    if !b.articles.is_empty() {
        s.push_str("<div class=\"carte\"><h3>Mes articles</h3>\n<ul>\n");
        let mut arts: Vec<&Article> = b.articles.iter().collect();
        arts.sort_by(|a, b| b.date.cmp(&a.date));
        for a in arts {
            let _ = writeln!(
                s,
                "<li><b>{}</b> ({}, ♥ {}, {} com's) <form method=\"post\" action=\"/gerer/supprimer?id={}\" style=\"display:inline\"><input type=\"submit\" value=\"Supprimer\"></form></li>",
                esc(&a.titre),
                date(a.date),
                a.kiffs.len(),
                a.coms.len(),
                a.id
            );
        }
        s.push_str("</ul></div>\n");
    }
    s.push_str(FIN);
    s
}

pub fn erreur(titre: &str, texte: &str) -> String {
    let mut s = debut(titre, theme("nuit"));
    s.push_str(logo());
    let _ = write!(s, "<div class=\"carte\"><h3>{}</h3><p>{}</p><p><a href=\"/\">Retour à l'accueil de Tinyblog</a></p></div>\n", esc(titre), esc(texte));
    s.push_str(FIN);
    s
}
