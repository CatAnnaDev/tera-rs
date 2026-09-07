use anyhow::{bail, Context, Result};
use clap::Parser;
use std::path::PathBuf;
use tera_protocol::{value, OpcodeMap, PacketBuffer, Registry, Session, Value, LEGACY, MODERN};

#[derive(Parser)]
#[command(about = "Décode une capture tcpdump d'une session TERA (handshake en clair -> déchiffre tout)")]
struct Cli {
    pcap: PathBuf,
    #[arg(long, default_value_t = 7800)]
    port: u16,
    #[arg(long, default_value = "data/opcodes/protocol.376012.map")]
    opcodes: PathBuf,
    #[arg(long, default_values = ["data/definitions"])]
    definitions: Vec<PathBuf>,
    #[arg(long, default_value_t = 100)]
    patch_version: u32,
    #[arg(long, help = "constantes pre-45")]
    legacy: bool,
    #[arg(long, help = "affiche les champs décodés de ces opcodes", default_values = ["C_CHECK_VERSION", "C_LOGIN_ARBITER", "S_CHECK_VERSION"])]
    detail: Vec<String>,
    #[arg(long, help = "montre tous les paquets, pas seulement le début")]
    all: bool,
}

struct Reader<'a> {
    data: &'a [u8],
    big_endian: bool,
}

impl Reader<'_> {
    fn u32(&self, at: usize) -> u32 {
        let bytes = [self.data[at], self.data[at + 1], self.data[at + 2], self.data[at + 3]];
        if self.big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        }
    }
}

fn be16(data: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([data[at], data[at + 1]])
}

