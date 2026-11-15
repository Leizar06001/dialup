//! Le Minitel de l'appli : quand l'appel aboutit sur un serveur Minitel
//! (V.23), les octets reçus ne sont plus du chat mais du Vidéotex. Ils
//! s'affichent sur un écran de 40 × 25 ([`v34_net::videotex::Ecran`]) que la
//! messagerie dessine ; ce qu'on tape (texte et touches de fonction) part à
//! 75 bit/s, et c'est le serveur qui renvoie l'écho.
//!
//! L'écran est tenu ici, dans le moteur : une messagerie ouverte en cours
//! d'appel demande l'écran complet (`minitel_ecran`), puis reçoit les
//! rangées qui changent (`v34:minitel`).

use std::time::{Duration, Instant};

use serde::Serialize;
use v34_net::videotex::{self as vt, Touche};

/// Après CONNEXION/FIN, le serveur dit au revoir et raccroche ; s'il ne le
/// fait pas, on raccroche nous-mêmes au bout de ce temps.
const FIN_MAX: Duration = Duration::from_secs(8);

/// Ce que reçoit la messagerie : des rangées (numéro, 40 caractères,
/// attributs de chacun, voir [`vt::Cellule::code_attributs`]), le curseur.
#[derive(Clone, Serialize)]
pub struct MinitelMsg {
    /// Toutes les rangées (sinon : celles qui ont changé)
    pub complet: bool,
    pub rangees: Vec<(usize, String, Vec<u32>)>,
    pub curseur: (usize, usize),
    pub curseur_visible: bool,
    pub connecte: bool,
    /// Bips reçus depuis le début (le Minitel sonne)
    pub bips: u32,
    /// Un appel Minitel a eu lieu (l'écran a quelque chose à montrer)
    pub actif: bool,
}

pub struct Minitel {
    ecran: vt::Ecran,
    /// L'appel en cours est un appel Minitel
    actif: bool,
    connecte: bool,
    /// CONNEXION/FIN envoyée (quand)
    fin_demandee: Option<Instant>,
}

impl Default for Minitel {
    fn default() -> Self {
        Self::new()
    }
}

impl Minitel {
    pub fn new() -> Minitel {
        Minitel { ecran: vt::Ecran::new(), actif: false, connecte: false, fin_demandee: None }
    }

    /// Un nouvel appel commence (Minitel ou non) : on oublie le précédent.
    pub fn nouvel_appel(&mut self) {
        self.actif = false;
        self.connecte = false;
        self.fin_demandee = None;
    }

    /// Le modem vient de reconnaître un serveur Minitel : écran neuf.
    pub fn debut(&mut self) {
        self.ecran = vt::Ecran::new();
        self.actif = true;
        self.connecte = false;
        self.fin_demandee = None;
    }

    pub fn actif(&self) -> bool {
        self.actif
    }

    /// Connexion établie (le « C » s'allume) ou perdue (« F »).
    pub fn connexion(&mut self, oui: bool) {
        self.connecte = oui;
        self.ecran.connexion(oui);
        if !oui {
            self.fin_demandee = None;
        }
    }

    pub fn connecte(&self) -> bool {
        self.connecte
    }

    pub fn recevoir(&mut self, octets: &[u8]) {
        self.ecran.recevoir(octets);
    }

    /// Les rangées à montrer : toutes (`complet`), ou celles qui ont changé
    /// (`None` : rien de neuf).
    pub fn message(&mut self, complet: bool) -> Option<MinitelMsg> {
        let sales = self.ecran.prendre_sales();
        let rangees: Vec<usize> = if complet { (0..vt::RANGEES).collect() } else { sales };
        if rangees.is_empty() && !complet {
            return None;
        }
        Some(MinitelMsg {
            complet,
            rangees: rangees
                .into_iter()
                .map(|r| {
                    let (t, a) = self.ecran.rangee_compacte(r);
                    (r, t, a)
                })
                .collect(),
            curseur: (self.ecran.rangee, self.ecran.colonne),
            curseur_visible: self.ecran.curseur_visible,
            connecte: self.connecte,
            bips: self.ecran.bips,
            actif: self.actif,
        })
    }

    /// Une touche de fonction, par son nom (« ENVOI », « connexion/fin »…) :
    /// les octets à envoyer.
    pub fn touche(&mut self, nom: &str) -> Option<Vec<u8>> {
        let t = Touche::depuis_nom(nom)?;
        if t == Touche::ConnexionFin {
            self.fin_demandee.get_or_insert_with(Instant::now);
        }
        Some(t.octets().to_vec())
    }

