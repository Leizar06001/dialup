//! Simplification d'une page web moderne en HTML 3.2 léger.
//!
//! La page est analysée par `html5ever` (via `scraper`, tolérant comme un
//! vrai navigateur), puis l'arbre est parcouru et réécrit avec une liste
//! blanche de balises :
//! - gardés : titres, paragraphes, sauts de ligne, listes, citations,
//!   texte préformaté, tableaux (sans `thead`/`tbody`), gras, italique,
//!   souligné, barré, indices, liens, images, formulaires GET simples ;
//! - supprimés avec leur contenu : scripts, styles, `iframe`, vidéos,
//!   sons, `svg`, `canvas`, objets, éléments cachés, publicités et
//!   bandeaux évidents (classes ou identifiants `ad`, `ads`, `pub`,
//!   `sponsor`, `cookie`, `consent`, `newsletter`…) ;
//! - tout le reste (`div`, `span`, `section`…) disparaît mais son contenu
//!   reste, avec un saut de ligne pour les blocs ;
//! - attributs : seulement les utiles (`href`, `alt`, `colspan`…), aucun
//!   style, aucune classe, aucun gestionnaire d'événement.
//!
//! Liens : `/p?u=<url absolue>` (la page sera simplifiée à son tour),
//! images : `/i?u=<url>` (converties en GIF ; [`with_images`] les envoie
//! vers `/h` en mode « V.Hallucination »), formulaires GET : action
//! `/f` avec l'action d'origine en champ caché `u`. Les ancres internes
//! (`href="#x"` vers `id="x"`) deviennent des `<a name="x">`.
//!
//! Si la page a un élément `<main>` (ou `role="main"`) qui porte l'essentiel
//! du texte, seul celui-ci est rendu : menus, en-têtes et pieds de page du
//! site sautent, ce qui allège beaucoup.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::Write as _;

use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};
use scraper::node::Element;
use scraper::{ElementRef, Html, Node};
use url::Url;

use super::ImageMode;

/// Images affichées au plus par page ; au-delà, un simple lien « [image] ».
pub const MAX_IMAGES: usize = 30;
/// Profondeur maximale de l'arbre parcouru (pile bornée).
const MAX_DEPTH: usize = 120;

/// Résultat de la simplification.
#[derive(Clone, Debug, Default)]
pub struct Simplified {
    /// Titre de la page (`<title>`), espaces normalisés
    pub title: String,
    /// Corps HTML 3.2 (sans `<html>`/`<body>` ni bandeau)
    pub body: String,
    /// Images gardées (réécrites vers `/i`)
    pub images: usize,
}

/// Décode le HTML en texte : BOM, sinon `charset` de l'en-tête HTTP, sinon
/// `<meta charset>` dans les premiers Ko, sinon UTF-8 s'il est valide,
/// sinon Windows-1252 (le « Latin-1 » du web de 1998).
pub fn decode(bytes: &[u8], header_charset: Option<&str>) -> String {
    if let Some((enc, bom)) = Encoding::for_bom(bytes) {
        return enc.decode_without_bom_handling(&bytes[bom..]).0.into_owned();
    }
    let enc = header_charset
        .and_then(|c| Encoding::for_label(c.trim().as_bytes()))
        .or_else(|| sniff_meta_charset(&bytes[..bytes.len().min(4096)]))
        .unwrap_or_else(|| match std::str::from_utf8(bytes) {
            Ok(_) => UTF_8,
            // séquence coupée en fin de tampon (page tronquée) : c'est de l'UTF-8
            Err(e) if e.error_len().is_none() => UTF_8,
            Err(_) => WINDOWS_1252,
        });
    // UTF-16 annoncé dans un <meta> d'une page lue en octets : impossible, c'est de l'UTF-8
    let enc = enc.output_encoding();
    enc.decode_without_bom_handling(bytes).0.into_owned()
}

fn sniff_meta_charset(head: &[u8]) -> Option<&'static Encoding> {
    let lower: Vec<u8> = head.iter().map(u8::to_ascii_lowercase).collect();
    let pos = lower.windows(8).position(|w| w == b"charset=")?;
    let rest = &lower[pos + 8..];
    let rest = rest.strip_prefix(b"\"").or_else(|| rest.strip_prefix(b"'")).unwrap_or(rest);
    let end = rest.iter().position(|c| !(c.is_ascii_alphanumeric() || b"-_:.".contains(c))).unwrap_or(rest.len());
    Encoding::for_label(&rest[..end])
}

