use crate::error::{PackageError, Result};
use crate::package::Package;
use crate::reader::Reader;

#[derive(Clone, Debug)]
pub enum PropertyValue {
    Int(i32),
    Float(f32),
    Bool(bool),
    Byte(u8),
    Enum(String),
    Name(String),
    Str(String),
    Object { index: i32, path: String },
    Struct { name: String, fields: Vec<Property>, raw: Vec<u8> },
    Array { count: i32, element_size: usize, raw: Vec<u8> },
    Raw(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Property {
    pub name: String,
    pub type_name: String,
    pub array_index: i32,
    pub value: PropertyValue,
    pub value_offset: usize,
    pub size: i32,
    pub offset: usize,
    pub end: usize,
    pub enum_name: String,
    pub struct_name: String,
}

pub fn read_export_properties(package: &Package, data: &[u8]) -> Result<(Vec<Property>, usize)> {
    if data.len() < 4 {
        return Ok((Vec::new(), 0));
    }
    let (mut properties, consumed) = read_properties(package, &data[4..])?;
    for property in &mut properties {
        property.value_offset += 4;
        property.offset += 4;
        property.end += 4;
    }
    Ok((properties, consumed + 4))
}

pub fn read_properties(package: &Package, data: &[u8]) -> Result<(Vec<Property>, usize)> {
    let mut reader = Reader::new(data);
    let mut properties = Vec::new();
    loop {
        if reader.remaining() < 8 {
            break;
        }
        let record_start = reader.offset();
        let name_index = reader.i32()?;
        let name_number = reader.i32()?;
        let name = package.name_text(crate::package::Name {
            index: name_index,
            number: name_number,
        });
        if name == "None" {
            break;
        }
        if reader.remaining() < 16 {
            break;
        }
        let type_name = {
            let index = reader.i32()?;
            let number = reader.i32()?;
            package.name_text(crate::package::Name { index, number })
        };
        let size = reader.i32()?;
        let array_index = reader.i32()?;
        let mut struct_name = String::new();
        let mut enum_name = String::new();
        let mut bool_value = false;
        match type_name.as_str() {
            "StructProperty" => {
                let index = reader.i32()?;
                let number = reader.i32()?;
                struct_name = package.name_text(crate::package::Name { index, number });
            }
            "ByteProperty" => {
                let index = reader.i32()?;
                let number = reader.i32()?;
                enum_name = package.name_text(crate::package::Name { index, number });
            }
            "BoolProperty" => {
                bool_value = reader.u8()? != 0;
            }
            _ => {}
        }
        let value_offset = match type_name.as_str() {
            "BoolProperty" => reader.offset() - 1,
            _ => reader.offset(),
        };
        if size < 0 || size as usize > reader.remaining() {
            break;
        }
        let payload = reader.take(size as usize)?.to_vec();
        let value = decode_value(
            package,
            &type_name,
            &struct_name,
            &enum_name,
            bool_value,
            payload,
        )?;
        properties.push(Property {
            name,
            type_name,
            array_index,
            value,
            value_offset,
            size,
            offset: record_start,
            end: reader.offset(),
            enum_name,
            struct_name,
        });
    }
    Ok((properties, reader.offset()))
}

fn decode_value(
    package: &Package,
    type_name: &str,
    struct_name: &str,
    enum_name: &str,
    bool_value: bool,
    payload: Vec<u8>,
) -> Result<PropertyValue> {
    let mut reader = Reader::new(&payload);
    Ok(match type_name {
        "BoolProperty" => PropertyValue::Bool(bool_value),
        "IntProperty" => PropertyValue::Int(reader.i32().unwrap_or_default()),
        "FloatProperty" => PropertyValue::Float(reader.f32().unwrap_or_default()),
        "ObjectProperty" | "ComponentProperty" | "ClassProperty" | "InterfaceProperty" => {
            let index = reader.i32().unwrap_or_default();
            PropertyValue::Object {
                index,
                path: package.full_object_path(index),
            }
        }
        "NameProperty" => {
            let index = reader.i32().unwrap_or_default();
            let number = reader.i32().unwrap_or_default();
            PropertyValue::Name(package.name_text(crate::package::Name { index, number }))
        }
        "StrProperty" => PropertyValue::Str(reader.string().unwrap_or_default()),
        "ByteProperty" => {
            if enum_name.is_empty() || enum_name == "None" {
                PropertyValue::Byte(reader.u8().unwrap_or_default())
            } else {
                let index = reader.i32().unwrap_or_default();
                let number = reader.i32().unwrap_or_default();
                PropertyValue::Enum(package.name_text(crate::package::Name { index, number }))
            }
        }
        "StructProperty" => {
            let fields = match read_properties(package, &payload) {
                Ok((fields, consumed)) if !fields.is_empty() && consumed <= payload.len() => fields,
                _ => Vec::new(),
            };
            PropertyValue::Struct {
                name: struct_name.to_string(),
                fields,
                raw: payload,
            }
        }
        "ArrayProperty" => {
            let count = reader.i32().unwrap_or_default();
            let rest = payload.len().saturating_sub(4);
            let element_size = if count > 0 { rest / count as usize } else { 0 };
            PropertyValue::Array {
                count,
                element_size,
                raw: payload[4.min(payload.len())..].to_vec(),
            }
        }
        _ => PropertyValue::Raw(payload),
    })
}

impl PropertyValue {
    pub fn describe(&self) -> String {
        match self {
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Byte(value) => value.to_string(),
            Self::Enum(value) | Self::Name(value) | Self::Str(value) => value.clone(),
            Self::Object { path, index } => {
                if path.is_empty() {
                    format!("<object {index}>")
                } else {
                    path.clone()
                }
            }
            Self::Struct { name, fields, raw } => {
                if fields.is_empty() {
                    format!("{name} <{} bytes>", raw.len())
                } else {
                    format!("{name} {{{}}}", fields.len())
                }
            }
            Self::Array {
                count,
                element_size,
                ..
            } => format!("[{count} x {element_size}b]"),
            Self::Raw(raw) => format!("<{} bytes>", raw.len()),
        }
    }
}

impl Property {
    pub fn set(&self, blob: &mut [u8], literal: &str) -> Result<()> {
        let at = self.value_offset;
        let mut write = |bytes: &[u8]| -> Result<()> {
            if at + bytes.len() > blob.len() {
                return Err(PackageError::Truncated {
                    offset: at,
                    needed: bytes.len(),
                    available: blob.len().saturating_sub(at),
                });
            }
            blob[at..at + bytes.len()].copy_from_slice(bytes);
            Ok(())
        };
        let bad =
            || PackageError::UnsupportedProperty(format!("`{literal}` does not fit {}", self.name));
        match self.type_name.as_str() {
            "IntProperty" => write(&literal.parse::<i32>().map_err(|_| bad())?.to_le_bytes()),
            "FloatProperty" => write(&literal.parse::<f32>().map_err(|_| bad())?.to_le_bytes()),
            "BoolProperty" => write(&[u8::from(parse_bool(literal).ok_or_else(bad)?)]),
            "ByteProperty" if matches!(self.value, PropertyValue::Byte(_)) => {
                write(&[literal.parse::<u8>().map_err(|_| bad())?])
            }
            other => Err(PackageError::UnsupportedProperty(format!(
                "{other} needs a resize, not an in-place write"
            ))),
        }
    }

    pub fn resizes(&self) -> bool {
        matches!(
            self.type_name.as_str(),
            "StrProperty" | "NameProperty" | "ObjectProperty" | "ComponentProperty" | "ClassProperty"
        ) || matches!(self.value, PropertyValue::Enum(_))
    }

    fn payload(&self, package: &Package, literal: &str) -> Result<Vec<u8>> {
        let bad =
            || PackageError::UnsupportedProperty(format!("`{literal}` does not fit {}", self.name));
        let name_bytes = |text: &str| -> Result<Vec<u8>> {
            let name = package.find_name(text).ok_or_else(|| {
                PackageError::UnsupportedProperty(format!(
                    "`{text}` is not in this package's name table"
                ))
            })?;
            let mut out = name.index.to_le_bytes().to_vec();
            out.extend_from_slice(&name.number.to_le_bytes());
            Ok(out)
        };
        match self.type_name.as_str() {
            "IntProperty" => Ok(literal.parse::<i32>().map_err(|_| bad())?.to_le_bytes().to_vec()),
            "FloatProperty" => Ok(literal.parse::<f32>().map_err(|_| bad())?.to_le_bytes().to_vec()),
            "BoolProperty" => Ok(Vec::new()),
            "StrProperty" => {
                let mut out = Vec::new();
                crate::summary::Text::from(literal).write(&mut out);
                Ok(out)
            }
            "NameProperty" => name_bytes(literal),
            "ByteProperty" => match self.value {
                PropertyValue::Byte(_) => Ok(vec![literal.parse::<u8>().map_err(|_| bad())?]),
                _ => name_bytes(literal),
            },
            "ObjectProperty" | "ComponentProperty" | "ClassProperty" | "InterfaceProperty" => {
                let index = match literal.parse::<i32>() {
                    Ok(index) => index,
                    Err(_) => package.object_index(literal).ok_or_else(|| {
                        PackageError::NoSuchObject(literal.to_string())
                    })?,
                };
                Ok(index.to_le_bytes().to_vec())
            }
            other => Err(PackageError::UnsupportedProperty(format!(
                "{other} cannot be written from a literal"
            ))),
        }
    }

    pub fn encode_payload(&self, package: &Package, payload: &[u8]) -> Result<Vec<u8>> {
        let name_pair = |text: &str| -> Result<[u8; 8]> {
            let name = package.find_name(text).ok_or_else(|| {
                PackageError::UnsupportedProperty(format!(
                    "`{text}` is not in this package's name table"
                ))
            })?;
            let mut pair = [0u8; 8];
            pair[..4].copy_from_slice(&name.index.to_le_bytes());
            pair[4..].copy_from_slice(&name.number.to_le_bytes());
            Ok(pair)
        };
        let mut out = Vec::with_capacity(payload.len() + 32);
        out.extend_from_slice(&name_pair(&self.name)?);
        out.extend_from_slice(&name_pair(&self.type_name)?);
        out.extend_from_slice(&(payload.len() as i32).to_le_bytes());
        out.extend_from_slice(&self.array_index.to_le_bytes());
        match self.type_name.as_str() {
            "StructProperty" => out.extend_from_slice(&name_pair(&self.struct_name)?),
            "ByteProperty" => out.extend_from_slice(&name_pair(&self.enum_name)?),
            "BoolProperty" => out.push(0),
            _ => {}
        }
        out.extend_from_slice(payload);
        Ok(out)
    }

    pub fn rewrite_payload(
        &self,
        package: &Package,
        blob: &[u8],
        payload: &[u8],
    ) -> Result<Vec<u8>> {
        if self.end > blob.len() || self.offset > self.end {
            return Err(PackageError::Truncated {
                offset: self.offset,
                needed: self.end,
                available: blob.len(),
            });
        }
        let record = self.encode_payload(package, payload)?;
        let mut out = Vec::with_capacity(blob.len() + record.len());
        out.extend_from_slice(&blob[..self.offset]);
        out.extend_from_slice(&record);
        out.extend_from_slice(&blob[self.end..]);
        Ok(out)
    }

    pub fn encode(&self, package: &Package, literal: &str) -> Result<Vec<u8>> {
        let name_pair = |text: &str| -> Result<[u8; 8]> {
            let name = package.find_name(text).ok_or_else(|| {
                PackageError::UnsupportedProperty(format!(
                    "`{text}` is not in this package's name table"
                ))
            })?;
            let mut pair = [0u8; 8];
            pair[..4].copy_from_slice(&name.index.to_le_bytes());
            pair[4..].copy_from_slice(&name.number.to_le_bytes());
            Ok(pair)
        };
        let payload = self.payload(package, literal)?;
        let mut out = Vec::with_capacity(payload.len() + 32);
        out.extend_from_slice(&name_pair(&self.name)?);
        out.extend_from_slice(&name_pair(&self.type_name)?);
        out.extend_from_slice(&(payload.len() as i32).to_le_bytes());
        out.extend_from_slice(&self.array_index.to_le_bytes());
        match self.type_name.as_str() {
            "StructProperty" => out.extend_from_slice(&name_pair(&self.struct_name)?),
            "ByteProperty" => out.extend_from_slice(&name_pair(&self.enum_name)?),
            "BoolProperty" => out.push(u8::from(parse_bool(literal).unwrap_or(false))),
            _ => {}
        }
        out.extend_from_slice(&payload);
        Ok(out)
    }

    pub fn rewrite(&self, package: &Package, blob: &[u8], literal: &str) -> Result<Vec<u8>> {
        if self.end > blob.len() || self.offset > self.end {
            return Err(PackageError::Truncated {
                offset: self.offset,
                needed: self.end,
                available: blob.len(),
            });
        }
        let record = self.encode(package, literal)?;
        let mut out = Vec::with_capacity(blob.len() + record.len());
        out.extend_from_slice(&blob[..self.offset]);
        out.extend_from_slice(&record);
        out.extend_from_slice(&blob[self.end..]);
        Ok(out)
    }
}

fn parse_bool(literal: &str) -> Option<bool> {
    match literal {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}
