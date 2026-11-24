use crate::package::Package;
use crate::properties::{read_export_properties_auto, PropertyValue};
use std::path::{Path, PathBuf};

const CHUNK_SUFFIXES: [&str; 12] = [
    "_add", "_aero", "_split", "_obj", "_sl", "_terrain", "_water", "_sub", "_set", "_asset",
    "_lightmap", "_part",
];

pub fn zone_base(stem: &str) -> String {
    let mut base = stem.to_string();
    loop {
        let lower = base.to_ascii_lowercase();
        if let Some(suffix) = CHUNK_SUFFIXES.iter().find(|suffix| lower.ends_with(**suffix)) {
            base.truncate(base.len() - suffix.len());
            continue;
        }
        if let Some((head, tail)) = base.rsplit_once('_') {
            if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) {
                base = head.to_string();
                continue;
            }
        }
        break;
    }
    base
}

pub fn zone_siblings(path: &Path) -> Vec<PathBuf> {
    let Some(directory) = path.parent() else {
        return vec![path.to_path_buf()];
    };
    let stem = path.file_stem().and_then(|value| value.to_str()).unwrap_or("");
    let base = zone_base(stem).to_ascii_lowercase();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let candidate = entry.path();
            if candidate.extension().and_then(|value| value.to_str()) != Some("gpk") {
                continue;
            }
            let candidate_stem = candidate.file_stem().and_then(|value| value.to_str()).unwrap_or("");
            if !candidate_stem.is_empty() && zone_base(candidate_stem).to_ascii_lowercase() == base {
                out.push(candidate);
            }
        }
    }
    out.sort();
    if out.is_empty() {
        out.push(path.to_path_buf());
    }
    out
}

#[derive(Clone, Debug)]
pub struct Placement {
    pub mesh: String,
    pub location: [f32; 3],
    pub rotation: [i32; 3],
    pub scale: [f32; 3],
}

