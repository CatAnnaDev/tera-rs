mod auth;
mod arborea;
mod heal;

static COMPACT: AtomicBool = AtomicBool::new(false);

use anyhow::{bail, Context, Result};
use clap::Parser;
use std::io::{BufRead, ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tera_protocol::handshake::{random_key, ClientHandshake, MAGIC};
use tera_protocol::session::{Decrypting, Encrypting, KEY_LEN, MODERN};
use tera_protocol::{OpcodeMap, PacketBuffer};
use tera_protocol::Registry;

#[derive(Parser)]
#[command(name = "tera-bot", about = "Client TERA headless : handshake, login, liste de personnages")]
struct Cli {
    #[arg(long, help = "adresse:port du serveur ; si absent, resolu automatiquement depuis la serverlist")]
    server: Option<String>,
    #[arg(long, default_value = "https://launcher.tera-europe.net/classicplus/serverlist.json", help = "URL de la serverlist officielle utilisee quand --server est absent")]
    serverlist: String,
    #[arg(long, help = "nom du serveur a choisir dans la serverlist (sinon le 1er disponible)")]
    server_name: Option<String>,
    #[arg(long, help = "insere un tera-proxy (inspection/mods) entre le bot et --server")]
    proxy: bool,
    #[arg(long, help = "route la connexion au serveur via un proxy SOCKS5 (ex: 127.0.0.1:1080)")]
    socks5: Option<String>,
    #[arg(long, default_value = "41946")]
    account: String,
    #[arg(long, default_value = "0123456789abcdef0123456789abcdef")]
    ticket: String,
    #[arg(long, help = "lit le ticket BRUT depuis un fichier .bin (capture par le shim)")]
    ticket_file: Option<PathBuf>,
    #[arg(long, help = "connexion navigateur (OAuth) pour obtenir/sauver un refresh_token")]
    login: bool,
    #[arg(long, help = "refresh + affiche 'AUTH <compte> <ticket>' et sort (sans se connecter)")]
    print_ticket: bool,
    #[arg(long, help = "fichier d'auth (refresh_token + ticket) ; refresh auto avant connexion")]
    auth_file: Option<PathBuf>,
    #[arg(long, help = "auth Arborea Reborn : refresh le ticket + serveur automatiquement")]
    arborea: bool,
    #[arg(long, help = "login Arborea une seule fois (email/password/captcha) puis sauve le refresh_token")]
    arborea_login: bool,
    #[arg(long)]
    arborea_email: Option<String>,
    #[arg(long)]
    arborea_password: Option<String>,
    #[arg(long, help = "token captcha (header Captcha du login web)")]
    arborea_captcha: Option<String>,
    #[arg(long, default_value = "tera-arborea-auth.json")]
    arborea_auth_file: PathBuf,
    #[arg(long, default_value = "data/opcodes/protocol.376012.map")]
    opcodes: PathBuf,
    #[arg(long, default_values = ["data/definitions"])]
    definitions: Vec<PathBuf>,
    #[arg(long, default_value_t = 10002, help = "valeur du CHAMP C_LOGIN_ARBITER (build client)")]
    patch_version: u32,
    #[arg(long, default_value_t = 100, help = "patch majeur pour CHOISIR les versions de def (Classic = 100)")]
    major_patch: u32,
    #[arg(long, default_value_t = 6, help = "6 = EUR")]
    language: u32,
    #[arg(long, default_value_t = 376012)]
    version_a: i64,
    #[arg(long, default_value_t = 376001)]
    version_b: i64,
    #[arg(long, help = "nom du perso a selectionner (defaut : le premier)")]
    character: Option<String>,
    #[arg(long, help = "mode chat interactif : tape des messages, affiche les S_CHAT recus")]
    chat: bool,
    #[arg(long, default_value_t = 0, help = "canal de chat par defaut (0=say, 1=party, 2=guild, 3=area)")]
    chat_channel: u32,
    #[arg(long, default_value = "double.meow", help = "pilote le bot : n'accepte les commandes (whisper) que de ce pseudo (vide = tout le monde)")]
    commander: String,
    #[arg(long, help = "active le heal de groupe automatique (le bot doit etre priest)")]
    heal: bool,
    #[arg(long, help = "heal aussi les joueurs hors groupe autour (WIP)")]
    heal_everyone: bool,
    #[arg(long, default_value_t = 190900, help = "id de base du focus heal (Focus Heal IX pretre)")]
    focus_heal_id: u32,
    #[arg(long, default_value_t = 370200, help = "id de base du healing immersion (Healing Immersion II pretre)")]
    healing_immersion_id: u32,
    #[arg(long, default_value_t = 85.0, help = "declenche le heal en dessous de ce %% de vie")]
    hp_limit: f64,
    #[arg(long, default_value_t = 20.0, help = "portee de heal en metres")]
    heal_range: f64,
    #[arg(long, default_value_t = 2, help = "1er champ 'type' du C_REQUEST_CONTRACT (ajustable via 'itype N'; unk2=type de contrat via 'iunk2 N')")]
    invite_type: i64,
    #[arg(long, default_value_t = 0, help = "secondes d'ecoute (0 = illimite, repond aux pings)")]
    listen: u64,
    #[arg(long, help = "ne teste que connexion + greeting + handshake (aucun fichier requis)")]
    probe: bool,
    #[arg(long, help = "envoie le handshake en premier au lieu d'attendre le greeting")]
    send_first: bool,
    #[arg(long, help = "affiche TOUS les paquets que le bot envoie, en clair, SANS se connecter")]
    show: bool,
    #[arg(long, help = "aller-retour parse/re-serialise sur C_CHECK_VERSION reel")]
    roundtrip: bool,
}

fn hexdump(prefix: &str, data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (32..127).contains(&b) { b as char } else { '.' })
            .collect();
        println!("{prefix}{:04x}  {:<47}  {ascii}", i * 16, hex.join(" "));
    }
}

fn show(
    dir: &str,
    t0: Instant,
    name: &str,
    opcode: u16,
    body: &[u8],
    registry: &Registry,
    full: &[u8],
) {
    if COMPACT.load(Ordering::Relaxed) {
        let preview: String = body
            .iter()
            .take(24)
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "[{:7.3}] {dir} {name} ({opcode}) {} o  {preview}",
            t0.elapsed().as_secs_f64(),
            body.len()
        );
        return;
    }
    println!(
        "\n[{:7.3}] {dir} {name} ({opcode}) {} octets",
        t0.elapsed().as_secs_f64(),
        body.len()
    );
    hexdump("           ", body);
    match registry.get(name) {
        Some(def) => match tera_protocol::read(def, full) {
            Ok(obj) => {
                for (k, v) in obj.fields.iter() {
                    println!("           . {k} = {v:?}");
                }
                if obj.fields.is_empty() {
                    println!("           . (aucun champ)");
                }
            }
            Err(e) => println!("           . decodage impossible : {e}"),
        },
        None => println!("           . pas de definition pour {name}"),
    }
}

fn read_exactly(stream: &mut TcpStream, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).with_context(|| format!("lecture de {len} octets"))?;
    Ok(buf)
}

fn send(
    stream: &mut TcpStream,
    enc: &mut Encrypting,
    registry: &Registry,
    opcodes: &OpcodeMap,
    name: &str,
    object: &tera_protocol::Object,
    t0: Instant,
) -> Result<()> {
    let def = registry.get(name).with_context(|| format!("definition {name} introuvable"))?;
    let opcode = opcodes.code(name).with_context(|| format!("opcode {name} introuvable"))?;
    let mut bytes = tera_protocol::write(def, opcode, object)?;
    show("->", t0, name, opcode, &bytes[4..], registry, &bytes);
    enc.apply(&mut bytes);
    stream.write_all(&bytes)?;
    Ok(())
}