    /// Du texte tapé au clavier (les accents partent en SS2, comme sur un
    /// Minitel 1B).
    pub fn texte(&self, texte: &str) -> Vec<u8> {
        vt::encoder(&texte.replace(['\r', '\n'], " "))
    }

    /// CONNEXION/FIN restée sans effet : il est temps de raccrocher soi-même.
    pub fn fin_sans_reponse(&self) -> bool {
        self.fin_demandee.is_some_and(|t| t.elapsed() > FIN_MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touches_texte_et_ecran() {
        let mut m = Minitel::new();
        assert_eq!(m.touche("envoi"), Some(vec![vt::SEP, 0x41]));
        assert_eq!(m.touche("RÉPÉTITION"), Some(vec![vt::SEP, 0x43]));
        assert_eq!(m.touche("inconnue"), None);
        assert_eq!(m.texte("é!"), vec![vt::SS2, 0x42, b'e', b'!']);
        m.debut();
        assert!(m.message(true).is_some_and(|x| x.rangees.len() == 25 && x.actif));
        assert!(m.message(false).is_none(), "rien de neuf");
        m.recevoir(b"\x0c\x1f\x41\x41ORACLE");
        let x = m.message(false).expect("des rangées changées");
        assert!(x.rangees.iter().any(|(r, t, _)| *r == 1 && t.starts_with("ORACLE")));
        m.touche("Connexion/Fin");
        assert!(!m.fin_sans_reponse());
    }

    /// 3615 ORACLE (code du central, hors ligne comme en boucle locale)
    /// s'affiche sur le Minitel de l'appli : ses pages passent par l'écran
    /// du moteur, la question part comme le moteur l'envoie (texte puis
    /// ENVOI), et CONNEXION/FIN termine la consultation.
    #[test]
    fn l_oracle_s_affiche_sur_le_minitel_de_l_appli() {
        let cfg = v34_server::config::Oracle { fichier_cle: String::new(), debit_affichage: 56000, ..Default::default() };
        let mut oracle = v34_server::oracle::Oracle::new(&cfg).unwrap();
        oracle.attente_cartes = Duration::ZERO;
        let (in_tx, in_rx) = std::sync::mpsc::channel();
        let (out_tx, out_rx) = std::sync::mpsc::sync_channel(4096);
        let fin = std::thread::spawn(move || v34_server::oracle::session(std::sync::Arc::new(oracle), "test", "visiteur", in_rx, out_tx));
        let mut m = Minitel::new();
        m.debut();
        m.connexion(true);
        // (le texte des rangées, mosaïques et moitiés droites en blancs)
        let texte = |t: &str, a: &[u32]| -> String {
            let s: String = t.chars().zip(a).map(|(c, a)| if (a >> 6) as u16 & (vt::A_MOSAIQUE | vt::A_DROITE) != 0 { ' ' } else { c }).collect();
            s.trim_end().to_string()
        };
        let ecran = |m: &mut Minitel| m.message(true).map(|x| x.rangees.iter().map(|(_, t, a)| texte(t, a)).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        let lire_jusqu_a = |m: &mut Minitel, motif: &str| {
            let t0 = Instant::now();
            loop {
                let e = ecran(m);
                if e.contains(motif) {
                    return e;
                }
                assert!(t0.elapsed() < Duration::from_secs(5), "« {motif} » jamais affiché :\n{e}");
                if let Ok(b) = out_rx.recv_timeout(Duration::from_millis(50)) {
                    m.recevoir(&b);
                }
            }
        };
        let e = lire_jusqu_a(&mut m, "raccrocher.");
        assert!(e.contains("O R A C L E") && e.contains("2,23 F la minute"), "{e}");
        let mut q = m.texte("Qui va gagner la Coupe du monde ?");
        q.extend(m.touche("ENVOI").unwrap());
        in_tx.send(q).unwrap();
        let e = lire_jusqu_a(&mut m, " F ]");
        assert!(e.contains("> Qui va gagner la Coupe du monde ?"), "{e}");
        assert!(e.contains("Brésil") || e.contains("Stade de France"), "{e}");
        // (MINITEL_JSON=fichier : l'écran tel que la messagerie le reçoit,
        // pour regarder le rendu de minitel.js hors de l'appli)
        if let Ok(f) = std::env::var("MINITEL_JSON") {
            let _ = std::fs::write(f, serde_json::to_string(&m.message(true)).unwrap_or_default());
        }
        in_tx.send(m.touche("CONNEXION/FIN").unwrap()).unwrap();
        lire_jusqu_a(&mut m, "Au revoir !");
        assert_eq!(fin.join().unwrap(), "consultation terminée (CONNEXION/FIN)");
    }
}
