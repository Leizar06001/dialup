//! L'écran des services du central qui répondent en Minitel (3615 ORACLE,
//! numéros cachés). Leur modem parle V.23 : 1200 bit/s vers l'écran, 75 bit/s
//! depuis le clavier.
//!
//! [`Ecran`], c'est le Minitel vu du serveur :
//! - un écrivain Vidéotex : pages (effacement, positionnement), titres en
//!   double hauteur, couleurs (des gris, sur un Minitel 1), mosaïques, et une
//!   zone de texte qui se met en page toute seule (mots coupés proprement ;
//!   quand elle déborde, la page suivante attend la touche SUITE) ;
//! - le clavier : caractères, accents (SS2), touches de fonction (SEP + code) ;
//! - l'écho : sur Télétel, ce n'est pas le Minitel qui affiche ce qu'on tape,
//!   c'est le réseau qui le lui renvoie. Ici, le serveur : chaque touche fait
//!   l'aller-retour (75 bit/s à l'aller, 1200 au retour) et s'inscrit dans le
//!   champ de saisie. CORRECTION efface le dernier caractère, ANNULATION tout
//!   le champ, ENVOI l'envoie.
//!
//! Tout part au débit de la ligne, sans prendre plus de 0,3 s d'avance sur
//! elle : l'écho ne fait jamais la queue derrière une page entière. Chaque
//! écriture commence par positionner le curseur : l'écho peut se glisser
//! entre deux sans rien déranger.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::time::{Duration, Instant};

use v34_net::videotex::{self as vt, Touche, BEL, BLANC, CAN, COFF, CON, CR, ESC, FF, LF, REP, SEP, SI, SO, SS2, US};

/// Avance maximale sur la ligne
const AVANCE: Duration = Duration::from_millis(300);
/// Caractère de remplissage d'un champ de saisie
const REMPLISSAGE: char = '.';

/// L'appelant a raccroché.
#[derive(Debug, PartialEq, Eq)]
pub struct Raccroche;

/// Taille des caractères.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Taille {
    #[default]
    Normale,
    DoubleHauteur,
    DoubleLargeur,
    Double,
}

/// Attributs d'un texte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    pub fg: u8,
    /// Fond : pris en compte à partir du premier espace (délimiteur)
    pub fond: Option<u8>,
    pub taille: Taille,
    pub clignote: bool,
    pub inverse: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style::NORMAL
    }
}

impl Style {
    pub const NORMAL: Style = Style { fg: BLANC, fond: None, taille: Taille::Normale, clignote: false, inverse: false };

    pub const fn fg(fg: u8) -> Style {
        Style { fg, ..Style::NORMAL }
    }

    pub const fn fond(self, bg: u8) -> Style {
        Style { fond: Some(bg), ..self }
    }

    pub const fn taille(self, taille: Taille) -> Style {
        Style { taille, ..self }
    }

    pub const fn clignotant(self) -> Style {
        Style { clignote: true, ..self }
    }

    pub const fn inverse(self) -> Style {
        Style { inverse: true, ..self }
    }

    fn octets(&self, out: &mut Vec<u8>) {
        if self.fg != BLANC {
            out.extend([ESC, 0x40 + self.fg]);
        }
        match self.taille {
            Taille::Normale => {}
            Taille::DoubleHauteur => out.extend([ESC, 0x4D]),
            Taille::DoubleLargeur => out.extend([ESC, 0x4E]),
            Taille::Double => out.extend([ESC, 0x4F]),
        }
        if self.clignote {
            out.extend([ESC, 0x48]);
        }
        if self.inverse {
            out.extend([ESC, 0x5D]);
        }
        if let Some(bg) = self.fond {
            out.extend([ESC, 0x50 + bg]);
        }
    }

    /// Colonnes occupées par un caractère.
    fn largeur(&self) -> usize {
        if matches!(self.taille, Taille::DoubleLargeur | Taille::Double) {
            2
        } else {
            1
        }
    }
}

/// Ce que l'appelant a fait au clavier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Saisie {
    /// ENVOI, avec le contenu du champ
    Ligne(String),
    /// Une autre touche de fonction (RETOUR, SUITE, GUIDE, SOMMAIRE…)
    Touche(Touche),
}

/// Le champ de saisie : où s'inscrit ce que tape l'appelant.
#[derive(Clone, Debug)]
struct Champ {
    rangee: u8,
    colonne: u8,
    taille: usize,
    texte: Vec<char>,
    style: Style,
}