fn is_disconnect(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>()
        .map(|e| {
            matches!(
                e.kind(),
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
            )
        })
        .unwrap_or(false)
}

fn reactive_send(
    stream: &mut TcpStream,
    enc: &mut Encrypting,
    registry: &Registry,
    opcodes: &OpcodeMap,
    name: &str,
    object: &tera_protocol::Object,
    t0: Instant,
) -> Result<bool> {
    match send(stream, enc, registry, opcodes, name, object, t0) {
        Ok(()) => Ok(false),
        Err(e) if is_disconnect(&e) => {
            println!("[{:7.3}] serveur a ferme (ecriture {name})", t0.elapsed().as_secs_f64());
            Ok(true)
        }
        Err(e) => Err(e),
    }
}

#[derive(serde::Deserialize)]
struct SlServer {
    name: String,
    address: String,
    port: u16,
    #[serde(default)]
    available: i64,
}

#[derive(serde::Deserialize)]
struct ServerListDoc {
    servers: Vec<SlServer>,
}

fn resolve_server(url: &str, name: Option<&str>) -> Result<String> {
    let mut resp = ureq::get(url)
        .header("User-Agent", "tera-bot")
        .call()
        .context("recuperation de la serverlist")?;
    let doc: ServerListDoc = resp.body_mut().read_json().context("parsing de la serverlist")?;
    let chosen = match name {
        Some(n) => {
            let n = n.to_lowercase();
            doc.servers.iter().find(|s| s.name.to_lowercase().contains(&n))
        }
        None => doc
            .servers
            .iter()
            .find(|s| s.available != 0)
            .or_else(|| doc.servers.first()),
    }
    .ok_or_else(|| anyhow::anyhow!("aucun serveur trouve dans la serverlist"))?;
    Ok(format!("{}:{}", chosen.address, chosen.port))
}

fn probe(server: &str) -> Result<()> {
    let t0 = Instant::now();
    println!("cible {server}");
    let mut stream = match TcpStream::connect(server) {
        Ok(s) => s,
        Err(e) => { println!("[{:6.2}s] CONNEXION IMPOSSIBLE : {e}", t0.elapsed().as_secs_f64()); return Ok(()) }
    };
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    println!("[{:6.2}s] TCP connecte", t0.elapsed().as_secs_f64());
    let mut greeting = [0u8; 4];
    match stream.read_exact(&mut greeting) {
        Ok(()) => println!("[{:6.2}s] GREETING RECU {greeting:02x?}  -> le serveur repond, ce reseau marche", t0.elapsed().as_secs_f64()),
        Err(e) => {
            println!("[{:6.2}s] AUCUN GREETING : {e}", t0.elapsed().as_secs_f64());
            println!("           -> le serveur accepte le TCP mais ne parle jamais depuis ce reseau");
            return Ok(());
        }
    }
    let hs = ClientHandshake::new(random_key(), random_key()).with_constants(MODERN);
    stream.write_all(hs.first())?;
    let s1 = read_exactly(&mut stream, KEY_LEN)?;
    stream.write_all(hs.second())?;
    let s2 = read_exactly(&mut stream, KEY_LEN)?;
    let _ = (s1, s2);
    println!("[{:6.2}s] HANDSHAKE COMPLET -> tout va bien depuis ce reseau", t0.elapsed().as_secs_f64());
    Ok(())
}

fn strip_markup(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut in_tag = false;
    for ch in message.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn print_chat(registry: &Registry, name: &str, packet: &tera_protocol::Packet) {
    let Some(def) = registry.get(name) else { return };
    let Ok(object) = tera_protocol::read(def, &packet.encode()) else { return };
    let message = object.get("message").and_then(tera_protocol::Value::as_str).unwrap_or("");
    let channel = object.get("channel").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
    let clean = strip_markup(message);
    if clean.trim().is_empty() {
        return;
    }
    let author = object.get("name").and_then(tera_protocol::Value::as_str).unwrap_or("");
    let label = match name {
        "S_WHISPER" | "S_PRIVATE_CHAT" => "chuchote",
        "S_DUNGEON_EVENT_MESSAGE" | "S_SYSTEM_MESSAGE" => "serveur",
        _ => "chat",
    };
    if author.is_empty() {
        println!("  [{label} c{channel}] {clean}");
    } else {
        println!("  [{label} c{channel}] {author}: {clean}");
    }
}

fn pick_character(
    registry: &Registry,
    packet: &tera_protocol::Packet,
    want: Option<&str>,
) -> Option<(u64, String)> {
    let def = registry.get("S_GET_USER_LIST")?;
    let object = tera_protocol::read(def, &packet.encode()).ok()?;
    let characters = match object.get("characters") {
        Some(tera_protocol::Value::Array(list)) => list,
        _ => return None,
    };
    let extract = |character: &tera_protocol::Object| -> Option<(u64, String)> {
        let id = character.get("id").and_then(tera_protocol::Value::as_uint)?;
        let name = character
            .get("name")
            .and_then(tera_protocol::Value::as_str)
            .unwrap_or("?")
            .to_string();
        Some((id, name))
    };
    match want {
        Some(wanted) => characters
            .iter()
            .find(|character| {
                character
                    .get("name")
                    .and_then(tera_protocol::Value::as_str)
                    .map(|name| name.eq_ignore_ascii_case(wanted))
                    .unwrap_or(false)
            })
            .and_then(extract),
        None => characters.first().and_then(extract),
    }
}

fn build_outgoing(cli: &Cli, account: &str, ticket_bytes: &[u8]) -> Vec<(&'static str, tera_protocol::Object)> {
    let u = |v: u64| tera_protocol::Value::Uint(v);
    let version = tera_protocol::Object::new().with("version", tera_protocol::Value::Array(vec![
        tera_protocol::Object::new().with("index", tera_protocol::Value::Int(0)).with("value", tera_protocol::Value::Int(cli.version_a)),
        tera_protocol::Object::new().with("index", tera_protocol::Value::Int(1)).with("value", tera_protocol::Value::Int(cli.version_b)),
    ]));
    let login = tera_protocol::Object::new()
        .with("unk1", tera_protocol::Value::Int(0)).with("unk2", u(0))
        .with("language", u(cli.language as u64)).with("patchVersion", tera_protocol::Value::Int(cli.patch_version as i64))
        .with("name", tera_protocol::Value::Str(account.to_string()))
        .with("ticket", tera_protocol::Value::Bytes(ticket_bytes.to_vec()));
    let range = tera_protocol::Object::new().with("range", u(2000));
    let hw = tera_protocol::Object::new()
        .with("systemMemory", u(16383)).with("videoMemory", u(0)).with("resWidth", u(1920)).with("resHeight", u(1080))
        .with("isFullScreen", tera_protocol::Value::Bool(false)).with("resScreenWidth", u(1920)).with("resScreenHeight", u(1080))
        .with("numDisplays", u(1)).with("resVirtualWidth", u(1920)).with("resVirtualHeight", u(1080))
        .with("physicalCores", u(10)).with("logicalCores", u(10))
        .with("os", tera_protocol::Value::Str("Windows 10".into()))
        .with("cpu", tera_protocol::Value::Str("VirtualApple @ 2.50GHz".into()))
        .with("gpu", tera_protocol::Value::Str(String::new()));
    vec![
        ("C_CHECK_VERSION", version), ("C_LOGIN_ARBITER", login),
        ("C_SET_VISIBLE_RANGE", range), ("C_GET_USER_LIST", tera_protocol::Object::new()),
        ("C_HARDWARE_INFO", hw), ("C_PONG", tera_protocol::Object::new()),
    ]
}

fn show_all(cli: &Cli, account: &str, opcodes: &OpcodeMap, registry: &Registry, ticket_bytes: &[u8]) -> Result<()> {
    println!("=== paquets que le bot ENVOIE (compte={}, ticket={} octets) ===", account, ticket_bytes.len());
    let t0 = Instant::now();
    for (name, obj) in build_outgoing(cli, account, ticket_bytes) {
        let def = registry.get(name).with_context(|| format!("def {name}"))?;
        let opcode = opcodes.code(name).with_context(|| format!("opcode {name}"))?;
        let bytes = tera_protocol::write(def, opcode, &obj)?;
        show("->", t0, name, opcode, &bytes[4..], registry, &bytes);
    }
    Ok(())
}

struct ProxyGuard(Option<std::process::Child>);

impl Drop for ProxyGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_proxy(
    upstream: &str,
    opcodes: &std::path::Path,
    definitions: &[PathBuf],
    major_patch: u32,
) -> Result<(std::process::Child, String)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").context("picking a proxy port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let listen = format!("127.0.0.1:{port}");

    let exe = std::env::current_exe()?;
    let binary = exe
        .parent()
        .map(|dir| dir.join("tera-proxy"))
        .context("locating tera-proxy next to tera-bot")?;
    let mut command = std::process::Command::new(&binary);
    command
        .arg("--listen")
        .arg(&listen)
        .arg("--upstream")
        .arg(upstream)
        .arg("--opcodes")
        .arg(opcodes)
        .arg("--patch-version")
        .arg(major_patch.to_string())
        .arg("--once");
    for definition in definitions {
        command.arg("--definitions").arg(definition);
    }
    let child = command
        .spawn()
        .with_context(|| format!("spawning {}", binary.display()))?;

    for _ in 0..100 {
        if std::net::TcpListener::bind(&listen).is_err() {
            return Ok((child, listen));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok((child, listen))
}

fn socks5_connect(proxy: &str, target: &str) -> Result<TcpStream> {
    let proxy_addr = proxy
        .to_socket_addrs()
        .context("resolution du proxy socks5")?
        .next()
        .with_context(|| format!("aucune adresse pour {proxy}"))?;
    let (host, port) = target
        .rsplit_once(':')
        .with_context(|| format!("cible invalide {target}"))?;
    let port: u16 = port.parse().with_context(|| format!("port invalide {port}"))?;
    let mut stream = TcpStream::connect_timeout(&proxy_addr, Duration::from_secs(10))
        .with_context(|| format!("connexion au proxy socks5 {proxy}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.write_all(&[0x05, 0x01, 0x00])?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method)?;
    if method != [0x05, 0x00] {
        bail!("proxy socks5 refuse le no-auth (reponse {method:02x?})");
    }
    let mut request = vec![0x05, 0x01, 0x00];
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            request.push(0x01);
            request.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            let bytes = host.as_bytes();
            if bytes.len() > 255 {
                bail!("hote trop long pour socks5 : {host}");
            }
            request.push(0x03);
            request.push(bytes.len() as u8);
            request.extend_from_slice(bytes);
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request)?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply)?;
    if reply[1] != 0x00 {
        bail!("socks5 connect echoue (code {})", reply[1]);
    }
    let bound = match reply[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len)?;
            len[0] as usize
        }
        other => bail!("socks5 atyp inconnu {other}"),
    };
    let mut tail = vec![0u8; bound + 2];
    stream.read_exact(&mut tail)?;
    Ok(stream)
}

