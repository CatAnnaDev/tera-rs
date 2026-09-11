use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use anyhow::{bail, Context, Result};
use tera_protocol::{value, OpcodeMap, PacketBuffer, Registry, Session, MODERN};

use crate::net::{object_to_event, Event};

const GREETING_LEN: usize = 4;
const HALF: usize = 128;
const SERVER_HANDSHAKE: usize = GREETING_LEN + HALF * 2;
const CLIENT_HANDSHAKE: usize = HALF * 2;

pub struct Config {
    pub opcodes: PathBuf,
    pub definitions: PathBuf,
    pub patch: u32,
    pub dump: Option<String>,
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}
fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
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
        _ => return None,
    };
    if ip.is_empty() || ip[0] >> 4 != 4 {
        return None;
    }
    Some(ip)
}

struct Endpoint {
    src: ([u8; 4], u16),
    dst: ([u8; 4], u16),
    seq: u32,
    payload: Vec<u8>,
}

fn parse_tcp(ip: &[u8]) -> Option<Endpoint> {
    if ip.len() < 20 {
        return None;
    }
    let ihl = (ip[0] & 0x0f) as usize * 4;
    if ip[9] != 6 || ip.len() < ihl + 20 {
        return None;
    }
    let src_ip = [ip[12], ip[13], ip[14], ip[15]];
    let dst_ip = [ip[16], ip[17], ip[18], ip[19]];
    let tcp = &ip[ihl..];
    let src_port = be16(tcp, 0);
    let dst_port = be16(tcp, 2);
    let seq = be32(tcp, 4);
    let data_offset = (tcp[12] >> 4) as usize * 4;
    if tcp.len() < data_offset {
        return None;
    }
    Some(Endpoint {
        src: (src_ip, src_port),
        dst: (dst_ip, dst_port),
        seq,
        payload: tcp[data_offset..].to_vec(),
    })
}

#[derive(Default)]
struct Direction {
    stream: Vec<u8>,
    next_seq: Option<u32>,
}

impl Direction {
    fn push(&mut self, seq: u32, payload: &[u8]) {
        if payload.is_empty() {
            return;
        }
        match self.next_seq {
            None => {
                self.stream.extend_from_slice(payload);
                self.next_seq = Some(seq.wrapping_add(payload.len() as u32));
            }
            Some(expected) => {
                let ahead = seq.wrapping_sub(expected) as i32;
                if ahead == 0 {
                    self.stream.extend_from_slice(payload);
                    self.next_seq = Some(seq.wrapping_add(payload.len() as u32));
                } else if ahead < 0 {
                    let skip = (-ahead) as usize;
                    if skip < payload.len() {
                        self.stream.extend_from_slice(&payload[skip..]);
                        self.next_seq = Some(seq.wrapping_add(payload.len() as u32));
                    }
                }
            }
        }
    }
}

struct Conn {
    server_key: Option<(([u8; 4], u16), ([u8; 4], u16))>,
    server: Direction,
    client: Direction,
    session: Option<Session>,
    decrypted: usize,
    buffer: PacketBuffer,
    dead: bool,
    reported: bool,
}

impl Conn {
    fn new() -> Self {
        Self {
            server_key: None,
            server: Direction::default(),
            client: Direction::default(),
            session: None,
            decrypted: 0,
            buffer: PacketBuffer::new(),
            dead: false,
            reported: false,
        }
    }
}

fn conn_key(a: &([u8; 4], u16), b: &([u8; 4], u16)) -> (([u8; 4], u16), ([u8; 4], u16)) {
    if a <= b {
        (*a, *b)
    } else {
        (*b, *a)
    }
}

