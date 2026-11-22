use std::collections::HashMap;
use std::path::PathBuf;

const HEADER_LEN: usize = 4;

fn u16_at(packet: &[u8], pos: usize) -> Option<u16> {
    if pos + 2 <= packet.len() {
        Some(u16::from_le_bytes([packet[pos], packet[pos + 1]]))
    } else {
        None
    }
}

fn valid_string(packet: &[u8], off: usize) -> bool {
    if off < HEADER_LEN || off + 2 > packet.len() {
        return false;
    }
    let mut pos = off;
    let mut units = 0usize;
    loop {
        let Some(unit) = u16_at(packet, pos) else {
            return false;
        };
        pos += 2;
        if unit == 0 {
            break;
        }
        if units == 0 && !(0x20..=0xFFFF).contains(&unit) {
            return false;
        }
        if unit < 0x20 && unit != 0x09 && unit != 0x0A {
            return false;
        }
        units += 1;
        if units > 128 {
            return false;
        }
    }
    units >= 1
}

fn array_nodes(packet: &[u8], count: usize, off: usize) -> bool {
    if count == 0 || count > 2048 {
        return false;
    }
    let mut cur = off;
    for index in 0..count {
        if cur + 4 > packet.len() || cur < HEADER_LEN {
            return false;
        }
        let Some(selfptr) = u16_at(packet, cur) else {
            return false;
        };
        if selfptr as usize != cur {
            return false;
        }
        let Some(next) = u16_at(packet, cur + 2) else {
            return false;
        };
        if index == count - 1 {
            if next != 0 {
                return false;
            }
        } else {
            let next = next as usize;
            if next <= cur || next + 4 > packet.len() {
                return false;
            }
            cur = next;
        }
    }
    true
}

enum Ref {
    Str(String),
    Array { name: String, string_elems: bool, elem_len: usize, first_off: usize },
}

fn primitive_for(chunk: usize) -> &'static str {
    match chunk {
        1 => "byte",
        2 => "uint16",
        _ => "int32",
    }
}

fn infer_elem_fields(packet: &[u8], first_off: usize, elem_len: usize, string_elems: bool) -> Vec<String> {
    let mut out = Vec::new();
    if string_elems {
        out.push("- string value".to_string());
        return out;
    }
    let start = first_off + 4;
    let end = (start + elem_len).min(packet.len());
    let mut pos = start;
    let mut index = 1;
    while pos < end {
        let remaining = end - pos;
        let chunk = if remaining >= 4 { 4 } else { remaining };
        out.push(format!("- {} field{index}", primitive_for(chunk)));
        pos += chunk;
        index += 1;
    }
    if out.is_empty() {
        out.push("- int32 field1".to_string());
    }
    out
}

