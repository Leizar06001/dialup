//! Bout « serveur » du tunnel FAI : connexions TCP sortantes vers Internet
//! pour le compte de l'abonné.
//!
//! Pas de « login: / password: » texte après CONNECT : l'abonné est déjà
//! authentifié par le central (défi-réponse), et son mot de passe ne doit
//! jamais passer en clair dans le signal du modem. Le serveur affiche donc
//! sa bannière et un message de bienvenue nominatif, puis le marqueur
//! [`TUNNEL_MARKER`] : la suite du flux est en trames.
//!
//! Comme le bout client, il ne voit du modem qu'une paire de canaux
//! d'octets ; on peut donc le tester sans modem.
//!
//! Hôte virtuel : un OPEN vers `web1998:80` n'ouvre pas de connexion sur
//! Internet, il est branché sur le serveur « web 1998 » interne
//! ([`crate::web1998`]) quand la politique en a un ([`TunnelPolicy::web1998`],
//! présent par défaut). Il échappe donc au filtre de ports et d'adresses ;
//! les récupérations que fait ce service, elles, sont filtrées.
//!
//! Second hôte virtuel : `salon:6667`, le salon de discussion partagé par
//! tous les abonnés connectés au FAI ([`crate::salon`]). L'abonné y parle
//! sous son nom d'abonnement.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use v34_net::tunnel::mux::{Mux, StreamEnd};
use v34_net::tunnel::proto::{Decoded, FailCode, Frame, TUNNEL_MARKER, VERSION};

use crate::config::{self, Fai};
use crate::filter::{is_forbidden, Cidr};
use crate::salon::{self, Salon};
use crate::web1998::{self, Web1998};
use crate::{debug, info};

/// Données reçues pour un flux encore en cours de connexion.
const EARLY_MAX: usize = 64 * 1024;

/// Règles appliquées par le bout serveur.
#[derive(Clone, Debug)]
pub struct TunnelPolicy {
    pub banner: String,
    pub max_streams: u16,
    pub allowed_ports: Vec<u16>,
    pub allow_private: bool,
    pub denied: Vec<Cidr>,
    pub connect_timeout: Duration,
    /// Service de l'hôte virtuel `web1998:80` (`None` : pas d'interception,
    /// le nom est inconnu comme sur Internet)
    pub web1998: Option<Arc<Web1998>>,
    /// Salon de discussion du FAI (`salon:6667`), partagé par tous les
    /// tunnels qui utilisent cette politique
    pub salon: Option<Arc<Salon>>,
}

impl Default for TunnelPolicy {
    /// Valeurs par défaut de la configuration (`[fai]` et `[web1998]`) :
    /// ports 80/443, réseau local interdit, hôte virtuel web 1998 actif.
    fn default() -> Self {
        Self::from_config(&Fai::default()).expect("configuration par défaut valide")
    }
}

impl TunnelPolicy {
    /// Politique d'après la section `[fai]`, avec le service web 1998 par
    /// défaut (actif).
    pub fn from_config(fai: &Fai) -> Result<TunnelPolicy, String> {
        Self::from_configs(fai, &config::Web1998::default())
    }

    /// Politique d'après les sections `[fai]` et `[web1998]`.
    pub fn from_configs(fai: &Fai, web: &config::Web1998) -> Result<TunnelPolicy, String> {
        let denied: Vec<Cidr> = fai
            .reseaux_interdits
            .iter()
            .map(|s| Cidr::parse(s).ok_or_else(|| format!("réseau invalide : {s}")))
            .collect::<Result<_, _>>()?;
        Ok(TunnelPolicy {
            banner: fai.banniere.clone(),
            max_streams: fai.flux_max,
            allowed_ports: fai.ports_autorises.clone(),
            allow_private: fai.autoriser_prive,
            web1998: Web1998::from_config(web, denied.clone()),
            salon: Some(Salon::new()),
            denied,
            connect_timeout: Duration::from_secs(fai.delai_connexion_s.max(1)),
        })
    }

