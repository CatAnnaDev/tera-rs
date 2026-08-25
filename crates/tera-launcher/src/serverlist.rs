use std::net::Ipv4Addr;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf16,
    Utf8,
}

#[derive(Clone, Copy)]
pub struct Encoding {
    pub text: TextEncoding,
    pub nul_terminated: bool,
    pub emit_empty_text: bool,
}

impl Encoding {
    pub const TERA: Self = Self {
        text: TextEncoding::Utf16,
        nul_terminated: false,
        emit_empty_text: true,
    };

    pub const PROTO3_DEFAULTS: Self = Self {
        emit_empty_text: false,
        ..Self::TERA
    };

    pub fn with_text(self, text: TextEncoding) -> Self {
        Self { text, ..self }
    }

    pub fn nul_terminated(self) -> Self {
        Self {
            nul_terminated: true,
            ..self
        }
    }

    fn text_bytes(&self, text: &str) -> Vec<u8> {
        match self.text {
            TextEncoding::Utf16 => {
                let terminator = usize::from(self.nul_terminated);
                let mut out = Vec::with_capacity((text.len() + terminator) * 2);
                for unit in text.encode_utf16() {
                    out.extend_from_slice(&unit.to_le_bytes());
                }
                if self.nul_terminated {
                    out.extend_from_slice(&[0, 0]);
                }
                out
            }
            TextEncoding::Utf8 => {
                let mut out = text.as_bytes().to_vec();
                if self.nul_terminated {
                    out.push(0);
                }
                out
            }
        }
    }
}

pub struct Server {
    pub id: i32,
    pub category: String,
    pub raw_name: String,
    pub name: String,
    pub crowdness: String,
    pub open: String,
    pub address: Ipv4Addr,
    pub port: u16,
    pub language: i32,
    pub popup: String,
}

impl Server {
    pub fn local(id: i32, name: &str, address: Ipv4Addr, port: u16, language: i32) -> Self {
        Self {
            id,
            category: "PvE".into(),
            raw_name: name.into(),
            name: name.into(),
            crowdness: "Low".into(),
            open: "Recommended".into(),
            address,
            port,
            language,
            popup: String::new(),
        }
    }

    fn encode(&self, encoding: &Encoding) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        push_fixed32(&mut out, 1, self.id);
        push_text(&mut out, 2, &self.category, encoding);
        push_text(&mut out, 3, &self.raw_name, encoding);
        push_text(&mut out, 4, &self.name, encoding);
        push_text(&mut out, 5, &self.crowdness, encoding);
        push_text(&mut out, 6, &self.open, encoding);
        push_fixed32(&mut out, 7, i32::from_be_bytes(self.address.octets()));
        push_fixed32(&mut out, 8, i32::from(self.port));
        push_fixed32(&mut out, 9, self.language);
        push_text(&mut out, 10, &self.popup, encoding);
        out
    }
}

pub struct ServerList {
    pub servers: Vec<Server>,
    pub last_played_id: i32,
    pub unknown: i32,
}

impl ServerList {
    pub fn encode(&self, encoding: &Encoding) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.servers.len() * 256);
        for server in &self.servers {
            push_delimited(&mut out, 1, &server.encode(encoding));
        }
        push_fixed32(&mut out, 2, self.last_played_id);
        push_fixed32(&mut out, 3, self.unknown);
        out
    }
}

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn push_delimited(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    push_varint(out, (u64::from(field) << 3) | 2);
    push_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn push_fixed32(out: &mut Vec<u8>, field: u32, value: i32) {
    push_varint(out, (u64::from(field) << 3) | 5);
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_text(out: &mut Vec<u8>, field: u32, text: &str, encoding: &Encoding) {
    if text.is_empty() && !encoding.emit_empty_text {
        return;
    }
    push_delimited(out, field, &encoding.text_bytes(text));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_uses_host_byte_order() {
        let mut out = Vec::new();
        push_fixed32(&mut out, 7, i32::from_be_bytes(Ipv4Addr::new(127, 0, 0, 1).octets()));
        assert_eq!(out, [0x3d, 0x01, 0x00, 0x00, 0x7f]);
    }

    #[test]
    fn tags_match_generated_writer() {
        let list = ServerList {
            servers: vec![Server::local(1, "L", Ipv4Addr::LOCALHOST, 10001, 1)],
            last_played_id: 1,
            unknown: 0,
        };
        let payload = list.encode(&Encoding::TERA);
        assert_eq!(payload[0], 0x0a);
        let body = &payload[2..payload.len() - 10];
        assert_eq!(body[0], 0x0d);
        assert_eq!(&payload[payload.len() - 10..], &[0x15, 1, 0, 0, 0, 0x1d, 0, 0, 0, 0]);
    }

    #[test]
    fn empty_popup_is_skipped_unless_requested() {
        let server = Server::local(1, "L", Ipv4Addr::LOCALHOST, 10001, 1);
        let skipped = server.encode(&Encoding::PROTO3_DEFAULTS);
        let emitted = server.encode(&Encoding::TERA);
        assert_eq!(skipped.len() + 2, emitted.len());
        assert_eq!(&emitted[skipped.len()..], &[0x52, 0x00]);
    }

    #[test]
    fn text_is_utf16_little_endian() {
        let encoding = Encoding::TERA;
        assert_eq!(encoding.text_bytes("PvE"), b"P\0v\0E\0");
        assert_eq!(encoding.nul_terminated().text_bytes("PvE"), b"P\0v\0E\0\0\0");
    }
}
