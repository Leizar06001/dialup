//! Configuration du serveur (fichier TOML, voir `deploy/config.example.toml`).

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use v34_core::LineParams;
use v34_net::auth::{valid_name, PasswordHash};
use v34_net::signal::valid_number;

use crate::filter::Cidr;
use crate::log::Level;

pub const DEFAULT_PATH: &str = "/etc/v34-server/config.toml";
/// 08 36 « ORACLE » sur le clavier du téléphone (6 7 2 2 5 3)
pub const ORACLE_NUMERO: &str = "0836672253";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub serveur: Serveur,
    pub central: Central,
    /// Annuaire : nom d'abonné → numéro attribué + empreinte du mot de passe
    pub abonnes: Vec<Abonne>,
    /// Numéros spéciaux desservis par un modem du central
    pub numeros_speciaux: Vec<NumeroSpecial>,
    pub modems: Modems,
    pub fai: Fai,
    /// Hôte virtuel « web 1998 » servi dans le tunnel du FAI
    pub web1998: Web1998,
    /// 3615 ORACLE : l'IA qui se croit en 1998
    pub oracle: Oracle,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            serveur: Serveur::default(),
            central: Central::default(),
            abonnes: Vec::new(),
            numeros_speciaux: vec![
                NumeroSpecial { numero: "0836154286".into(), service: Service::Fai },
                NumeroSpecial { numero: "0800000000".into(), service: Service::Echo },
                NumeroSpecial { numero: ORACLE_NUMERO.into(), service: Service::Oracle },
            ],
            modems: Modems::default(),
            fai: Fai::default(),
            web1998: Web1998::default(),
            oracle: Oracle::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Serveur {
    /// Adresse d'écoute UDP
    pub ecoute: String,
    /// error | warn | info | debug
    pub journal: String,
    /// Fichier des comptes créés par le site d'inscription (`v34-portail`),
    /// ajoutés à l'annuaire et relus quand il change ; vide = aucun
    pub comptes: String,
    /// État du central (JSON, réécrit chaque seconde) pour la page « État
    /// du réseau » du site ; vide = `etat.json` à côté des comptes (s'il y
    /// en a), « non » = jamais
    pub etat: String,
}

impl Serveur {
    /// Chemin du fichier d'état, s'il faut en écrire un.
    /// Dossier des données partagées avec le site (tickets de communication,
    /// succès) : celui du fichier des comptes, à défaut celui du fichier
    /// d'état. Sans l'un ni l'autre, pas de facturation.
    pub fn donnees_dir(&self) -> Option<std::path::PathBuf> {
        let c = self.comptes.trim();
        let f = if c.is_empty() { self.etat_path()? } else { c.into() };
        f.parent().filter(|d| !d.as_os_str().is_empty()).map(Into::into)
    }

    pub fn etat_path(&self) -> Option<std::path::PathBuf> {
        match self.etat.trim() {
            "non" => None,
            "" => {
                let c = self.comptes.trim();
                (!c.is_empty()).then(|| Path::new(c).with_file_name("etat.json"))
            }
            p => Some(p.into()),
        }
    }
}