pub fn infer_def(packet: &[u8], samples: usize) -> String {
    let len = packet.len();
    let mut refs: Vec<Ref> = Vec::new();
    let mut boundary = len;
    let mut pos = HEADER_LEN;
    let mut str_index = 1;
    let mut arr_index = 1;

    while pos + 2 <= boundary {
        if pos + 4 <= len {
            if let (Some(count), Some(off)) = (u16_at(packet, pos), u16_at(packet, pos + 2)) {
                let (count, off) = (count as usize, off as usize);
                if array_nodes(packet, count, off) {
                    let first_next = u16_at(packet, off + 2).unwrap_or(0) as usize;
                    let node_span = if first_next > off { first_next - off } else { len - off };
                    let string_elems = valid_string(packet, off + 6);
                    let elem_len = node_span.saturating_sub(4);
                    refs.push(Ref::Array {
                        name: format!("array{arr_index}"),
                        string_elems,
                        elem_len,
                        first_off: off,
                    });
                    arr_index += 1;
                    boundary = boundary.min(off);
                    pos += 4;
                    continue;
                }
            }
        }
        if let Some(off) = u16_at(packet, pos) {
            let off = off as usize;
            if valid_string(packet, off) {
                refs.push(Ref::Str(format!("str{str_index}")));
                str_index += 1;
                boundary = boundary.min(off);
                pos += 2;
                continue;
            }
        }
        break;
    }

    let mut lines = Vec::new();
    lines.push(format!(
        "# auto-genere par tera-bot (scaffold, {samples} echantillon(s)) : verifier/retyper les champs fixes"
    ));

    for r in &refs {
        match r {
            Ref::Str(name) => lines.push(format!("ref {name}")),
            Ref::Array { name, .. } => lines.push(format!("ref {name}")),
        }
    }

    let mut prim_pos = pos;
    let mut unk = 1;
    while prim_pos < boundary {
        let remaining = boundary - prim_pos;
        let chunk = if remaining >= 4 { 4 } else { remaining };
        lines.push(format!("{} unk{unk}", primitive_for(chunk)));
        prim_pos += chunk;
        unk += 1;
    }

    for r in &refs {
        match r {
            Ref::Str(name) => lines.push(format!("string {name}")),
            Ref::Array { name, string_elems, elem_len, first_off } => {
                if *string_elems {
                    lines.push(format!("array<string> {name}"));
                } else {
                    lines.push(format!("array {name}"));
                    for field in infer_elem_fields(packet, *first_off, *elem_len, *string_elems) {
                        lines.push(field);
                    }
                }
            }
        }
    }

    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn detecte_un_array_de_deux_entiers() {
        let full = hex("2000bc4d020008000800140000000000ccbc05001400000001000000c1bc0500");
        let def = infer_def(&full, 1);
        assert!(def.contains("ref array1"), "{def}");
        assert!(def.contains("array array1"), "{def}");
        let int_fields = def.matches("- int32").count();
        assert_eq!(int_fields, 2, "deux champs int32 dans l'element\n{def}");
    }

    #[test]
    fn la_def_generee_est_parsable_et_redecode() {
        let full = hex("2000bc4d020008000800140000000000ccbc05001400000001000000c1bc0500");
        let def_text = infer_def(&full, 1);
        let definition = tera_protocol::defs::parse(&def_text).expect("def parsable");
        let object = tera_protocol::value::read(&definition, &full).expect("relecture");
        match object.get("array1") {
            Some(tera_protocol::Value::Array(items)) => assert_eq!(items.len(), 2),
            other => panic!("array1 attendu, recu {other:?}\n{def_text}"),
        }
    }

    #[test]
    fn detecte_une_string() {
        let mut full = hex("0000ffff");
        full.extend_from_slice(&6u16.to_le_bytes());
        for unit in "Hi".encode_utf16() {
            full.extend_from_slice(&unit.to_le_bytes());
        }
        full.extend_from_slice(&0u16.to_le_bytes());
        let def = infer_def(&full, 1);
        assert!(def.contains("ref str1"), "{def}");
        assert!(def.contains("string str1"), "{def}");
    }
}

pub struct DefGen {
    enabled: bool,
    dir: PathBuf,
    version: u32,
    best: HashMap<String, usize>,
    samples: HashMap<String, usize>,
}

impl DefGen {
    pub fn new(enabled: bool, dir: PathBuf, version: u32) -> Self {
        Self {
            enabled,
            dir,
            version,
            best: HashMap::new(),
            samples: HashMap::new(),
        }
    }

    pub fn observe(&mut self, name: &str, full: &[u8]) -> Option<String> {
        if !self.enabled || full.len() <= HEADER_LEN {
            return None;
        }
        let count = self.samples.entry(name.to_string()).or_insert(0);
        *count += 1;
        let samples = *count;
        let body = full.len();
        if self.best.get(name).map(|best| *best >= body).unwrap_or(false) {
            return None;
        }
        self.best.insert(name.to_string(), body);
        let text = infer_def(full, samples);
        let path = self.dir.join(format!("{name}.{}.def", self.version));
        if std::fs::create_dir_all(&self.dir).is_err() {
            return None;
        }
        if std::fs::write(&path, text).is_err() {
            return None;
        }
        Some(path.display().to_string())
    }
}
