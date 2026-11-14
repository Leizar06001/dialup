//! Pages HTML 3.2 du service : accueil « Tinyverse 1998 », habillage des
//! pages allégées (bandeau), pages d'erreur rétro.

use std::fmt::Write as _;

use url::Url;

use super::html::{escape_attr, escape_text, page_link};

const DOCTYPE: &str = "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 3.2 Final//EN\">\n";
const BODY: &str = "<body bgcolor=\"#c0c0c0\" text=\"#000000\" link=\"#0000ee\" vlink=\"#551a8b\" alink=\"#ff0000\">\n<font face=\"Times New Roman,Times,serif\">\n";
const END: &str = "</font>\n</body>\n</html>\n";

fn head(title: &str) -> String {
    let mut s = String::from(DOCTYPE);
    s.push_str("<html>\n<head>\n<meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\">\n<title>");
    escape_text(&mut s, title);
    s.push_str("</title>\n</head>\n");
    s.push_str(BODY);
    s
}

/// Taille lisible : « 812 o », « 4,2 Ko », « 512 Ko », « 1,3 Mo ».
pub fn size(n: usize) -> String {
    let fr = |x: f64| format!("{x:.1}").replace('.', ",");
    match n {
        0..=999 => format!("{n} o"),
        1000..=10_239 => format!("{} Ko", fr(n as f64 / 1024.0)),
        10_240..=1_048_575 => format!("{} Ko", (n as f64 / 1024.0).round()),
        _ => format!("{} Mo", fr(n as f64 / 1_048_576.0)),
    }
}

/// Sites proposés sur la page d'accueil (nom, URL, description).
const SITES: &[(&str, &str, &str)] = &[
    ("FrogFind !", "https://frogfind.com/", "le moteur de recherche pour vieux ordinateurs"),
    // (68k.news ne sait plus lire les liens de Google News : articles cassés)
    ("franceinfo", "https://www.franceinfo.fr/", "les actualités du jour, allégées par nos soins"),
    ("CNN Lite", "https://lite.cnn.com/", "les nouvelles du monde en texte seul (en anglais)"),
    ("Wikipédia", "https://fr.wikipedia.org/wiki/Wikip%C3%A9dia:Accueil_principal", "l'encyclopédie libre, en français"),
    ("Hacker News", "https://news.ycombinator.com/", "les nouvelles de l'informatique (en anglais)"),
];

/// Page d'accueil. `visits` : numéro du visiteur.
pub fn home(visits: u64) -> String {
    let mut s = head("Tinyverse 1998 - Bienvenue !");
    s.push_str(
        "<center>\n<h1>Bienvenue sur Tinyverse 1998</h1>\n\
         <img src=\"/construction.gif\" width=\"100\" height=\"31\" alt=\"[EN CONSTRUCTION]\">\n\
         <p><i>Le Web comme en 1998 : pages allégées, images en 256 couleurs, pas de JavaScript.<br>\
         Idéal pour votre modem 33600 bauds !</i></p>\n\
         <hr width=\"80%\">\n\
         <form action=\"/p\" method=\"get\"><b>Aller à :</b> \
         <input type=\"text\" name=\"u\" value=\"http://\" size=\"40\"> \
         <input type=\"submit\" value=\"Go !\"></form>\n\
         <form action=\"/f\" method=\"get\"><input type=\"hidden\" name=\"u\" value=\"https://frogfind.com/\">\
         <b>Chercher sur FrogFind :</b> <input type=\"text\" name=\"q\" size=\"30\"> \
         <input type=\"submit\" value=\"Chercher\"></form>\n\
         <hr width=\"80%\">\n</center>\n\
         <h3>Sites conseillés par la rédaction</h3>\n<ul>\n",
    );
    for (name, url, desc) in SITES {
        let link = page_link(&Url::parse(url).expect("URL valide"));
        let _ = writeln!(s, "<li><a href=\"{}\"><b>{name}</b></a> - {desc}</li>", escape_attr(&link));
    }
    let _ = write!(
        s,
        "</ul>\n<hr>\n<center>\n<p>Vous êtes le visiteur n°<br>\
         <img src=\"/compteur.gif?n={visits:06}\" alt=\"{visits:06}\"></p>\n\
         <p><font size=\"2\">Meilleur affichage en 800x600, 256 couleurs, avec Netscape Navigator 4 ou Internet Explorer 4.<br>\
         Site réalisé avec le Bloc-notes. Signez notre livre d'or !<br>\
         &copy; 1998 Tinyverse - tous droits réservés</font></p>\n</center>\n"
    );
    s.push_str(END);
    s
}

