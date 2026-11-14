//! v34-cli : tester le modem sans interface graphique.
//!
//! ```text
//! v34-cli local  [--seconds N] [--preset NOM] [--wav fichier.wav] [--block N] [--max-rate R]
//! v34-cli answer [--port P] [--codec alaw|ulaw|l16] [--preset NOM] [--seconds N]
//! v34-cli call <ip:port> [--local-port P] [--codec ...] [--preset NOM] [--seconds N]
//! v34-cli presets
//! v34-cli register [--server HOTE:PORT] --user NOM [--password MDP]
//! v34-cli dial [--server HOTE:PORT] --user NOM [--password MDP] <numéro>
//! ```

mod wav;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use v34_core::{
    EchoPath, LineParams, LineSim, LinkParams, LogLevel, Modem, ModemCommand, ModemConfig, ModemEvent, ModemStatus, Role,
    SAMPLE_RATE,
};
use v34_net::central::{CentralClient, CentralConfig, CentralEvent, DEFAULT_SERVER};
use v34_net::tunnel::{ClientConfig, ModemPipe, TunnelClient, TunnelEvent, DEFAULT_PROXY};
use v34_net::{AudioLink, Codec, LinkConfig, DEFAULT_PORT};

use wav::WavWriter;

const USAGE: &str = "\
v34-cli — modem logiciel V.8 / V.34 en ligne de commande

USAGE :
  v34-cli local  [--seconds N] [--preset NOM] [--wav fichier.wav] [--block N] [--max-rate R]
        Deux modems (appelant + appelé) reliés en mémoire à travers le simulateur
        de ligne, simulés plus vite que le temps réel.
        WAV stéréo : gauche = appelant, droite = appelé.
  v34-cli answer [--port P] [--codec alaw|ulaw|l16] [--preset NOM] [--seconds N]
        Attend un appel sur le port UDP P (défaut 3434), décroche après une sonnerie.
  v34-cli call <ip:port> [--local-port P] [--codec ...] [--preset NOM] [--seconds N]
        Appelle un autre v34-cli (ou l'appli) en temps réel.
  v34-cli presets
        Liste les préréglages du simulateur de ligne.

Via le central téléphonique (v34-server, défaut dialup.tinyverse.fr:3434) :
  v34-cli register [--server HOTE:PORT] --user NOM [--password MDP] [--rings N]
        S'enregistre (le central attribue le numéro), attend les appels
        et décroche après N sonneries (défaut 1).
  v34-cli dial [--server HOTE:PORT] --user NOM [--password MDP] <numéro>
        Appelle <numéro>. Si c'est le FAI (annoncé par le central, ou
        --isp NUM), lance aussi un proxy local SOCKS5 / HTTP CONNECT
        (--socks ADRESSE, défaut 127.0.0.1:1080) une fois connecté.
        0800000000 : ligne de test (écho des octets).
        Sans --password, le mot de passe est lu dans V34_PASSWORD ou
        demandé au clavier (il n'apparaît alors pas dans `ps`).

Options temps réel : --jitter-ms N (tampon de gigue, défaut 60).
--preset applique le simulateur de ligne (et l'écho local) en réception.";

// ───────────────────────────── arguments ─────────────────────────────

struct Args {
    positional: Vec<String>,
    flags: HashMap<String, String>,
}

impl Args {
    fn parse() -> Result<Args, String> {
        let mut positional = Vec::new();
        let mut flags = HashMap::new();
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            if let Some(name) = a.strip_prefix("--") {
                if name == "help" {
                    flags.insert("help".into(), String::new());
                    continue;
                }
                let (k, v) = match name.split_once('=') {
                    Some((k, v)) => (k.to_string(), v.to_string()),
                    None => (name.to_string(), it.next().ok_or(format!("--{name} attend une valeur"))?),
                };
                flags.insert(k, v);
            } else if a == "-h" {
                flags.insert("help".into(), String::new());
            } else {
                positional.push(a);
            }
        }
        Ok(Args { positional, flags })
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.flags.get(k).map(String::as_str)
    }

    fn num<T: std::str::FromStr>(&self, k: &str, default: T) -> Result<T, String> {
        match self.get(k) {
            None => Ok(default),
            Some(v) => v.parse().map_err(|_| format!("valeur invalide pour --{k} : {v}")),
        }
    }

    fn preset(&self) -> Result<Option<(String, LineParams)>, String> {
        match self.get("preset") {
            None => Ok(None),
            Some(name) => LineParams::preset(name)
                .map(|p| Some((name.to_string(), p)))
                .ok_or_else(|| format!("préréglage inconnu : {name} (voir `v34-cli presets`)")),
        }
    }

    fn codec(&self) -> Result<Codec, String> {
        match self.get("codec") {
            None => Ok(Codec::Alaw),
            Some(c) => Codec::from_name(c).ok_or_else(|| format!("codec inconnu : {c} (alaw, ulaw, l16)")),
        }
    }

    fn modem_config(&self) -> Result<ModemConfig, String> {
        let mut cfg = ModemConfig::default();
        cfg.max_rate = self.num("max-rate", cfg.max_rate)?;
        // 56k (V.90) : --v90 non pour s'en passer, --express oui pour la
        // poignée de main pressée, --max-rate-56k R pour le plafonner
        cfg.v90 = self.get("v90") != Some("non");
        cfg.express = self.get("express") == Some("oui");
        cfg.max_rate_pcm = self.num("max-rate-56k", cfg.max_rate_pcm)?;
        Ok(cfg)
    }
}

// ───────────────────────────── affichage ─────────────────────────────

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Call => "APPELANT",
        Role::Answer => "APPELÉ  ",
    }
}

