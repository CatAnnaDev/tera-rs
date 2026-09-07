mod hooks;
mod loader;
mod plugins;

use anyhow::{bail, Context, Result};
use clap::Parser;
use hooks::{dispatch, Codec, Direction, Engine, Handler, Outcome, Stats};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufWriter, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tera_protocol::handshake::{random_key, ClientHandshake, ServerHandshake, Step, MAGIC};
use tera_protocol::session::{Constants, Decrypting, Encrypting, KEY_LEN, LEGACY, MODERN};
use tera_protocol::{value, Object, OpcodeMap, PacketBuffer, Registry};

#[derive(Parser, Clone)]
#[command(name = "tera-proxy", about = "MITM between the client and a server: decrypts, inspects, hooks")]
struct Cli {
    #[arg(long, default_values = ["0.0.0.0:9250", "0.0.0.0:9251"], help = "where the client connects (repeatable; 0.0.0.0 = toutes les IP locales)")]
    listen: Vec<String>,
    #[arg(long, help = "the real server, host:port")]
    upstream: String,
    #[arg(long, default_value = "data/opcodes/protocol.376012.map")]
    opcodes: PathBuf,
    #[arg(long, default_values = ["data/definitions"])]
    definitions: Vec<PathBuf>,
    #[arg(long, default_value_t = 100)]
    patch_version: u32,
    #[arg(long, default_value = "captures/capture.jsonl", help = "everything, one JSON object per line")]
    dump: PathBuf,
    #[arg(long, help = "pre-45 key constants")]
    legacy: bool,
    #[arg(long, help = "only show these opcodes (by name)")]
    only: Vec<String>,
    #[arg(long, help = "hide these opcodes (by name)")]
    hide: Vec<String>,
    #[arg(long, help = "show decoded fields on the console")]
    show_fields: bool,
    #[arg(long, help = "show the body hex on the console")]
    show_hex: bool,
    #[arg(long, help = "decode fields into the JSONL dump (slower)")]
    dump_fields: bool,
    #[arg(long, help = "print traffic counters at session end")]
    stats: bool,
    #[arg(long, help = "exit after serving a single connection")]
    once: bool,
    #[arg(long, help = "open the live packet inspector window (needs the gui feature)")]
    gui: bool,
    #[arg(long, default_value = "mods", help = "directory of dynamic mod libraries")]
    mods_dir: PathBuf,
    #[arg(long, help = "do not load any dynamic mods")]
    no_mods: bool,
    #[arg(long, help = "disable a mod by name (repeatable)")]
    disable_mod: Vec<String>,
    #[arg(long, help = "sortie de l'upstream via un proxy SOCKS5 (ex: 127.0.0.1:1081) pour passer par la Free")]
    socks5: Option<String>,
    #[arg(long, help = "orchestre tout: recupere le ticket et lance le jeu (tera-launcher.exe) dans le bottle")]
    launch: bool,
    #[arg(long, default_value = "target/release/tera-bot", help = "binaire tera-bot utilise pour recuperer le ticket")]
    tera_bot: PathBuf,
    #[arg(long, help = "fichier d'auth passe a tera-bot --print-ticket (refresh_token + ticket)")]
    auth_file: Option<PathBuf>,
    #[arg(long, default_value = "target/x86_64-pc-windows-gnu/release/tera-launcher.exe", help = "tera-launcher.exe (cote windows) lance via wine")]
    launcher_exe: PathBuf,
    #[arg(long, default_value = "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine", help = "binaire wine de CrossOver")]
    wine: PathBuf,
    #[arg(long, default_value = "/Users/anna/Library/Application Support/CrossOver/Bottles/tera", help = "WINEPREFIX du bottle CrossOver")]
    bottle: PathBuf,
    #[arg(long, default_value = "C:\\Games\\TERA Europe Classic\\Binaries\\TERA.exe", help = "chemin Windows de TERA.exe passe au launcher")]
    game_win: String,
    #[arg(long, default_value = "127.0.0.20:9250", help = "adresse loopback (partagee avec le bottle) que le jeu doit joindre = un listener du proxy")]
    client_endpoint: String,
    #[arg(long, default_value = "https://launcher.tera-europe.net/classicplus/serverlist.json", help = "serverlist officielle reprise puis re-encodee (facon enterance) avec l'adresse pointee vers le proxy")]
    serverlist_url: String,
    #[arg(long, default_value = "dxmt", help = "backend graphique force au lancement: dxmt (D3D11->Metal, recommande), dxvk, ou off")]
    d3d: String,
}

const MAX_EVENTS: usize = 20_000;
static EVENT_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
struct PacketEvent {
    seq: u64,
    at: f64,
    from_client: bool,
    opcode: u16,
    name: String,
    len: usize,
    hex: String,
    fields: Option<String>,
}

type EventSink = Arc<Mutex<VecDeque<PacketEvent>>>;

struct Capture {
    file: Mutex<BufWriter<std::fs::File>>,
    codec: Arc<Codec>,
    started: std::time::Instant,
    only: HashSet<String>,
    hide: HashSet<String>,
    show_fields: bool,
    show_hex: bool,
    dump_fields: bool,
    show_stats: bool,
    stats: Mutex<Stats>,
    warned: Mutex<HashSet<String>>,
    events: Option<EventSink>,
}

impl Capture {
    fn is_shown(&self, name: &str) -> bool {
        (self.only.is_empty() || self.only.contains(name)) && !self.hide.contains(name)
    }

    fn needs_object(&self, name: &str) -> bool {
        self.dump_fields || (self.show_fields && self.is_shown(name))
    }

