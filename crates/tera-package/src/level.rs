use crate::package::Package;
use crate::properties::{read_export_properties, PropertyValue};
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
        let Ok((props, _)) = read_export_properties(package, blob) else {
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