impl Champ {
    /// Position du k-ième caractère (le champ continue à la rangée suivante).
    fn pos(&self, k: usize) -> (u8, u8) {
        let i = self.colonne as usize - 1 + k;
        (self.rangee + (i / 40) as u8, (i % 40) as u8 + 1)
    }
}

/// La zone où [`Ecran::dire`] écrit.
#[derive(Clone, Debug)]
struct Zone {
    haut: u8,
    bas: u8,
    /// Prochaine rangée libre
    rangee: u8,
    /// Colonne après la fin de la dernière ligne écrite
    colonne: u8,
    style: Style,
    /// Lignes écrites depuis que l'appelant a envoyé quelque chose : quand la
    /// zone déborde, elles passent en haut de la page suivante
    recentes: Vec<(String, Style)>,
}

#[derive(Clone, Copy, Debug)]
enum Clavier {
    Normal,
    Sep,
    Ss2,
    Accent(u8),
}

/// Le Minitel de l'appelant, vu du serveur.
pub struct Ecran {
    sortie: SyncSender<Vec<u8>>,
    entree: Receiver<Vec<u8>>,
    /// Caractères par seconde
    cps: f64,
    /// Quand la ligne aura fini d'envoyer ce qu'on lui a donné
    libre_a: Instant,
    tampon: Vec<u8>,
    brut: VecDeque<u8>,
    clavier: Clavier,
    file: VecDeque<Saisie>,
    champ: Option<Champ>,
    zone: Zone,
    parti: bool,
    /// Dernière touche de l'appelant
    pub dernier_clavier: Instant,
    /// Une page pleine attend SUITE au plus ce temps (zéro : elle n'attend pas)
    pub attente_suite: Duration,
}

/// Coupe un paragraphe en lignes de `largeur` colonnes au plus, entre les
/// mots (un mot trop long est coupé où il faut).
pub fn couper(texte: &str, largeur: usize) -> Vec<String> {
    let mut lignes = Vec::new();
    let mut l = String::new();
    for mot in texte.split(' ').filter(|m| !m.is_empty()) {
        let (lm, ll) = (vt::largeur(mot), vt::largeur(&l));
        if !l.is_empty() && ll + 1 + lm <= largeur {
            l.push(' ');
            l.push_str(mot);
            continue;
        }
        if !l.is_empty() {
            lignes.push(std::mem::take(&mut l));
        }
        let mut m = mot;
        while vt::largeur(m) > largeur {
            let coupe = m.char_indices().nth(largeur).map_or(m.len(), |(i, _)| i);
            lignes.push(m[..coupe].to_string());
            m = &m[coupe..];
        }
        l.push_str(m);
    }
    if !l.is_empty() || lignes.is_empty() {
        lignes.push(l);
    }
    lignes
}

/// Un dessin en mosaïques : chaque caractère du Minitel est un pavé de 2 × 3
/// points. `dessin` : des rangées de points (`#` allumé, autre chose éteint),
/// de largeur paire et de hauteur multiple de 3. Rend une rangée de codes
/// G1 par bande de trois rangées de points.
pub fn mosaiques(dessin: &[&str]) -> Vec<Vec<u8>> {
    let points: Vec<Vec<bool>> = dessin.iter().map(|l| l.chars().map(|c| c == '#').collect()).collect();
    let largeur = points.iter().map(Vec::len).max().unwrap_or(0).div_ceil(2);
    let point = |y: usize, x: usize| points.get(y).and_then(|l| l.get(x)).copied().unwrap_or(false);
    (0..points.len().div_ceil(3))
        .map(|b| {
            (0..largeur)
                .map(|c| {
                    let (y, x) = (b * 3, c * 2);
                    let bits = [point(y, x), point(y, x + 1), point(y + 1, x), point(y + 1, x + 1), point(y + 2, x), point(y + 2, x + 1)];
                    let mut code = 0x20u8;
                    for (k, &on) in bits.iter().enumerate().take(5) {
                        code |= u8::from(on) << k;
                    }
                    if bits[5] {
                        code |= 0x40;
                    }
                    code
                })
                .collect()
        })
        .collect()
}