    fn note_drop(&self, name: &str) {
        if self.is_shown(name) {
            println!("{:8.3}  [drop] {name}", self.started.elapsed().as_secs_f64());
        }
    }

    fn warn_once(&self, name: &str, opcode: u16, object: Option<&Object>, attempted: bool) {
        let issue = if name.starts_with("UNKNOWN_") {
            Some(format!("unknown opcode {opcode}: not in the opcode map"))
        } else if self.codec.definition(name).is_none() {
            Some(format!("{name} ({opcode}): no definition loaded"))
        } else if attempted && object.is_none() {
            Some(format!("{name} ({opcode}): definition present but decode failed (wrong version?)"))
        } else {
            None
        };
        if let Some(message) = issue {
            if let Ok(mut warned) = self.warned.lock() {
                if warned.insert(message.clone()) {
                    eprintln!("[proxy] {message}");
                }
            }
        }
    }

    fn record(
        &self,
        direction: Direction,
        name: &str,
        opcode: u16,
        body: &[u8],
        object: Option<&Object>,
        attempted: bool,
    ) {
        let elapsed = self.started.elapsed().as_secs_f64();
        let shown = self.is_shown(name);
        let described = if self.dump_fields || (self.show_fields && shown) {
            object.map(describe)
        } else {
            None
        };

        if shown {
            let arrow = match direction {
                Direction::ClientToServer => "->",
                Direction::ServerToClient => "<-",
            };
            let mut line = format!("{elapsed:8.3} {arrow} {name} ({opcode}) {} b", body.len());
            if self.show_hex {
                line.push_str("\n           ");
                append_hex_spaced(&mut line, body);
            }
            if self.show_fields {
                if let Some(text) = &described {
                    line.push_str("\n           ");
                    line.push_str(text);
                }
            }
            println!("{line}");
        }

        if let Ok(mut stats) = self.stats.lock() {
            stats.record(name, body.len());
        }
        self.warn_once(name, opcode, object, attempted);

        let mut hex = String::with_capacity(body.len() * 2);
        append_hex(&mut hex, body);
        let fields = match &described {
            Some(text) => {
                let mut escaped = String::with_capacity(text.len() + 2);
                escaped.push('"');
                json_escape_into(&mut escaped, text);
                escaped.push('"');
                escaped
            }
            None => "null".to_string(),
        };
        let from = match direction {
            Direction::ClientToServer => "client",
            Direction::ServerToClient => "server",
        };
        let line = format!(
            "{{\"at\":{elapsed:.3},\"from\":\"{from}\",\"opcode\":{opcode},\"name\":\"{name}\",\"len\":{},\"hex\":\"{hex}\",\"fields\":{fields}}}",
            body.len()
        );
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "{line}");
        }

        if let Some(events) = &self.events {
            if let Ok(mut buffer) = events.lock() {
                while buffer.len() >= MAX_EVENTS {
                    buffer.pop_front();
                }
                buffer.push_back(PacketEvent {
                    seq: EVENT_SEQ.fetch_add(1, Ordering::Relaxed),
                    at: elapsed,
                    from_client: matches!(direction, Direction::ClientToServer),
                    opcode,
                    name: name.to_string(),
                    len: body.len(),
                    hex: hex.clone(),
                    fields: described.clone(),
                });
            }
        }
    }
}

fn describe(object: &Object) -> String {
    object
        .fields
        .iter()
        .take(16)
        .map(|(name, value)| format!("{name}={value:?}"))
        .collect::<Vec<_>>()
        .join(" ")
}

const NIBBLES: &[u8; 16] = b"0123456789abcdef";

fn append_hex(out: &mut String, body: &[u8]) {
    for byte in body {
        out.push(NIBBLES[(byte >> 4) as usize] as char);
        out.push(NIBBLES[(byte & 0x0f) as usize] as char);
    }
}

fn append_hex_spaced(out: &mut String, body: &[u8]) {
    for (index, byte) in body.iter().enumerate() {
        if index != 0 {
            out.push(' ');
        }
        out.push(NIBBLES[(byte >> 4) as usize] as char);
        out.push(NIBBLES[(byte & 0x0f) as usize] as char);
    }
}

fn json_escape_into(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", control as u32)),
            other => out.push(other),
        }
    }
}

fn read_exactly(stream: &mut TcpStream, count: usize) -> Result<Vec<u8>> {
    let mut buffer = vec![0u8; count];
    stream.read_exact(&mut buffer)?;
    Ok(buffer)
}

fn writer_for<'a>(
    direction: Direction,
    to_client: &'a SyncSender<Vec<u8>>,
    to_server: &'a SyncSender<Vec<u8>>,
) -> &'a SyncSender<Vec<u8>> {
    match direction {
        Direction::ServerToClient => to_client,
        Direction::ClientToServer => to_server,
    }
}

const MAX_BATCH: usize = 256 * 1024;

fn writer_thread(mut encrypt: Encrypting, sink: Arc<TcpStream>, rx: Receiver<Vec<u8>>) {
    let mut output: &TcpStream = &sink;
    while let Ok(first) = rx.recv() {
        let mut batch = first;
        while batch.len() < MAX_BATCH {
            match rx.try_recv() {
                Ok(more) => batch.extend_from_slice(&more),
                Err(_) => break,
            }
        }
        encrypt.apply(&mut batch);
        if output.write_all(&batch).is_err() {
            break;
        }
    }
    let _ = sink.shutdown(std::net::Shutdown::Both);
}