fn fmt_params(p: &LinkParams) -> String {
    let mut parts = Vec::new();
    if !p.v8_modes.is_empty() {
        parts.push(format!("modes {}", p.v8_modes.join(",")));
    }
    if let (Some(t), Some(r)) = (p.symbol_rate_tx, p.symbol_rate_rx) {
        parts.push(format!("symboles {t}/{r} Bd"));
    }
    if let (Some(t), Some(r)) = (p.carrier_tx_hz, p.carrier_rx_hz) {
        parts.push(format!("porteuses {t:.0}/{r:.0} Hz"));
    }
    if let (Some(t), Some(r)) = (p.data_rate_tx, p.data_rate_rx) {
        parts.push(format!("débit {t}/{r} bit/s"));
    }
    if let Some(d) = p.rtd_ms {
        parts.push(format!("RTD {d:.1} ms"));
    }
    if let Some(s) = p.snr_db {
        parts.push(format!("SNR {s:.1} dB"));
    }
    if let Some(t) = p.trellis_states {
        parts.push(format!("treillis {t} états"));
    }
    if parts.is_empty() {
        "(rien de connu)".into()
    } else {
        parts.join(" · ")
    }
}

fn fmt_event(ev: &ModemEvent) -> String {
    match ev {
        ModemEvent::PhaseChanged(p) => format!("── phase → {p:?}"),
        ModemEvent::TxSignal(s) => format!("émet    {s}"),
        ModemEvent::RxSignal(s) => format!("reçoit  {s}"),
        ModemEvent::Log { level, msg } => match level {
            LogLevel::Info => format!("info    {msg}"),
            LogLevel::Detail => format!("détail  {msg}"),
            LogLevel::Warn => format!("ALERTE  {msg}"),
        },
        ModemEvent::ParamsUpdated(p) => format!("params  {}", fmt_params(p)),
        ModemEvent::Connected { tx_rate, rx_rate } => format!("*** CONNECT {tx_rate}/{rx_rate} bit/s ***"),
        ModemEvent::Disconnected { reason } => format!("*** NO CARRIER ({reason}) ***"),
        ModemEvent::Data(d) => format!("données {} octets : {:?}", d.len(), String::from_utf8_lossy(d)),
    }
}

