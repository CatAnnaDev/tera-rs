use crate::world::Character;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use tera_protocol::defs::{Definition, Field, Primitive};
use tera_protocol::value::{Object, Value};

#[derive(Deserialize)]
pub struct Reply {
    pub packet: String,
    #[serde(default)]
    pub fields: serde_json::Value,
}

#[derive(Default)]
pub struct Responses {
    table: HashMap<String, Vec<Reply>>,
}

impl Responses {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(Self::default());
        };
        Ok(Self {
            table: serde_json::from_slice(&bytes)?,
        })
    }

    pub fn get(&self, request: &str) -> &[Reply] {
        self.table.get(request).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[derive(Default)]
pub struct Context {
    pub game_id: u64,
    pub character: Option<Character>,
    pub zone: i64,
    pub location: [f32; 3],
    pub angle: i64,
    pub uptime_ms: u64,
    pub request: Option<Object>,
}

impl Context {
    fn placeholder(&self, name: &str) -> Option<Value> {
        if let Some(field) = name.strip_prefix("$.") {
            return self.request.as_ref().and_then(|object| object.get(field)).cloned();
        }
        let character = self.character.as_ref();
        Some(match name {
            "$gameId" => Value::Uint(self.game_id),
            "$playerId" | "$characterId" => {
                Value::Uint(character.map(|value| u64::from(value.id)).unwrap_or(0))
            }
            "$templateId" => Value::Int(character.map(Character::template_id).unwrap_or(0)),
            "$name" => Value::Str(character.map(|value| value.name.clone()).unwrap_or_default()),
            "$level" => Value::Int(character.map(|value| value.level).unwrap_or(1)),
            "$zone" => Value::Int(self.zone),
            "$loc" => Value::Vec3(self.location),
            "$angle" => Value::Int(self.angle),
            "$time" => Value::Uint(self.uptime_ms),
            _ => return None,
        })
    }
}

pub fn object_from_json(
    definition: &Definition,
    json: &serde_json::Value,
    context: &Context,
) -> Object {
    let mut object = Object::new();
    let Some(map) = json.as_object() else {
        return object;
    };
    for (name, field) in &definition.fields {
        let Some(raw) = map.get(name) else {
            continue;
        };
        if let Some(value) = convert(field, raw, context) {
            let merged = match (object.get(name), &value) {
                (Some(Value::Object(existing)), Value::Object(fresh)) => {
                    let mut merged = existing.clone();
                    for (field, value) in &fresh.fields {
                        merged.set(field.clone(), value.clone());
                    }
                    Value::Object(merged)
                }
                _ => value,
            };
            object.set(name.clone(), merged);
        }
    }
    object
}

fn convert(field: &Field, raw: &serde_json::Value, context: &Context) -> Option<Value> {
    if let Some(text) = raw.as_str() {
        if let Some(value) = context.placeholder(text) {
            return Some(value);
        }
    }
    match field {
        Field::Value(primitive) => scalar(*primitive, raw),
        Field::Str => Some(Value::Str(raw.as_str().unwrap_or_default().to_string())),
        Field::Bytes => Some(Value::Bytes(bytes(raw))),
        Field::Object(inner) => Some(Value::Object(object_from_json(inner, raw, context))),
        Field::Array {
            subtype,
            string_subtype,
            definition: inner,
        } => {
            let items = raw.as_array()?;
            if *string_subtype || subtype.is_some() {
                let element = subtype.map(Field::Value).unwrap_or(Field::Str);
                Some(Value::List(
                    items
                        .iter()
                        .filter_map(|item| convert(&element, item, context))
                        .collect(),
                ))
            } else {
                Some(Value::Array(
                    items
                        .iter()
                        .map(|item| object_from_json(inner, item, context))
                        .collect(),
                ))
            }
        }
        Field::RefArray(_) | Field::RefBytes(_) | Field::RefString(_) => None,
    }
}

fn scalar(primitive: Primitive, raw: &serde_json::Value) -> Option<Value> {
    Some(match primitive {
        Primitive::Bool => Value::Bool(match raw {
            serde_json::Value::Bool(flag) => *flag,
            other => other.as_i64().unwrap_or(0) != 0,
        }),
        Primitive::Float | Primitive::Double => Value::Float(raw.as_f64().unwrap_or(0.0)),
        Primitive::Vec3 | Primitive::Vec3Fa => {
            let items = raw.as_array()?;
            let mut vector = [0.0f32; 3];
            for (slot, item) in vector.iter_mut().zip(items) {
                *slot = item.as_f64().unwrap_or(0.0) as f32;
            }
            Value::Vec3(vector)
        }
        _ => match raw {
            serde_json::Value::Bool(flag) => Value::Uint(u64::from(*flag)),
            other => match other.as_i64() {
                Some(number) => Value::Int(number),
                None => Value::Uint(other.as_u64().unwrap_or(0)),
            },
        },
    })
}

fn bytes(raw: &serde_json::Value) -> Vec<u8> {
    match raw {
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| item.as_u64().unwrap_or(0) as u8)
            .collect(),
        serde_json::Value::String(text) => text
            .as_bytes()
            .chunks(2)
            .filter_map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|value| u8::from_str_radix(value, 16).ok())
            })
            .collect(),
        _ => Vec::new(),
    }
}