fn timer_thread(
    mut timers: Vec<hooks::Timer>,
    codec: Arc<Codec>,
    to_client: SyncSender<Vec<u8>>,
    to_server: SyncSender<Vec<u8>>,
    closing: Arc<AtomicBool>,
) {
    let mut due: Vec<Instant> = timers
        .iter()
        .map(|timer| Instant::now() + timer.interval)
        .collect();
    let mut active: Vec<bool> = vec![true; timers.len()];
    while !closing.load(Ordering::Relaxed) {
        let mut wait = Duration::from_millis(250);
        for index in 0..timers.len() {
            if !active[index] {
                continue;
            }
            if Instant::now() >= due[index] {
                let injections = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    timers[index].fire(&codec)
                })) {
                    Ok(injections) => injections,
                    Err(_) => {
                        eprintln!("[hooks] timer panicked, disabled");
                        active[index] = false;
                        Vec::new()
                    }
                };
                for injection in injections {
                    let sender = match injection.direction {
                        Direction::ServerToClient => &to_client,
                        Direction::ClientToServer => &to_server,
                    };
                    if !forward(sender, injection.frame) {
                        return;
                    }
                }
                if active[index] {
                    if timers[index].repeat {
                        due[index] = Instant::now() + timers[index].interval;
                    } else {
                        active[index] = false;
                    }
                }
            }
            if active[index] {
                let remaining = due[index].saturating_duration_since(Instant::now());
                if remaining < wait {
                    wait = remaining;
                }
            }
        }
        std::thread::sleep(wait);
    }
}

struct ReaderTeardown {
    client: Arc<TcpStream>,
    server: Arc<TcpStream>,
    overflow: Arc<AtomicBool>,
}

impl Drop for ReaderTeardown {
    fn drop(&mut self) {
        let mode = if self.overflow.load(Ordering::Relaxed) {
            std::net::Shutdown::Both
        } else {
            std::net::Shutdown::Read
        };
        let _ = self.client.shutdown(mode);
        let _ = self.server.shutdown(mode);
    }
}

fn flush_capture(capture: &Capture) {
    if let Ok(mut file) = capture.file.lock() {
        let _ = file.flush();
    }
}

fn forward(sender: &SyncSender<Vec<u8>>, frame: Vec<u8>) -> bool {
    match sender.try_send(frame) {
        Ok(()) => true,
        Err(TrySendError::Full(frame)) => sender.send(frame).is_ok(),
        Err(TrySendError::Disconnected(_)) => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn reader(
    source: Arc<TcpStream>,
    mut decrypt: Decrypting,
    direction: Direction,
    capture: Arc<Capture>,
    mut hooks: HashMap<u16, Vec<Handler>>,
    codec: Arc<Codec>,
    to_client: SyncSender<Vec<u8>>,
    to_server: SyncSender<Vec<u8>>,
    mut leftover: Vec<u8>,
    client_socket: Arc<TcpStream>,
    server_socket: Arc<TcpStream>,
) {
    let mut buffer = [0u8; 16384];
    let mut packets = PacketBuffer::new();
    let mut input: &TcpStream = &source;
    let overflow = Arc::new(AtomicBool::new(false));
    let _teardown = ReaderTeardown {
        client: Arc::clone(&client_socket),
        server: Arc::clone(&server_socket),
        overflow: Arc::clone(&overflow),
    };
    if !leftover.is_empty() {
        decrypt.apply(&mut leftover);
        packets.push(&leftover);
        if !drain(&mut packets, &mut hooks, direction, &capture, &codec, &to_client, &to_server) {
            overflow.store(true, Ordering::Relaxed);
        }
        flush_capture(&capture);
    }
    let idle_limit = Duration::from_secs(120);
    let mut last_activity = Instant::now();
    while !overflow.load(Ordering::Relaxed) {
        let read = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                last_activity = Instant::now();
                read
            }
            Err(error)
                if error.kind() == ErrorKind::WouldBlock
                    || error.kind() == ErrorKind::TimedOut =>
            {
                if last_activity.elapsed() > idle_limit {
                    break;
                }
                continue;
            }
            Err(_) => break,
        };
        decrypt.apply(&mut buffer[..read]);
        packets.push(&buffer[..read]);
        if !drain(&mut packets, &mut hooks, direction, &capture, &codec, &to_client, &to_server) {
            overflow.store(true, Ordering::Relaxed);
        }
        flush_capture(&capture);
    }
}

fn drain(
    packets: &mut PacketBuffer,
    hooks: &mut HashMap<u16, Vec<Handler>>,
    direction: Direction,
    capture: &Capture,
    codec: &Codec,
    to_client: &SyncSender<Vec<u8>>,
    to_server: &SyncSender<Vec<u8>>,
) -> bool {
    while let Some(packet) = packets.take_packet() {
        let name = match codec.name(packet.opcode) {
            Some(known) => Cow::Borrowed(known),
            None => Cow::Owned(format!("UNKNOWN_{}", packet.opcode)),
        };
        let has_handler = hooks.contains_key(&packet.opcode);
        let attempted = has_handler || capture.needs_object(&name);
        let object = if attempted {
            codec.decode(&name, &packet.encode())
        } else {
            None
        };
        capture.record(direction, &name, packet.opcode, &packet.body, object.as_ref(), attempted);

        if !has_handler {
            if !forward(writer_for(direction, to_client, to_server), packet.encode()) {
                return false;
            }
            continue;
        }
        let mut injections = Vec::new();
        match dispatch(hooks, packet.opcode, &name, object, &mut injections, codec) {
            Outcome::Pass => {
                if !forward(writer_for(direction, to_client, to_server), packet.encode()) {
                    return false;
                }
            }
            Outcome::Drop => capture.note_drop(&name),
            Outcome::Modify(modified) => {
                let frame = match codec.definition(&name) {
                    Some(definition) => value::write(definition, packet.opcode, &modified)
                        .unwrap_or_else(|_| packet.encode()),
                    None => packet.encode(),
                };
                if !forward(writer_for(direction, to_client, to_server), frame) {
                    return false;
                }
            }
        }
        for injection in injections {
            if !forward(
                writer_for(injection.direction, to_client, to_server),
                injection.frame,
            ) {
                return false;
            }
        }
    }
    true
}