fn be32(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn ip_payload(linktype: u32, frame: &[u8]) -> Option<&[u8]> {
    let ip = match linktype {
        1 => {
            if frame.len() < 14 {
                return None;
            }
            let mut ethertype = be16(frame, 12);
            let mut offset = 14;
            while ethertype == 0x8100 && frame.len() >= offset + 4 {
                ethertype = be16(frame, offset + 2);
                offset += 4;
            }
            if ethertype != 0x0800 {
                return None;
            }
            &frame[offset..]
        }
        0 => {
            if frame.len() < 4 {
                return None;
            }
            &frame[4..]
        }
        101 => frame,
        113 => {
            if frame.len() < 16 || be16(frame, 14) != 0x0800 {
                return None;
            }
            &frame[16..]
        }
        _ => return None,
    };
    if ip.is_empty() || ip[0] >> 4 != 4 {
        return None;
    }
    Some(ip)
}

struct Segment {
    seq: u32,
    payload: Vec<u8>,
}

fn reassemble(mut segments: Vec<Segment>) -> Vec<u8> {
    segments.sort_by_key(|segment| segment.seq);
    let mut stream = Vec::new();
    let mut cursor: Option<u32> = None;
    for segment in segments {
        if segment.payload.is_empty() {
            continue;
        }
        match cursor {
            None => {
                cursor = Some(segment.seq.wrapping_add(segment.payload.len() as u32));
                stream.extend_from_slice(&segment.payload);
            }
            Some(expected) => {
                let ahead = segment.seq.wrapping_sub(expected) as i32;
                if ahead == 0 {
                    cursor = Some(segment.seq.wrapping_add(segment.payload.len() as u32));
                    stream.extend_from_slice(&segment.payload);
                } else if ahead < 0 {
                    let skip = (-ahead) as usize;
                    if skip < segment.payload.len() {
                        stream.extend_from_slice(&segment.payload[skip..]);
                        cursor = Some(segment.seq.wrapping_add(segment.payload.len() as u32));
                    }
                } else {
                    stream.extend_from_slice(&segment.payload);
                    cursor = Some(segment.seq.wrapping_add(segment.payload.len() as u32));
                }
            }
        }
    }
    stream
}

fn decode_stream(
    label: &str,
    direction: &str,
    stream: &[u8],
    session: &mut Session,
    registry: &Registry,
    opcodes: &OpcodeMap,
    detail: &[String],
    all: bool,
) {
    let mut encrypted = stream.to_vec();
    session.decrypt(&mut encrypted);
    let mut buffer = PacketBuffer::new();
    buffer.push(&encrypted);
    let mut shown = 0;
    while let Some(packet) = buffer.take_packet() {
        let name = opcodes
            .name(packet.opcode)
            .map(str::to_string)
            .unwrap_or_else(|| format!("UNKNOWN_{}", packet.opcode));
        if !all && shown > 60 {
            break;
        }
        shown += 1;
        print!("{label} {direction} {name} ({}) {} o", packet.opcode, packet.body.len());
        if detail.iter().any(|wanted| wanted.eq_ignore_ascii_case(&name)) {
            if let Some(definition) = registry.get(&name) {
                if let Ok(object) = value::read(definition, &packet.encode()) {
                    print!("  {}", describe(&object));
                }
            } else {
                print!("  (pas de def)");
            }
        }
        println!();
    }
}

fn format_value(value: &Value) -> String {
    match value {
        Value::Bool(v) => v.to_string(),
        Value::Int(v) => v.to_string(),
        Value::Uint(v) => v.to_string(),
        Value::Float(v) => format!("{v}"),
        Value::Vec3(a) => format!("({}, {}, {})", a[0], a[1], a[2]),
        Value::Str(s) => format!("{s:?}"),
        Value::Bytes(bytes) => {
            if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
                format!("{:?}", String::from_utf8_lossy(bytes))
            } else {
                format!("<{} octets: {}>", bytes.len(), hex(bytes))
            }
        }
        Value::Object(inner) => format!("{{{}}}", describe(inner)),
        Value::Array(items) => format!("[{} objets]", items.len()),
        Value::List(items) => {
            let inner: Vec<String> = items.iter().map(format_value).collect();
            format!("[{}]", inner.join(", "))
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().take(32).map(|b| format!("{b:02x}")).collect()
}

fn describe(object: &tera_protocol::Object) -> String {
    let mut parts = Vec::new();
    for (name, field) in &object.fields {
        parts.push(format!("{name}={}", format_value(field)));
    }
    parts.join(", ")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let bytes = std::fs::read(&cli.pcap).with_context(|| format!("lecture {}", cli.pcap.display()))?;
    if bytes.len() < 24 {
        bail!("pcap trop court");
    }
    let big_endian = match &bytes[0..4] {
        [0xa1, 0xb2, 0xc3, 0xd4] | [0xa1, 0xb2, 0x3c, 0x4d] => true,
        [0xd4, 0xc3, 0xb2, 0xa1] | [0x4d, 0x3c, 0xb2, 0xa1] => false,
        _ => bail!("magic pcap inconnu (pcapng non supporté, utilise tcpdump -w)"),
    };
    let reader = Reader { data: &bytes, big_endian };
    let linktype = reader.u32(20);
    eprintln!("linktype {linktype}");

    let mut to_server: Vec<Segment> = Vec::new();
    let mut to_client: Vec<Segment> = Vec::new();
    let mut offset = 24;
    while offset + 16 <= bytes.len() {
        let incl_len = reader.u32(offset + 8) as usize;
        let start = offset + 16;
        let end = start + incl_len;
        if end > bytes.len() {
            break;
        }
        let frame = &bytes[start..end];
        offset = end;
        let Some(ip) = ip_payload(linktype, frame) else {
            continue;
        };
        let ihl = ((ip[0] & 0x0f) as usize) * 4;
        if ip.len() < ihl + 20 || ip[9] != 6 {
            continue;
        }
        let tcp = &ip[ihl..];
        let src_port = be16(tcp, 0);
        let dst_port = be16(tcp, 2);
        let seq = be32(tcp, 4);
        let data_offset = ((tcp[12] >> 4) as usize) * 4;
        if tcp.len() < data_offset {
            continue;
        }
        let payload = tcp[data_offset..].to_vec();
        if dst_port == cli.port {
            to_server.push(Segment { seq, payload });
        } else if src_port == cli.port {
            to_client.push(Segment { seq, payload });
        }
    }

    let client_stream = reassemble(to_server);
    let server_stream = reassemble(to_client);
    eprintln!(
        "flux réassemblés: client->serveur {} o, serveur->client {} o",
        client_stream.len(),
        server_stream.len()
    );
    if client_stream.len() < 256 || server_stream.len() < 260 {
        bail!("handshake incomplet dans la capture (relance la capture depuis le début de la connexion)");
    }

    let client_first: [u8; 128] = client_stream[0..128].try_into().unwrap();
    let client_second: [u8; 128] = client_stream[128..256].try_into().unwrap();
    let server_first: [u8; 128] = server_stream[4..132].try_into().unwrap();
    let server_second: [u8; 128] = server_stream[132..260].try_into().unwrap();
    eprintln!("greeting serveur: {:02x?}", &server_stream[0..4]);

    let constants = if cli.legacy { LEGACY } else { MODERN };
    let mut client_to_server = Session::new(&client_first, &client_second, &server_first, &server_second, constants);
    let mut server_to_client =
        Session::new(&client_first, &client_second, &server_first, &server_second, constants).swapped();

    let opcodes = OpcodeMap::read(&cli.opcodes).with_context(|| format!("lecture {}", cli.opcodes.display()))?;
    let registry = Registry::load(&cli.definitions, Some(cli.patch_version)).context("définitions")?;

    println!("== paquets déchiffrés (constantes {}) ==", if cli.legacy { "LEGACY" } else { "MODERN" });
    decode_stream("C>S", "->", &client_stream[256..], &mut client_to_server, &registry, &opcodes, &cli.detail, cli.all);
    decode_stream("S>C", "<-", &server_stream[260..], &mut server_to_client, &registry, &opcodes, &cli.detail, cli.all);
    Ok(())
}
