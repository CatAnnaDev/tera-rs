use crate::error::{PackageError, Result};
use crate::objects::Needle;
use crate::package::{Bundle, Package};
use crate::properties::{read_export_properties_auto, PropertyValue};
use std::collections::BTreeMap;

const COLOR_CHANNELS: [&str; 4] = ["R", "G", "B", "A"];

pub struct ColorNode {
    pub path: String,
    pub class: String,
    pub channels: Vec<(String, f32)>,
}

pub fn color_nodes(data: &[u8]) -> Vec<ColorNode> {
    let mut found = Vec::new();
    for package in Bundle::new(data) {
        let Ok(package) = package else { break };
        for (index, export) in package.exports.iter().enumerate() {
            let class = package.export_class(export);
            if !class.starts_with("MaterialExpressionConstant") {
                continue;
            }
            let Ok(blob) = package.export_data(export) else {
                continue;
            };
            let Ok((properties, _)) = read_export_properties_auto(&package, blob) else {
                continue;
            };
            let mut channels = Vec::new();
            for channel in COLOR_CHANNELS {
                if let Some(property) = properties.iter().find(|property| property.name == channel) {
                    if let PropertyValue::Float(value) = property.value {
                        channels.push((channel.to_string(), value));
                    }
                }
            }
            if channels.is_empty() {
                continue;
            }
            found.push(ColorNode {
                path: package.export_path(index),
                class: class.to_string(),
                channels,
            });
        }
    }
    found
}

fn patch(
    package: &Package<'_>,
    needle: &Needle,
    set: &[(String, String)],
) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut overrides: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut touched = Vec::new();
    for (index, export) in package.exports.iter().enumerate() {
        if !package.export_class(export).starts_with("MaterialExpression") {
            continue;
        }
        let path = package.export_path(index);
        if !needle.matches(&path) {
            continue;
        }
        let blob: &[u8] = package.export_data(export)?;
        let (properties, _) = read_export_properties_auto(package, blob)?;
        let mut patched = blob.to_vec();
        for (name, literal) in set {
            let property = properties
                .iter()
                .find(|property| property.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| {
                    PackageError::UnsupportedProperty(format!("{path} has no float `{name}`"))
                })?;
            property.set(&mut patched, literal)?;
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
        return Err(PackageError::NoSuchObject("no matching expression".into()));
    }
    Ok(crate::Replaced {
        bytes: out,
        textures: touched,
    })
}

pub fn set_floats(data: &[u8], object: &str, set: &[(String, String)]) -> Result<crate::Replaced> {
    match set_matching(data, &Needle::strict(object), set) {
        Err(PackageError::NoSuchObject(_)) => {}
        other => return other,
    }
    set_matching(data, &Needle::loose(object), set)
}

pub struct PropView {
    pub name: String,
    pub type_name: String,
    pub value: String,
    pub editable: bool,
}

pub struct ObjectProps {
    pub path: String,
    pub class: String,
    pub props: Vec<PropView>,
}

fn scalar_editable(type_name: &str) -> bool {
    matches!(
        type_name,
        "IntProperty" | "FloatProperty" | "BoolProperty" | "ByteProperty"
    )
}

pub fn object_properties(data: &[u8], filter: &str) -> Vec<ObjectProps> {
    let needle = filter.to_ascii_lowercase();
    let mut found = Vec::new();
    for package in Bundle::new(data) {
        let Ok(package) = package else { break };
        for (index, export) in package.exports.iter().enumerate() {
            let path = package.export_path(index);
            if !needle.is_empty() && !path.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            let Ok(blob) = package.export_data(export) else {
                continue;
            };
            let Ok((properties, _)) = read_export_properties_auto(&package, blob) else {
                continue;
            };
            if properties.is_empty() {
                continue;
            }
            let props = properties
                .iter()
                .map(|property| PropView {
                    name: property.name.clone(),
                    type_name: property.type_name.clone(),
                    value: property.value.describe(),
                    editable: scalar_editable(&property.type_name) && !property.resizes(),
                })
                .collect();
            found.push(ObjectProps {
                path,
                class: package.export_class(export).to_string(),
                props,
            });
        }
    }
    found
}

fn set_props_patch(
    package: &Package<'_>,
    needle: &Needle,
    set: &[(String, String)],
) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut overrides: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut touched = Vec::new();
    for (index, export) in package.exports.iter().enumerate() {
        let path = package.export_path(index);
        if !needle.matches(&path) {
            continue;
        }
        let blob: &[u8] = package.export_data(export)?;
        let (properties, _) = read_export_properties_auto(package, blob)?;
        let mut patched = blob.to_vec();
        for (name, literal) in set {
            let property = properties
                .iter()
                .find(|property| property.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| {
                    PackageError::UnsupportedProperty(format!("{path} has no property `{name}`"))
                })?;
            property.set(&mut patched, literal)?;
        }
        overrides.insert(index, patched);
        touched.push(path);
    }
    if overrides.is_empty() {
        return Ok(None);
    }
    Ok(Some((crate::writer::rebuild(package, &overrides)?, touched)))
}

fn set_props_matching(data: &[u8], needle: &Needle, set: &[(String, String)]) -> Result<crate::Replaced> {
    let mut out = Vec::with_capacity(data.len());
    let mut touched = Vec::new();
    let mut bundle = Bundle::new(data);
    let mut start = 0usize;
    while let Some(package) = bundle.next() {
        let end = bundle.offset().min(data.len());
        let Ok(package) = package else { break };
        match set_props_patch(&package, needle, set)? {
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
        return Err(PackageError::NoSuchObject("no matching object".into()));
    }
    Ok(crate::Replaced {
        bytes: out,
        textures: touched,
    })
}

pub fn set_object_properties(data: &[u8], object: &str, set: &[(String, String)]) -> Result<crate::Replaced> {
    match set_props_matching(data, &Needle::strict(object), set) {
        Err(PackageError::NoSuchObject(_)) => {}
        other => return other,
    }
    set_props_matching(data, &Needle::loose(object), set)
}