fn serve(
    client: TcpStream,
    upstream: &str,
    socks5: Option<&str>,
    constants: Constants,
    capture: Arc<Capture>,
    mods: Arc<loader::LoadedMods>,
) -> Result<()> {
    let mut client = client;
    let t0 = Instant::now();
    let peer = client.peer_addr().map(|a| a.to_string()).unwrap_or_else(|_| "?".into());
    let log = |m: &str| println!("[{:7.3}s] [client {peer}] {m}", t0.elapsed().as_secs_f64());
    client.set_nodelay(true)?;
    client.set_read_timeout(Some(Duration::from_secs(30)))?;
    client.set_write_timeout(Some(Duration::from_secs(30)))?;
    log("connecte au proxy");
    let mut server = match socks5 {
        Some(proxy) => {
            log(&format!("-> upstream {upstream} via SOCKS5 {proxy} (sortie Free)..."));
            socks5_connect(proxy, upstream, Duration::from_secs(12))
                .with_context(|| format!("SOCKS5 {proxy} -> {upstream}"))?
        }
        None => {
            let address = upstream
                .to_socket_addrs()
                .with_context(|| format!("resolving {upstream}"))?
                .next()
                .with_context(|| format!("no address for {upstream}"))?;
            log(&format!("-> upstream {upstream} ({address}) : ouverture TCP..."));
            TcpStream::connect_timeout(&address, Duration::from_secs(10))
                .with_context(|| format!("connecting to {upstream}"))?
        }
    };
    server.set_nodelay(true)?;
    server.set_read_timeout(Some(Duration::from_secs(30)))?;
    server.set_write_timeout(Some(Duration::from_secs(30)))?;
    log("-> upstream : TCP etabli, attente du greeting serveur (le serveur parle en premier)...");

    let greeting = match read_exactly(&mut server, MAGIC.len()) {
        Ok(g) => g,
        Err(e) => {
            log(&format!(
                "<- upstream : AUCUN GREETING ({e}) -- TCP accepte mais le serveur ne repond pas (bloque / no-greeting cote ByteFilter ?)"
            ));
            return Err(e).context("greeting upstream");
        }
    };
    if greeting != MAGIC {
        log(&format!("<- upstream : greeting INATTENDU {greeting:02x?} (attendu {MAGIC:02x?})"));
        bail!("upstream greeting {greeting:?}, expected {MAGIC:?}");
    }
    log("<- upstream : greeting recu OK, echange des cles crypto...");
    let upward = ClientHandshake::new(random_key(), random_key()).with_constants(constants);
    server.write_all(upward.first())?;
    let server_first: [u8; KEY_LEN] = read_exactly(&mut server, KEY_LEN)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("short server key"))?;
    server.write_all(upward.second())?;
    let server_second: [u8; KEY_LEN] = read_exactly(&mut server, KEY_LEN)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("short server key"))?;
    let upstream_session = upward.finish(&server_first, &server_second);
    log("<- upstream : handshake crypto complet (session chiffree serveur OK)");

    let mut downward = ServerHandshake::new(random_key(), random_key()).with_constants(constants);
    client.write_all(&downward.greeting())?;
    let mut scratch = [0u8; 4096];
    let client_session = loop {
        let read = client.read(&mut scratch)?;
        if read == 0 {
            bail!("client closed during handshake");
        }
        match downward.feed(&scratch[..read]) {
            Step::Send(reply) => client.write_all(&reply)?,
            Step::Established(session) => {
                client.write_all(downward.server_second())?;
                break *session;
            }
            Step::Wait => {}
        }
    };
    let client_leftover = downward.leftover();
    log("-> client : handshake crypto complet");
    log("=== RELAIS ACTIF : client <-> serveur en clair, dump JSONL en cours ===");

    let mut plugins = plugins::builtin();
    plugins.extend(mods.instantiate());
    let (client_to_server, server_to_client, timers) =
        Engine::build(plugins, &capture.codec).split();
    let (client_encrypting, client_decrypting) = client_session.split();
    let (upstream_encrypting, upstream_decrypting) = upstream_session.split();

    let client = Arc::new(client);
    let server = Arc::new(server);
    let (to_client_tx, to_client_rx) = mpsc::sync_channel::<Vec<u8>>(8192);
    let (to_server_tx, to_server_rx) = mpsc::sync_channel::<Vec<u8>>(8192);
    let closing = Arc::new(AtomicBool::new(false));

    let client_writer = {
        let sink = Arc::clone(&client);
        std::thread::spawn(move || writer_thread(client_encrypting, sink, to_client_rx))
    };
    let server_writer = {
        let sink = Arc::clone(&server);
        std::thread::spawn(move || writer_thread(upstream_encrypting, sink, to_server_rx))
    };

    let timer = if timers.is_empty() {
        None
    } else {
        let codec = Arc::clone(&capture.codec);
        let to_client = to_client_tx.clone();
        let to_server = to_server_tx.clone();
        let closing = Arc::clone(&closing);
        Some(std::thread::spawn(move || {
            timer_thread(timers, codec, to_client, to_server, closing)
        }))
    };

    let upward = {
        let source = Arc::clone(&client);
        let capture = Arc::clone(&capture);
        let codec = Arc::clone(&capture.codec);
        let to_client = to_client_tx.clone();
        let to_server = to_server_tx.clone();
        let client_socket = Arc::clone(&client);
        let server_socket = Arc::clone(&server);
        std::thread::spawn(move || {
            reader(
                source,
                client_decrypting,
                Direction::ClientToServer,
                capture,
                client_to_server,
                codec,
                to_client,
                to_server,
                client_leftover,
                client_socket,
                server_socket,
            )
        })
    };

    reader(
        Arc::clone(&server),
        upstream_decrypting,
        Direction::ServerToClient,
        Arc::clone(&capture),
        server_to_client,
        Arc::clone(&capture.codec),
        to_client_tx,
        to_server_tx,
        Vec::new(),
        Arc::clone(&client),
        Arc::clone(&server),
    );

    closing.store(true, Ordering::Relaxed);
    let _ = upward.join();
    if let Some(timer) = timer {
        let _ = timer.join();
    }
    let _ = client_writer.join();
    let _ = server_writer.join();
    if let Ok(mut file) = capture.file.lock() {
        let _ = file.flush();
    }
    if capture.show_stats {
        if let Ok(stats) = capture.stats.lock() {
            stats.dump();
        }
    }
    Ok(())
}