impl Ecran {
    /// `bit_s` : débit de la ligne vers le Minitel (10 bits par caractère).
    pub fn new(sortie: SyncSender<Vec<u8>>, entree: Receiver<Vec<u8>>, bit_s: u32) -> Ecran {
        Ecran {
            sortie,
            entree,
            cps: bit_s.max(10) as f64 / 10.0 * 0.98,
            libre_a: Instant::now(),
            tampon: Vec::new(),
            brut: VecDeque::new(),
            clavier: Clavier::Normal,
            file: VecDeque::new(),
            champ: None,
            zone: Zone { haut: 1, bas: 24, rangee: 1, colonne: 1, style: Style::NORMAL, recentes: Vec::new() },
            parti: false,
            dernier_clavier: Instant::now(),
            attente_suite: Duration::from_secs(60),
        }
    }

    // ───────────────────────── sortie ─────────────────────────

    fn aller(&mut self, r: u8, c: u8) {
        self.tampon.extend([US, 0x40 + r.min(24), 0x40 + c.clamp(1, 40)]);
    }

    /// Envoie ce qui est prêt, au rythme de la ligne.
    fn envoyer(&mut self) -> Result<(), Raccroche> {
        if self.tampon.is_empty() {
            return Ok(());
        }
        loop {
            let now = Instant::now();
            if self.libre_a <= now + AVANCE {
                break;
            }
            let d = (self.libre_a - AVANCE - now).min(Duration::from_millis(50));
            self.ecouter(d);
        }
        let octets = std::mem::take(&mut self.tampon);
        let n = octets.len();
        self.sortie.send(octets).map_err(|_| Raccroche)?;
        let now = Instant::now();
        self.libre_a = self.libre_a.max(now) + Duration::from_secs_f64(n as f64 / self.cps);
        Ok(())
    }

    /// Fin d'une écriture : le curseur retourne au champ de saisie, et ça part.
    fn fin(&mut self) -> Result<(), Raccroche> {
        if self.tampon.is_empty() {
            return Ok(());
        }
        self.revenir_au_champ();
        self.envoyer()
    }

    fn revenir_au_champ(&mut self) {
        if let Some(c) = &self.champ {
            let (r, col) = c.pos(c.texte.len().min(c.taille.saturating_sub(1)));
            self.aller(r, col);
        }
    }

    /// Efface l'écran : plus de champ, la zone revient en haut.
    pub fn effacer(&mut self) -> Result<(), Raccroche> {
        self.tampon.extend([FF, COFF]);
        // la ligne de service aussi
        self.tampon.extend([US, 0x40, 0x41, CAN, LF]);
        self.champ = None;
        self.zone.rangee = self.zone.haut;
        self.zone.colonne = 1;
        self.envoyer()
    }

    /// Du texte à une position (rangée 1 à 24, colonne 1 à 40). En double
    /// hauteur, la rangée donnée est celle du bas du caractère.
    pub fn ecrire(&mut self, rangee: u8, colonne: u8, style: Style, texte: &str) -> Result<(), Raccroche> {
        self.aller(rangee, colonne);
        style.octets(&mut self.tampon);
        let t = vt::encoder(texte);
        self.tampon.extend(t);
        self.fin()
    }

    /// Du texte centré sur la rangée.
    pub fn centrer(&mut self, rangee: u8, style: Style, texte: &str) -> Result<(), Raccroche> {
        let l = vt::largeur(texte) * style.largeur();
        let c = (40usize.saturating_sub(l) / 2 + 1) as u8;
        self.ecrire(rangee, c, style, texte)
    }

    /// Une barre de `n` colonnes sur fond `bg`.
    pub fn barre(&mut self, rangee: u8, colonne: u8, n: u8, bg: u8) -> Result<(), Raccroche> {
        self.aller(rangee, colonne);
        self.tampon.extend([ESC, 0x50 + bg, b' ']);
        if n > 1 {
            self.tampon.extend([REP, 0x40 + (n - 1).min(63)]);
        }
        self.fin()
    }

    /// Une rangée de mosaïques (codes G1), couleur `fg` sur fond `bg`.
    pub fn mosaique(&mut self, rangee: u8, colonne: u8, fg: u8, bg: Option<u8>, codes: &[u8]) -> Result<(), Raccroche> {
        self.aller(rangee, colonne);
        self.tampon.extend([ESC, 0x40 + fg]);
        if let Some(bg) = bg {
            self.tampon.extend([ESC, 0x50 + bg]);
        }
        self.tampon.push(SO);
        // les suites de pavés identiques partent en REP
        let mut i = 0;
        while i < codes.len() {
            let c = codes[i];
            let mut n = 1;
            while i + n < codes.len() && codes[i + n] == c && n < 63 {
                n += 1;
            }
            self.tampon.push(c);
            if n > 2 {
                self.tampon.extend([REP, 0x40 + (n - 1) as u8]);
            } else if n == 2 {
                self.tampon.push(c);
            }
            i += n;
        }
        self.tampon.push(SI);
        self.fin()
    }

