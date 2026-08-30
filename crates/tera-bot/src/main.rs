mod auth;

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
    #[arg(long, default_value = "127.0.0.1:10001")]
    server: String,
    #[arg(long, help = "insere un tera-proxy (inspection/mods) entre le bot et --server")]
    proxy: bool,
    #[arg(long, default_value = "41946")]
    account: String,
    #[arg(long, default_value = "0123456789abcdef0123456789abcdef")]
    ticket: String,
    #[arg(long, help = "lit le ticket BRUT depuis un fichier .bin (capture par le shim)")]
    ticket_file: Option<PathBuf>,
    #[arg(long, help = "connexion navigateur (OAuth) pour obtenir/sauver un refresh_token")]
    login: bool,
    #[arg(long, help = "fichier d'auth (refresh_token + ticket) ; refresh auto avant connexion")]
    auth_file: Option<PathBuf>,
    #[arg(long, default_value = "data/opcodes/protocol.376012.map")]
    opcodes: PathBuf,
    #[arg(long, default_values = ["data/definitions"])]
    definitions: Vec<PathBuf>,
    #[arg(long, default_value_t = 1337420, help = "valeur du CHAMP C_LOGIN_ARBITER (build client)")]
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let auth_path = cli
        .auth_file
        .clone()
        .unwrap_or_else(|| PathBuf::from("tera-bot-auth.json"));

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

    let (account, ticket_bytes): (String, Vec<u8>) = if cli.auth_file.is_some() {
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
    let registry = Registry::load(&cli.definitions, Some(cli.major_patch))?;
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
    if cli.probe {
        return probe(&cli.server);
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
        let (child, addr) = spawn_proxy(&cli.server, &cli.opcodes, &cli.definitions, cli.major_patch)?;
        println!("proxy insere : {addr} -> {}", cli.server);
        proxy_guard = ProxyGuard(Some(child));
        addr
    } else {
        cli.server.clone()
    };
    let _ = &proxy_guard;
    println!("connexion a {target}");
    let addr = target
        .to_socket_addrs()
        .context("resolution de l'adresse")?
        .next()
        .with_context(|| format!("aucune adresse pour {target}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).context("connect")?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(25)))?;
    println!("[{:7.3}] connecte", t0.elapsed().as_secs_f64());

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
                continue;
            }
            Err(e) => { println!("[{:7.3}] lecture: {e}", t0.elapsed().as_secs_f64()); break }
        };
        dec.apply(&mut buf[..read]);
        packets.push(&buf[..read]);
        while let Some(packet) = packets.take_packet() {
            let name = opcodes.name(packet.opcode).unwrap_or("INCONNU").to_string();
            show("<-", t0, &name, packet.opcode, &packet.body, &registry, &packet.encode());
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
                _ => {}
            }
        }
    }
    println!("[{:7.3}] fin", t0.elapsed().as_secs_f64());
    Ok(())
}