fn print_event(t_samples: u64, role: Role, ev: &ModemEvent) {
    let ms = t_samples * 1000 / SAMPLE_RATE as u64;
    println!("[{ms:>7} ms] {}  {}", role_name(role), fmt_event(ev));
}

fn print_status(s: &ModemStatus) {
    let l = &s.leds;
    let led = |on: bool, n: &str| if on { format!("[{n}]") } else { format!(" {} ", n.to_lowercase()) };
    println!(
        "  {} phase {:?} | LEDs {}{}{}{}{}{}{}{} | tx {} o, rx {} o",
        role_name(s.role),
        s.phase,
        led(l.hs, "HS"),
        led(l.aa, "AA"),
        led(l.cd, "CD"),
        led(l.oh, "OH"),
        led(l.rd, "RD"),
        led(l.sd, "SD"),
        led(l.tr, "TR"),
        led(l.mr, "MR"),
        s.bytes_tx,
        s.bytes_rx
    );
    println!("           {}", fmt_params(&s.params));
}

// ───────────────────────────── mode local ─────────────────────────────

fn run_local(args: &Args) -> Result<(), String> {
    let seconds: f64 = args.num("seconds", 30.0)?;
    let block: usize = args.num("block", 16usize)?.clamp(1, 1600);
    let (preset_name, line) = args.preset()?.unwrap_or_else(|| ("Parfaite".into(), LineParams::perfect()));
    let cfg = args.modem_config()?;

    println!("Boucle locale : ligne « {preset_name} », blocs de {block} échantillons, {seconds} s max");
    let mut call = Modem::new(Role::Call, cfg.clone());
    let mut answer = Modem::new(Role::Answer, cfg);
    let mut line_c2a = LineSim::new(line.clone());
    let mut line_a2c = LineSim::new(line.clone());
    let mut echo_c = EchoPath::new(line.clone());
    let mut echo_a = EchoPath::new(line);

    let mut wav = match args.get("wav") {
        Some(p) => Some(WavWriter::create(&PathBuf::from(p), SAMPLE_RATE, 2).map_err(|e| format!("WAV : {e}"))?),
        None => None,
    };

    call.command(ModemCommand::Dial);
    answer.command(ModemCommand::Answer);

    let total = (seconds * SAMPLE_RATE as f64) as u64;
    let (mut call_rx, mut ans_rx) = (vec![0.0f32; block], vec![0.0f32; block]);
    let (mut call_tx, mut ans_tx) = (vec![0.0f32; block], vec![0.0f32; block]);
    let mut stereo = vec![0.0f32; 2 * block];
    let (mut call_up, mut ans_up) = (false, false);
    let mut stop_at = total;
    let started = Instant::now();
    let mut t = 0u64;

    while t < stop_at {
        call.process(&call_rx, &mut call_tx);
        answer.process(&ans_rx, &mut ans_tx);

        if let Some(w) = wav.as_mut() {
            for i in 0..block {
                stereo[2 * i] = call_tx[i];
                stereo[2 * i + 1] = ans_tx[i];
            }
            w.write_interleaved(&stereo).map_err(|e| format!("WAV : {e}"))?;
        }

        // Ce qui est émis pendant ce bloc arrive en face au bloc suivant.
        line_c2a.process(&call_tx, &mut ans_rx);
        echo_a.process(&ans_tx, &mut ans_rx);
        line_a2c.process(&ans_tx, &mut call_rx);
        echo_c.process(&call_tx, &mut call_rx);

        for (m, up) in [(&mut call, &mut call_up), (&mut answer, &mut ans_up)] {
            let role = m.role();
            for ev in m.drain_events() {
                if matches!(ev, ModemEvent::Connected { .. }) {
                    *up = true;
                }
                print_event(t, role, &ev);
            }
        }
        if call_up && ans_up && stop_at == total {
            // Les deux sont connectés : on laisse tourner 2 s puis on arrête.
            stop_at = (t + 2 * SAMPLE_RATE as u64).min(total);
        }
        t += block as u64;
    }

    if let Some(w) = wav {
        w.finish().map_err(|e| format!("WAV : {e}"))?;
        println!("WAV écrit : {}", args.get("wav").unwrap_or_default());
    }
    let sim_s = t as f64 / SAMPLE_RATE as f64;
    println!(
        "\nFin après {:.2} s simulées ({:.2} s de calcul, ×{:.0}).",
        sim_s,
        started.elapsed().as_secs_f64(),
        sim_s / started.elapsed().as_secs_f64().max(1e-6)
    );
    print_status(&call.status());
    print_status(&answer.status());
    if call_up && ans_up {
        Ok(())
    } else {
        Err("la connexion n'a pas abouti".into())
    }
}

