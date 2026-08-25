use anyhow::{bail, Context, Result};
use clap::Parser;
use std::io::{BufWriter, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tera_protocol::handshake::{random_key, ClientHandshake, ServerHandshake, Step, MAGIC};
use tera_protocol::session::{Constants, Decrypting, Encrypting, LEGACY, MODERN, KEY_LEN};
use tera_server::registry::Registry;
use tera_protocol::{OpcodeMap, PacketBuffer};

#[derive(Parser)]
#[command(name = "tera-proxy", about = "Sit between the client and a server and dump everything")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:10001", help = "Where the client connects")]
    listen: String,
    #[arg(long, help = "The real server, host:port")]
    upstream: String,
    #[arg(long, default_value = "data/opcodes/protocol.376012.map")]
    opcodes: PathBuf,
    #[arg(long, default_values = ["data/definitions"])]
    definitions: Vec<PathBuf>,
    #[arg(long, default_value_t = 100)]
    patch_version: u32,
    #[arg(long, default_value = "captures/capture.jsonl", help = "Every packet, one JSON object per line")]
    dump: PathBuf,
    #[arg(long, help = "Use the pre-45 key shift constants")]
    legacy: bool,
}

struct Capture {
    file: Mutex<BufWriter<std::fs::File>>,
    opcodes: OpcodeMap,
    registry: Registry,
    started: std::time::Instant,
}

impl Capture {
    fn record(&self, from_client: bool, packet: &tera_protocol::Packet) {
        let name = self
            .opcodes
            .name(packet.opcode)
            .map(str::to_string)
            .unwrap_or_else(|| format!("UNKNOWN_{}", packet.opcode));
        let decoded = self.registry.get(&name).and_then(|definition| {
            tera_protocol::value::read(definition, &packet.encode())
                .ok()
                .map(|object| describe(&object))
        });
        let hex: String = packet.body.iter().map(|byte| format!("{byte:02x}")).collect();
        let line = format!(
            "{{\"at\":{:.3},\"from\":\"{}\",\"opcode\":{},\"name\":\"{}\",\"len\":{},\"hex\":\"{}\",\"fields\":{}}}",
            self.started.elapsed().as_secs_f64(),
            if from_client { "client" } else { "server" },
            packet.opcode,
            name,
            packet.body.len(),
            hex,
            decoded.map(|text| format!("\"{}\"", text.replace('"', "'"))).unwrap_or_else(|| "null".into()),
        );
        println!(
            "{:8.3} {} {name} ({}) {} bytes",
            self.started.elapsed().as_secs_f64(),
            if from_client { "->" } else { "<-" },
            packet.opcode,
            packet.body.len()
        );
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }
}

fn describe(object: &tera_protocol::Object) -> String {
    object
        .fields
        .iter()
        .take(12)
        .map(|(name, value)| format!("{name}={value:?}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn read_exactly(stream: &mut TcpStream, count: usize) -> Result<Vec<u8>> {
    let mut buffer = vec![0u8; count];
    stream.read_exact(&mut buffer)?;
    Ok(buffer)
}

fn relay(
    mut source: TcpStream,
    mut sink: TcpStream,
    mut decrypt: Decrypting,
    mut encrypt: Encrypting,
    from_client: bool,
    capture: Arc<Capture>,
) {
    let mut buffer = [0u8; 16384];
    let mut packets = PacketBuffer::new();
    loop {
        let read = match source.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let mut plain = buffer[..read].to_vec();
        decrypt.apply(&mut plain);
        packets.push(&plain);
        while let Some(packet) = packets.take_packet() {
            capture.record(from_client, &packet);
        }
        let mut out = plain;
        encrypt.apply(&mut out);
        if sink.write_all(&out).is_err() {
            break;
        }
    }
    let _ = sink.shutdown(std::net::Shutdown::Both);
}

fn serve(client: TcpStream, upstream: &str, constants: Constants, capture: Arc<Capture>) -> Result<()> {
    let mut client = client;
    client.set_nodelay(true)?;
    let mut server = TcpStream::connect(upstream)
        .with_context(|| format!("connecting to {upstream}"))?;
    server.set_nodelay(true)?;

    let greeting = read_exactly(&mut server, MAGIC.len())?;
    if greeting != MAGIC {
        bail!("upstream greeting was {greeting:?}, expected {MAGIC:?}");
    }
    let client_first = random_key();
    let client_second = random_key();
    let upward = ClientHandshake::new(client_first, client_second).with_constants(constants);
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
            bail!("client closed during the handshake");
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
    println!("both handshakes complete, relaying");

    let (client_encrypting, client_decrypting) = client_session.split();
    let (upstream_encrypting, upstream_decrypting) = upstream_session.split();
    let (client_read, client_write) = (client.try_clone()?, client);
    let (server_read, server_write) = (server.try_clone()?, server);

    let upward_capture = Arc::clone(&capture);
    let upward = std::thread::spawn(move || {
        relay(
            client_read,
            server_write,
            client_decrypting,
            upstream_encrypting,
            true,
            upward_capture,
        )
    });
    relay(
        server_read,
        client_write,
        upstream_decrypting,
        client_encrypting,
        false,
        capture,
    );
    let _ = upward.join();
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let opcodes = OpcodeMap::read(&cli.opcodes)
        .with_context(|| format!("reading {}", cli.opcodes.display()))?;
    let registry = Registry::load(&cli.definitions, Some(cli.patch_version))?;
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
        opcodes,
        registry,
        started: std::time::Instant::now(),
    });
    let constants = if cli.legacy { LEGACY } else { MODERN };

    let listener = TcpListener::bind(&cli.listen)
        .with_context(|| format!("binding {}", cli.listen))?;
    println!(
        "listening on {}, forwarding to {}, writing {}",
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
        if let Err(error) = serve(client, &cli.upstream, constants, Arc::clone(&capture)) {
            println!("session ended: {error}");
        } else {
            println!("session closed");
        }
    }
    Ok(())
}