/// Habillage d'une page allégée : bandeau en haut (poids avant/après,
/// lien vers l'original), éventuelle remarque, puis le corps simplifié.
pub fn simplified(title: &str, body: &str, original: &Url, before: usize, notice: Option<&str>) -> String {
    let title = if title.is_empty() { original.host_str().unwrap_or("Sans titre") } else { title };
    let mut s = head(title);
    let orig = escape_attr(original.as_str());
    s.push_str("<table width=\"100%\" border=\"0\" cellpadding=\"2\" cellspacing=\"0\"><tr><td bgcolor=\"#000080\"><font color=\"#ffffff\" size=\"2\">");
    let _ = write!(
        s,
        "<b>Tinyverse 1998</b> &mdash; page allégée de {} à @TAILLE@ &mdash; \
         [<a href=\"{orig}\"><font color=\"#ffff00\">voir l'original</font></a>] \
         [<a href=\"/\"><font color=\"#ffff00\">accueil</font></a>]",
        size(before)
    );
    s.push_str("</font></td></tr></table>\n");
    if let Some(n) = notice {
        s.push_str("<p><font size=\"2\" color=\"#800000\"><i>");
        escape_text(&mut s, n);
        s.push_str("</i></font></p>\n");
    }
    s.push_str(body);
    s.push('\n');
    s.push_str(END);
    // poids final (le bandeau compris, à quelques octets près)
    let after = size(s.len() - "@TAILLE@".len() + 6);
    s.replacen("@TAILLE@", &after, 1)
}

/// Page d'erreur rétro. `original` : adresse demandée ; `alt` : lien
/// proposé (texte, cible).
pub fn error(code: u16, title: &str, message: &str, original: Option<&Url>, alt: Option<(&str, &str)>) -> String {
    let mut s = head(&format!("{code} {title}"));
    s.push_str("<h1>");
    escape_text(&mut s, title);
    s.push_str("</h1>\n<p>");
    escape_text(&mut s, message);
    s.push_str("</p>\n");
    if let Some(u) = original {
        s.push_str("<p>Adresse demandée : <tt>");
        escape_text(&mut s, u.as_str());
        s.push_str("</tt></p>\n");
    }
    if let Some((label, href)) = alt {
        let _ = write!(s, "<p><a href=\"{}\">", escape_attr(href));
        escape_text(&mut s, label);
        s.push_str("</a></p>\n");
    }
    let _ = write!(
        s,
        "<p><a href=\"/\">Retour à la page d'accueil</a></p>\n<hr>\n\
         <address>Tinyverse/1998 Server at web1998 Port 80 (erreur {code})</address>\n"
    );
    s.push_str(END);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailles_lisibles() {
        assert_eq!(size(812), "812 o");
        assert_eq!(size(4300), "4,2 Ko");
        assert_eq!(size(524_288), "512 Ko");
        assert_eq!(size(3 * 1_048_576 + 300_000), "3,3 Mo");
    }

    #[test]
    fn accueil_et_bandeau() {
        let h = home(42);
        assert!(h.starts_with("<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 3.2 Final//EN\">"));
        assert!(h.contains("bgcolor=\"#c0c0c0\"") && h.contains("Times"));
        assert!(h.contains("/compteur.gif?n=000042") && h.contains("/construction.gif"));
        assert!(h.contains("<form action=\"/p\" method=\"get\"><b>Aller à :</b> <input type=\"text\" name=\"u\""));
        for site in ["/p?u=https://frogfind.com/", "/p?u=https://www.franceinfo.fr/", "/p?u=https://fr.wikipedia.org/", "/p?u=https://news.ycombinator.com/"] {
            assert!(h.contains(site), "{site}");
        }
        let u = Url::parse("https://a.fr/x?y=1&z=2").unwrap();
        let p = simplified("Titre <x>", "<p>corps</p>", &u, 524_288, Some("page tronquée"));
        assert!(p.contains("<title>Titre &lt;x&gt;</title>"));
        assert!(p.contains("page allégée de 512 Ko à "), "{p}");
        assert!(!p.contains("@TAILLE@"));
        assert!(p.contains("<a href=\"https://a.fr/x?y=1&amp;z=2\"><font color=\"#ffff00\">voir l'original</font></a>"));
        assert!(p.contains("page tronquée") && p.contains("<p>corps</p>"));
        let e = error(404, "Fichier introuvable", "L'URL demandée n'a pas été trouvée.", Some(&u), Some(("Essayer", "/p?u=x")));
        assert!(e.contains("<h1>Fichier introuvable</h1>") && e.contains("erreur 404"));
        assert!(e.contains("<p><a href=\"/p?u=x\">Essayer</a></p>"));
    }
}