impl Default for Serveur {
    fn default() -> Self {
        Self { ecoute: "0.0.0.0:3434".into(), journal: "info".into(), comptes: String::new(), etat: String::new() }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Central {
    /// Intervalle de keepalive annoncé aux clients (s)
    pub keepalive_s: u16,
    /// Enregistrement oublié sans nouvelles pendant ce temps (s)
    pub expiration_s: u16,
    /// Appel non décroché au bout de… (s)
    pub sonnerie_max_s: u32,
    /// Appels simultanés au total
    pub appels_max: usize,
    /// Abonnés connectés simultanément
    pub connectes_max: usize,
    /// Échecs d'authentification tolérés par adresse IP…
    pub echecs_max: u32,
    /// …par fenêtre glissante de (s)
    pub echecs_fenetre_s: u32,
    /// Simulateur de ligne appliqué par le central (vide = aucun)
    pub simulateur_ligne: String,
}

impl Default for Central {
    fn default() -> Self {
        Self {
            keepalive_s: 15,
            expiration_s: 60,
            sonnerie_max_s: 60,
            appels_max: 32,
            connectes_max: 1000,
            echecs_max: 5,
            echecs_fenetre_s: 60,
            simulateur_ligne: String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Abonne {
    pub nom: String,
    pub numero: String,
    /// `scram-sha256$<itérations>$<sel>$<clé>` (voir `v34_net::auth`)
    pub mot_de_passe_hash: String,
    /// Accès Internet (FAI) autorisé ; vrai par défaut pour les abonnés
    /// déclarés ici par l'administrateur
    #[serde(default = "oui")]
    pub fai: bool,
}

fn oui() -> bool {
    true
}

/// Ce qu'un numéro spécial fait quand on l'appelle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    /// Fournisseur d'accès : tunnel TCP vers Internet (proxy SOCKS5 côté client)
    Fai,
    /// Ligne de test : renvoie en écho les octets reçus
    Echo,
    /// 3615 ORACLE : une IA qui se croit en 1998, en trames de chat
    Oracle,
}

impl Service {
    pub fn name(self) -> &'static str {
        match self {
            Service::Fai => "fai",
            Service::Echo => "echo",
            Service::Oracle => "oracle",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumeroSpecial {
    pub numero: String,
    pub service: Service,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Modems {
    /// Appels simultanés vers les numéros spéciaux (un modem temps réel chacun)
    pub appels_max: usize,
    /// Sonnerie avant décroché (ms)
    pub delai_decroche_ms: u64,
    /// Délai maximal pour obtenir la porteuse (s)
    pub delai_porteuse_s: u64,
    /// Débit maximal (bit/s)
    pub debit_max: u32,
    /// 56k (V.90) : le FAI, raccordé en numérique au central, le propose
    /// aux abonnés qui le demandent
    pub v90: bool,
    /// Débit descendant maximal en 56k (bit/s, 28 000 à 56 000)
    pub debit_max_56k: u32,
}

impl Default for Modems {
    fn default() -> Self {
        Self { appels_max: 4, delai_decroche_ms: 2000, delai_porteuse_s: 90, debit_max: 33600, v90: true, debit_max_56k: 56000 }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Fai {
    /// Texte affiché après CONNECT
    pub banniere: String,
    /// Flux TCP simultanés par appel
    pub flux_max: u16,
    /// Ports de destination autorisés
    pub ports_autorises: Vec<u16>,
    /// Autoriser les adresses privées / locales (DANGEREUX, tests seulement)
    pub autoriser_prive: bool,
    /// Réseaux interdits en plus (ex. l'adresse publique du VPS)
    pub reseaux_interdits: Vec<String>,
    /// Délai de connexion TCP sortante (s)
    pub delai_connexion_s: u64,
}

impl Default for Fai {
    fn default() -> Self {
        Self {
            banniere: "V34NET - fournisseur d'acces Internet par modem".into(),
            flux_max: 16,
            ports_autorises: vec![80, 443],
            autoriser_prive: false,
            reseaux_interdits: Vec::new(),
            delai_connexion_s: 10,
        }
    }
}

/// Navigateur « web 1998 » : hôte virtuel `http://web1998/` servi par le
/// serveur lui-même dans le tunnel du FAI (voir [`crate::web1998`]).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Web1998 {
    /// Intercepter l'hôte `web1998` (sinon il est inconnu, comme sur Internet)
    pub actif: bool,
    /// Taille maximale d'une page HTML téléchargée (Ko) ; au-delà, tronquée
    pub page_max_ko: u32,
    /// Taille maximale d'une image téléchargée (Ko) ; au-delà, refusée
    pub image_max_ko: u32,
    /// Largeur maximale des images converties (pixels)
    pub largeur_image_max: u32,
    /// Poids visé pour une image convertie (Ko)
    pub image_cible_ko: u32,
    /// Délai maximal d'une récupération, redirections comprises (s)
    pub delai_s: u64,
    /// Cache mémoire des pages et images allégées (Mo, 0 = pas de cache)
    pub cache_mo: u32,
}

impl Default for Web1998 {
    fn default() -> Self {
        Self {
            actif: true,
            page_max_ko: 3 * 1024,
            image_max_ko: 5 * 1024,
            largeur_image_max: 320,
            image_cible_ko: 15,
            delai_s: 15,
            cache_mo: 32,
        }
    }
}

/// 3615 ORACLE (voir [`crate::oracle`]) : une IA jointe par OpenRouter
/// (API compatible OpenAI), persuadée qu'on est en 1998. Sans clé, ou en
/// cas de panne, l'oracle répond avec ses cartes (réponses toutes faites).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Oracle {
    /// Modèle demandé à OpenRouter
    pub modele: String,
    /// Point d'accès « chat completions » (https ; http seulement vers la
    /// machine elle-même, pour les essais)
    pub url: String,
    /// Fichier contenant la clé d'API (jamais dans ce fichier-ci, jamais
    /// journalisée) ; relu à chaque appel. Absent ou vide : hors ligne.
    pub fichier_cle: String,
    /// Qui peut appeler : « fai » (abonnés ayant l'accès Internet) ou « tous »
    pub acces: String,
    /// Questions posées à l'IA par abonné et par jour (UTC)
    pub questions_par_jour: u32,
    /// Questions posées à l'IA par jour, tous abonnés confondus
    pub questions_par_jour_total: u32,
    /// Longueur maximale d'une réponse (jetons)
    pub jetons_max: u32,
    /// Délai maximal d'une réponse de l'IA (s)
    pub delai_s: u64,
    /// Cadence d'affichage des réponses (bit/s, 10 bits par caractère
    /// comme sur un Minitel)
    pub debit_affichage: u32,
    /// Sans question pendant ce temps, l'oracle raccroche (s)
    pub inactivite_s: u64,
}

impl Default for Oracle {
    fn default() -> Self {
        Self {
            modele: "deepseek/deepseek-v4.1-flash".into(),
            url: "https://openrouter.ai/api/v1/chat/completions".into(),
            fichier_cle: "/etc/v34-server/openrouter.key".into(),
            acces: "fai".into(),
            questions_par_jour: 30,
            questions_par_jour_total: 1000,
            jetons_max: 200,
            delai_s: 20,
            debit_affichage: 1200,
            inactivite_s: 300,
        }
    }
}

impl Oracle {
    /// Vérifie la section ; `Ok(vrai)` si tous les abonnés peuvent appeler.
    pub fn validate(&self) -> Result<bool, String> {
        let tous = match self.acces.trim() {
            "fai" => false,
            "tous" => true,
            a => return Err(format!("oracle.acces invalide « {a} » (fai ou tous)")),
        };
        let m = self.modele.trim();
        if m.is_empty() || m.len() > 200 || m.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("oracle.modele invalide".into());
        }
        let u = url::Url::parse(self.url.trim()).map_err(|_| format!("oracle.url invalide : {}", self.url))?;
        let local = matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if !(u.scheme() == "https" || (u.scheme() == "http" && local)) {
            return Err("oracle.url : https obligatoire (la clé d'API y passe), http seulement vers 127.0.0.1".into());
        }
        if self.questions_par_jour == 0 || self.questions_par_jour_total == 0 {
            return Err("oracle : questions_par_jour et questions_par_jour_total doivent être > 0".into());
        }
        if !(16..=4000).contains(&self.jetons_max) {
            return Err("oracle.jetons_max doit être entre 16 et 4000".into());
        }
        if !(1..=120).contains(&self.delai_s) {
            return Err("oracle.delai_s doit être entre 1 et 120".into());
        }
        if !(50..=56000).contains(&self.debit_affichage) {
            return Err("oracle.debit_affichage doit être entre 50 et 56000 bit/s".into());
        }
        if !(30..=3600).contains(&self.inactivite_s) {
            return Err("oracle.inactivite_s doit être entre 30 et 3600".into());
        }
        Ok(tous)
    }
}

/// Annuaire prêt à l'emploi (empreintes analysées).
#[derive(Clone, Debug, Default)]
pub struct Annuaire {
    par_nom: HashMap<String, (String, PasswordHash)>,
    /// Abonnés sans accès Internet (FAI)
    sans_fai: HashSet<String>,
}

impl Annuaire {
    /// Numéro et empreinte d'un abonné.
    pub fn get(&self, nom: &str) -> Option<&(String, PasswordHash)> {
        self.par_nom.get(nom)
    }

    /// L'abonné peut-il appeler le FAI ?
    pub fn fai_autorise(&self, nom: &str) -> bool {
        self.par_nom.contains_key(nom) && !self.sans_fai.contains(nom)
    }

    /// (nom, numéro) de chaque abonné, triés par nom.
    pub fn entrees(&self) -> Vec<(&str, &str)> {
        let mut v: Vec<(&str, &str)> = self.par_nom.iter().map(|(n, (num, _))| (n.as_str(), num.as_str())).collect();
        v.sort();
        v
    }

    /// Nom déjà pris (sans tenir compte des majuscules) ?
    pub fn nom_pris(&self, nom: &str) -> bool {
        self.par_nom.keys().any(|n| n.eq_ignore_ascii_case(nom))
    }

    pub fn numero_pris(&self, numero: &str) -> bool {
        self.par_nom.values().any(|(n, _)| n == numero)
    }

    /// Ajoute les comptes créés par le site. Ceux de la configuration
    /// gardent la priorité : un compte du site en conflit (même nom ou même
    /// numéro) est ignoré et signalé.
    pub fn ajouter_comptes(&mut self, comptes: &[crate::comptes::Compte]) -> Vec<String> {
        let mut avert = Vec::new();
        for c in comptes {
            if self.nom_pris(&c.nom) || self.numero_pris(&c.numero) {
                avert.push(format!("compte du site « {} » ({}) ignoré : nom ou numéro déjà dans la configuration", c.nom, c.numero));
                continue;
            }
            match PasswordHash::parse(&c.mot_de_passe_hash) {
                Ok(h) => {
                    self.par_nom.insert(c.nom.clone(), (c.numero.clone(), h));
                    if !c.fai {
                        self.sans_fai.insert(c.nom.clone());
                    }
                }
                Err(e) => avert.push(format!("compte du site « {} » ignoré : {e}", c.nom)),
            }
        }
        avert
    }

    pub fn len(&self) -> usize {
        self.par_nom.len()
    }

    pub fn is_empty(&self) -> bool {
        self.par_nom.is_empty()
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("lecture de {} : {e}", path.display()))?;
        Self::from_toml(&text).map_err(|e| format!("{} : {e}", path.display()))
    }

    pub fn from_toml(text: &str) -> Result<Config, String> {
        let cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Vérifie la cohérence ; renvoie des avertissements non bloquants.
    pub fn validate(&self) -> Result<Vec<String>, String> {
        let mut warnings = Vec::new();
        self.listen_addr()?;
        self.log_level()?;
        let c = &self.central;
        if c.keepalive_s < 5 || c.expiration_s < c.keepalive_s.saturating_mul(2) {
            return Err("central : keepalive_s ≥ 5 et expiration_s ≥ 2 × keepalive_s".into());
        }
        if c.appels_max == 0 || c.connectes_max == 0 || c.echecs_max == 0 || c.echecs_fenetre_s == 0 {
            return Err("central : appels_max, connectes_max, echecs_max et echecs_fenetre_s doivent être > 0".into());
        }
        self.line_params()?;
        self.annuaire()?;
        if self.abonnes.is_empty() {
            warnings.push("annuaire vide : personne ne peut s'enregistrer (v34-server ajouter-abonne NOM NUMERO)".into());
        }
        let numeros_abonnes: HashSet<&str> = self.abonnes.iter().map(|a| a.numero.as_str()).collect();
        let mut speciaux = HashSet::new();
        for s in &self.numeros_speciaux {
            if !valid_number(&s.numero) {
                return Err(format!("numéro spécial invalide « {} »", s.numero));
            }
            if !speciaux.insert(s.numero.as_str()) {
                return Err(format!("numéro spécial en double : {}", s.numero));
            }
            if numeros_abonnes.contains(s.numero.as_str()) {
                return Err(format!("le numéro spécial {} est aussi attribué à un abonné", s.numero));
            }
        }
        let m = &self.modems;
        if !(28000..=56000).contains(&m.debit_max_56k) {
            return Err("modems.debit_max_56k doit être entre 28000 et 56000".into());
        }
        if !(2400..=33600).contains(&m.debit_max) {
            return Err("modems.debit_max doit être entre 2400 et 33600".into());
        }
        if self.fai.flux_max == 0 {
            return Err("fai.flux_max doit être > 0".into());
        }
        self.denied_networks()?;
        let w = &self.web1998;
        if w.page_max_ko < 16 || w.image_max_ko < 16 {
            return Err("web1998 : page_max_ko et image_max_ko doivent être ≥ 16".into());
        }
        if !(16..=1024).contains(&w.largeur_image_max) {
            return Err("web1998.largeur_image_max doit être entre 16 et 1024".into());
        }
        if w.image_cible_ko == 0 || !(1..=120).contains(&w.delai_s) || w.cache_mo > 4096 {
            return Err("web1998 : image_cible_ko > 0, delai_s entre 1 et 120, cache_mo ≤ 4096".into());
        }
        self.oracle.validate()?;
        if self.fai.autoriser_prive {
            warnings.push("fai.autoriser_prive = true : le réseau local du VPS est accessible par le tunnel !".into());
        }
        Ok(warnings)
    }

    /// Analyse et vérifie l'annuaire (noms et numéros uniques).
    pub fn annuaire(&self) -> Result<Annuaire, String> {
        let mut par_nom = HashMap::new();
        let mut sans_fai = HashSet::new();
        let mut numeros = HashSet::new();
        for a in &self.abonnes {
            if !valid_name(&a.nom) {
                return Err(format!("nom d'abonné invalide « {} » (1 à 32 caractères A-Z a-z 0-9 . _ -)", a.nom));
            }
            if !valid_number(&a.numero) {
                return Err(format!("abonné {} : numéro invalide « {} » (1 à 20 chiffres)", a.nom, a.numero));
            }
            if !numeros.insert(a.numero.clone()) {
                return Err(format!("numéro {} attribué deux fois", a.numero));
            }
            let h = PasswordHash::parse(&a.mot_de_passe_hash).map_err(|e| format!("abonné {} : mot_de_passe_hash : {e}", a.nom))?;
            if par_nom.insert(a.nom.clone(), (a.numero.clone(), h)).is_some() {
                return Err(format!("abonné en double : {}", a.nom));
            }
            if !a.fai {
                sans_fai.insert(a.nom.clone());
            }
        }
        Ok(Annuaire { par_nom, sans_fai })
    }

    pub fn listen_addr(&self) -> Result<SocketAddr, String> {
        self.serveur.ecoute.parse().map_err(|_| format!("serveur.ecoute invalide : {}", self.serveur.ecoute))
    }

    pub fn log_level(&self) -> Result<Level, String> {
        Level::parse(&self.serveur.journal).ok_or_else(|| format!("serveur.journal invalide : {}", self.serveur.journal))
    }

    pub fn line_params(&self) -> Result<Option<LineParams>, String> {
        let p = self.central.simulateur_ligne.trim();
        if p.is_empty() {
            return Ok(None);
        }
        LineParams::preset(p).map(Some).ok_or_else(|| format!("central.simulateur_ligne inconnu : {p}"))
    }

    pub fn denied_networks(&self) -> Result<Vec<Cidr>, String> {
        self.fai
            .reseaux_interdits
            .iter()
            .map(|s| Cidr::parse(s).ok_or_else(|| format!("fai.reseaux_interdits : réseau invalide « {s} »")))
            .collect()
    }

    /// Service associé à un numéro spécial.
    pub fn service_for(&self, numero: &str) -> Option<Service> {
        self.numeros_speciaux.iter().find(|s| s.numero == numero).map(|s| s.service)
    }

    /// Liste (numéro, service) annoncée aux clients.
    pub fn services(&self) -> Vec<(String, String)> {
        self.numeros_speciaux.iter().map(|s| (s.numero.clone(), s.service.name().to_string())).collect()
    }

    pub fn ring_timeout(&self) -> Duration {
        Duration::from_secs(self.central.sonnerie_max_s as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXEMPLE: &str = include_str!("../../../deploy/config.example.toml");

    fn hash(pw: &str) -> String {
        PasswordHash::new(pw, 1000).to_string()
    }

    #[test]
    fn exemple_de_deploiement_valide() {
        let c = Config::from_toml(EXEMPLE).expect("deploy/config.example.toml doit être valide");
        assert_eq!(c.service_for("0836154286"), Some(Service::Fai));
        assert_eq!(c.service_for("0800000000"), Some(Service::Echo));
        assert_eq!(c.service_for(ORACLE_NUMERO), Some(Service::Oracle));
        assert_eq!(c.oracle, Oracle::default(), "l'exemple reprend les valeurs par défaut");
        assert_eq!(c.listen_addr().unwrap().port(), 3434);
        assert!(!c.fai.autoriser_prive);
        assert_eq!(c.web1998, Web1998::default(), "l'exemple reprend les valeurs par défaut");
    }

    #[test]
    fn section_web1998() {
        let c = Config::from_toml("").unwrap();
        assert!(c.web1998.actif, "actif par défaut");
        assert_eq!(c.web1998.largeur_image_max, 320);
        let c = Config::from_toml("[web1998]\nactif = false\nlargeur_image_max = 240\ncache_mo = 0").unwrap();
        assert!(!c.web1998.actif);
        assert_eq!(c.web1998.largeur_image_max, 240);
        assert_eq!(c.web1998.delai_s, 15, "les autres clés gardent leur valeur par défaut");
        assert!(Config::from_toml("[web1998]\nlargeur_image_max = 0").is_err());
        assert!(Config::from_toml("[web1998]\ndelai_s = 0").is_err());
        assert!(Config::from_toml("[web1998]\npage_max_ko = 1").is_err());
        assert!(Config::from_toml("[web1998]\ninconnue = 1").is_err());
    }

    #[test]
    fn section_oracle() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.oracle.modele, "deepseek/deepseek-v4.1-flash");
        assert_eq!(c.oracle.fichier_cle, "/etc/v34-server/openrouter.key");
        assert_eq!((c.oracle.questions_par_jour, c.oracle.questions_par_jour_total, c.oracle.jetons_max), (30, 1000, 200));
        assert_eq!(c.oracle.validate(), Ok(false), "accès réservé aux abonnés du FAI par défaut");
        let c = Config::from_toml("[oracle]\nacces = \"tous\"\ndebit_affichage = 300").unwrap();
        assert_eq!(c.oracle.validate(), Ok(true));
        assert_eq!(c.oracle.debit_affichage, 300);
        assert_eq!(c.oracle.delai_s, 20, "les autres clés gardent leur valeur par défaut");
        for faux in [
            "acces = \"personne\"",
            "url = \"http://openrouter.ai/api/v1/chat/completions\"",
            "url = \"pas une adresse\"",
            "modele = \"\"",
            "questions_par_jour = 0",
            "questions_par_jour_total = 0",
            "jetons_max = 5",
            "delai_s = 0",
            "debit_affichage = 10",
            "inactivite_s = 1",
            // la clé n'a rien à faire dans config.toml
            "cle = \"sk-or-v1-0123\"",
        ] {
            assert!(Config::from_toml(&format!("[oracle]\n{faux}")).is_err(), "{faux} doit être refusé");
        }
        // http seulement vers la machine elle-même (essais)
        assert!(Config::from_toml("[oracle]\nurl = \"http://127.0.0.1:8080/v1/chat/completions\"").is_ok());
    }

    #[test]
    fn valeurs_par_defaut_et_annuaire() {
        let c = Config::from_toml(&format!(
            "[[abonnes]]\nnom = \"gaston\"\nnumero = \"0142424242\"\nmot_de_passe_hash = \"{}\"\n",
            hash("x")
        ))
        .unwrap();
        assert_eq!(c.fai.ports_autorises, vec![80, 443]);
        assert_eq!(c.services().len(), 3, "FAI + ligne de test + oracle par défaut");
        let a = c.annuaire().unwrap();
        assert_eq!(a.get("gaston").map(|(n, _)| n.as_str()), Some("0142424242"));
        assert!(a.get("inconnu").is_none());
        // configuration vide : valide (avec avertissement), pour pouvoir ajouter des abonnés ensuite
        let vide = Config::from_toml("").unwrap();
        assert!(!vide.validate().unwrap().is_empty());
    }

    #[test]
    fn erreurs_detectees() {
        let h = hash("x");
        let ab = |nom: &str, num: &str| format!("[[abonnes]]\nnom = \"{nom}\"\nnumero = \"{num}\"\nmot_de_passe_hash = \"{h}\"\n");
        assert!(Config::from_toml(&ab("a b", "01")).is_err(), "nom invalide");
        assert!(Config::from_toml(&ab("a", "01x")).is_err(), "numéro invalide");
        assert!(Config::from_toml(&format!("{}{}", ab("a", "01"), ab("b", "01"))).is_err(), "numéro en double");
        assert!(Config::from_toml(&format!("{}{}", ab("a", "01"), ab("a", "02"))).is_err(), "nom en double");
        assert!(Config::from_toml(&ab("a", "0836154286")).is_err(), "collision avec un numéro spécial");
        assert!(Config::from_toml("[[abonnes]]\nnom = \"a\"\nnumero = \"1\"\nmot_de_passe_hash = \"clair\"").is_err());
        assert!(Config::from_toml("[central]\nfaute_de_frappe = 1").is_err());
        assert!(Config::from_toml("[central]\nsimulateur_ligne = \"Inexistant\"").is_err());
        assert!(Config::from_toml("[fai]\nreseaux_interdits = [\"1.2.3.4/40\"]").is_err());
        assert!(Config::from_toml("[[numeros_speciaux]]\nnumero = \"36\"\nservice = \"minitel\"").is_err());
        let c = Config::from_toml("numeros_speciaux = []").unwrap();
        assert!(c.services().is_empty(), "liste vide = aucun numéro spécial");
    }
}