fn build_capture(cli: &Cli, codec: Arc<Codec>, events: Option<EventSink>) -> Result<Arc<Capture>> {
    if let Some(parent) = cli.dump.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cli.dump)
        .with_context(|| format!("opening {}", cli.dump.display()))?;
    Ok(Arc::new(Capture {
        file: Mutex::new(BufWriter::new(file)),
        codec,
        started: std::time::Instant::now(),
        only: cli.only.iter().cloned().collect(),
        hide: cli.hide.iter().cloned().collect(),
        show_fields: cli.show_fields,
        show_hex: cli.show_hex,
        dump_fields: cli.dump_fields || events.is_some(),
        show_stats: cli.stats,
        stats: Mutex::new(Stats::default()),
        warned: Mutex::new(HashSet::new()),
        events,
    }))
}

fn build_mods(cli: &Cli) -> Arc<Mutex<Arc<loader::LoadedMods>>> {
    if cli.no_mods {
        Arc::new(Mutex::new(Arc::new(loader::LoadedMods::empty())))
    } else {
        Arc::new(Mutex::new(Arc::new(loader::LoadedMods::load(
            &cli.mods_dir,
            &cli.disable_mod,
        ))))
    }
}

fn spawn_mod_watcher(cli: &Cli, mods: &Arc<Mutex<Arc<loader::LoadedMods>>>) {
    if cli.no_mods {
        return;
    }
    let mods = Arc::clone(mods);
    let dir = cli.mods_dir.clone();
    let disabled = cli.disable_mod.clone();
    std::thread::spawn(move || {
        let mut last = loader::signature(&dir, &disabled);
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let current = loader::signature(&dir, &disabled);
            if current != last {
                last = current;
                println!("[mods] change detected, reloading");
                let fresh = Arc::new(loader::LoadedMods::load(&dir, &disabled));
                *mods.lock().unwrap_or_else(|poison| poison.into_inner()) = fresh;
            }
        }
    });
}

fn accept_loop(
    listen: &str,
    cli: &Cli,
    capture: Arc<Capture>,
    mods: Arc<Mutex<Arc<loader::LoadedMods>>>,
    constants: Constants,
) -> Result<()> {
    let listener = TcpListener::bind(listen)
        .with_context(|| format!("binding {listen}"))?;
    println!(
        "listening on {}, relaying to {}, dump {}",
        listen,
        cli.upstream,
        cli.dump.display()
    );
    for client in listener.incoming().flatten() {
        let peer = client
            .peer_addr()
            .map(|address| address.to_string())
            .unwrap_or_else(|_| "?".into());
        println!("client connected from {peer}");
        let upstream = cli.upstream.clone();
        let socks5 = cli.socks5.clone();
        let capture = Arc::clone(&capture);
        let mods = {
            let guard = mods.lock().unwrap_or_else(|poison| poison.into_inner());
            Arc::clone(&guard)
        };
        let handle = std::thread::spawn(move || {
            match serve(client, &upstream, socks5.as_deref(), constants, capture, mods) {
                Err(error) => println!("session ended: {error:#}"),
                Ok(()) => println!("session closed"),
            }
        });
        if cli.once {
            let _ = handle.join();
            break;
        }
    }
    Ok(())
}

fn socks5_connect(proxy: &str, target: &str, timeout: Duration) -> Result<TcpStream> {
    let paddr = proxy
        .to_socket_addrs()
        .with_context(|| format!("resolving socks5 {proxy}"))?
        .next()
        .with_context(|| format!("no address for {proxy}"))?;
    let mut s = TcpStream::connect_timeout(&paddr, timeout)?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    s.write_all(&[0x05, 0x01, 0x00])?;
    let mut hello = [0u8; 2];
    s.read_exact(&mut hello)?;
    if hello[0] != 0x05 || hello[1] != 0x00 {
        bail!("SOCKS5 no-auth refuse: {:02x} {:02x}", hello[0], hello[1]);
    }
    let (host, port) = target.rsplit_once(':').with_context(|| format!("cible sans port: {target}"))?;
    let port: u16 = port.parse().with_context(|| format!("port invalide: {port}"))?;
    let mut req = vec![0x05, 0x01, 0x00];
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(v4) => {
            req.push(0x01);
            req.extend_from_slice(&v4.octets());
        }
        Err(_) => {
            let hb = host.as_bytes();
            if hb.len() > 255 {
                bail!("hostname trop long");
            }
            req.push(0x03);
            req.push(hb.len() as u8);
            req.extend_from_slice(hb);
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req)?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head)?;
    if head[1] != 0x00 {
        bail!("SOCKS5 CONNECT echoue, code {:#04x}", head[1]);
    }
    let skip = match head[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l)?;
            l[0] as usize
        }
        other => bail!("ATYP inconnu {other:#04x}"),
    };
    let mut rest = vec![0u8; skip + 2];
    s.read_exact(&mut rest)?;
    s.set_read_timeout(None).ok();
    s.set_write_timeout(None).ok();
    Ok(s)
}