/// Paramètre d'URL encodé au plus court : seuls les caractères qui
/// gêneraient (`%`, `&`, `+`, `#`, espaces, guillemets…) sont échappés ;
/// `:` et `/` restent lisibles, et les suites `%XX` qui codent de l'UTF-8
/// non ASCII sont écrites en clair (`R%C3%A9seau` → `Réseau` : la page est
/// en UTF-8, le navigateur les réencode à l'identique).
pub fn encode_param(s: &str) -> String {
    let decoded = decode_utf8_escapes(s);
    let mut out = String::with_capacity(decoded.len() + 8);
    for c in decoded.chars() {
        if c.is_ascii_alphanumeric() || "-._~:/?=;,@!$()*'[]".contains(c) || (!c.is_ascii() && !c.is_control()) {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Remplace les suites `%XX` d'octets ≥ 0x80 formant de l'UTF-8 valide
/// par les caractères correspondants ; le reste est inchangé.
fn decode_utf8_escapes(s: &str) -> String {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        let mut raw = Vec::new();
        let mut j = i;
        while j + 2 < b.len() && b[j] == b'%' {
            match (hex(b[j + 1]), hex(b[j + 2])) {
                (Some(h), Some(l)) if (h << 4 | l) >= 0x80 => {
                    raw.push(h << 4 | l);
                    j += 3;
                }
                _ => break,
            }
        }
        match std::str::from_utf8(&raw) {
            Ok(t) if !raw.is_empty() && !t.chars().any(char::is_control) => {
                out.push_str(t);
                i = j;
            }
            _ => {
                // un caractère tel quel (s est de l'UTF-8 valide)
                let c = s[i..].chars().next().expect("indice sur une frontière de caractère");
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

/// Lien vers la version simplifiée d'une page.
pub fn page_link(url: &Url) -> String {
    format!("/p?u={}", encode_param(url.as_str()))
}

/// Lien vers la version convertie d'une image.
pub fn image_link(url: &Url) -> String {
    image_link_for(url, ImageMode::Normal)
}

/// Lien vers une image, GIF (`/i`) ou hallucinée (`/h`) selon le mode.
pub fn image_link_for(url: &Url, mode: ImageMode) -> String {
    format!("{}?u={}", mode.image_path(), encode_param(url.as_str()))
}

/// Liens d'images d'un corps simplifié selon le mode : [`simplify`] les
/// écrit vers `/i`, « V.Hallucination » les veut vers `/h`. Sans ambiguïté :
/// textes et attributs étant échappés, un `<` en clair n'ouvre que nos
/// propres balises, donc `<img src="/i?u=` et `<a href="/i?u=` (images en
/// surnombre) ne peuvent être que nos liens d'images.
pub fn with_images(body: &str, mode: ImageMode) -> Cow<'_, str> {
    if mode == ImageMode::Normal {
        return Cow::Borrowed(body);
    }
    let path = mode.image_path();
    Cow::Owned(body.replace("<img src=\"/i?u=", &format!("<img src=\"{path}?u=")).replace("<a href=\"/i?u=", &format!("<a href=\"{path}?u=")))
}

/// Échappe un texte pour le contenu HTML.
pub fn escape_text(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}

/// Échappe une valeur d'attribut (entre guillemets doubles).
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

fn collapse(s: &str) -> String {
    s.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

fn pct_decode(s: &str) -> String {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Jetons de classe / d'identifiant qui trahissent une publicité, un
/// bandeau de cookies, des boutons de partage, etc.
const JUNK_TOKENS: &[&str] = &[
    "ad", "ads", "advert", "adverts", "advertisement", "advertising", "adsbygoogle", "sponsor", "sponsored", "pub",
    "publicite", "dfp", "outbrain", "taboola", "cookie", "cookies", "consent", "gdpr", "cmp", "newsletter", "popup",
    "modal", "share", "sharing", "social", "noprint", "editsection", "navbox", "dropdown", "interlanguage", "toolbar",
    "portlet",
];

/// Classes entières d'éléments réservés aux lecteurs d'écran ou aux
/// raccourcis clavier (invisibles à l'écran).
const JUNK_CLASSES: &[&str] = &["sr-only", "visually-hidden", "visuallyhidden", "screen-reader-text", "skip-link", "mw-jump-link"];

fn is_junk(el: &Element) -> bool {
    if el.attr("class").is_some_and(|c| c.split_ascii_whitespace().any(|c| JUNK_CLASSES.iter().any(|j| c.eq_ignore_ascii_case(j)))) {
        return true;
    }
    [el.attr("class"), el.attr("id")].into_iter().flatten().any(|v| {
        v.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|t| t.len() >= 2 && JUNK_TOKENS.iter().any(|j| t.eq_ignore_ascii_case(j)))
    })
}

fn is_hidden(el: &Element) -> bool {
    el.attr("hidden").is_some()
        || el.attr("aria-hidden").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
        || el.attr("style").is_some_and(|s| {
            let s: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
            s.contains("display:none") || s.contains("visibility:hidden")
        })
}

/// Longueur du texte visible (hors scripts et styles), sans récursion.
fn text_len(e: ElementRef) -> usize {
    let mut total = 0;
    let mut stack = vec![*e];
    while let Some(n) = stack.pop() {
        match n.value() {
            Node::Text(t) => total += t.trim().len(),
            Node::Element(el) if matches!(el.name(), "script" | "style" | "noscript" | "template") => {}
            _ => stack.extend(n.children()),
        }
    }
    total
}

fn find<'a>(root: ElementRef<'a>, pred: impl Fn(&Element) -> bool) -> Option<ElementRef<'a>> {
    root.descendants().filter_map(ElementRef::wrap).find(|e| pred(e.value()))
}

/// Simplifie une page. `page` : son URL (pour résoudre les liens relatifs).
pub fn simplify(html: &str, page: &Url) -> Simplified {
    let doc = Html::parse_document(html);
    let root = doc.root_element();
    let mut page = page.clone();
    page.set_fragment(None);
    let base = find(root, |e| e.name() == "base" && e.attr("href").is_some())
        .and_then(|b| page.join(b.value().attr("href")?.trim()).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .unwrap_or_else(|| page.clone());
    let title = find(root, |e| e.name() == "title")
        .map(|t| collapse(&t.text().collect::<String>()))
        .unwrap_or_default();
    let title: String = title.chars().take(150).collect();

    let mut w = Walker::new(base, page);
    // ancres réellement visées par un lien interne
    for a in root.descendants().filter_map(ElementRef::wrap).filter(|e| e.value().name() == "a") {
        if let Some(frag) = a.value().attr("href").and_then(|h| w.rewrite_link(h)).and_then(|l| l.strip_prefix('#').map(str::to_string)) {
            w.anchors.insert(pct_decode(&frag));
        }
    }
    // redirection par <meta http-equiv="refresh" content="0; url=…">
    if let Some(m) = find(root, |e| e.name() == "meta" && e.attr("http-equiv").is_some_and(|v| v.eq_ignore_ascii_case("refresh"))) {
        let content = m.value().attr("content").unwrap_or("");
        let target = content.split(';').skip(1).find_map(|p| {
            let p = p.trim();
            let (k, v) = p.split_once('=')?;
            k.trim().eq_ignore_ascii_case("url").then(|| v.trim().trim_matches(|c| c == '"' || c == '\''))
        });
        if let Some(link) = target.and_then(|t| w.rewrite_link(t)) {
            let _ = writeln!(w.out, "<p><b>Cette page redirige vers <a href=\"{}\">la page suivante</a>.</b></p>", escape_attr(&link));
            w.block = true;
        }
    }

    let body = find(root, |e| e.name() == "body").unwrap_or(root);
    let main = find(body, |e| e.name() == "main" || e.attr("role").is_some_and(|r| r.eq_ignore_ascii_case("main")));
    let start = match main {
        // <main> trop maigre : on garde toute la page
        Some(m) if text_len(m) * 10 >= text_len(body) * 3 => {
            w.drop_nav = true; // barres de navigation internes au contenu : inutiles
            m
        }
        _ => body,
    };
    w.children(start, 0);
    let body = w.out.trim().to_string();
    Simplified { title, body, images: w.images }
}

struct Walker {
    base: Url,
    page: Url,
    out: String,
    anchors: HashSet<String>,
    /// dans un <pre>
    pre: usize,
    /// dans un formulaire GET gardé
    form: bool,
    /// dans un lien gardé (pas de liens imbriqués)
    link: bool,
    /// la sortie est au début d'un bloc (pas de <br> nécessaire)
    block: bool,
    /// le dernier caractère émis est un espace (ou début de bloc)
    space: bool,
    images: usize,
    /// supprimer les <nav> (rendu du seul <main>)
    drop_nav: bool,
}

impl Walker {
    fn new(base: Url, page: Url) -> Self {
        Self { base, page, out: String::new(), anchors: HashSet::new(), pre: 0, form: false, link: false, block: true, space: true, images: 0, drop_nav: false }
    }

    /// Cible réécrite d'un lien, ou `None` s'il ne mène nulle part d'utile
    /// (`javascript:`, protocole inconnu…).
    fn rewrite_link(&self, href: &str) -> Option<String> {
        let href = href.trim();
        if href.is_empty() {
            return None;
        }
        if let Some(frag) = href.strip_prefix('#') {
            return (!frag.is_empty()).then(|| format!("#{frag}"));
        }
        let lower = href.to_ascii_lowercase();
        if lower.starts_with("mailto:") {
            return Some(href.to_string());
        }
        let mut u = self.base.join(href).ok()?;
        if !matches!(u.scheme(), "http" | "https") {
            return None;
        }
        let frag = u.fragment().map(str::to_string);
        u.set_fragment(None);
        if u == self.page {
            if let Some(f) = frag.as_deref().filter(|f| !f.is_empty()) {
                return Some(format!("#{f}"));
            }
        }
        // toujours /p : une page « Fichier:X.jpg » est du HTML, et /p
        // redirige les vraies images vers /i
        let mut link = page_link(&u);
        if let Some(f) = frag.filter(|f| !f.is_empty()) {
            link.push('#');
            link.push_str(&f);
        }
        Some(link)
    }

    fn text(&mut self, t: &str) {
        if self.pre > 0 {
            escape_text(&mut self.out, t);
            if !t.is_empty() {
                self.block = false;
                self.space = t.ends_with(|c: char| c.is_ascii_whitespace());
            }
            return;
        }
        for c in t.chars() {
            if c.is_ascii_whitespace() {
                if !self.space {
                    self.out.push(' ');
                    self.space = true;
                }
            } else {
                match c {
                    '&' => self.out.push_str("&amp;"),
                    '<' => self.out.push_str("&lt;"),
                    '>' => self.out.push_str("&gt;"),
                    c => self.out.push(c),
                }
                self.space = false;
                self.block = false;
            }
        }
    }

    /// Saut de ligne si du contenu « en ligne » précède.
    fn line_break(&mut self) {
        if !self.block {
            self.out.push_str("<br>\n");
            self.block = true;
            self.space = true;
        }
    }

    fn children(&mut self, e: ElementRef, depth: usize) {
        let structural = matches!(e.value().name(), "table" | "thead" | "tbody" | "tfoot" | "tr" | "ul" | "ol" | "dl" | "select" | "menu" | "dir");
        for child in e.children() {
            match child.value() {
                Node::Text(t) => {
                    if !(structural && t.trim().is_empty()) {
                        self.text(t);
                    }
                }
                Node::Element(_) => {
                    if let Some(c) = ElementRef::wrap(child) {
                        self.element(c, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    /// Balise gardée : `<tag attrs>` contenu `</tag>`. Un élément resté vide
    /// est retiré, sauf `keep_empty` (cellules de tableau, ancres).
    fn kept(&mut self, e: ElementRef, tag: &str, attrs: &str, block: bool, keep_empty: bool, depth: usize) {
        let saved = (self.out.len(), self.block, self.space);
        let _ = write!(self.out, "<{tag}{attrs}>");
        if block {
            self.block = true;
            self.space = true;
        }
        let inner = self.out.len();
        self.children(e, depth);
        if !keep_empty && self.out[inner..].trim().is_empty() {
            self.out.truncate(saved.0);
            (self.block, self.space) = (saved.1, saved.2);
            return;
        }
        let _ = write!(self.out, "</{tag}>");
        if block {
            self.out.push('\n');
            self.block = true;
            self.space = true;
        }
    }

    fn element(&mut self, e: ElementRef, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let el = e.value();
        let name = el.name();
        if !matches!(name, "html" | "body" | "main" | "article") && (is_hidden(el) || is_junk(el)) {
            return;
        }
        if name == "nav" && self.drop_nav {
            return;
        }
        if let Some(id) = el.id() {
            if self.anchors.remove(id) {
                let _ = write!(self.out, "<a name=\"{}\"></a>", escape_attr(id));
            }
        }
        match name {
            // supprimés avec leur contenu
            "script" | "style" | "noscript" | "iframe" | "frame" | "frameset" | "object" | "embed" | "applet" | "video"
            | "audio" | "source" | "track" | "canvas" | "svg" | "math" | "template" | "link" | "meta" | "head" | "title"
            | "map" | "area" | "dialog" | "param" | "noembed" | "noframes" | "base" | "slot" | "portal" | "colgroup"
            | "col" | "datalist" | "optgroup" | "option" | "progress" | "meter" => {}
            "br" => {
                self.out.push_str("<br>\n");
                self.block = true;
                self.space = true;
            }
            "hr" => {
                self.out.push_str("<hr>\n");
                self.block = true;
                self.space = true;
            }
            "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let attrs = align_attr(el);
                self.kept(e, name, &attrs, true, false, depth);
            }
            "blockquote" | "center" | "address" | "dl" | "dt" | "dd" | "li" | "caption" | "ul" => {
                self.kept(e, name, "", true, false, depth)
            }
            "menu" | "dir" => self.kept(e, "ul", "", true, false, depth),
            "ol" => {
                let mut attrs = String::new();
                if let Some(s) = el.attr("start").and_then(|s| s.trim().parse::<i64>().ok()) {
                    let _ = write!(attrs, " start=\"{s}\"");
                }
                if let Some(t) = el.attr("type").filter(|t| matches!(*t, "1" | "a" | "A" | "i" | "I")) {
                    let _ = write!(attrs, " type=\"{t}\"");
                }
                self.kept(e, "ol", &attrs, true, false, depth)
            }
            "pre" | "listing" | "xmp" | "plaintext" => {
                self.pre += 1;
                self.kept(e, "pre", "", true, false, depth);
                self.pre -= 1;
            }
            "b" | "strong" | "mark" => self.kept(e, "b", "", false, false, depth),
            "i" | "em" | "cite" | "dfn" | "var" => self.kept(e, "i", "", false, false, depth),
            "u" | "ins" => self.kept(e, "u", "", false, false, depth),
            "s" | "strike" | "del" => self.kept(e, "strike", "", false, false, depth),
            "tt" | "code" | "kbd" | "samp" => self.kept(e, "tt", "", false, false, depth),
            "sub" | "sup" | "small" | "big" => self.kept(e, name, "", false, false, depth),
            "a" => self.anchor(e, depth),
            "img" => self.image(el),
            "table" => {
                let border = el.attr("border").is_some_and(|b| !matches!(b.trim(), "" | "0")) || has_header_cells(e);
                self.kept(e, "table", if border { " border=\"1\"" } else { "" }, true, false, depth)
            }
            "tr" => self.kept(e, "tr", "", true, false, depth),
            "td" | "th" => {
                let mut attrs = String::new();
                for a in ["colspan", "rowspan"] {
                    if let Some(n) = el.attr(a).and_then(|v| v.trim().parse::<u16>().ok()).filter(|&n| (2..=100).contains(&n)) {
                        let _ = write!(attrs, " {a}=\"{n}\"");
                    }
                }
                attrs.push_str(&align_attr(el));
                let saved = self.block;
                self.kept(e, name, &attrs, false, true, depth);
                // une cellule n'impose pas de saut de ligne après elle
                self.block = saved;
                self.space = true;
            }
            "form" => self.form(e, depth),
            "input" | "select" | "textarea" | "button" => {
                if self.form {
                    self.control(e);
                }
            }
            // blocs sans équivalent : contenu gardé, sauts de ligne autour
            "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside" | "figure" | "figcaption"
            | "details" | "summary" | "fieldset" | "legend" | "hgroup" | "search" | "body" | "html" => {
                self.line_break();
                self.children(e, depth);
                self.line_break();
            }
            // le reste (span, font, label, abbr, picture, thead, tbody…) : contenu seul
            _ => self.children(e, depth),
        }
    }

    fn anchor(&mut self, e: ElementRef, depth: usize) {
        let el = e.value();
        let href = el.attr("href").and_then(|h| self.rewrite_link(h));
        if self.link || href.is_none() {
            // lien imbriqué ou sans cible utile : le texte seul
            if let Some(n) = el.attr("name").filter(|n| !n.trim().is_empty() && !self.link) {
                let _ = write!(self.out, "<a name=\"{}\"></a>", escape_attr(n.trim()));
            }
            return self.children(e, depth);
        }
        let href = href.unwrap_or_default();
        let mut attrs = format!(" href=\"{}\"", escape_attr(&href));
        if let Some(n) = el.attr("name").filter(|n| !n.trim().is_empty()) {
            let _ = write!(attrs, " name=\"{}\"", escape_attr(n.trim()));
        }
        self.link = true;
        self.kept(e, "a", &attrs, false, false, depth);
        self.link = false;
    }

    fn image_url(&self, el: &Element) -> Option<Url> {
        let ok = |v: &str| -> Option<Url> {
            let v = v.trim();
            if v.is_empty() || v.to_ascii_lowercase().starts_with("data:") {
                return None;
            }
            self.base.join(v).ok().filter(|u| matches!(u.scheme(), "http" | "https"))
        };
        for a in ["data-src", "data-lazy-src", "data-original", "src"] {
            if let Some(u) = el.attr(a).and_then(ok) {
                return Some(u);
            }
        }
        for a in ["srcset", "data-srcset"] {
            if let Some(u) = el.attr(a).and_then(pick_srcset).and_then(ok) {
                return Some(u);
            }
        }
        None
    }

    fn image(&mut self, el: &Element) {
        let tiny = ["width", "height"].iter().any(|a| el.attr(a).and_then(|v| v.trim().trim_end_matches("px").parse::<u32>().ok()).is_some_and(|n| n <= 2));
        if tiny {
            return; // pixel espion
        }
        let alt = collapse(el.attr("alt").unwrap_or(""));
        let alt: String = alt.chars().take(100).collect();
        let Some(url) = self.image_url(el) else {
            if !alt.is_empty() {
                self.text(&format!("[{alt}]"));
            }
            return;
        };
        if url.path().to_ascii_lowercase().ends_with(".svg") {
            if !alt.is_empty() {
                self.text(&format!("[{alt}]"));
            }
            return;
        }
        self.images += 1;
        let src = escape_attr(&image_link(&url));
        if self.images > MAX_IMAGES {
            // trop d'images : un lien à cliquer à la place
            let label = if alt.is_empty() { "[image]".to_string() } else { format!("[image : {alt}]") };
            if self.link {
                self.text(&label);
            } else {
                let _ = write!(self.out, "<a href=\"{src}\">");
                self.text(&label);
                self.out.push_str("</a>");
            }
            return;
        }
        let _ = write!(self.out, "<img src=\"{src}\" alt=\"{}\">", escape_attr(&alt));
        self.block = false;
        self.space = false;
    }

    fn form(&mut self, e: ElementRef, depth: usize) {
        let el = e.value();
        let get = el.attr("method").is_none_or(|m| m.trim().eq_ignore_ascii_case("get") || m.trim().is_empty());
        let action = match el.attr("action").map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) => self.base.join(a).ok(),
            None => Some(self.page.clone()),
        }
        .filter(|u| matches!(u.scheme(), "http" | "https"));
        match action {
            Some(mut action) if get && !self.form => {
                action.set_fragment(None);
                self.line_break();
                let _ = write!(
                    self.out,
                    "<form action=\"/f\" method=\"get\"><input type=\"hidden\" name=\"u\" value=\"{}\">",
                    escape_attr(action.as_str())
                );
                self.form = true;
                self.children(e, depth);
                self.form = false;
                self.out.push_str("</form>\n");
                self.block = true;
                self.space = true;
            }
            // formulaire POST (connexion, commentaires…) : pas de champs,
            // mais le texte qu'il contient reste (certains sites mettent
            // toute la page dans un formulaire)
            _ => self.children(e, depth),
        }
    }

    fn control(&mut self, e: ElementRef) {
        let el = e.value();
        let attr = |k: &str| el.attr(k).map(escape_attr);
        let name = attr("name").filter(|n| !n.is_empty());
        let value = attr("value").unwrap_or_default();
        match el.name() {
            "input" => {
                let ty = el.attr("type").unwrap_or("text").trim().to_ascii_lowercase();
                match ty.as_str() {
                    "text" | "search" | "email" | "url" | "tel" | "number" | "" => {
                        let Some(name) = name else { return };
                        let size = el.attr("size").and_then(|s| s.trim().parse::<u16>().ok()).unwrap_or(30).clamp(5, 40);
                        let _ = write!(self.out, "<input type=\"text\" name=\"{name}\" value=\"{value}\" size=\"{size}\">");
                    }
                    "hidden" => {
                        let Some(name) = name else { return };
                        let _ = write!(self.out, "<input type=\"hidden\" name=\"{name}\" value=\"{value}\">");
                        return;
                    }
                    "checkbox" | "radio" => {
                        let Some(name) = name else { return };
                        let checked = if el.attr("checked").is_some() { " checked" } else { "" };
                        let value = if value.is_empty() { "on".to_string() } else { value };
                        let _ = write!(self.out, "<input type=\"{ty}\" name=\"{name}\" value=\"{value}\"{checked}>");
                    }
                    "submit" | "image" => {
                        let label = if value.is_empty() { attr("alt").filter(|a| !a.is_empty()).unwrap_or_else(|| "Envoyer".into()) } else { value };
                        let name = name.map(|n| format!(" name=\"{n}\"")).unwrap_or_default();
                        let _ = write!(self.out, "<input type=\"submit\"{name} value=\"{label}\">");
                    }
                    _ => return,
                }
            }
            "select" => {
                let Some(name) = name else { return };
                let _ = write!(self.out, "<select name=\"{name}\">");
                for o in e.descendants().filter_map(ElementRef::wrap).filter(|o| o.value().name() == "option") {
                    let text = collapse(&o.text().collect::<String>());
                    let v = o.value().attr("value").map(escape_attr).unwrap_or_else(|| escape_attr(&text));
                    let sel = if o.value().attr("selected").is_some() { " selected" } else { "" };
                    let _ = write!(self.out, "<option value=\"{v}\"{sel}>");
                    escape_text(&mut self.out, &text);
                }
                self.out.push_str("</select>");
            }
            "textarea" => {
                let Some(name) = name else { return };
                let _ = write!(self.out, "<textarea name=\"{name}\" rows=\"4\" cols=\"40\">");
                escape_text(&mut self.out, &e.text().collect::<String>());
                self.out.push_str("</textarea>");
            }
            "button" => {
                let ty = el.attr("type").unwrap_or("submit").trim().to_ascii_lowercase();
                if ty != "submit" {
                    return;
                }
                let label: String = collapse(&e.text().collect::<String>()).chars().take(40).collect();
                let label = if label.is_empty() { "Envoyer".to_string() } else { escape_attr(&label) };
                let _ = write!(self.out, "<input type=\"submit\" value=\"{label}\">");
            }
            _ => return,
        }
        self.out.push(' ');
        self.block = false;
        self.space = true;
    }
}

fn align_attr(el: &Element) -> String {
    match el.attr("align").map(|a| a.trim().to_ascii_lowercase()) {
        Some(a) if matches!(a.as_str(), "left" | "center" | "right") => format!(" align=\"{a}\""),
        _ => String::new(),
    }
}

/// Tableau de données (à bordure) : il a des cellules d'en-tête.
fn has_header_cells(table: ElementRef) -> bool {
    let is = |n: &ElementRef, name: &str| n.value().name() == name;
    table.children().filter_map(ElementRef::wrap).any(|c| {
        let rows: Vec<ElementRef> = if is(&c, "tr") {
            vec![c]
        } else if matches!(c.value().name(), "thead" | "tbody" | "tfoot") {
            c.children().filter_map(ElementRef::wrap).filter(|r| is(r, "tr")).take(3).collect()
        } else {
            Vec::new()
        };
        rows.iter().any(|r| r.children().filter_map(ElementRef::wrap).any(|cell| is(&cell, "th")))
    })
}

/// Choisit dans un `srcset` la plus petite image d'au moins 320 px de large
/// (sinon la plus grande disponible, ou la première).
fn pick_srcset(srcset: &str) -> Option<&str> {
    let cands: Vec<(&str, Option<u32>)> = srcset
        .split(',')
        .filter_map(|c| {
            let mut it = c.split_ascii_whitespace();
            let url = it.next()?;
            let w = it.next().and_then(|d| d.strip_suffix('w')).and_then(|n| n.parse().ok());
            Some((url, w))
        })
        .collect();
    let widths: Vec<(&str, u32)> = cands.iter().filter_map(|&(u, w)| Some((u, w?))).collect();
    if !widths.is_empty() {
        if let Some(&(u, _)) = widths.iter().filter(|(_, w)| *w >= 320).min_by_key(|(_, w)| *w) {
            return Some(u);
        }
        return widths.iter().max_by_key(|(_, w)| *w).map(|&(u, _)| u);
    }
    cands.first().map(|&(u, _)| u)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r##"<!DOCTYPE html>
<html lang="fr"><head>
<meta charset="utf-8"><title>  Le   Journal
 du Web </title>
<link rel="stylesheet" href="/s.css"><style>body{color:red}</style>
<script>alert("pirate")</script>
<script src="/tracker.js"></script>
</head>
<body class="has-cookie-banner">
<div class="cookie-banner">Acceptez nos cookies !</div>
<nav><a href="/">Accueil</a> <a href="javascript:void(0)">Menu</a></nav>
<div class="wrapper"><div><div>
<h1 class="titre" style="font-size:3em" onclick="x()">Titre &amp; sous-titre</h1>
<p>Un <b>gras</b>, un <strong>fort</strong>, un <em>italique</em> et un <a href="article.html?id=4&amp;x=1#suite">lien relatif</a>.</p>
<p><a href="#notes">aller aux notes</a> — <a href="https://example.org/journal/page.html#notes">même page</a></p>
<img src="/img/photo.jpg" alt="Une photo" width="800" height="600" class="hero">
<img src="data:image/gif;base64,R0lGOD" data-src="https://cdn.example.org/lazy.png" alt="paresseuse">
<img src="/pixel.gif" width="1" height="1">
<img src="/logo.svg" alt="Logo">
<iframe src="https://ads.example.com/"></iframe>
<video src="film.mp4"></video>
<div class="ad-slot">ACHETEZ MAINTENANT</div>
<div id="sponsored-box">Contenu sponsorisé</div>
<aside aria-hidden="true">caché</aside>
<p style="display: none">invisible</p>
<ul><li>un</li><li>deux <i>italique</i></li><li></li></ul>
<table class="wikitable"><thead><tr><th>Nom</th><th>Valeur</th></tr></thead>
<tbody><tr><td colspan="2" style="color:red">large</td></tr></tbody></table>
<form action="/recherche" method="get"><input type="search" name="q" placeholder="Chercher"><input type="hidden" name="lang" value="fr"><button>OK</button></form>
<form action="/connexion" method="post"><label>Identifiant</label><input name="login"><input type="password" name="pw"></form>
<div id="notes"><p>Les notes.</p></div>
<pre>  code   préformaté
  ligne 2</pre>
<p>Texte avec        espaces
   multiples et caractères spéciaux : é à ü « » &lt;balise&gt;</p>
</div></div></div>
<noscript><img src="/noscript.gif"></noscript>
<footer>Pied de page</footer>
</body></html>"##;

    fn page_url() -> Url {
        Url::parse("https://example.org/journal/page.html").unwrap()
    }

    #[test]
    fn scripts_styles_et_publicites_supprimes() {
        let s = simplify(PAGE, &page_url());
        assert_eq!(s.title, "Le Journal du Web");
        let b = &s.body;
        for absent in [
            "<script", "alert(", "tracker", "<style", "color:red", "<link", "<iframe", "<video", "film.mp4", "ACHETEZ",
            "sponsorisé", "cookies", "caché", "invisible", "onclick", "class=", "style=", "javascript", "<div", "<span",
            "noscript", "<thead", "<tbody", "<nav", "type=\"password\"", "name=\"login\"",
        ] {
            assert!(!b.contains(absent), "« {absent} » devrait avoir disparu :\n{b}");
        }
        for present in ["Titre &amp; sous-titre", "<b>gras</b>", "<b>fort</b>", "<i>italique</i>", "Pied de page", "Les notes.", "Identifiant"] {
            assert!(b.contains(present), "« {present} » manque :\n{b}");
        }
        assert!(b.contains("<h1>Titre"), "titre sans attributs :\n{b}");
        assert!(b.contains("<ul><li>un</li>\n<li>deux <i>italique</i></li>\n</ul>"), "liste, élément vide retiré :\n{b}");
        assert!(b.contains("é à ü « » &lt;balise&gt;"), "texte UTF-8 et échappements");
        assert!(b.contains("Texte avec espaces multiples"), "espaces compactés");
        assert!(b.contains("<pre>  code   préformaté\n  ligne 2</pre>"), "pre intact :\n{b}");
    }

    #[test]
    fn liens_et_images_reecrits() {
        let s = simplify(PAGE, &page_url());
        let b = &s.body;
        assert!(b.contains("<a href=\"/p?u=https://example.org/journal/article.html?id=4%26x=1#suite\">lien relatif</a>"), "{b}");
        assert!(b.contains("<a href=\"/p?u=https://example.org/\">Accueil</a>"), "{b}");
        assert!(b.contains("<a href=\"#notes\">aller aux notes</a>"), "{b}");
        assert!(b.contains("<a href=\"#notes\">même page</a>"), "lien absolu vers la même page : ancre locale\n{b}");
        assert!(b.contains("<a name=\"notes\"></a>"), "ancre créée pour l'id visé\n{b}");
        assert!(!b.contains(">Menu</a>") && b.contains("Menu"), "lien javascript: réduit à son texte");
        assert!(b.contains("<img src=\"/i?u=https://example.org/img/photo.jpg\" alt=\"Une photo\">"), "{b}");
        assert!(b.contains("<img src=\"/i?u=https://cdn.example.org/lazy.png\" alt=\"paresseuse\">"), "data-src préféré au data: URI\n{b}");
        assert!(!b.contains("pixel.gif"), "pixel espion supprimé");
        assert!(!b.contains("logo.svg") && b.contains("[Logo]"), "SVG remplacé par son texte alternatif");
        assert!(!b.contains("width=") && !b.contains("height="));
        assert_eq!(s.images, 2);
        assert!(b.contains("<table border=\"1\"><tr><th>Nom</th><th>Valeur</th></tr>"), "{b}");
        assert!(b.contains("<td colspan=\"2\">large</td>"), "{b}");
        assert!(
            b.contains("<form action=\"/f\" method=\"get\"><input type=\"hidden\" name=\"u\" value=\"https://example.org/recherche\"><input type=\"text\" name=\"q\" value=\"\" size=\"30\"> <input type=\"hidden\" name=\"lang\" value=\"fr\"><input type=\"submit\" value=\"OK\">"),
            "{b}"
        );
        // aucune URL absolue d'origine hors des paramètres u= et du formulaire
        assert!(!b.contains("href=\"https://"), "{b}");
        assert!(!b.contains("src=\"https://"), "{b}");
    }

    #[test]
    fn main_prefere_et_trop_d_images() {
        let mut html = String::from("<body><nav>menu menu menu</nav><main><nav>sommaire</nav><h1>Article</h1><p>Le contenu principal de la page, assez long pour compter.</p><span class=\"sr-only\">lecteur</span>");
        for i in 0..MAX_IMAGES + 3 {
            html.push_str(&format!("<img src=\"/i{i}.png\">"));
        }
        html.push_str("</main><footer>pied</footer></body>");
        let s = simplify(&html, &Url::parse("http://a.fr/").unwrap());
        assert!(!s.body.contains("menu") && !s.body.contains("pied"), "{}", s.body);
        assert!(!s.body.contains("sommaire"), "nav interne à <main> supprimé : {}", s.body);
        assert!(!s.body.contains("lecteur"), "texte pour lecteurs d'écran supprimé");
        assert!(s.body.contains("<h1>Article</h1>"));
        assert_eq!(s.body.matches("<img ").count(), MAX_IMAGES);
        assert_eq!(s.body.matches(">[image]</a>").count(), 3);
        // <main> presque vide : toute la page est rendue
        let s = simplify("<body><main>x</main><p>beaucoup de texte ailleurs dans la page</p></body>", &Url::parse("http://a.fr/").unwrap());
        assert!(s.body.contains("beaucoup de texte"));
    }

    #[test]
    fn redirection_meta_et_base() {
        let s = simplify(
            "<html><head><base href=\"https://cdn.b.fr/x/\"><meta http-equiv=\"Refresh\" content=\"0; URL='https://b.fr/nouveau'\"></head><body><a href=\"y.html\">y</a></body></html>",
            &Url::parse("http://b.fr/").unwrap(),
        );
        assert!(s.body.contains("<a href=\"/p?u=https://b.fr/nouveau\">"), "{}", s.body);
        assert!(s.body.contains("<a href=\"/p?u=https://cdn.b.fr/x/y.html\">y</a>"), "{}", s.body);
    }

    #[test]
    fn decodage_des_jeux_de_caracteres() {
        let latin1 = b"<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=iso-8859-1\"></head><body>\xe9t\xe9 \xe0 Paris</body></html>";
        assert!(decode(latin1, None).contains("été à Paris"));
        assert!(decode(b"caf\xe9", Some("windows-1252")).contains("café"));
        assert_eq!(decode("déjà".as_bytes(), None), "déjà");
        assert_eq!(decode(b"\xef\xbb\xbfutf8 \xc3\xa9", Some("iso-8859-1")), "utf8 é", "le BOM l'emporte");
        assert_eq!(decode(b"sans d\xe9claration", None), "sans déclaration", "UTF-8 invalide : Windows-1252");
        assert_eq!(decode(b"coup\xc3", None), "coup\u{fffd}", "fin tronquée : reste de l'UTF-8");
    }

    #[test]
    fn encodage_des_parametres() {
        assert_eq!(encode_param("https://a.fr/b?c=1&d=e f+g#h%"), "https://a.fr/b?c=1%26d=e%20f%2Bg%23h%25");
        assert_eq!(encode_param("é\"<\u{85}"), "é%22%3C%C2%85");
        assert_eq!(encode_param("/wiki/R%C3%A9seau?q=a%26b%E9%c3%a9"), "/wiki/Réseau?q=a%2526b%25E9é", "UTF-8 en clair, le reste échappé");
        assert_eq!(encode_param("%C3"), "%25C3", "suite incomplète échappée");
        assert_eq!(pct_decode("%C3%A9t%C3%A9"), "été");
        assert_eq!(pct_decode("100%"), "100%");
        assert_eq!(pct_decode("%aé%zz"), "%aé%zz", "pas de coupure au milieu d'un caractère");
        assert_eq!(pick_srcset("a.jpg 100w, b.jpg 400w, c.jpg 1200w"), Some("b.jpg"));
        assert_eq!(pick_srcset("a.jpg 100w, b.jpg 200w"), Some("b.jpg"));
        assert_eq!(pick_srcset("a.jpg 1x, b.jpg 2x"), Some("a.jpg"));
    }

    #[test]
    fn liens_d_images_selon_le_mode() {
        let mut html = String::from("<p>Le code &lt;img src=\"/i?u=piege\"&gt; cité, et <a href=\"/i?u=lien\">un lien</a>.</p>");
        for i in 0..=MAX_IMAGES {
            html.push_str(&format!("<img src=\"/i{i}.png\" alt=\"n°{i}\">"));
        }
        let s = simplify(&html, &Url::parse("http://a.fr/").unwrap());
        assert!(matches!(with_images(&s.body, ImageMode::Normal), Cow::Borrowed(_)));
        let h = with_images(&s.body, ImageMode::Hallucination);
        assert_eq!(h.matches("<img src=\"/h?u=http://a.fr/i").count(), MAX_IMAGES, "{h}");
        assert!(h.contains(&format!("<a href=\"/h?u=http://a.fr/i{MAX_IMAGES}.png\">[image : n°{MAX_IMAGES}]</a>")), "image en surnombre : {h}");
        assert!(!h.contains("/i?u=http://a.fr/i"), "{h}");
        assert!(h.contains("&lt;img src=\"/i?u=piege\"&gt;"), "texte intact : {h}");
        assert!(h.contains("<a href=\"/p?u=http://a.fr/i?u=lien\">un lien</a>"), "liens de pages intacts : {h}");
        assert_eq!(h.len(), s.body.len(), "même poids");
        assert_eq!(image_link_for(&Url::parse("http://a.fr/x.png").unwrap(), ImageMode::Hallucination), "/h?u=http://a.fr/x.png");
    }

    #[test]
    fn imbrication_profonde_sans_debordement() {
        let mut html = String::new();
        for _ in 0..5000 {
            html.push_str("<div><span>");
        }
        html.push_str("fond");
        let s = simplify(&html, &Url::parse("http://a.fr/").unwrap());
        assert!(!s.body.contains("fond"), "contenu trop profond ignoré, sans planter");
    }
}
