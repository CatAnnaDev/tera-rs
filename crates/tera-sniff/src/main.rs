use anyhow::{bail, Context, Result};
use clap::Parser;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Instant;
use tera_protocol::handshake::{random_key, ServerHandshake, Step};
use tera_protocol::session::{LEGACY, MODERN};
use tera_protocol::{OpcodeMap, PacketBuffer, Registry};

#[derive(Parser)]
#[command(
    name = "tera-sniff",
    about = "Faux-serveur TERA passif: greeting + handshake crypto + decodage en clair de ce que le client envoie"
)]
struct Cli {
    #[arg(long, default_value = "127.0.0.20:9250")]
    listen: String,
    #[arg(long, default_value = "data/opcodes/protocol.376012.map")]
    opcodes: PathBuf,
    #[arg(long, default_values = ["data/definitions"])]
    definitions: Vec<PathBuf>,
    #[arg(long, default_value_t = 100)]
    patch_version: u32,
    #[arg(long, help = "constantes crypto pre-45")]
    legacy: bool,
    #[arg(long, help = "affiche le hex du corps de chaque paquet")]
    hex: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let opcodes = OpcodeMap::read(&cli.opcodes)
        .with_context(|| format!("lecture opcodes {}", cli.opcodes.display()))?;
    let registry = Registry::load(&cli.definitions, Some(cli.patch_version))
        .context("chargement des definitions")?;
    println!(
        "tera-sniff: {} definitions (patch {}), opcodes {}",
        registry.len(),
        cli.patch_version,
        cli.opcodes.display()
    );

    let listener = TcpListener::bind(&cli.listen).with_context(|| format!("bind {}", cli.listen))?;
    println!(
        "ecoute sur {} — pointe la serverlist du jeu ici. Rien n'est emule, on ne fait que decoder ce que le client envoie.",
        cli.listen
    );

    for stream in listener.incoming() {
        match stream {
            Ok(client) => {
                if let Err(error) = handle(client, &opcodes, &registry, cli.legacy, cli.hex) {
                    eprintln!("session terminee: {error:#}");
                }
            }
            Err(error) => eprintln!("accept: {error}"),
        }
    }
    Ok(())
}

fn handle(
    mut stream: TcpStream,
    opcodes: &OpcodeMap,
    registry: &Registry,
    legacy: bool,
    hex: bool,
) -> Result<()> {
    let t0 = Instant::now();
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());
    println!("\n=== client {peer} connecte ===");
    stream.set_nodelay(true)?;

    let constants = if legacy { LEGACY } else { MODERN };
    let mut handshake = ServerHandshake::new(random_key(), random_key()).with_constants(constants);
    stream.write_all(&handshake.greeting())?;
    println!("[{:7.3}s] -> greeting (4 octets) envoye", t0.elapsed().as_secs_f64());

    let mut buffer = [0u8; 8192];
    let mut session = loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            bail!("client ferme pendant le handshake");
        }
        match handshake.feed(&buffer[..read]) {
            Step::Send(reply) => stream.write_all(&reply)?,
            Step::Established(session) => {
                stream.write_all(handshake.server_second())?;
                break *session;
            }
            Step::Wait => {}
        }
    };
    println!(
        "[{:7.3}s] handshake crypto complet — trafic client dechiffre ci-dessous",
        t0.elapsed().as_secs_f64()
    );

    let mut packets = PacketBuffer::new();
    let leftover = handshake.leftover();
    if !leftover.is_empty() {
        let mut data = leftover;
        session.decrypt(&mut data);
        packets.push(&data);
    }

    loop {
        while let Some(packet) = packets.take_packet() {
            let name = opcodes
                .name(packet.opcode)
                .map(str::to_string)
                .unwrap_or_else(|| format!("UNKNOWN_{}", packet.opcode));
            println!(
                "[{:7.3}s] <- {name} ({}) {} octets",
                t0.elapsed().as_secs_f64(),
                packet.opcode,
                packet.body.len()
            );
            match registry.get(&name) {
                Some(definition) => match tera_protocol::value::read(definition, &packet.encode()) {
                    Ok(object) => println!("        {object:?}"),
                    Err(error) => println!("        (decodage impossible: {error})"),
                },
                None => println!("        (pas de definition pour {name})"),
            }
            if hex {
                println!("        hex {}", hexdump(&packet.body));
            }
        }
        let read = match stream.read(&mut buffer) {
            Ok(0) => {
                println!("=== client deconnecte ===");
                return Ok(());
            }
            Ok(read) => read,
            Err(error) => return Err(error.into()),
        };
        let mut data = buffer[..read].to_vec();
        session.decrypt(&mut data);
        packets.push(&data);
    }
}

fn hexdump(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (index, byte) in bytes.iter().take(96).enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&format!("{byte:02x}"));
    }
    if bytes.len() > 96 {
        out.push_str(" ...");
    }
    out
}