fn main() -> Result<()> {
    let mut cli = Cli::parse();

    if cli.arborea_login {
        let email = cli.arborea_email.as_deref().context("--arborea-email requis")?;
        let password = cli.arborea_password.as_deref().context("--arborea-password requis")?;
        let captcha = cli.arborea_captcha.as_deref().context("--arborea-captcha requis")?;
        let saved = arborea::login(email, password, captcha)?;
        arborea::save(&cli.arborea_auth_file, &saved)?;
        println!(
            "Arborea : refresh_token sauve dans {}",
            cli.arborea_auth_file.display()
        );
        return Ok(());
    }

    let auth_path = cli
        .auth_file
        .clone()
        .unwrap_or_else(|| PathBuf::from("tera-bot-auth.json"));

    if cli.print_ticket {
        let saved = if cli.login {
            let fresh = auth::login()?;
            auth::save(&auth_path, &fresh)?;
            fresh
        } else {
            let saved = auth::load(&auth_path).with_context(|| {
                format!("aucune auth dans {} (relance avec --login)", auth_path.display())
            })?;
            let fresh = auth::refresh(&auth_path, &saved)?;
            auth::save(&auth_path, &fresh)?;
            fresh
        };
        println!("AUTH\t{}\t{}", saved.account(), saved.auth_key);
        return Ok(());
    }

    if cli.login {
        let saved = auth::login()?;
        auth::save(&auth_path, &saved)?;
        println!(
            "Connecte en tant que {} (UserNo {}). Auth sauvee dans {}.",
            saved.account(),
            saved.user_no,
            auth_path.display()
        );
        println!("Ticket frais : {}", saved.auth_key);
        return Ok(());
    }

    let (account, ticket_bytes): (String, Vec<u8>) = if cli.arborea {
        let saved = arborea::load(&cli.arborea_auth_file)?;
        println!("Arborea : refresh du ticket...");
        let game = arborea::prepare(&saved, &cli.arborea_auth_file)?;
        println!(
            "Arborea : {} (id {}) -> serveur {} (ticket {})",
            game.display_name, game.name, game.server, game.ticket
        );
        cli.server = Some(game.server);
        cli.patch_version = arborea::BUILD_VERSION;
        (game.name, game.ticket.into_bytes())
    } else if cli.auth_file.is_some() {
        let saved = auth::load(&auth_path)?;
        println!("Refresh du ticket via le refresh_token sauve...");
        let fresh = auth::refresh(&auth_path, &saved)?;
        auth::save(&auth_path, &fresh)?;
        println!(
            "Ticket frais pour {} : {} ({} octets)",
            fresh.account(),
            fresh.auth_key,
            fresh.auth_key.len()
        );
        (fresh.account(), fresh.auth_key.as_bytes().to_vec())
    } else {
        let bytes = match &cli.ticket_file {
            Some(f) => {
                let b = std::fs::read(f)
                    .with_context(|| format!("lecture du ticket {}", f.display()))?;
                println!("ticket lu depuis {} : {} octets bruts", f.display(), b.len());
                b
            }
            None => cli.ticket.as_bytes().to_vec(),
        };
        (cli.account.clone(), bytes)
    };
    let opcodes = OpcodeMap::read(&cli.opcodes)
        .with_context(|| format!("lecture de {}", cli.opcodes.display()))?;
    let arborea_pins: std::collections::HashMap<String, u32> = [
        ("C_REQUEST_CONTRACT", 2),
        ("C_REPLY_THROUGH_ARBITER_CONTRACT", 1),
        ("S_BEGIN_THROUGH_ARBITER_CONTRACT", 1),
        ("S_REQUEST_CONTRACT", 1),
        ("S_PARTY_MEMBER_LIST", 7),
        ("S_PARTY_MEMBER_INTERVAL_POS_UPDATE", 3),
        ("S_PARTY_MEMBER_CHANGE_HP", 4),
        ("C_START_SKILL", 8),
        ("C_CAN_LOCKON_TARGET", 3),
        ("S_START_COOLTIME_SKILL", 3),
        ("S_EACH_SKILL_RESULT", 14),
        ("S_ACTION_STAGE", 9),
        ("S_SPAWN_NPC", 11),
        ("S_DIALOG", 3),
        ("C_NPC_CONTACT", 2),
        ("C_DIALOG", 1),
        ("S_SPAWN_USER", 16),
        ("S_SPAWN_ME", 3),
        ("S_USER_LOCATION", 5),
        ("S_LOGIN", 14),
        ("C_PLAYER_LOCATION", 5),
    ]
    .iter()
    .map(|(name, version)| ((*name).to_string(), *version))
    .collect();
    let registry = Registry::pinned(&cli.definitions, Some(cli.major_patch), &arborea_pins)?;
    println!("{} opcodes, {} definitions", opcodes.len(), registry.len());

    if cli.roundtrip {
        let real: Vec<u8> = vec![
            0x20,0x00,0xbc,0x4d, 0x02,0x00,0x08,0x00,0x08,0x00,0x14,0x00,
            0x00,0x00,0x00,0x00,0xcc,0xbc,0x05,0x00,0x14,0x00,0x00,0x00,
            0x01,0x00,0x00,0x00,0xc1,0xbc,0x05,0x00,
        ];
        let def = registry.get("C_CHECK_VERSION").context("def C_CHECK_VERSION")?;
        println!("octets reels du client ({} o) :", real.len());
        for c in real.chunks(16) { println!("  {}", c.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")); }
        let obj = tera_protocol::read(def, &real).context("PARSE echoue")?;
        println!("\nparse -> {obj:?}");
        let reser = tera_protocol::write(def, 19900, &obj).context("re-serialisation echouee")?;
        println!("\nre-serialise ({} o) :", reser.len());
        for c in reser.chunks(16) { println!("  {}", c.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")); }
        println!("\n=> {}", if reser == real { "IDENTIQUE : l'array se parse ET se re-encode a l'octet pres" } else { "DIFFERENT : il y a un probleme dans la def array" });
        return Ok(());
    }
    if cli.server.is_none() {
        let resolved = resolve_server(&cli.serverlist, cli.server_name.as_deref())?;
        println!("serveur resolu depuis la serverlist : {resolved}");
        cli.server = Some(resolved);
    }
    let server = cli.server.clone().expect("serveur resolu");
    if cli.probe {
        return probe(&server);
    }
    if cli.chat {
        COMPACT.store(true, Ordering::Relaxed);
    }
    // Toujours montrer ce que le bot ENVERRA, avant toute connexion.
    show_all(&cli, &account, &opcodes, &registry, &ticket_bytes)?;
    if cli.show {
        return Ok(());
    }
    println!("\n--- connexion et echange reel ---");

    let t0 = Instant::now();
    let mut proxy_guard = ProxyGuard(None);
    let target = if cli.proxy {
        let (child, addr) = spawn_proxy(&server, &cli.opcodes, &cli.definitions, cli.major_patch)?;
        println!("proxy insere : {addr} -> {server}");
        proxy_guard = ProxyGuard(Some(child));
        addr
    } else {
        server.clone()
    };
    let _ = &proxy_guard;
    println!("connexion a {target}");
    let mut stream = match &cli.socks5 {
        Some(proxy) => {
            println!("via SOCKS5 {proxy}");
            socks5_connect(proxy, &target)?
        }
        None => {
            let addr = target
                .to_socket_addrs()
                .context("resolution de l'adresse")?
                .next()
                .with_context(|| format!("aucune adresse pour {target}"))?;
            TcpStream::connect_timeout(&addr, Duration::from_secs(10)).context("connect")?
        }
    };
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    println!("[{:7.3}] connecte", t0.elapsed().as_secs_f64());
    match stream.local_addr() {
        Ok(local) => {
            println!("[{:7.3}] IP source locale reellement utilisee : {local}", t0.elapsed().as_secs_f64());
            let ip = local.ip();
            let hint = if cli.socks5.is_some() {
                "via SOCKS5 -> l'IP publique de sortie est celle du proxy, pas celle-ci"
            } else if ip.to_string().starts_with("10.66.66.") {
                "= interface WireGuard : la connexion sort bien par le tunnel"
            } else if ip.is_loopback() {
                "= loopback : connexion detournee localement (relais)"
            } else {
                "= interface locale directe (pas de tunnel sur cette route)"
            };
            println!("           {hint}");
        }
        Err(e) => println!("[{:7.3}] local_addr indisponible : {e}", t0.elapsed().as_secs_f64()),
    }
    if let Ok(remote) = stream.peer_addr() {
        println!("[{:7.3}] pair distant (peer) : {remote}", t0.elapsed().as_secs_f64());
    }

    if cli.send_first {
        println!("[{:7.3}] mode send-first : on parle en premier (pas d'attente de greeting)", t0.elapsed().as_secs_f64());
    } else {
        println!("[{:7.3}] en attente du greeting du serveur (il parle en premier)...", t0.elapsed().as_secs_f64());
        let greeting = read_exactly(&mut stream, MAGIC.len()).context("greeting du serveur")?;
        if greeting != MAGIC {
            bail!("greeting inattendu : {greeting:02x?} (attendu {MAGIC:02x?})");
        }
        println!("[{:7.3}] <- greeting", t0.elapsed().as_secs_f64());
        hexdump("           ", &greeting);
    }

    let hs = ClientHandshake::new(random_key(), random_key()).with_constants(MODERN);
    println!("[{:7.3}] -> cle client 1 ({} o)", t0.elapsed().as_secs_f64(), KEY_LEN);
    hexdump("           ", hs.first());
    stream.write_all(hs.first())?;
    let s1: [u8; KEY_LEN] = read_exactly(&mut stream, KEY_LEN)?.try_into().unwrap();
    println!("[{:7.3}] <- cle serveur 1", t0.elapsed().as_secs_f64());
    hexdump("           ", &s1);
    println!("[{:7.3}] -> cle client 2", t0.elapsed().as_secs_f64());
    hexdump("           ", hs.second());
    stream.write_all(hs.second())?;
    let s2: [u8; KEY_LEN] = read_exactly(&mut stream, KEY_LEN)?.try_into().unwrap();
    println!("[{:7.3}] <- cle serveur 2", t0.elapsed().as_secs_f64());
    hexdump("           ", &s2);
    let (mut enc, mut dec): (Encrypting, Decrypting) = hs.finish(&s1, &s2).split();
    println!("[{:7.3}] session chiffree etablie", t0.elapsed().as_secs_f64());

    let version = tera_protocol::Object::new()
        .with("version", tera_protocol::Value::Array(vec![
            tera_protocol::Object::new()
                .with("index", tera_protocol::Value::Int(0))
                .with("value", tera_protocol::Value::Int(cli.version_a)),
            tera_protocol::Object::new()
                .with("index", tera_protocol::Value::Int(1))
                .with("value", tera_protocol::Value::Int(cli.version_b)),
        ]));
    send(&mut stream, &mut enc, &registry, &opcodes, "C_CHECK_VERSION", &version, t0)?;

    let login = tera_protocol::Object::new()
        .with("unk1", tera_protocol::Value::Int(0))
        .with("unk2", tera_protocol::Value::Uint(0))
        .with("language", tera_protocol::Value::Uint(cli.language as u64))
        .with("patchVersion", tera_protocol::Value::Int(cli.patch_version as i64))
        .with("name", tera_protocol::Value::Str(account.clone()))
        .with("ticket", tera_protocol::Value::Bytes(ticket_bytes.clone()));
    send(&mut stream, &mut enc, &registry, &opcodes, "C_LOGIN_ARBITER", &login, t0)?;

    let mut packets = PacketBuffer::new();
    let mut buf = [0u8; 8192];
    let mut asked_list = false;
    let mut sent_hw = false;
    let mut selected = false;
    let mut player_loc: Option<tera_protocol::Value> = None;
    let mut player_w: i64 = 0;
    let mut in_world = false;
    let keepalive_interval = Duration::from_secs(50);
    let mut last_keepalive = Instant::now();
    let mut world = tera_world::World::new();
    let mut bot_name = String::new();
    let mut invite_type: i64 = cli.invite_type;
    let mut invite_unk2: i64 = 4;
    let mut reply_response: u64 = 1;
    let mut pending_sends: Vec<(Instant, &'static str, tera_protocol::Object)> = Vec::new();
    let mut bot_pos: Option<[f32; 3]> = None;
    let mut user_ids: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut heal = heal::HealBot::new(heal::HealConfig {
        enabled: cli.heal,
        focus_heal: cli.focus_heal_id,
        healing_immersion: cli.healing_immersion_id,
        hp_limit: cli.hp_limit,
        max_distance: cli.heal_range,
        ..heal::HealConfig::default()
    });
    let heal_packets = [
        "S_LOGIN", "S_SPAWN_ME", "S_PLAYER_STAT_UPDATE", "S_PARTY_MEMBER_LIST", "S_PARTY_MEMBER_CHANGE_HP",
        "S_SPAWN_USER", "S_USER_LOCATION", "S_PARTY_MEMBER_INTERVAL_POS_UPDATE",
        "S_EACH_SKILL_RESULT", "S_START_COOLTIME_SKILL", "S_CANNOT_START_SKILL", "S_SKILL_LIST",
        "S_LEAVE_PARTY", "S_LEAVE_PARTY_MEMBER", "S_BAN_PARTY_MEMBER", "S_LOGOUT_PARTY_MEMBER",
        "S_SPAWN_NPC", "S_DESPAWN_NPC", "S_NPC_LOCATION", "S_ACTION_STAGE", "S_DIALOG",
    ];
    let mut last_heal_tick = Instant::now();
    let heal_tick_interval = Duration::from_millis(500);
    let mut commander_party: Option<(u64, u64)> = None;
    let mut move_target: Option<[f32; 3]> = None;
    let mut follow_id: Option<u64> = None;
    let mut auto_follow = true;
    let move_speed = 180.0f32;
    let mut last_step = Instant::now();
    let world_packets = [
        "S_LOGIN", "S_SPAWN_ME", "S_LOAD_TOPO", "S_PLAYER_STAT_UPDATE", "S_SPAWN_NPC",
        "S_SPAWN_USER", "S_NPC_LOCATION", "S_USER_LOCATION", "S_DESPAWN_NPC",
        "S_DESPAWN_USER", "S_CREATURE_LIFE", "S_ITEMLIST", "C_PLAYER_LOCATION",
    ];

    let chat_rx = if cli.chat {
        stream.set_read_timeout(Some(Duration::from_millis(200)))?;
        let (tx, rx) = mpsc::channel::<String>();
        thread::spawn(move || {
            let stdin = std::io::stdin();
            loop {
                let mut line = String::new();
                if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let text = line.trim_end().to_string();
                if !text.is_empty() && tx.send(text).is_err() {
                    break;
                }
            }
        });
        println!("=== chat interactif : tape un message + Entree (ou !help pour les commandes serveur) ===");
        Some(rx)
    } else {
        None
    };

    let keep_alive = cli.listen == 0 || cli.chat;
    let listen_secs = cli.listen.min(86_400);
    let deadline = Instant::now() + Duration::from_secs(listen_secs.max(1));
    if !cli.chat {
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    }
    let inactivity_limit = Duration::from_secs(60);
    let mut last_recv = Instant::now();
    'session: while keep_alive || Instant::now() < deadline {
        {
            let now = Instant::now();
            let mut index = 0;
            while index < pending_sends.len() {
                if pending_sends[index].0 <= now {
                    let (_, opcode_name, packet_object) = pending_sends.remove(index);
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, opcode_name, &packet_object, t0)? {
                        break 'session;
                    }
                } else {
                    index += 1;
                }
            }
        }
        if last_heal_tick.elapsed() >= heal_tick_interval {
            last_heal_tick = Instant::now();
            heal.set_self(bot_pos, player_w);
                            heal.set_moving(move_target.is_some() || follow_id.is_some());
            let now = Instant::now();
            for (delay_ms, opcode_name, packet_object) in heal.tick(now) {
                pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
            }
        }
        heal.set_self(bot_pos, player_w);
        if auto_follow && follow_id.is_none() {
            follow_id = heal.first_target();
        }
        let move_now = Instant::now();
        if let Some(dodge) = heal.take_dodge() {
            move_target = Some(dodge);
            println!("[{:7.3}] ESQUIVE -> pas de cote", t0.elapsed().as_secs_f64());
        } else if heal.needs_heal(move_now) {
            move_target = None;
        } else if let Some(id) = follow_id {
            if let Some(loc) = heal.ally_loc(id) {
                move_target = Some(loc);
            }
        }
        if let (Some(target), Some(pos)) = (move_target, bot_pos) {
            if last_step.elapsed() >= Duration::from_millis(200) {
                let dt = last_step.elapsed().as_secs_f32();
                last_step = Instant::now();
                let dx = target[0] - pos[0];
                let dy = target[1] - pos[1];
                let distance = (dx * dx + dy * dy).sqrt();
                let arrive = if follow_id.is_some() { 200.0 } else { 60.0 };
                let stamp = (t0.elapsed().as_millis() as u64) & 0xffff_ffff;
                if distance <= arrive {
                    if follow_id.is_none() {
                        let stopped = [target[0], target[1], pos[2]];
                        bot_pos = Some(stopped);
                        player_loc = Some(tera_protocol::Value::Vec3(stopped));
                        let stop = tera_protocol::Object::new()
                            .with("loc", tera_protocol::Value::Vec3(stopped))
                            .with("w", tera_protocol::Value::Int(player_w))
                            .with("lookDirection", tera_protocol::Value::Int(0))
                            .with("dest", tera_protocol::Value::Vec3(stopped))
                            .with("type", tera_protocol::Value::Int(7))
                            .with("jumpDistance", tera_protocol::Value::Int(0))
                            .with("inShuttle", tera_protocol::Value::Bool(false))
                            .with("time", tera_protocol::Value::Uint(stamp));
                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_PLAYER_LOCATION", &stop, t0)? {
                            break 'session;
                        }
                        move_target = None;
                        last_keepalive = Instant::now();
                    }
                } else {
                    let step = (move_speed * dt).min(distance);
                    let next = [pos[0] + dx / distance * step, pos[1] + dy / distance * step, pos[2]];
                    bot_pos = Some(next);
                    player_loc = Some(tera_protocol::Value::Vec3(next));
                    player_w = ((dy.atan2(dx) / (2.0 * std::f32::consts::PI)) * 65536.0) as i32 as i16 as i64;
                    let moving = tera_protocol::Object::new()
                        .with("loc", tera_protocol::Value::Vec3(next))
                        .with("w", tera_protocol::Value::Int(player_w))
                        .with("lookDirection", tera_protocol::Value::Int(0))
                        .with("dest", tera_protocol::Value::Vec3(target))
                        .with("type", tera_protocol::Value::Int(0))
                        .with("jumpDistance", tera_protocol::Value::Int(0))
                        .with("inShuttle", tera_protocol::Value::Bool(false))
                        .with("time", tera_protocol::Value::Uint(stamp));
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_PLAYER_LOCATION", &moving, t0)? {
                        break 'session;
                    }
                    last_keepalive = Instant::now();
                }
            }
        }
        let read = match stream.read(&mut buf) {
            Ok(0) => { println!("[{:7.3}] serveur a ferme", t0.elapsed().as_secs_f64()); break }
            Ok(n) => { last_recv = Instant::now(); n }
            Err(e)
                if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
            {
                if let Some(rx) = &chat_rx {
                    while let Ok(text) = rx.try_recv() {
                        let message = tera_protocol::Object::new()
                            .with("channel", tera_protocol::Value::Uint(cli.chat_channel as u64))
                            .with("message", tera_protocol::Value::Str(text));
                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHAT", &message, t0)? {
                            break 'session;
                        }
                    }
                }
                if last_recv.elapsed() > inactivity_limit {
                    println!("[{:7.3}] serveur muet {}s, on coupe", t0.elapsed().as_secs_f64(), inactivity_limit.as_secs());
                    break 'session;
                }
                if in_world && last_keepalive.elapsed() > keepalive_interval {
                    if let Some(location) = &player_loc {
                        last_keepalive = Instant::now();
                        let stamp = (t0.elapsed().as_millis() as u64) & 0xffff_ffff;
                        let packet = tera_protocol::Object::new()
                            .with("loc", location.clone())
                            .with("w", tera_protocol::Value::Int(player_w))
                            .with("lookDirection", tera_protocol::Value::Int(player_w))
                            .with("dest", location.clone())
                            .with("type", tera_protocol::Value::Int(7))
                            .with("jumpDistance", tera_protocol::Value::Int(0))
                            .with("inShuttle", tera_protocol::Value::Bool(false))
                            .with("time", tera_protocol::Value::Uint(stamp));
                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_PLAYER_LOCATION", &packet, t0)? {
                            break 'session;
                        }
                        println!("[{:7.3}] keep-alive (C_PLAYER_LOCATION)", t0.elapsed().as_secs_f64());
                    }
                }
                continue;
            }
            Err(e) => { println!("[{:7.3}] lecture: {e}", t0.elapsed().as_secs_f64()); break }
        };
        dec.apply(&mut buf[..read]);
        packets.push(&buf[..read]);
        while let Some(packet) = packets.take_packet() {
            let name = opcodes.name(packet.opcode).unwrap_or("INCONNU").to_string();
            show("<-", t0, &name, packet.opcode, &packet.body, &registry, &packet.encode());
            let want_world = world_packets.contains(&name.as_str());
            let want_heal = heal_packets.contains(&name.as_str());
            if want_world || want_heal {
                if let Some(definition) = registry.get(&name) {
                    if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                        if want_world {
                            world.apply(&name, &object);
                            if name == "S_SPAWN_USER" {
                                if let Some(user) = object.get("name").and_then(tera_protocol::Value::as_str) {
                                    let game_id = object.get("gameId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                    user_ids.insert(user.to_string(), game_id);
                                }
                            }
                        }
                        if want_heal {
                            heal.set_self(bot_pos, player_w);
                            heal.set_moving(move_target.is_some() || follow_id.is_some());
                            let now = Instant::now();
                            for (delay_ms, opcode_name, packet_object) in heal.observe(&name, &object, now) {
                                pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                            }
                        }
                        if name == "S_PARTY_MEMBER_LIST" && !cli.commander.is_empty() {
                            if let Some(tera_protocol::Value::Array(members)) = object.get("members") {
                                for member in members {
                                    let member_name = member.get("name").and_then(tera_protocol::Value::as_str).unwrap_or("");
                                    if cli.commander.eq_ignore_ascii_case(member_name) {
                                        let server_id = member.get("serverId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                        let player_id = member.get("playerId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                        commander_party = Some((server_id, player_id));
                                    }
                                }
                            }
                        }
                        if name == "S_DIALOG" {
                            if let Some(tera_protocol::Value::Array(buttons)) = object.get("buttons") {
                                println!("[{:7.3}] DIALOGUE ({} boutons) :", t0.elapsed().as_secs_f64(), buttons.len());
                                for (i, button) in buttons.iter().enumerate() {
                                    let text = button.get("text").and_then(tera_protocol::Value::as_str).unwrap_or("");
                                    println!("           {} -> {text:?}", i + 1);
                                }
                            }
                        }
                    }
                }
            }
            if cli.chat {
                match name.as_str() {
                    "S_CHAT" | "S_WHISPER" | "S_PRIVATE_CHAT"
                    | "S_DUNGEON_EVENT_MESSAGE" | "S_SYSTEM_MESSAGE" => {
                        print_chat(&registry, &name, &packet)
                    }
                    _ => {}
                }
            }
            match name.as_str() {
                "S_PING" => {
                    let empty = tera_protocol::Object::new();
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_PONG", &empty, t0)? {
                        break 'session;
                    }
                }
                "S_LOGIN_ARBITER" | "S_LOGIN_ACCOUNT_INFO" if !asked_list => {
                    asked_list = true;
                    let range = tera_protocol::Object::new()
                        .with("range", tera_protocol::Value::Uint(2000));
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_SET_VISIBLE_RANGE", &range, t0)? {
                        break 'session;
                    }
                    let empty = tera_protocol::Object::new();
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_GET_USER_LIST", &empty, t0)? {
                        break 'session;
                    }
                }
                "S_GET_USER_LIST" if !sent_hw => {
                    sent_hw = true;
                    let u = |v: u64| tera_protocol::Value::Uint(v);
                    let hw = tera_protocol::Object::new()
                        .with("systemMemory", u(16383)).with("videoMemory", u(0))
                        .with("resWidth", u(1920)).with("resHeight", u(1080))
                        .with("isFullScreen", tera_protocol::Value::Bool(false))
                        .with("resScreenWidth", u(1920)).with("resScreenHeight", u(1080))
                        .with("numDisplays", u(1))
                        .with("resVirtualWidth", u(1920)).with("resVirtualHeight", u(1080))
                        .with("physicalCores", u(10)).with("logicalCores", u(10))
                        .with("os", tera_protocol::Value::Str("Windows 10".into()))
                        .with("cpu", tera_protocol::Value::Str("VirtualApple @ 2.50GHz".into()))
                        .with("gpu", tera_protocol::Value::Str(String::new()));
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_HARDWARE_INFO", &hw, t0)? {
                        break 'session;
                    }

                    if !selected {
                        if let Some(chosen) = pick_character(&registry, &packet, cli.character.as_deref()) {
                            selected = true;
                            bot_name = chosen.1.clone();
                            println!("[{:7.3}] selection du perso \"{}\" (id {})", t0.elapsed().as_secs_f64(), chosen.1, chosen.0);
                            let select = tera_protocol::Object::new()
                                .with("id", tera_protocol::Value::Int(chosen.0 as i64))
                                .with("unk", tera_protocol::Value::Uint(0));
                            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_SELECT_USER", &select, t0)? {
                                break 'session;
                            }
                        } else {
                            println!("[{:7.3}] aucun perso a selectionner", t0.elapsed().as_secs_f64());
                        }
                    }
                }
                "S_LOAD_TOPO" => {
                    let empty = tera_protocol::Object::new();
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_LOAD_TOPO_FIN", &empty, t0)? {
                        break 'session;
                    }
                    println!("[{:7.3}] >>> ENTRE DANS LE MONDE <<<", t0.elapsed().as_secs_f64());
                }
                "S_SPAWN_ME" => {
                    if let Some(definition) = registry.get("S_SPAWN_ME") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            player_loc = object.get("loc").cloned();
                            if let Some(tera_protocol::Value::Vec3(a)) = &player_loc {
                                bot_pos = Some(*a);
                            }
                            player_w = object.get("w").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                            in_world = true;
                            last_keepalive = Instant::now();
                            let leave = tera_protocol::Object::new();
                            let _ = reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_LEAVE_PARTY", &leave, t0);
                            println!(
                                "[{:7.3}] ETAT nom={} gameId={} zone={} vie={}/{} -> C_LEAVE_PARTY defensif",
                                t0.elapsed().as_secs_f64(), bot_name, world.player.game_id,
                                world.player.zone, world.player.hp, world.player.max_hp
                            );
                        }
                    }
                }
                "S_BEGIN_THROUGH_ARBITER_CONTRACT" => {
                    if let Some(definition) = registry.get("S_BEGIN_THROUGH_ARBITER_CONTRACT") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            let from = object.get("sender").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                            let contract_type = object.get("type").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let contract_id = object.get("id").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let me_name = object
                                .get("recipient")
                                .and_then(tera_protocol::Value::as_str)
                                .map(str::to_string)
                                .filter(|name| !name.is_empty())
                                .unwrap_or_else(|| bot_name.clone());
                            let trusted = cli.commander.is_empty() || cli.commander.eq_ignore_ascii_case(&from);
                            if trusted {
                                let reply = tera_protocol::Object::new()
                                    .with("type", tera_protocol::Value::Uint(contract_type))
                                    .with("id", tera_protocol::Value::Uint(contract_id))
                                    .with("response", tera_protocol::Value::Uint(reply_response))
                                    .with("recipient", tera_protocol::Value::Str(me_name));
                                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_REPLY_THROUGH_ARBITER_CONTRACT", &reply, t0)? {
                                    break 'session;
                                }
                                println!("[{:7.3}] contrat type {contract_type} de {from} -> accepte auto", t0.elapsed().as_secs_f64());
                            }
                        }
                    }
                }
                "S_REQUEST_CONTRACT" => {
                    if let Some(definition) = registry.get("S_REQUEST_CONTRACT") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            let contract_type = object.get("type").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let contract_id = object.get("id").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let sender_name = object.get("senderName").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                            println!(
                                "[{:7.3}] S_REQUEST_CONTRACT type={contract_type} id={contract_id} sender={sender_name:?} -> C_ACCEPT_CONTRACT",
                                t0.elapsed().as_secs_f64()
                            );
                            let accept = tera_protocol::Object::new()
                                .with("type", tera_protocol::Value::Int(contract_type as i64))
                                .with("id", tera_protocol::Value::Int(contract_id as i64));
                            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_ACCEPT_CONTRACT", &accept, t0)? {
                                break 'session;
                            }
                        }
                    }
                }
                "S_WHISPER" => {
                    if let Some(definition) = registry.get("S_WHISPER") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            let sender = object.get("name").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                            let server = object.get("senderServerId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let text = strip_markup(object.get("message").and_then(tera_protocol::Value::as_str).unwrap_or(""));
                            let authorized = cli.commander.is_empty() || cli.commander.eq_ignore_ascii_case(&sender);
                            if authorized && !sender.is_empty() {
                                println!("[{:7.3}] << {sender}: {text}", t0.elapsed().as_secs_f64());
                                let command = text.split_whitespace().next().unwrap_or("").to_lowercase();
                                let argument = text.splitn(2, char::is_whitespace).nth(1).unwrap_or("").trim().to_string();
                                let whisper = |body: String| -> tera_protocol::Object {
                                    tera_protocol::Object::new()
                                        .with("targetServer", tera_protocol::Value::Uint(server))
                                        .with("target", tera_protocol::Value::Str(sender.clone()))
                                        .with("message", tera_protocol::Value::Str(body))
                                };
                                let moved: Option<tera_protocol::Object> = match command.as_str() {
                                    "come" | "viens" => {
                                        let who = if argument.is_empty() { cli.commander.clone() } else { argument.clone() };
                                        let position = user_ids.get(&who).copied().and_then(|id| heal.ally_loc(id))
                                            .or_else(|| heal.first_target().and_then(|id| heal.ally_loc(id)));
                                        match position {
                                            Some(position) => { move_target = Some(position); follow_id = None; auto_follow = false; Some(whisper(format!("j'arrive vers {who}"))) }
                                            None => Some(whisper(format!("je ne vois pas {who}"))),
                                        }
                                    }
                                    "follow" | "suis" => {
                                        let who = if argument.is_empty() { cli.commander.clone() } else { argument.clone() };
                                        match user_ids.get(&who).copied().or_else(|| heal.first_target()) {
                                            Some(id) => { follow_id = Some(id); auto_follow = true; Some(whisper(format!("je te suis {who}"))) }
                                            None => Some(whisper(format!("je ne vois pas {who}"))),
                                        }
                                    }
                                    "goto" => {
                                        let coords: Vec<f32> = argument.split_whitespace().filter_map(|part| part.parse().ok()).collect();
                                        if coords.len() >= 2 {
                                            let z = coords.get(2).copied().or_else(|| bot_pos.map(|p| p[2])).unwrap_or(0.0);
                                            move_target = Some([coords[0], coords[1], z]);
                                            follow_id = None;
                                            auto_follow = false;
                                            Some(whisper("j'y vais".to_string()))
                                        } else {
                                            Some(whisper("usage: goto x y".to_string()))
                                        }
                                    }
                                    "stop" => { move_target = None; follow_id = None; auto_follow = false; Some(whisper("stop (suivi auto off, 'follow' pour reprendre)".to_string())) }
                                    "invite" | "inviter" | "grp" | "groupe" | "party" => {
                                        let who = if argument.is_empty() { cli.commander.clone() } else { argument.clone() };
                                        let request = tera_protocol::Object::new()
                                            .with("type", tera_protocol::Value::Int(invite_type))
                                            .with("unk2", tera_protocol::Value::Int(invite_unk2))
                                            .with("name", tera_protocol::Value::Str(who.clone()))
                                            .with("data", tera_protocol::Value::Bytes(vec![0u8]));
                                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_REQUEST_CONTRACT", &request, t0)? {
                                            break 'session;
                                        }
                                        Some(whisper(format!("invite {who} (type {invite_type}), accepte")))
                                    }
                                    "testheal" | "castheal" | "testh" => {
                                        let target = user_ids
                                            .get(&sender)
                                            .copied()
                                            .or_else(|| heal.first_target())
                                            .unwrap_or(0);
                                        if target == 0 {
                                            Some(whisper("je ne te vois pas encore (bouge un peu pour spawn)".to_string()))
                                        } else {
                                            heal.set_self(bot_pos, player_w);
                            heal.set_moving(move_target.is_some() || follow_id.is_some());
                                            let now = Instant::now();
                                            for (delay_ms, opcode_name, packet_object) in heal.force_heal(target) {
                                                pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                                            }
                                            Some(whisper(format!("test heal lance sur toi (target {target})")))
                                        }
                                    }
                                    "enter" | "entrer" | "donjon" => {
                                        let choice = argument.trim().parse::<usize>().ok();
                                        match heal.enter_dungeon(choice) {
                                            Some((_, opcode_name, packet_object)) => {
                                                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, opcode_name, &packet_object, t0)? {
                                                    break 'session;
                                                }
                                                Some(whisper(match choice {
                                                    Some(n) => format!("je contacte le portail, bouton {n}"),
                                                    None => "je contacte le portail (dis 'enter N' pour choisir dans le menu)".to_string(),
                                                }))
                                            }
                                            None => Some(whisper("je ne vois pas de portail (Teleportal) ; rapproche-toi du telporteur du donjon".to_string())),
                                        }
                                    }
                                    "lead" | "leader" | "chef" => {
                                        match commander_party {
                                            Some((server_id, player_id)) => {
                                                let manager = tera_protocol::Object::new()
                                                    .with("serverId", tera_protocol::Value::Uint(server_id))
                                                    .with("playerId", tera_protocol::Value::Uint(player_id));
                                                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHANGE_PARTY_MANAGER", &manager, t0)? {
                                                    break 'session;
                                                }
                                                Some(whisper("je te passe le lead du groupe".to_string()))
                                            }
                                            None => Some(whisper("je ne te trouve pas dans le groupe (invite-toi d'abord)".to_string())),
                                        }
                                    }
                                    _ => None,
                                };
                                let reply_packet: Option<(&str, tera_protocol::Object)> = if let Some(confirm) = moved {
                                    Some(("C_WHISPER", confirm))
                                } else { match command.as_str() {
                                    "say" => Some(("C_CHAT", tera_protocol::Object::new()
                                        .with("channel", tera_protocol::Value::Uint(u64::from(cli.chat_channel)))
                                        .with("message", tera_protocol::Value::Str(argument)))),
                                    "w" | "tell" | "repete" => Some(("C_WHISPER", whisper(argument))),
                                    "where" | "pos" => {
                                        let body = match &player_loc {
                                            Some(tera_protocol::Value::Vec3(a)) => format!("je suis a ({:.0}, {:.0}, {:.0})", a[0], a[1], a[2]),
                                            _ => "pas encore en jeu".to_string(),
                                        };
                                        Some(("C_WHISPER", whisper(body)))
                                    }
                                    "ping" => Some(("C_WHISPER", whisper("pong".to_string()))),
                                    "who" | "qui" => Some(("C_WHISPER", whisper(format!("{} joueurs autour", world.user_count())))),
                                    "mobs" | "mob" => {
                                        let body = match world.nearest_npc() {
                                            Some(npc) => format!("{} npc, plus proche template {} a {:.0} u", world.npc_count(), npc.template_id, npc.location.distance_to(world.player.location)),
                                            None => format!("{} npc", world.npc_count()),
                                        };
                                        Some(("C_WHISPER", whisper(body)))
                                    }
                                    "hp" | "vie" => Some(("C_WHISPER", whisper(format!("vie {}/{} mana {}/{}", world.player.hp, world.player.max_hp, world.player.mp, world.player.max_mp)))),
                                    "zone" => Some(("C_WHISPER", whisper(format!("zone {}", world.player.zone)))),
                                    "leave" | "leaveparty" | "quitgrp" | "quitgroupe" => {
                                        let leave = tera_protocol::Object::new();
                                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_LEAVE_PARTY", &leave, t0)? {
                                            break 'session;
                                        }
                                        Some(("C_WHISPER", whisper("j'ai quitte tout groupe".to_string())))
                                    }
                                    "etat" | "state" | "status" => Some(("C_WHISPER", whisper(format!(
                                        "zone {} vie {}/{} pos {:?}", world.player.zone, world.player.hp, world.player.max_hp, bot_pos
                                    )))),
                                    "rtype" => {
                                        if let Ok(value) = argument.trim().parse::<u64>() {
                                            reply_response = value;
                                            Some(("C_WHISPER", whisper(format!("response d'accept = {reply_response}"))))
                                        } else {
                                            Some(("C_WHISPER", whisper(format!("response actuel = {reply_response} (usage: rtype N)"))))
                                        }
                                    }
                                    "itype" => {
                                        if let Ok(value) = argument.trim().parse::<i64>() {
                                            invite_type = value;
                                            Some(("C_WHISPER", whisper(format!("type d'invite = {invite_type} (unk2={invite_unk2})"))))
                                        } else {
                                            Some(("C_WHISPER", whisper(format!("type actuel = {invite_type} (usage: itype N)"))))
                                        }
                                    }
                                    "iunk2" => {
                                        if let Ok(value) = argument.trim().parse::<i64>() {
                                            invite_unk2 = value;
                                            Some(("C_WHISPER", whisper(format!("unk2 d'invite = {invite_unk2} (type={invite_type})"))))
                                        } else {
                                            Some(("C_WHISPER", whisper(format!("unk2 actuel = {invite_unk2} (usage: iunk2 N)"))))
                                        }
                                    }
                                    "heal" | "soin" => {
                                        heal.cfg.enabled = !heal.cfg.enabled;
                                        Some(("C_WHISPER", whisper(heal.status())))
                                    }
                                    "dodge" | "esquive" => {
                                        heal.dodge_enabled = !heal.dodge_enabled;
                                        Some(("C_WHISPER", whisper(heal.status())))
                                    }
                                    "healinfo" => Some(("C_WHISPER", whisper(heal.status()))),
                                    "healset" => {
                                        let ids: Vec<u32> = argument.split_whitespace().filter_map(|part| part.parse().ok()).collect();
                                        if ids.len() >= 2 {
                                            heal.cfg.focus_heal = ids[0];
                                            heal.cfg.healing_immersion = ids[1];
                                            Some(("C_WHISPER", whisper(heal.status())))
                                        } else {
                                            Some(("C_WHISPER", whisper("usage: healset <focus_id> <immersion_id>".to_string())))
                                        }
                                    }
                                    "help" | "aide" => Some(("C_WHISPER", whisper("invite lead enter come follow goto stop heal dodge testheal healset healinfo etat leave say w where ping who mobs hp zone bye".to_string()))),
                                    "bye" | "logout" | "quit" => {
                                        let _ = reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_WHISPER", &whisper("a plus !".to_string()), t0);
                                        println!("[{:7.3}] logout demande par {sender}", t0.elapsed().as_secs_f64());
                                        break 'session;
                                    }
                                    _ => None,
                                } };
                                if let Some((name, object)) = reply_packet {
                                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, name, &object, t0)? {
                                        break 'session;
                                    }
                                }
                            } else if !cli.commander.is_empty() && !sender.is_empty() && !sender.eq_ignore_ascii_case(&bot_name) {
                                let notify = tera_protocol::Object::new()
                                    .with("targetServer", tera_protocol::Value::Uint(server))
                                    .with("target", tera_protocol::Value::Str(cli.commander.clone()))
                                    .with("message", tera_protocol::Value::Str(format!("[DM de {sender}] {text}")));
                                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_WHISPER", &notify, t0)? {
                                    break 'session;
                                }
                                println!("[{:7.3}] DM de {sender} relaye a {}", t0.elapsed().as_secs_f64(), cli.commander);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    println!("[{:7.3}] fin", t0.elapsed().as_secs_f64());
    Ok(())
}