    /// `host:port` est-il l'hôte virtuel web 1998 (et le service actif) ?
    pub fn is_virtual(&self, host: &str, port: u16) -> bool {
        (self.web1998.is_some() && web1998::is_virtual_host(host, port)) || (self.salon.is_some() && salon::is_virtual_host(host, port))
    }

    /// Adresse autorisée comme destination ?
    pub fn ip_allowed(&self, addr: &SocketAddr) -> bool {
        let ip = addr.ip();
        if self.denied.iter().any(|c| c.contains(ip)) {
            return false;
        }
        self.allow_private || !is_forbidden(ip)
    }
}

// ───────────────────────── moteur ─────────────────────────

struct PendingOpen {
    target: String,
    cancelled: bool,
    early: Vec<u8>,
}

type ConnectResult = (u16, Result<TcpStream, (FailCode, String)>);

struct Engine {
    policy: Arc<TunnelPolicy>,
    label: String,
    /// Abonné (authentifié par le central) : son nom dans le salon
    user: String,
    from_modem: Receiver<Vec<u8>>,
    to_modem: SyncSender<Vec<u8>>,
    mux: Mux,
    pending: HashMap<u16, PendingOpen>,
    conn_tx: Sender<ConnectResult>,
    conn_rx: Receiver<ConnectResult>,
}

/// Fait tourner le bout serveur jusqu'à la fin de la liaison (bloquant ;
/// à lancer dans un thread). `user` : nom de l'abonné (déjà authentifié
/// par le central) ; `label` sert aux journaux. Renvoie la cause de la fin.
pub fn run(
    policy: Arc<TunnelPolicy>,
    label: String,
    user: &str,
    from_modem: Receiver<Vec<u8>>,
    to_modem: SyncSender<Vec<u8>>,
) -> String {
    let (conn_tx, conn_rx) = mpsc::channel();
    let mut e = Engine { policy, label, user: user.to_string(), from_modem, to_modem, mux: Mux::new(), pending: HashMap::new(), conn_tx, conn_rx };
    let hello = format!(
        "\r\n{}\r\n\r\nBienvenue {user} ! Vous etes identifie par le central, pas besoin de mot de passe.{TUNNEL_MARKER}",
        e.policy.banner
    );
    e.mux.send_raw(hello.into_bytes());
    let end = e.run();
    e.mux.reset_all(false);
    end
}