#[derive(serde::Deserialize)]
struct SlJson {
    #[serde(default)]
    sort_criterion: Option<u32>,
    servers: Vec<SlSrv>,
}

#[derive(serde::Deserialize)]
struct SlSrv {
    id: u32,
    name: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    queue: String,
    #[serde(default)]
    population: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    port: u32,
    #[serde(default)]
    available: u32,
    #[serde(default)]
    unavailable_message: String,
}

fn pb_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn pb_tag(out: &mut Vec<u8>, field: u32, wire: u8) {
    pb_varint(out, ((field as u64) << 3) | wire as u64);
}

fn pb_fixed32(out: &mut Vec<u8>, field: u32, value: u32) {
    pb_tag(out, field, 5);
    out.extend_from_slice(&value.to_le_bytes());
}

fn pb_bytes(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    pb_tag(out, field, 2);
    pb_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

const PROXY_ID_OFFSET: u32 = 900000;

#[allow(clippy::too_many_arguments)]
fn encode_server_info(
    out: &mut Vec<u8>,
    id: u32,
    name: &str,
    category: &str,
    title: &str,
    queue: &str,
    population: &str,
    addr: u32,
    port: u16,
    available: u32,
    unavailable_message: &str,
) {
    let mut m = Vec::new();
    pb_fixed32(&mut m, 1, id);
    pb_bytes(&mut m, 2, &utf16le(&format!("{name}(0)")));
    pb_bytes(&mut m, 3, &utf16le(category));
    pb_bytes(&mut m, 4, &utf16le(&format!("{title}(0)")));
    pb_bytes(&mut m, 5, &utf16le(queue));
    pb_bytes(&mut m, 6, &utf16le(population));
    pb_fixed32(&mut m, 7, addr);
    pb_fixed32(&mut m, 8, port as u32);
    pb_fixed32(&mut m, 9, available);
    pb_bytes(&mut m, 10, &utf16le(unavailable_message));
    pb_bytes(out, 1, &m);
}

fn build_enterance_serverlist(url: &str, host: std::net::Ipv4Addr, port: u16) -> Result<Vec<u8>> {
    let mut resp = ureq::get(url)
        .header("User-Agent", "tera-proxy")
        .call()
        .context("recuperation de la serverlist")?;
    let doc: SlJson = resp.body_mut().read_json().context("parsing serverlist")?;
    let proxy_addr = u32::from_be_bytes(host.octets());
    let mut out = Vec::new();
    for s in &doc.servers {
        let population = s
            .population
            .clone()
            .unwrap_or_else(|| "<b><font color=\"#FF0000\">Offline</font></b>".to_string());
        let real_addr = s
            .address
            .as_deref()
            .and_then(|a| a.parse::<std::net::Ipv4Addr>().ok())
            .map(|ip| u32::from_be_bytes(ip.octets()))
            .unwrap_or(0);

        encode_server_info(
            &mut out,
            s.id + PROXY_ID_OFFSET,
            &format!("{}(Meow)", s.name),
            &s.category,
            &format!("{}(Meow)", s.title),
            &s.queue,
            &population,
            proxy_addr,
            port,
            s.available,
            &s.unavailable_message,
        );
        encode_server_info(
            &mut out,
            s.id,
            &s.name,
            &s.category,
            &s.title,
            &s.queue,
            &population,
            real_addr,
            s.port as u16,
            s.available,
            &s.unavailable_message,
        );
        println!(
            "[launch] serverlist: {}(Meow) (id {}) -> {}:{} [proxy]  +  {} (id {}) -> {}:{} [direct]",
            s.name,
            s.id + PROXY_ID_OFFSET,
            host,
            port,
            s.name,
            s.id,
            s.address.as_deref().unwrap_or("?"),
            s.port
        );
    }
    pb_fixed32(&mut out, 2, 0);
    pb_fixed32(&mut out, 3, doc.sort_criterion.unwrap_or(3));
    Ok(out)
}

fn win_to_mac(win: &str, bottle: &std::path::Path) -> PathBuf {
    let rest = win
        .trim_start_matches(|c: char| c.is_ascii_alphabetic())
        .trim_start_matches(':')
        .trim_start_matches('\\');
    bottle.join("drive_c").join(rest.replace('\\', "/"))
}

fn setup_d3d(cli: &Cli, game_dir: &std::path::Path) -> Result<Option<String>> {
    let backend = cli.d3d.to_lowercase();
    if backend == "off" || backend == "none" {
        return Ok(None);
    }
    let cx = cli
        .wine
        .parent()
        .and_then(|p| p.parent())
        .context("chemin wine inattendu (attendu .../bin/wine)")?;
    let dlls: &[&str] = match backend.as_str() {
        "dxmt" => &["d3d11.dll", "dxgi.dll", "d3d10core.dll", "winemetal.dll"],
        "dxvk" => &["d3d11.dll", "d3d10core.dll", "dxgi.dll", "d3d9.dll"],
        other => bail!("--d3d inconnu: {other} (dxmt | dxvk | off)"),
    };
    let src = cx.join("lib").join(&backend).join("x86_64-windows");
    let mut names = Vec::new();
    for dll in dlls {
        let from = src.join(dll);
        if from.exists() {
            std::fs::copy(&from, game_dir.join(dll))
                .with_context(|| format!("copie {dll} -> {}", game_dir.display()))?;
            names.push(dll.trim_end_matches(".dll").to_string());
        }
    }
    if names.is_empty() {
        bail!("aucune DLL {backend} trouvee dans {}", src.display());
    }
    println!("[launch] backend {backend} installe: {} (copie dans {})", names.join(", "), game_dir.display());
    Ok(Some(format!("{}=n,b", names.join(","))))
}

fn launch(cli: &Cli) -> Result<()> {
    let auth_file = cli
        .auth_file
        .as_ref()
        .context("--launch requiert --auth-file (pour recuperer le ticket via tera-bot)")?;
    println!("[launch] recuperation du ticket via {} ...", cli.tera_bot.display());
    let out = std::process::Command::new(&cli.tera_bot)
        .arg("--auth-file")
        .arg(auth_file)
        .arg("--print-ticket")
        .output()
        .with_context(|| format!("execution de {}", cli.tera_bot.display()))?;
    if !out.status.success() {
        bail!("tera-bot --print-ticket a echoue: {}", String::from_utf8_lossy(&out.stderr));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.starts_with("AUTH\t"))
        .context("ligne 'AUTH\\t<compte>\\t<ticket>' introuvable dans la sortie tera-bot")?;
    let mut parts = line.split('\t');
    parts.next();
    let account = parts.next().context("compte manquant")?.to_string();
    let ticket = parts.next().context("ticket manquant")?.to_string();
    println!("[launch] ticket recupere pour le compte {account} ({} o)", ticket.len());

    let (host, port) = cli
        .client_endpoint
        .rsplit_once(':')
        .context("--client-endpoint invalide (attendu host:port)")?;
    let host_ip: std::net::Ipv4Addr = host.parse().context("client-endpoint: IP invalide")?;
    let port_num: u16 = port.parse().context("client-endpoint: port invalide")?;

    let serverlist = build_enterance_serverlist(&cli.serverlist_url, host_ip, port_num)
        .context("construction de la serverlist facon enterance")?;
    let sl_path = cli.bottle.join("drive_c").join("tera_proxy_serverlist.bin");
    std::fs::write(&sl_path, &serverlist)
        .with_context(|| format!("ecriture serverlist {}", sl_path.display()))?;
    let sl_win = "C:\\tera_proxy_serverlist.bin";
    println!(
        "[launch] serverlist facon enterance ecrite ({} o) -> {} ({})",
        serverlist.len(),
        sl_path.display(),
        sl_win
    );

    println!(
        "[launch] lancement de {} dans le bottle (jeu -> {}:{})",
        cli.launcher_exe.display(),
        host,
        port
    );
    let bottle_name = cli
        .bottle
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tera".into());

    let game_mac = win_to_mac(&cli.game_win, &cli.bottle);
    let game_dir = game_mac.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| cli.bottle.clone());
    let dll_overrides = setup_d3d(cli, &game_dir).unwrap_or_else(|e| {
        eprintln!("[launch] backend graphique ignore: {e:#}");
        None
    });

    let mut command = std::process::Command::new(&cli.wine);
    command
        .env("CX_BOTTLE", &bottle_name)
        .env("WINEPREFIX", &cli.bottle);
    if let Some(ov) = &dll_overrides {
        command.env("WINEDLLOVERRIDES", ov);
        println!("[launch] WINEDLLOVERRIDES={ov}");
    }
    command
        .arg(&cli.launcher_exe)
        .args(["--account", &account])
        .args(["--ticket", &ticket])
        .args(["--host", host])
        .args(["--port", port])
        .args(["--server-name", "Elinu"])
        .args(["--game", &cli.game_win])
        .args(["--language", "EUR"])
        .args(["--serverlist", sl_win])
        .spawn()
        .with_context(|| format!("lancement de wine {}", cli.wine.display()))?;
    println!("[launch] launcher lance ; il va servir le ticket a TERA.exe et lancer le jeu");
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let opcodes = OpcodeMap::read(&cli.opcodes)
        .with_context(|| format!("reading {}", cli.opcodes.display()))?;
    let registry = Registry::load(&cli.definitions, Some(cli.patch_version))?;
    let codec = Codec::new(opcodes, registry);
    let constants = if cli.legacy { LEGACY } else { MODERN };

    let mods = build_mods(&cli);
    spawn_mod_watcher(&cli, &mods);

    if cli.gui {
        #[cfg(feature = "gui")]
        {
            return gui::run(cli, codec, mods, constants);
        }
        #[cfg(not(feature = "gui"))]
        {
            anyhow::bail!("--gui needs a build with `--features gui`");
        }
    }

    let capture = build_capture(&cli, codec, None)?;
    let mut listens = cli.listen.clone();
    if cli.launch {
        let ep_port = cli.client_endpoint.rsplit_once(':').map(|(_, p)| p);
        let covered = ep_port
            .map(|p| listens.iter().any(|l| l.rsplit_once(':').map(|(_, lp)| lp) == Some(p)))
            .unwrap_or(false);
        if !covered {
            listens.push(cli.client_endpoint.clone());
        }
    }
    let mut handles = Vec::new();
    for listen in listens {
        let cli = cli.clone();
        let capture = Arc::clone(&capture);
        let mods = Arc::clone(&mods);
        handles.push(std::thread::spawn(move || {
            if let Err(error) = accept_loop(&listen, &cli, capture, mods, constants) {
                eprintln!("listener {listen}: {error}");
            }
        }));
    }
    if cli.launch {
        std::thread::sleep(Duration::from_millis(500));
        if let Err(error) = launch(&cli) {
            eprintln!("[launch] echec: {error:#}");
        }
    }
    for handle in handles {
        let _ = handle.join();
    }
    Ok(())
}