    /// Un dessin en mosaïques (voir [`mosaiques`]), à partir de (rangée, colonne).
    pub fn dessin(&mut self, rangee: u8, colonne: u8, fg: u8, bg: Option<u8>, dessin: &[&str]) -> Result<(), Raccroche> {
        for (k, codes) in mosaiques(dessin).iter().enumerate() {
            self.mosaique(rangee + k as u8, colonne, fg, bg, codes)?;
        }
        Ok(())
    }

    /// Un filet (une ligne fine de mosaïques) sur toute la largeur.
    pub fn filet(&mut self, rangee: u8, fg: u8) -> Result<(), Raccroche> {
        // pavé dont seule la rangée du milieu est allumée
        self.mosaique(rangee, 1, fg, None, &[0x2C; 40])
    }

    /// La ligne de service (rangée 0) : un court message.
    pub fn service(&mut self, texte: &str) -> Result<(), Raccroche> {
        self.tampon.extend([US, 0x40, 0x41]);
        self.tampon.extend(vt::encoder(texte));
        self.tampon.extend([CAN, LF]);
        self.fin()
    }

    /// Un bip (le Minitel sonne).
    pub fn bip(&mut self) -> Result<(), Raccroche> {
        self.tampon.push(BEL);
        self.envoyer()
    }

    // ───────────────────────── zone de texte ─────────────────────────

    /// Où écriront [`Ecran::dire`] et [`Ecran::suite`] : les rangées `haut`
    /// à `bas`, dans ce style.
    pub fn zone(&mut self, haut: u8, bas: u8, style: Style) {
        self.zone = Zone { haut, bas: bas.max(haut), rangee: haut, colonne: 1, style, recentes: Vec::new() };
    }

    /// Efface la zone ; la suite repart d'en haut.
    pub fn vider_zone(&mut self) -> Result<(), Raccroche> {
        for r in self.zone.haut..=self.zone.bas {
            self.aller(r, 1);
            self.tampon.push(CAN);
        }
        self.zone.rangee = self.zone.haut;
        self.zone.colonne = 1;
        self.fin()
    }

    /// Change le style des prochaines lignes de la zone.
    pub fn style_zone(&mut self, style: Style) {
        self.zone.style = style;
    }

    /// De nouvelles lignes dans la zone (paragraphes séparés par `\n`, mis
    /// en page sur 40 colonnes).
    pub fn dire(&mut self, texte: &str) -> Result<(), Raccroche> {
        self.dire_style(texte, self.zone.style)
    }

    pub fn dire_style(&mut self, texte: &str, style: Style) -> Result<(), Raccroche> {
        self.traiter();
        for paragraphe in texte.split('\n') {
            for ligne in couper(paragraphe, 40) {
                if self.zone.rangee > self.zone.bas {
                    self.page_suivante()?;
                }
                self.ligne_zone(&ligne, style);
                self.zone.recentes.push((ligne, style));
                self.fin()?;
                self.traiter();
            }
        }
        Ok(())
    }

    /// Écrit une ligne à la prochaine rangée de la zone.
    fn ligne_zone(&mut self, ligne: &str, style: Style) {
        let r = self.zone.rangee;
        self.aller(r, 1);
        style.octets(&mut self.tampon);
        self.tampon.extend(vt::encoder(ligne));
        let l = vt::largeur(ligne);
        if l < 40 {
            self.tampon.push(CAN);
        }
        self.zone.rangee += 1;
        self.zone.colonne = (l + 1).min(41) as u8;
    }

    /// La suite de la dernière ligne de la zone (points d'attente…).
    pub fn suite(&mut self, texte: &str) -> Result<(), Raccroche> {
        self.traiter();
        let l = vt::largeur(texte);
        if self.zone.rangee <= self.zone.haut || self.zone.colonne as usize + l > 41 {
            return self.dire(texte);
        }
        let (r, c) = (self.zone.rangee - 1, self.zone.colonne);
        self.aller(r, c);
        self.zone.style.octets(&mut self.tampon);
        self.tampon.extend(vt::encoder(texte));
        self.zone.colonne += l as u8;
        if let Some((derniere, _)) = self.zone.recentes.last_mut() {
            derniere.push_str(texte);
        }
        self.fin()
    }