pub fn run(cfg: Config, tx: Sender<Event>) -> Result<()> {
    let opcodes = OpcodeMap::read(&cfg.opcodes)
        .with_context(|| format!("lecture des opcodes {}", cfg.opcodes.display()))?;
    let registry =
        Registry::load(&[cfg.definitions.clone()], Some(cfg.patch)).context("definitions")?;
    let mut dumper = cfg.dump.as_deref().map(crate::dump::Dumper::new);

    let mut input = BufReader::new(std::io::stdin());
    let mut global = [0u8; 24];
    input.read_exact(&mut global).context("en-tete pcap (stdin vide ? lance via tcpdump -w -)")?;
    let big_endian = match &global[0..4] {
        [0xa1, 0xb2, 0xc3, 0xd4] => true,
        [0xd4, 0xc3, 0xb2, 0xa1] => false,
        _ => bail!("flux non-pcap sur stdin"),
    };
    let read_u32 = |b: &[u8], at: usize| {
        let v = [b[at], b[at + 1], b[at + 2], b[at + 3]];
        if big_endian { u32::from_be_bytes(v) } else { u32::from_le_bytes(v) }
    };
    let linktype = read_u32(&global, 20);

    let mut conns: HashMap<(([u8; 4], u16), ([u8; 4], u16)), Conn> = HashMap::new();
    let mut header = [0u8; 16];

    loop {
        if input.read_exact(&mut header).is_err() {
            return Ok(());
        }
        let incl_len = read_u32(&header, 8) as usize;
        if incl_len == 0 || incl_len > 262_144 {
            bail!("longueur de trame pcap invalide: {incl_len}");
        }
        let mut frame = vec![0u8; incl_len];
        if input.read_exact(&mut frame).is_err() {
            return Ok(());
        }

        let Some(ip) = ip_payload(linktype, &frame) else { continue };
        let Some(ep) = parse_tcp(ip) else { continue };
        let key = conn_key(&ep.src, &ep.dst);
        let conn = conns.entry(key).or_insert_with(Conn::new);
        if conn.dead {
            continue;
        }

        // First data direction is the server (TERA is server-first).
        if conn.server_key.is_none() && !ep.payload.is_empty() {
            conn.server_key = Some((ep.src, ep.dst));
        }
        let is_server = conn.server_key == Some((ep.src, ep.dst));
        if is_server {
            conn.server.push(ep.seq, &ep.payload);
        } else {
            conn.client.push(ep.seq, &ep.payload);
        }

        if conn.session.is_none()
            && conn.server.stream.len() >= SERVER_HANDSHAKE
            && conn.client.stream.len() >= CLIENT_HANDSHAKE
        {
            let s = &conn.server.stream;
            let c = &conn.client.stream;
            let server_first: [u8; HALF] = s[GREETING_LEN..GREETING_LEN + HALF].try_into().unwrap();
            let server_second: [u8; HALF] =
                s[GREETING_LEN + HALF..SERVER_HANDSHAKE].try_into().unwrap();
            let client_first: [u8; HALF] = c[0..HALF].try_into().unwrap();
            let client_second: [u8; HALF] = c[HALF..CLIENT_HANDSHAKE].try_into().unwrap();
            let session =
                Session::new(&client_first, &client_second, &server_first, &server_second, MODERN)
                    .swapped();
            conn.session = Some(session);
            conn.decrypted = SERVER_HANDSHAKE;
            eprintln!("[sniff] handshake TERA detecte, dechiffrement demarre");
        }

        if let Some(session) = conn.session.as_mut() {
            if conn.server.stream.len() > conn.decrypted {
                let mut chunk = conn.server.stream[conn.decrypted..].to_vec();
                conn.decrypted = conn.server.stream.len();
                session.decrypt(&mut chunk);
                conn.buffer.push(&chunk);
                let mut valid = false;
                while let Some(packet) = conn.buffer.take_packet() {
                    if let Some(d) = dumper.as_mut() {
                        d.record(packet.opcode, opcodes.name(packet.opcode), &packet.body, &registry);
                    }
                    if let Some(name) = opcodes.name(packet.opcode) {
                        if let Some(def) = registry.get(name) {
                            if let Ok(obj) = value::read(def, &packet.encode()) {
                                valid = true;
                                if !conn.reported {
                                    conn.reported = true;
                                    eprintln!("[sniff] flux TERA en clair OK — le meter recoit les paquets");
                                }
                                if let Some(event) = object_to_event(name, &obj) {
                                    if tx.send(event).is_err() {
                                        return Ok(());
                                    }
                                }
                            }
                        }
                    }
                }
                // A connection that never yields a valid TERA packet is not the game stream.
                if !valid && conn.decrypted > SERVER_HANDSHAKE + 4096 {
                    conn.dead = true;
                }
            }
        }
    }
}