#[cfg(feature = "gui")]
mod gui {
    use super::{accept_loop, build_capture, loader, Cli, EventSink, PacketEvent};
    use anyhow::Result;
    use eframe::egui;
    use std::sync::{Arc, Mutex};
    use tera_hook::Codec;
    use tera_protocol::session::Constants;

    pub fn run(
        cli: Cli,
        codec: Arc<Codec>,
        mods: Arc<Mutex<Arc<loader::LoadedMods>>>,
        constants: Constants,
    ) -> Result<()> {
        let events: EventSink = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let capture = build_capture(&cli, codec, Some(Arc::clone(&events)))?;
        {
            let cli = cli.clone();
            std::thread::spawn(move || {
                let listen = cli.listen.first().cloned().unwrap_or_else(|| "0.0.0.0:9250".into());
                if let Err(error) = accept_loop(&listen, &cli, capture, mods, constants) {
                    eprintln!("proxy: {error}");
                }
            });
        }
        let app = ProxyApp::new(events);
        eframe::run_native(
            "tera-proxy",
            eframe::NativeOptions::default(),
            Box::new(|_cc| Ok(Box::new(app))),
        )
        .map_err(|error| anyhow::anyhow!("eframe: {error}"))?;
        Ok(())
    }

    #[derive(PartialEq)]
    enum DirFilter {
        All,
        ToServer,
        ToClient,
    }

