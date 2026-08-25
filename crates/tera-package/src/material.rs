use crate::error::{PackageError, Result};
use crate::objects::Needle;
use crate::package::{Bundle, Package};
use crate::properties::{read_export_properties, read_properties, PropertyValue};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Texture,
    Vector,
    Scalar,
}

impl Kind {
    fn of(array: &str) -> Option<Self> {
        match array {
            "TextureParameterValues" => Some(Self::Texture),
            "VectorParameterValues" => Some(Self::Vector),
            "ScalarParameterValues" => Some(Self::Scalar),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::Vector => "colour",
            Self::Scalar => "scalar",
        }
    }
}

pub struct Parameter {
    pub name: String,
    pub kind: Kind,
    pub value: String,
    pub at: usize,
}

pub struct Material {
    pub path: String,
    pub parameters: Vec<Parameter>,
}

fn parameters_of(package: &Package<'_>, blob: &[u8]) -> Vec<Parameter> {
    let Ok((properties, _)) = read_export_properties(package, blob) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for property in &properties {
        let Some(kind) = Kind::of(&property.name) else {
            continue;
        };
        let PropertyValue::Array {
            count,
            element_size,
            ..
        } = &property.value
        else {
            continue;
        };
        let base = property.value_offset + 4;
        for index in 0..(*count).max(0) as usize {
            let start = base + index * element_size;
            let Some(element) = blob.get(start..start + element_size) else {
                break;
            };
            let Ok((fields, _)) = read_properties(package, element) else {
                continue;
            };
            let name = fields
                .iter()
                .find(|field| field.name == "ParameterName")
                .and_then(|field| match &field.value {
                    PropertyValue::Name(text) => Some(text.clone()),
                    _ => None,
                });
            let value = fields
                .iter()
                .find(|field| field.name == "ParameterValue");
            let (Some(name), Some(value)) = (name, value) else {
                continue;
            };
            found.push(Parameter {
                name,
                kind,
                value: describe(kind, element, value.value_offset, value),
                at: start + value.value_offset,
            });
        }
    }
    found
}

fn describe(
    kind: Kind,
    element: &[u8],
    at: usize,
    property: &crate::properties::Property,
) -> String {
    match kind {
        Kind::Texture => property.value.describe(),
        Kind::Scalar => element
            .get(at..at + 4)
            .and_then(|slice| slice.try_into().ok())
            .map(|bytes: [u8; 4]| format!("{}", f32::from_le_bytes(bytes)))
            .unwrap_or_default(),
        Kind::Vector => {
            let channel = |index: usize| {
                element
                    .get(at + index * 4..at + index * 4 + 4)
                    .and_then(|slice| slice.try_into().ok())
                    .map(|bytes: [u8; 4]| f32::from_le_bytes(bytes))
                    .unwrap_or_default()
            };
            format!(
                "{},{},{},{}",
                channel(0),
                channel(1),
                channel(2),
                channel(3)
            )
        }
    }
}

pub fn materials(data: &[u8]) -> Vec<Material> {
    let mut found = Vec::new();
    for package in Bundle::new(data) {
        let Ok(package) = package else { break };
        for (index, export) in package.exports.iter().enumerate() {
            if !package.export_class(export).starts_with("MaterialInstance") {
                continue;
            }
            let Ok(blob) = package.export_data(export) else {
                continue;
            };
            let parameters = parameters_of(&package, blob);
            if parameters.is_empty() {
                continue;
            }
            found.push(Material {
                path: package.export_path(index),
                parameters,
            });
        }
    }
    found
}

fn write_parameter(
    package: &Package<'_>,
    blob: &mut [u8],
    parameter: &Parameter,
    literal: &str,
) -> Result<()> {
    let bad = || {
        PackageError::UnsupportedProperty(format!(
            "`{literal}` is not a {} value for {}",
            parameter.kind.label(),
            parameter.name
        ))
    };
    match parameter.kind {
        Kind::Texture => {
            let index = match literal.parse::<i32>() {
                Ok(index) => index,
                Err(_) => package
                    .object_index(literal)
                    .ok_or_else(|| PackageError::NoSuchObject(literal.to_string()))?,
            };
            let at = parameter.at;
            blob.get_mut(at..at + 4)
                .ok_or_else(bad)?
                .copy_from_slice(&index.to_le_bytes());
        }
        Kind::Scalar => {
            let value: f32 = literal.parse().map_err(|_| bad())?;
            let at = parameter.at;
            blob.get_mut(at..at + 4)
                .ok_or_else(bad)?
                .copy_from_slice(&value.to_le_bytes());
        }
        Kind::Vector => {
            let channels: Vec<f32> = literal
                .split(',')
                .map(|part| part.trim().parse::<f32>())
                .collect::<std::result::Result<_, _>>()
                .map_err(|_| bad())?;
            if channels.len() != 4 {
                return Err(bad());
            }
            for (index, value) in channels.iter().enumerate() {
                let at = parameter.at + index * 4;
                blob.get_mut(at..at + 4)
                    .ok_or_else(bad)?
                    .copy_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(())
}

fn patch(
    package: &Package<'_>,
    needle: &Needle,
    set: &[(String, String)],
) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut overrides: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut touched = Vec::new();
    for (index, export) in package.exports.iter().enumerate() {
        if !package.export_class(export).starts_with("MaterialInstance") {
            continue;
        }
        let path = package.export_path(index);
        if !needle.matches(&path) {
            continue;
        }
        let blob: &[u8] = package.export_data(export)?;
        let parameters = parameters_of(package, blob);
        let mut patched = blob.to_vec();
        for (name, literal) in set {
            let parameter = parameters
                .iter()
                .find(|parameter| parameter.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| {
                    PackageError::UnsupportedProperty(format!("{path} has no parameter `{name}`"))
                })?;
            write_parameter(package, &mut patched, parameter, literal)?;
        }
        overrides.insert(index, patched);
        touched.push(path);
    }
    if overrides.is_empty() {
        return Ok(None);
    }
    Ok(Some((crate::writer::rebuild(package, &overrides)?, touched)))
}

fn set_matching(data: &[u8], needle: &Needle, set: &[(String, String)]) -> Result<crate::Replaced> {
    let mut out = Vec::with_capacity(data.len());
    let mut touched = Vec::new();
    let mut bundle = Bundle::new(data);
    let mut start = 0usize;
    while let Some(package) = bundle.next() {
        let end = bundle.offset().min(data.len());
        let Ok(package) = package else { break };
        match patch(&package, needle, set)? {
            Some((bytes, paths)) => {
                out.extend_from_slice(&bytes);
                touched.extend(paths);
            }
            None => out.extend_from_slice(&data[start..end]),
        }
        start = end;
    }
    out.extend_from_slice(&data[start.min(data.len())..]);
    if touched.is_empty() {
        return Err(PackageError::NoSuchObject("no matching material".into()));
    }
    Ok(crate::Replaced {
        bytes: out,
        textures: touched,
    })
}

pub fn set_parameters(
    data: &[u8],
    object: &str,
    set: &[(String, String)],
) -> Result<crate::Replaced> {
    match set_matching(data, &Needle::strict(object), set) {
        Err(PackageError::NoSuchObject(_)) => {}
        other => return other,
    }
    set_matching(data, &Needle::loose(object), set)
}