    /// La zone est pleine. Si tout ce qu'elle montre vient d'être écrit (un
    /// long texte), on attend que l'appelant l'ait lu : touche SUITE. Sinon,
    /// la page suivante reprend en haut ce qui a été écrit depuis sa
    /// dernière saisie (il n'en perd rien).
    fn page_suivante(&mut self) -> Result<(), Raccroche> {
        let hauteur = (self.zone.bas - self.zone.haut + 1) as usize;
        let long = self.zone.recentes.len() >= hauteur;
        if long && !self.attente_suite.is_zero() {
            self.tampon.extend([US, 0x40, 0x41, ESC, 0x48]);
            self.tampon.extend(vt::encoder("Suite du texte : touche "));
            self.tampon.extend([ESC, 0x5D]);
            self.tampon.extend(vt::encoder(" SUITE "));
            self.tampon.extend([CAN, LF]);
            self.fin()?;
            let fin = Instant::now() + self.attente_suite;
            'attente: loop {
                self.traiter();
                // SUITE ou ENVOI (vide) : on continue ; le reste attend son tour
                let mut i = 0;
                while i < self.file.len() {
                    match &self.file[i] {
                        Saisie::Touche(Touche::Suite) => {
                            self.file.remove(i);
                            break 'attente;
                        }
                        Saisie::Ligne(l) if l.trim().is_empty() => {
                            self.file.remove(i);
                            break 'attente;
                        }
                        Saisie::Touche(Touche::ConnexionFin) => break 'attente,
                        _ => i += 1,
                    }
                }
                if self.parti {
                    return Err(Raccroche);
                }
                let now = Instant::now();
                if now >= fin {
                    break;
                }
                self.ecouter((fin - now).min(Duration::from_millis(100)));
            }
            self.tampon.extend([US, 0x40, 0x41, CAN, LF]);
        }
        let reprendre = if long { Vec::new() } else { std::mem::take(&mut self.zone.recentes) };
        self.zone.recentes.clear();
        self.vider_zone()?;
        for (ligne, style) in reprendre {
            self.ligne_zone(&ligne, style);
            self.zone.recentes.push((ligne, style));
        }
        self.fin()
    }

    // ───────────────────────── clavier ─────────────────────────

    /// Le champ de saisie : `taille` caractères à partir de (rangée, colonne),
    /// en continuant à la rangée suivante si besoin. Le curseur s'y allume.
    pub fn champ(&mut self, rangee: u8, colonne: u8, taille: usize, style: Style) -> Result<(), Raccroche> {
        let c = Champ { rangee, colonne, taille: taille.max(1), texte: Vec::new(), style };
        self.champ = Some(c);
        self.dessiner_champ();
        self.tampon.push(CON);
        self.fin()
    }

    /// Plus de champ de saisie (le curseur s'éteint).
    pub fn sans_champ(&mut self) -> Result<(), Raccroche> {
        self.champ = None;
        self.tampon.push(COFF);
        self.envoyer()
    }

    /// Ce qui est tapé dans le champ (pas encore envoyé).
    pub fn champ_en_cours(&self) -> String {
        self.champ.as_ref().map(|c| c.texte.iter().collect()).unwrap_or_default()
    }

    fn dessiner_champ(&mut self) {
        let Some(c) = self.champ.clone() else { return };
        let mut k = 0;
        while k < c.taille {
            let (r, col) = c.pos(k);
            let n = (c.taille - k).min(41 - col as usize);
            self.aller(r, col);
            c.style.octets(&mut self.tampon);
            let deja: String = c.texte.iter().skip(k).take(n).collect();
            self.tampon.extend(vt::encoder(&deja));
            let reste = n - deja.chars().count();
            if reste > 0 {
                self.tampon.push(REMPLISSAGE as u8);
                if reste > 1 {
                    self.tampon.extend([REP, 0x40 + (reste - 1) as u8]);
                }
            }
            k += n;
        }
    }

    /// Lit ce qui arrive du modem, au plus `d`.
    fn ecouter(&mut self, d: Duration) {
        match self.entree.recv_timeout(d) {
            Ok(b) => self.brut.extend(b),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => self.parti = true,
        }
        loop {
            match self.entree.try_recv() {
                Ok(b) => self.brut.extend(b),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.parti = true;
                    break;
                }
            }
        }
    }

    /// Traite les touches reçues : écho dans le champ, saisies en file.
    fn traiter(&mut self) {
        while let Some(b) = self.brut.pop_front() {
            let b = b & 0x7F;
            match self.clavier {
                Clavier::Normal => match b {
                    SEP => self.clavier = Clavier::Sep,
                    SS2 => self.clavier = Clavier::Ss2,
                    // un terminal qui n'est pas un Minitel : Entrée vaut ENVOI
                    CR | LF => self.touche(Touche::Envoi),
                    0x20..=0x7E => self.caractere(b as char),
                    _ => {}
                },
                Clavier::Sep => {
                    self.clavier = Clavier::Normal;
                    if let Some(t) = Touche::depuis_code(b) {
                        self.touche(t);
                    }
                }
                Clavier::Ss2 => {
                    if vt::est_accent(b) {
                        self.clavier = Clavier::Accent(b);
                    } else {
                        self.clavier = Clavier::Normal;
                        self.caractere(vt::signe(b));
                    }
                }
                Clavier::Accent(a) => {
                    self.clavier = Clavier::Normal;
                    self.caractere(vt::composer(a, b));
                }
            }
        }
    }

    fn caractere(&mut self, ch: char) {
        self.dernier_clavier = Instant::now();
        let Some(c) = self.champ.as_mut() else { return };
        if c.texte.len() >= c.taille {
            // champ plein : le Minitel bipe
            self.tampon.push(BEL);
            return;
        }
        let (r, col) = c.pos(c.texte.len());
        let style = c.style;
        c.texte.push(ch);
        self.aller(r, col);
        style.octets(&mut self.tampon);
        vt::encoder_car(ch, &mut self.tampon);
    }

    fn touche(&mut self, t: Touche) {
        self.dernier_clavier = Instant::now();
        match t {
            Touche::Correction => {
                if let Some(c) = self.champ.as_mut() {
                    if c.texte.pop().is_some() {
                        let (r, col) = c.pos(c.texte.len());
                        let style = c.style;
                        self.aller(r, col);
                        style.octets(&mut self.tampon);
                        self.tampon.push(REMPLISSAGE as u8);
                        self.aller(r, col);
                    }
                }
            }
            Touche::Annulation => {
                if let Some(c) = self.champ.as_mut() {
                    c.texte.clear();
                    self.dessiner_champ();
                }
            }
            Touche::Envoi => {
                let texte: String = match self.champ.as_mut() {
                    Some(c) => {
                        let t = c.texte.drain(..).collect();
                        self.dessiner_champ();
                        t
                    }
                    None => String::new(),
                };
                self.file.push_back(Saisie::Ligne(texte));
            }
            t => self.file.push_back(Saisie::Touche(t)),
        }
    }

    /// Saisies en attente.
    pub fn en_attente(&self) -> usize {
        self.file.len()
    }

    /// Ne garde que les `n` premières saisies en attente.
    pub fn garder(&mut self, n: usize) {
        self.file.truncate(n);
    }

    /// Ce que l'appelant a fait, en attendant au plus `attente` (`None` :
    /// rien). Pendant ce temps, l'écho de ce qu'il tape part.
    pub fn lire(&mut self, attente: Duration) -> Result<Option<Saisie>, Raccroche> {
        let fin = Instant::now() + attente;
        loop {
            self.traiter();
            self.fin()?;
            if let Some(s) = self.file.pop_front() {
                self.zone.recentes.clear();
                return Ok(Some(s));
            }
            if self.parti {
                return Err(Raccroche);
            }
            let now = Instant::now();
            if now >= fin {
                return Ok(None);
            }
            self.ecouter((fin - now).min(Duration::from_millis(100)));
        }
    }

    /// Laisse passer `d` sans rien perdre de ce que tape l'appelant (écho
    /// compris ; ses saisies attendent).
    pub fn pause(&mut self, d: Duration) -> Result<(), Raccroche> {
        let fin = Instant::now() + d;
        loop {
            self.traiter();
            self.fin()?;
            if self.parti {
                return Err(Raccroche);
            }
            let now = Instant::now();
            if now >= fin {
                return Ok(());
            }
            self.ecouter((fin - now).min(Duration::from_millis(100)));
        }
    }

    /// Le temps que tout ce qui est parti s'affiche (avant de raccrocher).
    pub fn vider(&mut self) {
        let _ = self.envoyer();
        let reste = self.libre_a.saturating_duration_since(Instant::now());
        if !reste.is_zero() {
            std::thread::sleep(reste.min(Duration::from_secs(10)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, sync_channel};

    fn ecran(debit: u32) -> (Ecran, std::sync::mpsc::Sender<Vec<u8>>, Receiver<Vec<u8>>) {
        let (tx, rx_sortie) = sync_channel(10_000);
        let (tx_entree, entree) = channel();
        (Ecran::new(tx, entree, debit), tx_entree, rx_sortie)
    }

    fn sorti(rx: &Receiver<Vec<u8>>) -> Vec<u8> {
        rx.try_iter().flatten().collect()
    }

    #[test]
    fn mise_en_page() {
        assert_eq!(couper("l'avenir en direct depuis 1998", 12), ["l'avenir en", "direct", "depuis 1998"]);
        assert_eq!(couper("", 40), [""]);
        assert_eq!(couper("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        let m = mosaiques(&["##..", "##..", "##.#"]);
        assert_eq!(m, [vec![0x20 | 0x1F | 0x40, 0x20 | 0x40]]);
    }

    #[test]
    fn page_zone_et_suite() {
        let (mut e, _clavier, rx) = ecran(10_000_000);
        e.attente_suite = Duration::ZERO;
        e.effacer().unwrap();
        e.centrer(2, Style::fg(vt::JAUNE).taille(Taille::DoubleHauteur), "3615 ORACLE").unwrap();
        e.zone(5, 7, Style::NORMAL);
        e.dire("Première ligne.\nDeuxième ligne, qui est bien trop longue pour tenir sur une seule rangée.\nQuatrième.").unwrap();
        let mut m = vt::Ecran::new();
        m.recevoir(&sorti(&rx));
        assert_eq!(m.texte_rangee(2), "              3615 ORACLE");
        // la zone a débordé : la page suivante ne montre que la fin
        assert_eq!(m.texte_rangee(5), "Quatrième.");
        assert_eq!(m.texte_rangee(6), "");
        e.suite("...").unwrap();
        m.recevoir(&sorti(&rx));
        assert_eq!(m.texte_rangee(5), "Quatrième....");
    }

    #[test]
    fn echo_correction_annulation_envoi() {
        let (mut e, clavier, rx) = ecran(10_000_000);
        e.effacer().unwrap();
        e.champ(23, 3, 10, Style::NORMAL).unwrap();
        let mut touches = b"Qui \x19\x42etes".to_vec();
        touches.extend(Touche::Correction.octets());
        touches.extend(b"z ?");
        touches.extend(Touche::Envoi.octets());
        touches.extend(b"oups");
        touches.extend(Touche::Annulation.octets());
        touches.extend(Touche::Guide.octets());
        clavier.send(touches).unwrap();
        assert_eq!(e.lire(Duration::from_secs(1)).unwrap(), Some(Saisie::Ligne("Qui étez ?".into())));
        assert_eq!(e.lire(Duration::from_secs(1)).unwrap(), Some(Saisie::Touche(Touche::Guide)));
        let mut m = vt::Ecran::new();
        m.recevoir(&sorti(&rx));
        // le champ a été vidé (ENVOI), retapé, annulé
        assert_eq!(m.texte_rangee(23), "  ..........");
        assert!(m.curseur_visible);
        drop(clavier);
        assert_eq!(e.lire(Duration::from_secs(1)), Err(Raccroche));
    }

    #[test]
    fn cadence_de_la_ligne() {
        let (mut e, _clavier, rx) = ecran(1200);
        e.zone(1, 24, Style::NORMAL);
        let t0 = Instant::now();
        e.dire(&"x".repeat(40 * 3)).unwrap();
        e.dire(&"y".repeat(40 * 2)).unwrap();
        let dt = t0.elapsed().as_secs_f64();
        // 1200 bit/s ≈ 118 caractères par seconde : 215 octets en 1,8 s, dont
        // les 0,3 dernières secondes d'avance (et la dernière ligne partie d'un coup)
        assert!((0.9..1.8).contains(&dt), "{dt:.2} s");
        assert!(sorti(&rx).len() > 200);
    }
}
