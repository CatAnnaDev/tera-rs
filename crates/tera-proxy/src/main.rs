mod hooks;
mod loader;
mod plugins;

use anyhow::{bail, Context, Result};
use clap::Parser;
use hooks::{dispatch, Codec, Direction, Engine, Handler, Outcome, Stats};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tera_protocol::handshake::{random_key, ClientHandshake, ServerHandshake, Step, MAGIC};
use tera_protocol::session::{Constants, Decrypting, Encrypting, KEY_LEN, LEGACY, MODERN};
use tera_protocol::{value, Object, OpcodeMap, PacketBuffer, Registry};

#[derive(Parser)]
#[command(name = "tera-proxy", about = "MITM between the client and a server: decrypts, inspects, hooks")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:9250", help = "where the client connects")]
    listen: String,
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
    #[arg(long, default_value = "mods", help = "directory of dynamic mod libraries")]
    mods_dir: PathBuf,
    #[arg(long, help = "do not load any dynamic mods")]
    no_mods: bool,
    #[arg(long, help = "disable a mod by name (repeatable)")]
    disable_mod: Vec<String>,
}

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
    constants: Constants,
    capture: Arc<Capture>,
    mods: Arc<loader::LoadedMods>,
) -> Result<()> {
    let mut client = client;
    client.set_nodelay(true)?;
    client.set_read_timeout(Some(Duration::from_secs(30)))?;
    client.set_write_timeout(Some(Duration::from_secs(30)))?;
    let address = upstream
        .to_socket_addrs()
        .with_context(|| format!("resolving {upstream}"))?
        .next()
        .with_context(|| format!("no address for {upstream}"))?;
    let mut server = TcpStream::connect_timeout(&address, Duration::from_secs(10))
        .with_context(|| format!("connecting to {upstream}"))?;
    server.set_nodelay(true)?;
    server.set_read_timeout(Some(Duration::from_secs(30)))?;
    server.set_write_timeout(Some(Duration::from_secs(30)))?;

    let greeting = read_exactly(&mut server, MAGIC.len())?;
    if greeting != MAGIC {
        bail!("upstream greeting {greeting:?}, expected {MAGIC:?}");
    }
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
    println!("both handshakes complete, relaying");

    let mut plugins = plugins::builtin();
    plugins.extend(mods.instantiate());
    let (client_to_server, server_to_client) = Engine::build(plugins, &capture.codec).split();
    let (client_encrypting, client_decrypting) = client_session.split();
    let (upstream_encrypting, upstream_decrypting) = upstream_session.split();

    let client = Arc::new(client);
    let server = Arc::new(server);
    let (to_client_tx, to_client_rx) = mpsc::sync_channel::<Vec<u8>>(8192);
    let (to_server_tx, to_server_rx) = mpsc::sync_channel::<Vec<u8>>(8192);

    let client_writer = {
        let sink = Arc::clone(&client);
        std::thread::spawn(move || writer_thread(client_encrypting, sink, to_client_rx))
    };
    let server_writer = {
        let sink = Arc::clone(&server);
        std::thread::spawn(move || writer_thread(upstream_encrypting, sink, to_server_rx))
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

    let _ = upward.join();
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let opcodes = OpcodeMap::read(&cli.opcodes)
        .with_context(|| format!("reading {}", cli.opcodes.display()))?;
    let registry = Registry::load(&cli.definitions, Some(cli.patch_version))?;
    let codec = Codec::new(opcodes, registry);

    if let Some(parent) = cli.dump.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&cli.dump)
        .with_context(|| format!("opening {}", cli.dump.display()))?;
    let capture = Arc::new(Capture {
        file: Mutex::new(BufWriter::new(file)),
        codec,
        started: std::time::Instant::now(),
        only: cli.only.into_iter().collect(),
        hide: cli.hide.into_iter().collect(),
        show_fields: cli.show_fields,
        show_hex: cli.show_hex,
        dump_fields: cli.dump_fields,
        show_stats: cli.stats,
        stats: Mutex::new(Stats::default()),
        warned: Mutex::new(HashSet::new()),
    });
    let constants = if cli.legacy { LEGACY } else { MODERN };

    let mods: Arc<Mutex<Arc<loader::LoadedMods>>> = if cli.no_mods {
        Arc::new(Mutex::new(Arc::new(loader::LoadedMods::empty())))
    } else {
        Arc::new(Mutex::new(Arc::new(loader::LoadedMods::load(
            &cli.mods_dir,
            &cli.disable_mod,
        ))))
    };

    if !cli.no_mods {
        let mods = Arc::clone(&mods);
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

    let listener = TcpListener::bind(&cli.listen)
        .with_context(|| format!("binding {}", cli.listen))?;
    println!(
        "listening on {}, relaying to {}, dump {}",
        cli.listen,
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
        let capture = Arc::clone(&capture);
        let mods = {
            let guard = mods.lock().unwrap_or_else(|poison| poison.into_inner());
            Arc::clone(&guard)
        };
        let handle = std::thread::spawn(move || {
            match serve(client, &upstream, constants, capture, mods) {
                Err(error) => println!("session ended: {error}"),
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