fn read_i32(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

fn read_f32(data: &[u8], offset: usize) -> f32 {
    f32::from_bits(read_i32(data, offset) as u32)
}

fn component_mesh(package: &Package<'_>, component_index: i32) -> Option<String> {
    if component_index <= 0 {
        return None;
    }
    let component = package.exports.get((component_index - 1) as usize)?;
    let blob = package.export_data(component).ok()?;
    let mesh_name = package.find_name("StaticMesh")?;
    let object_type = package.find_name("ObjectProperty")?;
    let mut offset = 0usize;
    while offset + 28 <= blob.len() {
        if read_i32(blob, offset) == mesh_name.index && read_i32(blob, offset + 8) == object_type.index {
            let reference = read_i32(blob, offset + 24);
            if reference == 0 {
                return None;
            }
            let path = package.full_object_path(reference);
            return path.rsplit('.').next().map(str::to_string);
        }
        offset += 4;
    }
    None
}

pub fn dedup_placements(placements: Vec<Placement>) -> Vec<Placement> {
    let mut seen = std::collections::HashSet::with_capacity(placements.len());
    let mut out = Vec::with_capacity(placements.len());
    for placement in placements {
        let key = (
            placement.location.map(f32::to_bits),
            placement.rotation,
            placement.scale.map(f32::to_bits),
            placement.mesh.clone(),
        );
        if seen.insert(key) {
            out.push(placement);
        }
    }
    out
}

pub fn parse_level(package: &Package<'_>) -> Vec<Placement> {
    let mut placements = Vec::new();
    for export in package.exports.iter() {
        let class = package.export_class(export);
        if class != "StaticMeshActor" && class != "InterpActor" && class != "DynamicSMActor" {
            continue;
        }
        let Ok(blob) = package.export_data(export) else {
            continue;
        };
        let Ok((props, _)) = read_export_properties_auto(package, blob) else {
            continue;
        };
        let mut location = [0.0f32; 3];
        let mut rotation = [0i32; 3];
        let mut scale = [1.0f32; 3];
        let mut uniform = 1.0f32;
        let mut component = 0i32;
        for property in &props {
            match (property.name.as_str(), &property.value) {
                ("Location", PropertyValue::Struct { raw, .. }) if raw.len() >= 12 => {
                    for axis in 0..3 {
                        location[axis] = read_f32(raw, axis * 4);
                    }
                }
                ("Rotation", PropertyValue::Struct { raw, .. }) if raw.len() >= 12 => {
                    for axis in 0..3 {
                        rotation[axis] = read_i32(raw, axis * 4);
                    }
                }
                ("DrawScale3D", PropertyValue::Struct { raw, .. }) if raw.len() >= 12 => {
                    for axis in 0..3 {
                        scale[axis] = read_f32(raw, axis * 4);
                    }
                }
                ("DrawScale", PropertyValue::Float(value)) => uniform = *value,
                ("StaticMeshComponent", PropertyValue::Object { index, .. }) => component = *index,
                _ => {}
            }
        }
        if let Some(mesh) = component_mesh(package, component) {
            placements.push(Placement {
                mesh,
                location,
                rotation,
                scale: [scale[0] * uniform, scale[1] * uniform, scale[2] * uniform],
            });
        }
    }
    placements
}

pub struct LevelActor {
    pub object: String,
    pub class: String,
    pub placement: Placement,
}

fn actor_placement(package: &Package<'_>, blob: &[u8]) -> Option<Placement> {
    let (props, _) = read_export_properties_auto(package, blob).ok()?;
    let mut location = [0.0f32; 3];
    let mut rotation = [0i32; 3];
    let mut scale = [1.0f32; 3];
    let mut uniform = 1.0f32;
    let mut component = 0i32;
    for property in &props {
        match (property.name.as_str(), &property.value) {
            ("Location", PropertyValue::Struct { raw, .. }) if raw.len() >= 12 => {
                for axis in 0..3 {
                    location[axis] = read_f32(raw, axis * 4);
                }
            }
            ("Rotation", PropertyValue::Struct { raw, .. }) if raw.len() >= 12 => {
                for axis in 0..3 {
                    rotation[axis] = read_i32(raw, axis * 4);
                }
            }
            ("DrawScale3D", PropertyValue::Struct { raw, .. }) if raw.len() >= 12 => {
                for axis in 0..3 {
                    scale[axis] = read_f32(raw, axis * 4);
                }
            }
            ("DrawScale", PropertyValue::Float(value)) => uniform = *value,
            ("StaticMeshComponent", PropertyValue::Object { index, .. }) => component = *index,
            _ => {}
        }
    }
    let mesh = component_mesh(package, component).unwrap_or_default();
    Some(Placement {
        mesh,
        location,
        rotation,
        scale: [scale[0] * uniform, scale[1] * uniform, scale[2] * uniform],
    })
}

pub fn level_actors(data: &[u8]) -> Vec<LevelActor> {
    let mut actors = Vec::new();
    for package in crate::package::Bundle::new(data) {
        let Ok(package) = package else { break };
        for (index, export) in package.exports.iter().enumerate() {
            let class = package.export_class(export);
            if class != "StaticMeshActor" && class != "InterpActor" && class != "DynamicSMActor" {
                continue;
            }
            let Ok(blob) = package.export_data(export) else {
                continue;
            };
            if let Some(placement) = actor_placement(&package, blob) {
                actors.push(LevelActor {
                    object: package.export_path(index),
                    class: class.to_string(),
                    placement,
                });
            }
        }
    }
    actors
}

#[derive(Default, Clone, Copy)]
pub struct Transform {
    pub location: Option<[f32; 3]>,
    pub rotation: Option<[i32; 3]>,
    pub scale: Option<[f32; 3]>,
}

fn patch_vector(patched: &mut [u8], at: usize, values: [f32; 3]) {
    for (axis, value) in values.iter().enumerate() {
        let bytes = value.to_le_bytes();
        patched[at + axis * 4..at + axis * 4 + 4].copy_from_slice(&bytes);
    }
}

fn patch_rotator(patched: &mut [u8], at: usize, values: [i32; 3]) {
    for (axis, value) in values.iter().enumerate() {
        let bytes = value.to_le_bytes();
        patched[at + axis * 4..at + axis * 4 + 4].copy_from_slice(&bytes);
    }
}

fn patch_actor(
    package: &Package<'_>,
    needle: &crate::objects::Needle,
    transform: &Transform,
) -> crate::error::Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut overrides: std::collections::BTreeMap<usize, Vec<u8>> = std::collections::BTreeMap::new();
    let mut touched = Vec::new();
    for (index, export) in package.exports.iter().enumerate() {
        let class = package.export_class(export);
        if class != "StaticMeshActor" && class != "InterpActor" && class != "DynamicSMActor" {
            continue;
        }
        let path = package.export_path(index);
        if !needle.matches(&path) {
            continue;
        }
        let blob = package.export_data(export)?;
        let (props, _) = read_export_properties_auto(package, blob)?;
        let mut patched = blob.to_vec();
        let mut changed = false;
        for property in &props {
            match (property.name.as_str(), &property.value) {
                ("Location", PropertyValue::Struct { raw, .. }) if raw.len() >= 12 => {
                    if let Some(values) = transform.location {
                        patch_vector(&mut patched, property.value_offset, values);
                        changed = true;
                    }
                }
                ("Rotation", PropertyValue::Struct { raw, .. }) if raw.len() >= 12 => {
                    if let Some(values) = transform.rotation {
                        patch_rotator(&mut patched, property.value_offset, values);
                        changed = true;
                    }
                }
                ("DrawScale3D", PropertyValue::Struct { raw, .. }) if raw.len() >= 12 => {
                    if let Some(values) = transform.scale {
                        patch_vector(&mut patched, property.value_offset, values);
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        if changed {
            overrides.insert(index, patched);
            touched.push(path);
        }
    }
    if overrides.is_empty() {
        return Ok(None);
    }
    Ok(Some((crate::writer::rebuild(package, &overrides)?, touched)))
}

pub fn set_actor_transform(
    data: &[u8],
    object: &str,
    transform: Transform,
) -> crate::error::Result<crate::Replaced> {
    if object.trim().is_empty() {
        return Err(crate::error::PackageError::NoSuchObject(
            "empty actor object".into(),
        ));
    }
    for needle in [
        crate::objects::Needle::strict(object),
        crate::objects::Needle::loose(object),
    ] {
        let mut out = Vec::with_capacity(data.len());
        let mut touched = Vec::new();
        let mut bundle = crate::package::Bundle::new(data);
        let mut start = 0usize;
        while let Some(package) = bundle.next() {
            let end = bundle.offset().min(data.len());
            let Ok(package) = package else { break };
            match patch_actor(&package, &needle, &transform)? {
                Some((bytes, paths)) => {
                    out.extend_from_slice(&bytes);
                    touched.extend(paths);
                }
                None => out.extend_from_slice(&data[start..end]),
            }
            start = end;
        }
        out.extend_from_slice(&data[start.min(data.len())..]);
        if !touched.is_empty() {
            return Ok(crate::Replaced {
                bytes: out,
                textures: touched,
            });
        }
    }
    Err(crate::error::PackageError::NoSuchObject(object.to_string()))
}

pub fn rotator_to_quaternion(rotation: [i32; 3]) -> [f32; 4] {
    let scale = std::f32::consts::PI / 32768.0;
    let pitch = rotation[0] as f32 * scale;
    let yaw = rotation[1] as f32 * scale;
    let roll = rotation[2] as f32 * scale;
    let (sp, cp) = (pitch * 0.5).sin_cos();
    let (sy, cy) = (yaw * 0.5).sin_cos();
    let (sr, cr) = (roll * 0.5).sin_cos();
    [
        cr * sp * sy - sr * cp * cy,
        -cr * sp * cy - sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    ]
}
