use crate::defs::{Definition, Field, Primitive};
use crate::framing::HEADER_LEN;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum ValueError {
    #[error("packet is truncated at offset {offset} (needed {needed})")]
    Truncated { offset: usize, needed: usize },
    #[error("field `{0}` has no reference")]
    MissingReference(String),
    #[error("field `{name}` expected {expected}")]
    Type { name: String, expected: &'static str },
    #[error("field `{0}` is missing")]
    Missing(String),
    #[error("packet is larger than 65535 bytes")]
    TooLarge,
}

pub type Result<T> = std::result::Result<T, ValueError>;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Vec3([f32; 3]),
    Str(String),
    Bytes(Vec<u8>),
    Object(Object),
    Array(Vec<Object>),
    List(Vec<Value>),
}

impl Value {
    pub fn as_uint(&self) -> Option<u64> {
        match self {
            Self::Uint(value) => Some(*value),
            Self::Int(value) => Some(*value as u64),
            Self::Bool(value) => Some(u64::from(*value)),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            Self::Uint(value) => Some(*value as i64),
            Self::Bool(value) => Some(i64::from(*value)),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Object {
    pub fields: Vec<(String, Value)>,
}

impl Object {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value)
    }

    pub fn set(&mut self, name: impl Into<String>, value: Value) -> &mut Self {
        let name = name.into();
        match self.fields.iter_mut().find(|(field, _)| *field == name) {
            Some(slot) => slot.1 = value,
            None => self.fields.push((name, value)),
        }
        self
    }

    pub fn with(mut self, name: impl Into<String>, value: Value) -> Self {
        self.set(name, value);
        self
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self.position + count;
        if end > self.data.len() {
            return Err(ValueError::Truncated {
                offset: self.position,
                needed: count,
            });
        }
        let slice = &self.data[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
}

fn read_primitive(cursor: &mut Cursor<'_>, primitive: Primitive) -> Result<Value> {
    let bytes = cursor.take(primitive.size())?;
    Ok(match primitive {
        Primitive::Bool => Value::Bool(bytes[0] != 0),
        Primitive::Byte => Value::Uint(u64::from(bytes[0])),
        Primitive::Int16 => Value::Int(i64::from(i16::from_le_bytes([bytes[0], bytes[1]]))),
        Primitive::Uint16 => Value::Uint(u64::from(u16::from_le_bytes([bytes[0], bytes[1]]))),
        Primitive::Angle => Value::Int(i64::from(i16::from_le_bytes([bytes[0], bytes[1]]))),
        Primitive::Int32 => Value::Int(i64::from(i32::from_le_bytes(bytes.try_into().unwrap()))),
        Primitive::Uint32 | Primitive::SkillId32 => {
            Value::Uint(u64::from(u32::from_le_bytes(bytes.try_into().unwrap())))
        }
        Primitive::Float => Value::Float(f64::from(f32::from_le_bytes(bytes.try_into().unwrap()))),
        Primitive::Int64 => Value::Int(i64::from_le_bytes(bytes.try_into().unwrap())),
        Primitive::Uint64 | Primitive::SkillId | Primitive::Customize => {
            Value::Uint(u64::from_le_bytes(bytes.try_into().unwrap()))
        }
        Primitive::Double => Value::Float(f64::from_le_bytes(bytes.try_into().unwrap())),
        Primitive::Vec3 | Primitive::Vec3Fa => Value::Vec3([
            f32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            f32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            f32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        ]),
    })
}

fn write_primitive(out: &mut Vec<u8>, primitive: Primitive, value: &Value, name: &str) -> Result<()> {
    let expect = |expected| ValueError::Type {
        name: name.to_string(),
        expected,
    };
    match primitive {
        Primitive::Bool => out.push(u8::from(matches!(value, Value::Bool(true)) || value.as_uint().unwrap_or(0) != 0)),
        Primitive::Byte => out.push(value.as_uint().ok_or(expect("byte"))? as u8),
        Primitive::Int16 | Primitive::Angle => {
            out.extend_from_slice(&(value.as_int().ok_or(expect("int16"))? as i16).to_le_bytes())
        }
        Primitive::Uint16 => {
            out.extend_from_slice(&(value.as_uint().ok_or(expect("uint16"))? as u16).to_le_bytes())
        }
        Primitive::Int32 => {
            out.extend_from_slice(&(value.as_int().ok_or(expect("int32"))? as i32).to_le_bytes())
        }
        Primitive::Uint32 | Primitive::SkillId32 => {
            out.extend_from_slice(&(value.as_uint().ok_or(expect("uint32"))? as u32).to_le_bytes())
        }
        Primitive::Int64 => {
            out.extend_from_slice(&value.as_int().ok_or(expect("int64"))?.to_le_bytes())
        }
        Primitive::Uint64 | Primitive::SkillId | Primitive::Customize => {
            out.extend_from_slice(&value.as_uint().ok_or(expect("uint64"))?.to_le_bytes())
        }
        Primitive::Float => {
            let number = match value {
                Value::Float(number) => *number,
                other => other.as_int().ok_or(expect("float"))? as f64,
            };
            out.extend_from_slice(&(number as f32).to_le_bytes());
        }
        Primitive::Double => {
            let number = match value {
                Value::Float(number) => *number,
                other => other.as_int().ok_or(expect("double"))? as f64,
            };
            out.extend_from_slice(&number.to_le_bytes());
        }
        Primitive::Vec3 | Primitive::Vec3Fa => {
            let Value::Vec3(vector) = value else {
                return Err(expect("vec3"));
            };
            for component in vector {
                out.extend_from_slice(&component.to_le_bytes());
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct References {
    counts: HashMap<String, usize>,
    offsets: HashMap<String, usize>,
}

pub fn read(definition: &Definition, packet: &[u8]) -> Result<Object> {
    let mut cursor = Cursor {
        data: packet,
        position: HEADER_LEN,
    };
    let mut references = References::default();
    read_level(definition, packet, &mut cursor, &mut references, "")
}

fn read_level(
    definition: &Definition,
    packet: &[u8],
    cursor: &mut Cursor<'_>,
    references: &mut References,
    prefix: &str,
) -> Result<Object> {
    let mut object = Object::new();
    for (name, field) in &definition.fields {
        match field {
            Field::RefArray(target) => {
                let count = cursor.u16()? as usize;
                let offset = cursor.u16()? as usize;
                references.counts.insert(target.clone(), count);
                references.offsets.insert(target.clone(), offset);
            }
            Field::RefBytes(target) => {
                let offset = cursor.u16()? as usize;
                let count = cursor.u16()? as usize;
                references.counts.insert(target.clone(), count);
                references.offsets.insert(target.clone(), offset);
            }
            Field::RefString(target) => {
                let offset = cursor.u16()? as usize;
                references.offsets.insert(target.clone(), offset);
            }
            Field::Value(primitive) => {
                object.set(name.clone(), read_primitive(cursor, *primitive)?);
            }
            Field::Str => {
                let key = format!("{prefix}{name}");
                let offset = *references
                    .offsets
                    .get(&key)
                    .ok_or_else(|| ValueError::MissingReference(name.clone()))?;
                object.set(name.clone(), Value::Str(read_string(packet, offset)?));
            }
            Field::Bytes => {
                let key = format!("{prefix}{name}");
                let offset = *references
                    .offsets
                    .get(&key)
                    .ok_or_else(|| ValueError::MissingReference(name.clone()))?;
                let count = *references.counts.get(&key).unwrap_or(&0);
                let end = offset + count;
                if end > packet.len() {
                    return Err(ValueError::Truncated {
                        offset,
                        needed: count,
                    });
                }
                object.set(name.clone(), Value::Bytes(packet[offset..end].to_vec()));
            }
            Field::Object(inner) => {
                let nested = read_level(
                    inner,
                    packet,
                    cursor,
                    references,
                    &format!("{prefix}{name}."),
                )?;
                let merged = match object.get(name) {
                    Some(Value::Object(existing)) => {
                        let mut merged = existing.clone();
                        for (field, value) in nested.fields {
                            merged.set(field, value);
                        }
                        merged
                    }
                    _ => nested,
                };
                object.set(name.clone(), Value::Object(merged));
            }
            Field::Array {
                subtype,
                string_subtype,
                definition: inner,
            } => {
                let key = format!("{prefix}{name}");
                let count = *references.counts.get(&key).unwrap_or(&0);
                let mut offset = *references.offsets.get(&key).unwrap_or(&0);
                let cap = count.min(packet.len().saturating_sub(offset) / 4);
                let mut items = Vec::with_capacity(cap);
                let mut values = Vec::with_capacity(cap);
                for _ in 0..count {
                    if offset + 4 > packet.len() {
                        break;
                    }
                    let next = u16::from_le_bytes([packet[offset + 2], packet[offset + 3]]) as usize;
                    if *string_subtype {
                        values.push(Value::Str(read_string(packet, offset + 6)?));
                    } else if let Some(primitive) = subtype {
                        let mut element = Cursor {
                            data: packet,
                            position: offset + 4,
                        };
                        values.push(read_primitive(&mut element, *primitive)?);
                    } else {
                        let mut element = Cursor {
                            data: packet,
                            position: offset + 4,
                        };
                        let mut nested = References::default();
                        items.push(read_level(inner, packet, &mut element, &mut nested, "")?);
                    }
                    offset = next;
                    if offset == 0 {
                        break;
                    }
                }
                if values.is_empty() {
                    object.set(name.clone(), Value::Array(items));
                } else {
                    object.set(name.clone(), Value::List(values));
                }
            }
        }
    }
    Ok(object)
}

fn read_string(packet: &[u8], offset: usize) -> Result<String> {
    let mut units = Vec::new();
    let mut position = offset;
    while position + 2 <= packet.len() {
        let unit = u16::from_le_bytes([packet[position], packet[position + 1]]);
        position += 2;
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    Ok(String::from_utf16_lossy(&units))
}

pub fn write(definition: &Definition, opcode: u16, object: &Object) -> Result<Vec<u8>> {
    let mut out = vec![0u8; HEADER_LEN];
    let mut deferred: Vec<String> = Vec::new();
    let mut patches: HashMap<String, Vec<usize>> = HashMap::new();
    write_level(definition, object, &mut out, &mut patches, &mut deferred, "")?;
    order_deferred(definition, &mut deferred);
    write_deferred(definition, object, &mut out, &patches, &deferred)?;

    let size = u16::try_from(out.len()).map_err(|_| ValueError::TooLarge)?;
    out[0..2].copy_from_slice(&size.to_le_bytes());
    out[2..4].copy_from_slice(&opcode.to_le_bytes());
    Ok(out)
}

fn default_value(primitive: Primitive) -> Value {
    match primitive {
        Primitive::Vec3 | Primitive::Vec3Fa => Value::Vec3([0.0; 3]),
        Primitive::Bool => Value::Bool(false),
        Primitive::Float | Primitive::Double => Value::Float(0.0),
        _ => Value::Uint(0),
    }
}

fn write_utf16(out: &mut Vec<u8>, text: &str) {
    out.reserve(text.len() * 2 + 2);
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes());
}

fn patch_offset(
    out: &mut [u8],
    patches: &HashMap<String, Vec<usize>>,
    name: &str,
    start: usize,
) -> Result<()> {
    let Some(positions) = patches.get(name) else {
        return Ok(());
    };
    let offset = u16::try_from(start).map_err(|_| ValueError::TooLarge)?.to_le_bytes();
    for position in positions {
        out[*position..*position + 2].copy_from_slice(&offset);
    }
    Ok(())
}

fn link_chain(out: &mut [u8], positions: &[usize]) {
    for (index, position) in positions.iter().enumerate() {
        let next = positions.get(index + 1).copied().unwrap_or(0) as u16;
        out[position + 2..position + 4].copy_from_slice(&next.to_le_bytes());
    }
}

fn order_deferred(definition: &Definition, deferred: &mut [String]) {
    let mut order = Vec::new();
    data_order(definition, "", &mut order);
    deferred.sort_by_key(|name| {
        order
            .iter()
            .position(|declared| declared == name)
            .unwrap_or(usize::MAX)
    });
}

fn data_order(definition: &Definition, prefix: &str, out: &mut Vec<String>) {
    for (name, field) in &definition.fields {
        match field {
            Field::Str | Field::Bytes | Field::Array { .. } => {
                out.push(format!("{prefix}{name}"));
            }
            Field::Object(inner) => data_order(inner, &format!("{prefix}{name}."), out),
            _ => {}
        }
    }
}

fn write_deferred(
    definition: &Definition,
    object: &Object,
    out: &mut Vec<u8>,
    patches: &HashMap<String, Vec<usize>>,
    deferred: &[String],
) -> Result<()> {
    for name in deferred {
        let start = out.len();
        match lookup_path(object, name) {
            Some(Value::Str(text)) => write_utf16(out, text),
            Some(Value::Bytes(bytes)) => out.extend_from_slice(bytes),
            Some(Value::List(values)) => {
                let (subtype, string_subtype) = match find_path(definition, name) {
                    Some(Field::Array {
                        subtype,
                        string_subtype,
                        ..
                    }) => (*subtype, *string_subtype),
                    _ => (None, false),
                };
                write_simple_array(out, values, subtype, string_subtype, name)?;
            }
            Some(Value::Array(items)) => write_object_array(definition, name, items, out)?,
            _ if matches!(find_path(definition, name), Some(Field::Str)) => write_utf16(out, ""),
            _ => {}
        }
        patch_offset(out, patches, name, start)?;
    }
    Ok(())
}

fn write_simple_array(
    out: &mut Vec<u8>,
    values: &[Value],
    subtype: Option<Primitive>,
    string_subtype: bool,
    name: &str,
) -> Result<()> {
    let mut positions = Vec::with_capacity(values.len());
    for value in values {
        let here = out.len();
        positions.push(here);
        out.extend_from_slice(&(here as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        if string_subtype {
            out.extend_from_slice(&((here + 6) as u16).to_le_bytes());
            let text = value.as_str().unwrap_or_default();
            write_utf16(out, text);
        } else if let Some(primitive) = subtype {
            write_primitive(out, primitive, value, name)?;
        } else {
            match value {
                Value::Int(number) => out.extend_from_slice(&(*number as i32).to_le_bytes()),
                other => out.extend_from_slice(&(other.as_uint().unwrap_or(0) as u32).to_le_bytes()),
            }
        }
    }
    link_chain(out, &positions);
    Ok(())
}

fn lookup_path<'a>(object: &'a Object, path: &str) -> Option<&'a Value> {
    let mut current = object;
    let mut parts = path.split('.').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return current.get(part);
        }
        match current.get(part) {
            Some(Value::Object(nested)) => current = nested,
            _ => return None,
        }
    }
    None
}

fn find_path<'a>(definition: &'a Definition, path: &str) -> Option<&'a Field> {
    match path.split_once('.') {
        None => definition.find(path),
        Some((head, tail)) => definition
            .fields
            .iter()
            .filter_map(|(name, field)| match field {
                Field::Object(inner) if name == head => Some(inner),
                _ => None,
            })
            .find_map(|inner| find_path(inner, tail)),
    }
}

fn write_object_array(
    definition: &Definition,
    name: &str,
    items: &[Object],
    out: &mut Vec<u8>,
) -> Result<()> {
    let Some(Field::Array {
        definition: inner, ..
    }) = find_path(definition, name)
    else {
        return Ok(());
    };
    let mut positions = Vec::with_capacity(items.len());
    for item in items {
        let here = out.len();
        positions.push(here);
        out.extend_from_slice(&(here as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        let mut patches: HashMap<String, Vec<usize>> = HashMap::new();
        let mut deferred: Vec<String> = Vec::new();
        write_level(inner, item, out, &mut patches, &mut deferred, "")?;
        order_deferred(inner, &mut deferred);
        write_deferred(inner, item, out, &patches, &deferred)?;
    }
    link_chain(out, &positions);
    Ok(())
}

fn write_level(
    definition: &Definition,
    object: &Object,
    out: &mut Vec<u8>,
    patches: &mut HashMap<String, Vec<usize>>,
    deferred: &mut Vec<String>,
    prefix: &str,
) -> Result<()> {
    for (name, field) in &definition.fields {
        match field {
            Field::RefArray(target) => {
                let count = match lookup_path(object, target) {
                    Some(Value::Array(items)) => items.len(),
                    Some(Value::List(values)) => values.len(),
                    _ => 0,
                };
                out.extend_from_slice(&(count as u16).to_le_bytes());
                patches.entry(target.clone()).or_default().push(out.len());
                out.extend_from_slice(&0u16.to_le_bytes());
                deferred.push(target.clone());
            }
            Field::RefBytes(target) => {
                patches.entry(target.clone()).or_default().push(out.len());
                out.extend_from_slice(&0u16.to_le_bytes());
                let count = lookup_path(object, target)
                    .and_then(Value::as_bytes)
                    .map(<[u8]>::len)
                    .unwrap_or(0);
                out.extend_from_slice(&(count as u16).to_le_bytes());
                deferred.push(target.clone());
            }
            Field::RefString(target) => {
                patches.entry(target.clone()).or_default().push(out.len());
                out.extend_from_slice(&0u16.to_le_bytes());
                deferred.push(target.clone());
            }
            Field::Value(primitive) => {
                let value = object
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| default_value(*primitive));
                write_primitive(out, *primitive, &value, name)?;
            }
            Field::Str | Field::Bytes | Field::Array { .. } => {}
            Field::Object(inner) => {
                let nested = match object.get(name) {
                    Some(Value::Object(nested)) => nested.clone(),
                    _ => Object::new(),
                };
                write_level(
                    inner,
                    &nested,
                    out,
                    patches,
                    deferred,
                    &format!("{prefix}{name}."),
                )?;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkillId {
    pub id: u32,
    pub hunting_zone: u16,
    pub kind: u8,
    pub npc: bool,
    pub reserved: bool,
}

impl SkillId {
    pub fn player(id: u32) -> Self {
        Self {
            id,
            kind: 1,
            ..Self::default()
        }
    }

    pub fn creature(id: u32, hunting_zone: u16) -> Self {
        Self {
            id,
            hunting_zone,
            kind: 1,
            npc: true,
            ..Self::default()
        }
    }

    fn zoned(&self) -> bool {
        self.npc && self.kind == 1
    }

    pub fn raw(&self) -> u64 {
        let mask = if self.zoned() { 0xffff } else { 0x0fff_ffff };
        let mut raw = u64::from(self.id & mask);
        if self.zoned() {
            raw |= u64::from(self.hunting_zone & 0x0fff) << 16;
        }
        raw |= u64::from(self.kind & 0x0f) << 28;
        raw |= u64::from(self.npc) << 32;
        raw |= u64::from(self.reserved) << 33;
        raw
    }

    pub fn from_raw(raw: u64) -> Self {
        let npc = (raw >> 32) & 1 == 1;
        let kind = ((raw >> 28) & 0x0f) as u8;
        let zoned = npc && kind == 1;
        Self {
            id: (raw & if zoned { 0xffff } else { 0x0fff_ffff }) as u32,
            hunting_zone: if zoned { ((raw >> 16) & 0x0fff) as u16 } else { 0 },
            kind,
            npc,
            reserved: (raw >> 33) & 1 == 1,
        }
    }
}

#[cfg(test)]
mod skill_id_tests {
    use super::SkillId;

    #[test]
    fn a_player_skill_carries_its_type_in_the_high_nibble() {
        let skill = SkillId::player(1842);
        assert_eq!(skill.raw(), 1842 | (1 << 28));
        assert_eq!(SkillId::from_raw(skill.raw()), skill);
    }

    #[test]
    fn a_creature_skill_carries_its_hunting_zone() {
        let skill = SkillId::creature(0x1234, 13);
        let raw = skill.raw();
        assert_eq!(raw & 0xffff, 0x1234);
        assert_eq!((raw >> 16) & 0x0fff, 13);
        assert_eq!((raw >> 32) & 1, 1);
        assert_eq!(SkillId::from_raw(raw), skill);
    }

    #[test]
    fn a_raw_identifier_is_not_a_skill_identifier() {
        assert_ne!(SkillId::player(1842).raw(), 1842);
    }

    #[test]
    fn every_field_survives_a_round_trip() {
        for kind in 0..16u8 {
            for npc in [false, true] {
                let zoned = npc && kind == 1;
                let skill = SkillId {
                    id: 0x0abc,
                    hunting_zone: if zoned { 7 } else { 0 },
                    kind,
                    npc,
                    reserved: kind % 3 == 0,
                };
                assert_eq!(SkillId::from_raw(skill.raw()), skill, "kind {kind} npc {npc}");
            }
        }
    }

    #[test]
    fn a_hunting_zone_is_only_carried_by_creature_skills() {
        let player = SkillId {
            hunting_zone: 13,
            ..SkillId::player(5)
        };
        assert_eq!(
            SkillId::from_raw(player.raw()).hunting_zone,
            0,
            "the wire has nowhere to put it for a player skill"
        );
        assert_eq!(SkillId::from_raw(SkillId::creature(5, 13).raw()).hunting_zone, 13);
    }
}