// ───────────────────────────── temps réel ─────────────────────────────

fn run_realtime(args: &Args, role: Role) -> Result<(), String> {
    let codec = args.codec()?;
    let preset = args.preset()?;
    let seconds: Option<f64> = args.get("seconds").map(|s| s.parse().map_err(|_| format!("--seconds invalide : {s}"))).transpose()?;
    let link_cfg = LinkConfig { codec, jitter_target_ms: args.num("jitter-ms", 60u32)?, ..LinkConfig::default() };

    let (bind_port, target) = match role {
        Role::Answer => (args.num("port", DEFAULT_PORT)?, None),
        Role::Call => {
            let t = args.positional.get(1).ok_or("adresse manquante : v34-cli call <ip:port>")?.clone();
            let t = if t.contains(':') { t } else { format!("{t}:{DEFAULT_PORT}") };
            (args.num("local-port", 0u16)?, Some(t))
        }
    };

    let mut link = AudioLink::bind(("0.0.0.0", bind_port), link_cfg).map_err(|e| format!("socket UDP : {e}"))?;
    let local = link.local_addr().map_err(|e| e.to_string())?;
    let mut modem = Modem::new(role, args.modem_config()?);
    let mut line = preset.as_ref().map(|(_, p)| (LineSim::new(p.clone()), EchoPath::new(p.clone())));

    println!(
        "Modem {} sur UDP {local}, codec {}{}",
        role_name(role).trim(),
        codec.name(),
        preset.as_ref().map(|(n, _)| format!(", ligne « {n} »")).unwrap_or_default()
    );
    let mut answered = false;
    let mut ring_at: Option<Instant> = None;
    match &target {
        Some(t) => {
            link.connect(t.as_str()).map_err(|e| format!("adresse {t} : {e}"))?;
            println!("ATDT {t}");
            modem.command(ModemCommand::Dial);
            answered = true;
        }
        None => println!("En attente d'un appel… (Ctrl-C pour quitter)"),
    }

    const BLOCK: usize = 160;
    let period = Duration::from_millis(20);
    let (mut rx, mut rx_line, mut tx, mut prev_tx) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK], vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    let start = Instant::now();
    let mut next = start;
    let mut t = 0u64;
    let mut last_stats = Instant::now();

    loop {
        link.recv_block(&mut rx);

        if !answered && link.has_peer_audio() {
            match ring_at {
                None => {
                    println!("DRIIING ! Appel entrant de {}", link.peer().map(|p| p.to_string()).unwrap_or_default());
                    ring_at = Some(Instant::now());
                }
                Some(r) if r.elapsed() >= Duration::from_secs(1) => {
                    println!("ATA");
                    modem.command(ModemCommand::Answer);
                    answered = true;
                }
                _ => {}
            }
        }

        let input: &[f32] = match line.as_mut() {
            Some((sim, echo)) => {
                sim.process(&rx, &mut rx_line);
                echo.process(&prev_tx, &mut rx_line);
                &rx_line
            }
            None => &rx,
        };
        modem.process(input, &mut tx);
        link.send_block(&tx).map_err(|e| format!("envoi UDP : {e}"))?;
        prev_tx.copy_from_slice(&tx);

        for ev in modem.drain_events() {
            print_event(t, role, &ev);
        }
        t += BLOCK as u64;

        if last_stats.elapsed() >= Duration::from_secs(10) && link.has_peer_audio() {
            let s = link.stats();
            println!(
                "            réseau : {} reçus, {} perdus, {} en retard, gigue {:.1} ms, tampon {:.0}/{:.0} ms",
                s.packets_received, s.jitter.lost, s.jitter.late, s.jitter.jitter_ms, s.jitter.depth_ms, s.jitter.target_ms
            );
            last_stats = Instant::now();
        }

        if let Some(limit) = seconds {
            if start.elapsed().as_secs_f64() >= limit {
                modem.command(ModemCommand::Hangup);
                for ev in modem.drain_events() {
                    print_event(t, role, &ev);
                }
                print_status(&modem.status());
                return Ok(());
            }
        }
        // Cadence de 20 ms, rattrapage si on a pris du retard.
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_millis(200) {
            next = now;
        }
    }
}

