use crate::package::{Export, Package};
use crate::properties::{read_export_properties, PropertyValue};

#[derive(Clone, Debug)]
pub struct AnimTrack {
    pub bone: String,
    pub translations: Vec<[f32; 3]>,
    pub rotations: Vec<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct Animation {
    pub name: String,
    pub duration: f32,
    pub frames: usize,
    pub tracks: Vec<AnimTrack>,
}

fn u32le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

fn f32le(data: &[u8], offset: usize) -> f32 {
    f32::from_bits(u32le(data, offset))
}

pub fn animations(package: &Package<'_>) -> Vec<Animation> {
    let mut sets: Vec<(String, Vec<String>)> = Vec::new();
    for (index, export) in package.exports.iter().enumerate() {
        if package.export_class(export) != "AnimSet" {
            continue;
        }
        let bones = anim_set_bones(package, export);
        if !bones.is_empty() {
            sets.push((package.export_path(index), bones));
        }
    }
    let mut out = Vec::new();
    for (index, export) in package.exports.iter().enumerate() {
        if package.export_class(export) != "AnimSequence" {
            continue;
        }
        let path = package.export_path(index);
        let Some((_, bones)) = sets
            .iter()
            .filter(|(prefix, _)| path.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
        else {
            continue;
        };
        if let Some(animation) = parse_sequence(package, export, bones) {
            out.push(animation);
        }
    }
    out
}

fn anim_set_bones(package: &Package<'_>, export: &Export) -> Vec<String> {
    let Ok(blob) = package.export_data(export) else {
        return Vec::new();
    };
    let Ok((props, _)) = read_export_properties(package, blob) else {
        return Vec::new();
    };
    for property in &props {
        if property.name != "TrackBoneNames" {
            continue;
        }
        if let PropertyValue::Array { count, raw, .. } = &property.value {
            let bone_count = ((*count).max(0) as usize).min(raw.len() / 8);
            return (0..bone_count)
                .map(|slot| {
                    let base = slot * 8;
                    if base + 4 > raw.len() {
                        return String::new();
                    }
                    let index = i32::from_le_bytes([raw[base], raw[base + 1], raw[base + 2], raw[base + 3]]);
                    package.names.get(index.max(0) as usize).cloned().unwrap_or_default()
                })
                .collect();
        }
    }
    Vec::new()
}

fn parse_sequence(package: &Package<'_>, export: &Export, bones: &[String]) -> Option<Animation> {
    let blob = package.export_data(export).ok()?;
    let (props, consumed) = read_export_properties(package, blob).ok()?;
    let mut name = String::new();
    let mut frames = 0i32;
    let mut duration = 0.0f32;
    for property in &props {
        match (property.name.as_str(), &property.value) {
            ("SequenceName", PropertyValue::Name(value)) => name = value.clone(),
            ("NumFrames", PropertyValue::Int(value)) => frames = *value,
            ("SequenceLength", PropertyValue::Float(value)) => duration = *value,
            _ => {}
        }
    }
    if frames <= 0 || bones.is_empty() {
        return None;
    }
    let mut offset = consumed;
    if offset + 4 > blob.len() {
        return None;
    }
    let track_count = u32le(blob, offset) as usize;
    offset += 4;
    if track_count == 0 || track_count != bones.len() {
        return None;
    }
    let mut tracks = Vec::with_capacity(track_count);
    for index in 0..track_count {
        if offset + 8 > blob.len() {
            return None;
        }
        let position_size = u32le(blob, offset) as usize;
        let position_count = u32le(blob, offset + 4) as usize;
        offset += 8;
        if position_size != 12 || position_count > 1_000_000 || offset + position_count * position_size > blob.len() {
            return None;
        }
        let translations = (0..position_count)
            .map(|key| {
                let base = offset + key * 12;
                [f32le(blob, base), f32le(blob, base + 4), f32le(blob, base + 8)]
            })
            .collect();
        offset += position_count * position_size;
        if offset + 8 > blob.len() {
            return None;
        }
        let rotation_size = u32le(blob, offset) as usize;
        let rotation_count = u32le(blob, offset + 4) as usize;
        offset += 8;
        if rotation_size != 16 || rotation_count > 1_000_000 || offset + rotation_count * rotation_size > blob.len() {
            return None;
        }
        let rotations = (0..rotation_count)
            .map(|key| {
                let base = offset + key * 16;
                [
                    -f32le(blob, base),
                    -f32le(blob, base + 4),
                    -f32le(blob, base + 8),
                    f32le(blob, base + 12),
                ]
            })
            .collect();
        offset += rotation_count * rotation_size;
        tracks.push(AnimTrack {
            bone: bones.get(index).cloned().unwrap_or_default(),
            translations,
            rotations,
        });
    }
    Some(Animation {
        name,
        duration,
        frames: frames as usize,
        tracks,
    })
}