    struct ProxyApp {
        events: EventSink,
        rows: Vec<PacketEvent>,
        filter: String,
        dir: DirFilter,
        paused: bool,
        autoscroll: bool,
        selected: Option<PacketEvent>,
    }

    impl ProxyApp {
        fn new(events: EventSink) -> Self {
            Self {
                events,
                rows: Vec::new(),
                filter: String::new(),
                dir: DirFilter::All,
                paused: false,
                autoscroll: true,
                selected: None,
            }
        }
    }

    impl eframe::App for ProxyApp {
        fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
            if !self.paused {
                let needle = self.filter.to_lowercase();
                if let Ok(buffer) = self.events.lock() {
                    self.rows = buffer
                        .iter()
                        .filter(|event| match self.dir {
                            DirFilter::All => true,
                            DirFilter::ToServer => event.from_client,
                            DirFilter::ToClient => !event.from_client,
                        })
                        .filter(|event| {
                            needle.is_empty() || event.name.to_lowercase().contains(&needle)
                        })
                        .cloned()
                        .collect();
                }
            }

            egui::TopBottomPanel::top("bar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("filtre");
                    ui.text_edit_singleline(&mut self.filter);
                    ui.separator();
                    ui.selectable_value(&mut self.dir, DirFilter::All, "tout");
                    ui.selectable_value(&mut self.dir, DirFilter::ToServer, "-> serveur");
                    ui.selectable_value(&mut self.dir, DirFilter::ToClient, "<- serveur");
                    ui.separator();
                    ui.checkbox(&mut self.paused, "pause");
                    ui.checkbox(&mut self.autoscroll, "suivi");
                    ui.separator();
                    ui.label(format!("{} paquets", self.rows.len()));
                });
            });

            egui::SidePanel::right("detail")
                .min_width(360.0)
                .show(ctx, |ui| match &self.selected {
                    Some(event) => {
                        ui.heading(&event.name);
                        ui.label(format!(
                            "opcode {}  ·  {} o  ·  {:.3}s  ·  {}",
                            event.opcode,
                            event.len,
                            event.at,
                            if event.from_client {
                                "-> serveur"
                            } else {
                                "<- serveur"
                            }
                        ));
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.monospace(hex_dump(&event.hex));
                            if let Some(fields) = &event.fields {
                                ui.separator();
                                ui.label("champs decodes:");
                                ui.monospace(fields);
                            }
                        });
                    }
                    None => {
                        ui.label("clique un paquet pour le detail");
                    }
                });

            egui::CentralPanel::default().show(ctx, |ui| {
                let row_height = 16.0;
                let count = self.rows.len();
                let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
                if self.autoscroll && !self.paused {
                    area = area.stick_to_bottom(true);
                }
                area.show_rows(ui, row_height, count, |ui, range| {
                    for index in range {
                        let event = &self.rows[index];
                        let arrow = if event.from_client { "->" } else { "<-" };
                        let label = format!(
                            "{:8.3}  {}  {:<34} {:>5} {:>6}o",
                            event.at, arrow, event.name, event.opcode, event.len
                        );
                        let selected =
                            self.selected.as_ref().map(|other| other.seq) == Some(event.seq);
                        if ui
                            .selectable_label(selected, egui::RichText::new(label).monospace())
                            .clicked()
                        {
                            self.selected = Some(event.clone());
                        }
                    }
                });
            });
        }
    }

    fn hex_dump(hex: &str) -> String {
        let bytes: Vec<u8> = (0..hex.len() / 2)
            .filter_map(|index| u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok())
            .collect();
        let mut out = String::new();
        for (row, chunk) in bytes.chunks(16).enumerate() {
            let hexpart: Vec<String> = chunk.iter().map(|byte| format!("{byte:02x}")).collect();
            let ascii: String = chunk
                .iter()
                .map(|&byte| if (32..127).contains(&byte) { byte as char } else { '.' })
                .collect();
            out.push_str(&format!(
                "{:04x}  {:<47}  {ascii}\n",
                row * 16,
                hexpart.join(" ")
            ));
        }
        out
    }
}