// ───────────────────────────── via le central ─────────────────────────────

/// Mot de passe lu au clavier sans écho (`stty -echo` ; sous Windows ou
/// hors terminal, la saisie reste visible / est lue sur l'entrée standard).
fn read_password(prompt: &str) -> Result<String, String> {
    use std::io::{BufRead, IsTerminal, Write};
    use std::process::{Command, Stdio};
    let stdin = std::io::stdin();
    let tty = stdin.is_terminal();
    if tty {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
    }
    let hidden = tty && Command::new("stty").arg("-echo").stdin(Stdio::inherit()).status().is_ok_and(|s| s.success());
    let mut line = String::new();
    let res = stdin.lock().read_line(&mut line);
    if hidden {
        let _ = Command::new("stty").arg("echo").stdin(Stdio::inherit()).status();
        eprintln!();
    }
    res.map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn central_config(args: &Args) -> Result<CentralConfig, String> {
    let server = args.get("server").unwrap_or(DEFAULT_SERVER);
    let server = if server.contains(':') { server.to_string() } else { format!("{server}:{DEFAULT_PORT}") };
    let user = args.get("user").ok_or("--user NOM manquant (votre nom d'abonné)")?;
    let password = match args.get("password") {
        Some(p) => p.to_string(),
        None => match std::env::var("V34_PASSWORD") {
            Ok(p) => p,
            Err(_) => read_password(&format!("Mot de passe de {user} : "))?,
        },
    };
    let mut cfg = CentralConfig::new(server, user, password);
    cfg.codec = args.codec()?;
    // même tampon que l'appli (adapté au modem : sans glissement)
    cfg.jitter_target_ms = args.num("jitter-ms", cfg.jitter_target_ms)?;
    cfg.local_port = args.num("local-port", 0u16)?;
    Ok(cfg)
}

/// Appel en cours côté modem.
struct CentralCall {
    modem: Modem,
    connected: bool,
    pipe: Option<ModemPipe>,
    tunnel: Option<TunnelClient>,
}

fn run_central(args: &Args, dial: Option<String>) -> Result<(), String> {
    let cfg = central_config(args)?;
    let preset = args.preset()?;
    let seconds: Option<f64> = args.get("seconds").map(|s| s.parse().map_err(|_| format!("--seconds invalide : {s}"))).transpose()?;
    let rings: f64 = args.num("rings", 1.0)?;
    let server = cfg.server.clone();
    let user = cfg.user.clone();
    let mut client = CentralClient::connect(cfg).map_err(|e| format!("central {server} : {e}"))?;
    println!("Central {server} ({}) : connexion de {user}…", client.server_addr());

    // Enregistrement (le mot de passe est dérivé localement, 15 s max)
    let t0 = Instant::now();
    let (number, services) = loop {
        client.poll().map_err(|e| format!("socket UDP : {e}"))?;
        match client.next_event() {
            Some(CentralEvent::Registered { number, services }) => break (number, services),
            Some(CentralEvent::AuthFailed { reason }) => return Err(format!("connexion refusée : {}", reason.describe())),
            _ => {}
        }
        if t0.elapsed() > Duration::from_secs(15) {
            return Err(format!("pas de réponse du central {server} (injoignable ? port UDP fermé ?)"));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let isp_number = args
        .get("isp")
        .map(str::to_string)
        .or_else(|| client.isp_number().map(str::to_string))
        .unwrap_or_else(|| "0836154286".into());
    println!("Connecté : {user}, votre numéro est le {number}.");
    if !services.is_empty() {
        let list: Vec<String> = services.iter().map(|(n, k)| format!("{n} ({k})")).collect();
        println!("Numéros spéciaux : {}", list.join(", "));
    }

    let is_isp = dial.as_deref() == Some(isp_number.as_str());
    // Le port du proxy est ouvert tout de suite : erreur immédiate s'il est pris.
    let mut proxy = if is_isp {
        let addr = args.get("socks").unwrap_or(DEFAULT_PROXY);
        Some(std::net::TcpListener::bind(addr).map_err(|e| format!("proxy local {addr} : {e}"))?)
    } else {
        None
    };
    // Le FAI n'a pas de login texte (l'abonné est déjà authentifié par le central).
    let isp_login = ClientConfig::default();

    match &dial {
        Some(n) => {
            client.dial(n).map_err(|e| format!("numérotation : {e}"))?;
            println!("ATDT {n}");
        }
        None => println!("En attente d'appels… (Ctrl-C pour quitter)"),
    }

    const BLOCK: usize = 160;
    let period = Duration::from_millis(20);
    let (mut rx, mut rx_line, mut tx, mut prev_tx) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK], vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    let mut line = preset.as_ref().map(|(_, p)| (LineSim::new(p.clone()), EchoPath::new(p.clone())));
    // comme l'appli : notre émission traverse aussi la boucle locale
    let mut line_up = preset.as_ref().map(|(_, p)| LineSim::with_seed(p.clone(), 0x5eed_0bad));
    let mut tx_line = vec![0.0f32; BLOCK];
    // comme l'appli : cadence calée sur l'horloge du central
    let mut pacer = v34_net::Pacer::new();
    // --charge N : envoie N octets/s en continu (ligne de test : ils
    // reviennent en écho) et fait le bilan toutes les 10 s
    let charge: usize = args.num("charge", 0usize)?;
    let (mut sent, mut got, mut got_win, mut win_t, mut connect_t) = (0usize, 0usize, 0usize, 0u64, None::<u64>);
    let mut call: Option<CentralCall> = None;
    let mut ring_at: Option<Instant> = None;
    let start = Instant::now();
    let mut next = start;
    let mut t = 0u64;

    loop {
        client.recv_block(&mut rx);

        // ── signalisation
        while let Some(ev) = client.next_event() {
            match ev {
                CentralEvent::Incoming { from, .. } => {
                    println!("DRIIING ! Appel de {from}");
                    ring_at = Some(Instant::now());
                }
                CentralEvent::Ringing { .. } => println!("Ça sonne chez le correspondant…"),
                CentralEvent::Answered { .. } => {
                    println!("Décroché ! Le modem appelant démarre.");
                    let mut modem = Modem::new(Role::Call, args.modem_config()?);
                    modem.command(ModemCommand::Dial);
                    call = Some(CentralCall { modem, connected: false, pipe: None, tunnel: None });
                }
                CentralEvent::Busy { number } => return Err(format!("{number} : OCCUPÉ")),
                CentralEvent::UnknownNumber { number } => return Err(format!("{number} : numéro inconnu ou non joignable")),
                CentralEvent::HungUp { reason, .. } => {
                    println!("Fin de l'appel : {}.", reason.describe());
                    ring_at = None;
                    if let Some(c) = call.take() {
                        print_status(&c.modem.status());
                    }
                    if dial.is_some() {
                        return Ok(());
                    }
                    println!("En attente d'appels…");
                }
                CentralEvent::ServerLost => println!("Central injoignable : ré-enregistrement…"),
                CentralEvent::Registered { number, .. } => println!("Ré-enregistré auprès du central (numéro {number})."),
                CentralEvent::AuthFailed { reason } => return Err(format!("connexion refusée : {}", reason.describe())),
            }
        }
        if ring_at.is_some_and(|r| r.elapsed().as_secs_f64() >= rings) {
            ring_at = None;
            println!("ATA");
            client.answer().map_err(|e| format!("décroché : {e}"))?;
            let mut modem = Modem::new(Role::Answer, args.modem_config()?);
            modem.command(ModemCommand::Answer);
            call = Some(CentralCall { modem, connected: false, pipe: None, tunnel: None });
        }

        // ── modem
        let mut finished: Option<Result<(), String>> = None;
        if let Some(c) = call.as_mut() {
            let input: &[f32] = match line.as_mut() {
                Some((sim, echo)) => {
                    sim.process(&rx, &mut rx_line);
                    echo.process(&prev_tx, &mut rx_line);
                    &rx_line
                }
                None => &rx,
            };
            c.modem.process(input, &mut tx);
            let out: &[f32] = match line_up.as_mut() {
                Some(sim) => {
                    sim.process(&tx, &mut tx_line);
                    &tx_line
                }
                None => &tx,
            };
            client.send_block(out).map_err(|e| format!("envoi UDP : {e}"))?;
            prev_tx.copy_from_slice(&tx);
            let role = c.modem.role();
            for ev in c.modem.drain_events() {
                match ev {
                    ModemEvent::Data(d) if charge > 0 && c.pipe.is_none() => {
                        got += d.len();
                        got_win += d.len();
                    }
                    ModemEvent::Data(d) if c.pipe.is_some() => {
                        if let Some(p) = c.pipe.as_mut() {
                            p.on_modem_data(d);
                        }
                    }
                    ModemEvent::Connected { tx_rate, .. } => {
                        print_event(t, role, &ev);
                        c.connected = true;
                        if let Some(listener) = proxy.take() {
                            let (pipe, tin, tout) = ModemPipe::new(tx_rate);
                            let tun = TunnelClient::spawn(listener, isp_login.clone(), tin, tout).map_err(|e| format!("tunnel : {e}"))?;
                            println!("proxy SOCKS5 sur {} (et HTTP CONNECT)", tun.local_addr());
                            c.pipe = Some(pipe);
                            c.tunnel = Some(tun);
                        }
                    }
                    ModemEvent::Disconnected { .. } => {
                        print_event(t, role, &ev);
                        finished = Some(if c.connected { Ok(()) } else { Err("la connexion n'a pas abouti".into()) });
                    }
                    _ => print_event(t, role, &ev),
                }
            }
            if let Some(p) = c.pipe.as_mut() {
                if let Some(bytes) = p.take_for_modem() {
                    c.modem.command(ModemCommand::Send(bytes));
                }
            }
            if charge > 0 && c.connected && c.pipe.is_none() {
                let ct = *connect_t.get_or_insert(t);
                if t % 1600 == 0 {
                    // 1/5 s de données, numérotées (octet = rang modulo 251)
                    let n = charge / 5;
                    let chunk: Vec<u8> = (sent..sent + n).map(|i| (i % 251) as u8).collect();
                    sent += n;
                    c.modem.command(ModemCommand::Send(chunk));
                }
                if t - ct >= win_t + 80_000 {
                    win_t += 80_000;
                    let st = c.modem.status();
                    let p = &st.params;
                    println!(
                        "[{:3} s de données] reçu en écho {:6.1} Ko/s ({} o au total sur {} envoyés) — liaison ↓{} ↑{}",
                        win_t / 8000,
                        got_win as f64 / 10.0 / 1024.0,
                        got,
                        sent,
                        p.data_rate_rx.unwrap_or(0),
                        p.data_rate_tx.unwrap_or(0)
                    );
                    got_win = 0;
                }
            }
            if let Some(tun) = c.tunnel.as_ref() {
                while let Some(ev) = tun.try_event() {
                    match ev {
                        TunnelEvent::Text(txt) => print!("{txt}"),
                        TunnelEvent::Ready { max_streams } => println!("\nTunnel établi ({max_streams} connexions simultanées max)."),
                        TunnelEvent::Opened { id, target } => println!("            tunnel : flux {id} → {target}"),
                        TunnelEvent::OpenFailed { target, reason, .. } => println!("            tunnel : {target} refusé ({reason})"),
                        TunnelEvent::Closed { .. } => {}
                        TunnelEvent::Corruption => println!("            tunnel : erreur de transmission, connexions coupées"),
                        TunnelEvent::Ended { reason } => {
                            println!("\nTunnel terminé : {reason}");
                            c.modem.command(ModemCommand::Hangup);
                        }
                    }
                }
            }
        }
        if let Some(res) = finished {
            let c = call.take().expect("appel en cours");
            print_status(&c.modem.status());
            let _ = client.hangup();
            if dial.is_some() {
                // laisser partir le HANGUP
                let t1 = Instant::now();
                while t1.elapsed() < Duration::from_millis(500) {
                    let _ = client.poll();
                    std::thread::sleep(Duration::from_millis(20));
                }
                return res;
            }
            println!("En attente d'appels…");
        }

        t += BLOCK as u64;
        if let Some(limit) = seconds {
            if start.elapsed().as_secs_f64() >= limit {
                if let Some(c) = call.as_mut() {
                    c.modem.command(ModemCommand::Hangup);
                    print_status(&c.modem.status());
                }
                let _ = client.unregister();
                let t1 = Instant::now();
                while t1.elapsed() < Duration::from_millis(500) {
                    let _ = client.poll();
                    std::thread::sleep(Duration::from_millis(20));
                }
                return Ok(());
            }
        }
        // +1 : un bloc de plus tout de suite ; −1 : un bloc de moins
        let adj = pacer.update(client.jitter_level());
        next += period.mul_f64((1.0 - adj).max(0.0));
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_millis(200) {
            next = now;
        }
    }
}

fn list_presets() {
    println!("Préréglages du simulateur de ligne :");
    for (name, p) in LineParams::presets() {
        println!(
            "  {name:<16} bande {:.0}-{:.0} Hz, atténuation {:.0} dB, pente {:.0} dB, SNR {:.0} dB, clics {}/s, décalage {} Hz, délai {:.0} ms, écho {:.0} dB/{:.0} ms, coupures {}/s",
            p.band_low_hz, p.band_high_hz, p.attenuation_db, p.hf_tilt_db, p.snr_db, p.impulses_per_s, p.freq_offset_hz, p.delay_ms, p.echo_db, p.echo_delay_ms, p.dropouts_per_s
        );
    }
}

fn main() -> ExitCode {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("erreur : {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.get("help").is_some() {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let res = match args.positional.first().map(String::as_str) {
        Some("local") => run_local(&args),
        Some("answer") => run_realtime(&args, Role::Answer),
        Some("call") => run_realtime(&args, Role::Call),
        Some("register") => run_central(&args, None),
        Some("dial") => match args.positional.get(1) {
            Some(n) => run_central(&args, Some(n.clone())),
            None => Err("numéro manquant : v34-cli dial --user NOM <numéro>".into()),
        },
        Some("presets") => {
            list_presets();
            Ok(())
        }
        _ => {
            println!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erreur : {e}");
            ExitCode::FAILURE
        }
    }
}