impl Engine {
    fn run(&mut self) -> String {
        loop {
            let mut busy = false;
            loop {
                match self.from_modem.try_recv() {
                    Ok(bytes) => {
                        busy = true;
                        self.mux.dec.push(&bytes);
                        self.decode_frames();
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return "liaison modem terminée".into(),
                }
            }
            while let Ok((id, res)) = self.conn_rx.try_recv() {
                busy = true;
                self.on_connected(id, res);
            }
            let (p, ended) = self.mux.pump();
            busy |= p;
            for e in ended {
                let (StreamEnd::Local(id) | StreamEnd::Remote(id)) = e;
                debug!("{} : flux {id} fermé", self.label);
            }
            match self.mux.flush(&self.to_modem) {
                Ok(p) => busy |= p,
                Err(_) => return "liaison modem terminée".into(),
            }
            if !busy {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    fn decode_frames(&mut self) {
        while let Some(d) = self.mux.next_decoded() {
            match d {
                Decoded::Frame(f) => self.on_frame(f),
                Decoded::Loss => {
                    info!("{} : erreur de transmission détectée, flux réinitialisés", self.label);
                    self.mux.reset_all(true);
                    let ids: Vec<u16> = self.pending.keys().copied().collect();
                    for id in ids {
                        if let Some(p) = self.pending.get_mut(&id) {
                            p.cancelled = true;
                        }
                        self.fail(id, FailCode::General, "erreur de transmission");
                    }
                }
            }
        }
    }

    fn fail(&mut self, id: u16, code: FailCode, msg: &str) {
        self.mux.send(&Frame::OpenFail { id, code, msg: msg.to_string() });
    }

    fn on_frame(&mut self, f: Frame) {
        match f {
            Frame::Hello { version, .. } => {
                debug!("{} : HELLO client v{version}", self.label);
                self.mux.send(&Frame::Hello { version: VERSION, max_streams: self.policy.max_streams });
            }
            Frame::Open { id, host, port } => self.on_open(id, host, port),
            Frame::Data { id, data } => {
                if self.mux.contains(id) {
                    self.mux.write(id, &data);
                } else if let Some(p) = self.pending.get_mut(&id) {
                    if p.early.len() + data.len() > EARLY_MAX {
                        p.cancelled = true;
                        self.mux.send(&Frame::Close { id });
                    } else {
                        p.early.extend_from_slice(&data);
                    }
                } else {
                    self.mux.send(&Frame::Close { id });
                }
            }
            Frame::Close { id } => {
                if let Some(p) = self.pending.get_mut(&id) {
                    p.cancelled = true;
                }
                self.mux.close_remote(id);
            }
            // Réponses réservées au serveur : ignorées
            Frame::OpenOk { .. } | Frame::OpenFail { .. } => {}
        }
    }

    fn on_open(&mut self, id: u16, host: String, port: u16) {
        let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
        if id == 0 || self.mux.contains(id) || self.pending.get(&id).is_some_and(|p| !p.cancelled) {
            return self.fail(id, FailCode::General, "identifiant de flux déjà utilisé");
        }
        if self.mux.len() + self.pending.len() >= self.policy.max_streams as usize {
            return self.fail(id, FailCode::General, "trop de flux simultanés");
        }
        // l'hôte virtuel est interne : pas de filtre de ports
        if !self.policy.is_virtual(&host, port) && !self.policy.allowed_ports.contains(&port) {
            info!("{} : refusé {target} (port non autorisé)", self.label);
            return self.fail(id, FailCode::NotAllowed, &format!("port {port} non autorisé"));
        }
        if host.is_empty() || host.len() > 253 || host.chars().any(|c| c.is_whitespace() || c.is_control() || c == '/') {
            return self.fail(id, FailCode::HostUnreachable, "nom d'hôte invalide");
        }
        debug!("{} : flux {id} → {target}", self.label);
        self.pending.insert(id, PendingOpen { target, cancelled: false, early: Vec::new() });
        let policy = self.policy.clone();
        let tx = self.conn_tx.clone();
        let user = self.user.clone();
        // connect_as() branche aussi les hôtes virtuels (web 1998, salon)
        let spawned = std::thread::Builder::new().name("isp-connect".into()).spawn(move || {
            let _ = tx.send((id, connect_as(&host, port, &policy, &user)));
        });
        if spawned.is_err() {
            self.pending.remove(&id);
            self.fail(id, FailCode::General, "ressources épuisées");
        }
    }

    fn on_connected(&mut self, id: u16, res: Result<TcpStream, (FailCode, String)>) {
        let Some(p) = self.pending.remove(&id) else { return };
        if p.cancelled {
            return;
        }
        match res {
            Ok(sock) => {
                if self.mux.attach(id, sock, &p.early).is_ok() {
                    self.mux.send(&Frame::OpenOk { id });
                } else {
                    self.fail(id, FailCode::General, "socket inutilisable");
                }
            }
            Err((code, msg)) => {
                info!("{} : échec {} ({msg})", self.label, p.target);
                self.fail(id, code, &msg);
            }
        }
    }
}

/// Résolution + filtrage + connexion (bloquant, dans un thread dédié).
///
/// Exception : l'hôte virtuel `web1998:80` (si `policy.web1998` existe)
/// est relié au serveur web 1998 interne, sans résolution ni filtre.
pub fn connect(host: &str, port: u16, policy: &TunnelPolicy) -> Result<TcpStream, (FailCode, String)> {
    connect_as(host, port, policy, "anonyme")
}

/// Comme [`connect`], pour le compte de l'abonné `user` (son nom dans le
/// salon).
pub fn connect_as(host: &str, port: u16, policy: &TunnelPolicy, user: &str) -> Result<TcpStream, (FailCode, String)> {
    if let Some(s) = policy.salon.as_ref().filter(|_| salon::is_virtual_host(host, port)) {
        return s.open_stream(user).map_err(|e| (FailCode::General, format!("salon indisponible : {e}")));
    }
    if let Some(web) = policy.web1998.as_ref().filter(|_| web1998::is_virtual_host(host, port)) {
        return web.open_stream().map_err(|e| (FailCode::General, format!("web 1998 indisponible : {e}")));
    }
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|_| (FailCode::HostUnreachable, format!("résolution de « {host} » impossible")))?
        .collect();
    let allowed: Vec<SocketAddr> = addrs.iter().copied().filter(|a| policy.ip_allowed(a)).collect();
    if allowed.is_empty() {
        return Err(if addrs.is_empty() {
            (FailCode::HostUnreachable, format!("« {host} » n'a pas d'adresse"))
        } else {
            (FailCode::NotAllowed, "destination interdite (adresse privée ou locale)".into())
        });
    }
    let mut last = None;
    for a in allowed {
        match TcpStream::connect_timeout(&a, policy.connect_timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    let e = last.expect("au moins un essai");
    Err(match e.kind() {
        std::io::ErrorKind::ConnectionRefused => (FailCode::Refused, "connexion refusée".into()),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => (FailCode::Timeout, "délai dépassé".into()),
        _ => (FailCode::HostUnreachable, format!("injoignable : {e}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> TunnelPolicy {
        TunnelPolicy {
            banner: "TEST".into(),
            max_streams: 4,
            allowed_ports: vec![80, 443],
            allow_private: false,
            denied: vec![Cidr::parse("8.8.4.0/24").unwrap()],
            connect_timeout: Duration::from_secs(1),
            web1998: None,
            salon: None,
        }
    }

    #[test]
    fn destinations_filtrees() {
        let p = policy();
        assert!(!p.ip_allowed(&"127.0.0.1:80".parse().unwrap()));
        assert!(!p.ip_allowed(&"192.168.1.1:443".parse().unwrap()));
        assert!(!p.ip_allowed(&"8.8.4.4:443".parse().unwrap()), "réseau de denied_networks");
        assert!(p.ip_allowed(&"8.8.8.8:443".parse().unwrap()));
        let mut lax = policy();
        lax.allow_private = true;
        assert!(lax.ip_allowed(&"127.0.0.1:80".parse().unwrap()));
        assert!(!lax.ip_allowed(&"8.8.4.4:443".parse().unwrap()), "denied_networks s'applique toujours");
        // « localhost » se résout en adresse locale : refusé sans même se connecter
        match connect("localhost", 80, &p) {
            Err((FailCode::NotAllowed, _)) => {}
            other => panic!("attendu NotAllowed, obtenu {:?}", other.map(|_| ())),
        }
    }

    #[test]
    fn hote_virtuel_branche_par_connect() {
        use std::io::{Read, Write};
        // sans service : « web1998 » est un nom inconnu comme un autre
        assert!(!policy().is_virtual("web1998", 80));
        assert!(connect("web1998", 80, &policy()).is_err());
        let p = TunnelPolicy { allowed_ports: vec![], ..TunnelPolicy::default() };
        assert!(p.is_virtual("Web1998", 80) && !p.is_virtual("web1998", 443));
        let mut s = connect("WEB1998", 80, &p).expect("hôte virtuel, sans filtre de ports");
        s.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        let mut r = String::new();
        s.read_to_string(&mut r).unwrap();
        assert!(r.starts_with("HTTP/1.0 200 OK") && r.contains("Tinyverse 1998"), "{r}");
        let off = TunnelPolicy::from_configs(&Fai::default(), &config::Web1998 { actif: false, ..Default::default() }).unwrap();
        assert!(off.web1998.is_none());
    }
}
