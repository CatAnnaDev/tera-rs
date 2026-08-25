use crate::bc::{encode_blocks, BlockFormat};
use crate::dds::Dds;
use crate::error::{PackageError, Result};
use crate::package::Bundle;
use crate::texture::Texture2D;
use std::collections::BTreeMap;

impl Dds {
    pub fn from_rgba(rgba: &[u8], width: u32, height: u32, format: BlockFormat) -> Self {
        let four_cc = match format {
            BlockFormat::Bc1 => *b"DXT1",
            BlockFormat::Bc2 => *b"DXT3",
            _ => *b"DXT5",
        };
        let mut mips = Vec::new();
        let mut level = rgba.to_vec();
        let mut level_width = width.max(1);
        let mut level_height = height.max(1);
        loop {
            mips.push(encode_blocks(
                format,
                &level,
                level_width as usize,
                level_height as usize,
            ));
            if level_width == 1 && level_height == 1 {
                break;
            }
            let next_width = (level_width / 2).max(1);
            let next_height = (level_height / 2).max(1);
            level = halve(&level, level_width, level_height, next_width, next_height);
            level_width = next_width;
            level_height = next_height;
        }
        Self {
            width,
            height,
            four_cc: Some(four_cc),
            bits_per_pixel: 0,
            mips,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(128 + self.mips.iter().map(Vec::len).sum::<usize>());
        out.extend_from_slice(b"DDS ");
        let mut header = [0u32; 31];
        header[0] = 124;
        header[1] = 0x0002_100F;
        header[2] = self.height;
        header[3] = self.width;
        header[4] = self.mips.first().map(Vec::len).unwrap_or(0) as u32;
        header[6] = self.mips.len() as u32;
        header[18] = 32;
        header[19] = if self.four_cc.is_some() { 0x4 } else { 0x40 };
        header[20] = self
            .four_cc
            .map(u32::from_le_bytes)
            .unwrap_or(0);
        header[21] = self.bits_per_pixel;
        header[26] = 0x1000;
        for value in header {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for mip in &self.mips {
            out.extend_from_slice(mip);
        }
        out
    }
}

fn halve(rgba: &[u8], width: u32, height: u32, next_width: u32, next_height: u32) -> Vec<u8> {
    let mut out = vec![0u8; (next_width * next_height * 4) as usize];
    let scale_x = width as f32 / next_width as f32;
    let scale_y = height as f32 / next_height as f32;
    for y in 0..next_height {
        for x in 0..next_width {
            let start_x = (x as f32 * scale_x) as u32;
            let start_y = (y as f32 * scale_y) as u32;
            let end_x = (((x + 1) as f32 * scale_x) as u32).min(width).max(start_x + 1);
            let end_y = (((y + 1) as f32 * scale_y) as u32).min(height).max(start_y + 1);
            let mut sums = [0u32; 4];
            let mut count = 0u32;
            for source_y in start_y..end_y {
                for source_x in start_x..end_x {
                    let base = ((source_y * width + source_x) * 4) as usize;
                    for channel in 0..4 {
                        sums[channel] += u32::from(rgba[base + channel]);
                    }
                    count += 1;
                }
            }
            let base = ((y * next_width + x) * 4) as usize;
            for channel in 0..4 {
                out[base + channel] = (sums[channel] / count.max(1)) as u8;
            }
        }
    }
    out
}

pub fn read_source(bytes: &[u8], format: BlockFormat) -> Result<Dds> {
    if bytes.len() >= 4 && &bytes[..4] == b"DDS " {
        return Dds::parse(bytes);
    }
    let image = crate::png::decode(bytes)?;
    Ok(Dds::from_rgba(
        &image.rgba,
        image.width,
        image.height,
        format,
    ))
}

pub fn format_of(unreal_format: &str) -> BlockFormat {
    match unreal_format {
        "PF_DXT1" => BlockFormat::Bc1,
        "PF_DXT3" => BlockFormat::Bc2,
        _ => BlockFormat::Bc3,
    }
}

pub struct Replaced {
    pub bytes: Vec<u8>,
    pub textures: Vec<String>,
}

pub struct Wanted<'a> {
    pub object: crate::objects::Needle,
    pub source: &'a [u8],
}

fn patch(
    package: &crate::package::Package<'_>,
    wanted: &[Wanted<'_>],
    mut appender: Option<&mut crate::texture::CacheAppender>,
) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut overrides: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut textures = Vec::new();
    for (export_index, export) in package.exports.iter().enumerate() {
        if package.export_class(export) != "Texture2D" {
            continue;
        }
        let path = package.export_path(export_index);
        let Some(entry) = wanted.iter().find(|entry| entry.object.matches(&path)) else {
            continue;
        };
        let texture = Texture2D::parse(package, export)?;
        let dds = read_source(entry.source, format_of(&texture.format))?;
        let blob = package.export_data(export)?;
        let (patched, _) = texture.replace_mips_with(blob, &dds, appender.as_deref_mut())?;
        overrides.insert(export_index, patched);
        textures.push(package.export_path(export_index));
    }
    if overrides.is_empty() {
        return Ok(None);
    }
    Ok(Some((crate::writer::rebuild(package, &overrides)?, textures)))
}

pub fn replace_textures_into(
    data: &[u8],
    wanted: &[(&str, &[u8])],
    mut appender: Option<&mut crate::texture::CacheAppender>,
) -> Result<Replaced> {
    let strict: Vec<Wanted<'_>> = wanted
        .iter()
        .map(|(object, source)| Wanted {
            object: crate::objects::Needle::strict(object),
            source,
        })
        .collect();
    let names: Vec<&str> = wanted.iter().map(|(object, _)| *object).collect();
    match replace_matching(data, &strict, appender.as_deref_mut()) {
        Ok(replaced) => return Ok(replaced),
        Err(PackageError::NoSuchObject(_)) => {}
        Err(error) => return Err(error),
    }
    let loose: Vec<Wanted<'_>> = wanted
        .iter()
        .map(|(object, source)| Wanted {
            object: crate::objects::Needle::loose(object),
            source,
        })
        .collect();
    replace_matching(data, &loose, appender).map_err(|error| match error {
        PackageError::NoSuchObject(_) => PackageError::NoSuchObject(names.join("`, `")),
        other => other,
    })
}

fn replace_matching(
    data: &[u8],
    wanted: &[Wanted<'_>],
    mut appender: Option<&mut crate::texture::CacheAppender>,
) -> Result<Replaced> {
    let mut out = Vec::with_capacity(data.len());
    let mut textures = Vec::new();
    let mut bundle = Bundle::new(data);
    let mut unreadable = None;
    let mut start = 0usize;
    while let Some(package) = bundle.next() {
        let end = bundle.offset().min(data.len());
        let package = match package {
            Ok(package) => package,
            Err(error) => {
                unreadable = Some(error);
                break;
            }
        };
        match patch(&package, wanted, appender.as_deref_mut())? {
            Some((bytes, paths)) => {
                out.extend_from_slice(&bytes);
                textures.extend(paths);
            }
            None => out.extend_from_slice(&data[start..end]),
        }
        start = end;
    }
    out.extend_from_slice(&data[start.min(data.len())..]);
    if textures.is_empty() {
        return Err(match unreadable {
            Some(error) => error,
            None => PackageError::NoSuchObject("no matching Texture2D".into()),
        });
    }
    Ok(Replaced {
        bytes: out,
        textures,
    })
}

pub fn replace_textures(data: &[u8], wanted: &[(&str, &[u8])]) -> Result<Replaced> {
    replace_textures_into(data, wanted, None)
}

pub fn replace_texture(data: &[u8], object: &str, source: &[u8]) -> Result<Replaced> {
    replace_textures(data, &[(object, source)])
}

fn patch_sounds(
    package: &crate::package::Package<'_>,
    wanted: &[Wanted<'_>],
) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut overrides: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut sounds = Vec::new();
    for (export_index, export) in package.exports.iter().enumerate() {
        if package.export_class(export) != "SoundNodeWave" {
            continue;
        }
        let path = package.export_path(export_index);
        let Some(entry) = wanted.iter().find(|entry| entry.object.matches(&path)) else {
            continue;
        };
        let wave = crate::sound::SoundNodeWave::parse(package, export)?;
        let blob = package.export_data(export)?;
        let base = export.serial_offset.max(0) as usize;
        overrides.insert(export_index, wave.replace(blob, base, entry.source)?);
        sounds.push(path);
    }
    if overrides.is_empty() {
        return Ok(None);
    }
    Ok(Some((crate::writer::rebuild(package, &overrides)?, sounds)))
}

fn replace_sounds_matching(data: &[u8], wanted: &[Wanted<'_>]) -> Result<Replaced> {
    let mut out = Vec::with_capacity(data.len());
    let mut sounds = Vec::new();
    let mut bundle = Bundle::new(data);
    let mut unreadable = None;
    let mut start = 0usize;
    while let Some(package) = bundle.next() {
        let end = bundle.offset().min(data.len());
        let package = match package {
            Ok(package) => package,
            Err(error) => {
                unreadable = Some(error);
                break;
            }
        };
        match patch_sounds(&package, wanted)? {
            Some((bytes, paths)) => {
                out.extend_from_slice(&bytes);
                sounds.extend(paths);
            }
            None => out.extend_from_slice(&data[start..end]),
        }
        start = end;
    }
    out.extend_from_slice(&data[start.min(data.len())..]);
    if sounds.is_empty() {
        return Err(match unreadable {
            Some(error) => error,
            None => PackageError::NoSuchObject("no matching SoundNodeWave".into()),
        });
    }
    Ok(Replaced {
        bytes: out,
        textures: sounds,
    })
}

pub fn replace_sounds(data: &[u8], wanted: &[(&str, &[u8])]) -> Result<Replaced> {
    let strict: Vec<Wanted<'_>> = wanted
        .iter()
        .map(|(object, source)| Wanted {
            object: crate::objects::Needle::strict(object),
            source,
        })
        .collect();
    match replace_sounds_matching(data, &strict) {
        Err(PackageError::NoSuchObject(_)) => {}
        other => return other,
    }
    let loose: Vec<Wanted<'_>> = wanted
        .iter()
        .map(|(object, source)| Wanted {
            object: crate::objects::Needle::loose(object),
            source,
        })
        .collect();
    replace_sounds_matching(data, &loose)
}

fn patch_blobs(
    package: &crate::package::Package<'_>,
    wanted: &[Wanted<'_>],
) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut overrides: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut objects = Vec::new();
    for (export_index, export) in package.exports.iter().enumerate() {
        let path = package.export_path(export_index);
        let Some(entry) = wanted.iter().find(|entry| entry.object.matches(&path)) else {
            continue;
        };
        package.export_data(export)?;
        overrides.insert(export_index, entry.source.to_vec());
        objects.push(path);
    }
    if overrides.is_empty() {
        return Ok(None);
    }
    Ok(Some((crate::writer::rebuild(package, &overrides)?, objects)))
}

fn replace_blobs_matching(data: &[u8], wanted: &[Wanted<'_>]) -> Result<Replaced> {
    let mut out = Vec::with_capacity(data.len());
    let mut objects = Vec::new();
    let mut bundle = Bundle::new(data);
    let mut unreadable = None;
    let mut start = 0usize;
    while let Some(package) = bundle.next() {
        let end = bundle.offset().min(data.len());
        let package = match package {
            Ok(package) => package,
            Err(error) => {
                unreadable = Some(error);
                break;
            }
        };
        match patch_blobs(&package, wanted)? {
            Some((bytes, paths)) => {
                out.extend_from_slice(&bytes);
                objects.extend(paths);
            }
            None => out.extend_from_slice(&data[start..end]),
        }
        start = end;
    }
    out.extend_from_slice(&data[start.min(data.len())..]);
    if objects.is_empty() {
        return Err(match unreadable {
            Some(error) => error,
            None => PackageError::NoSuchObject("no matching export".into()),
        });
    }
    Ok(Replaced {
        bytes: out,
        textures: objects,
    })
}

pub fn replace_blobs(data: &[u8], wanted: &[(&str, &[u8])]) -> Result<Replaced> {
    let strict: Vec<Wanted<'_>> = wanted
        .iter()
        .map(|(object, source)| Wanted {
            object: crate::objects::Needle::strict(object),
            source,
        })
        .collect();
    match replace_blobs_matching(data, &strict) {
        Err(PackageError::NoSuchObject(_)) => {}
        other => return other,
    }
    let loose: Vec<Wanted<'_>> = wanted
        .iter()
        .map(|(object, source)| Wanted {
            object: crate::objects::Needle::loose(object),
            source,
        })
        .collect();
    replace_blobs_matching(data, &loose)
}

fn patch_meshes(
    package: &crate::package::Package<'_>,
    wanted: &[Wanted<'_>],
) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut overrides: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut meshes = Vec::new();
    for (export_index, export) in package.exports.iter().enumerate() {
        if package.export_class(export) != "StaticMesh" {
            continue;
        }
        let path = package.export_path(export_index);
        let Some(entry) = wanted.iter().find(|entry| entry.object.matches(&path)) else {
            continue;
        };
        let mesh = crate::mesh::parse_static_mesh(package, export)
            .ok_or_else(|| PackageError::NoPayload(path.clone()))?;
        let text = std::str::from_utf8(entry.source)
            .map_err(|_| PackageError::UnsupportedProperty("obj must be utf-8 text".into()))?;
        let positions = crate::mesh::read_obj_vertices(text);
        let blob = package.export_data(export)?;
        overrides.insert(export_index, mesh.replace_vertices(blob, &positions)?);
        meshes.push(path);
    }
    if overrides.is_empty() {
        return Ok(None);
    }
    Ok(Some((crate::writer::rebuild(package, &overrides)?, meshes)))
}

fn replace_meshes_matching(data: &[u8], wanted: &[Wanted<'_>]) -> Result<Replaced> {
    let mut out = Vec::with_capacity(data.len());
    let mut meshes = Vec::new();
    let mut bundle = Bundle::new(data);
    let mut unreadable = None;
    let mut start = 0usize;
    while let Some(package) = bundle.next() {
        let end = bundle.offset().min(data.len());
        let package = match package {
            Ok(package) => package,
            Err(error) => {
                unreadable = Some(error);
                break;
            }
        };
        match patch_meshes(&package, wanted)? {
            Some((bytes, paths)) => {
                out.extend_from_slice(&bytes);
                meshes.extend(paths);
            }
            None => out.extend_from_slice(&data[start..end]),
        }
        start = end;
    }
    out.extend_from_slice(&data[start.min(data.len())..]);
    if meshes.is_empty() {
        return Err(match unreadable {
            Some(error) => error,
            None => PackageError::NoSuchObject("no matching StaticMesh".into()),
        });
    }
    Ok(Replaced {
        bytes: out,
        textures: meshes,
    })
}

pub fn replace_meshes(data: &[u8], wanted: &[(&str, &[u8])]) -> Result<Replaced> {
    let strict: Vec<Wanted<'_>> = wanted
        .iter()
        .map(|(object, source)| Wanted {
            object: crate::objects::Needle::strict(object),
            source,
        })
        .collect();
    match replace_meshes_matching(data, &strict) {
        Err(PackageError::NoSuchObject(_)) => {}
        other => return other,
    }
    let loose: Vec<Wanted<'_>> = wanted
        .iter()
        .map(|(object, source)| Wanted {
            object: crate::objects::Needle::loose(object),
            source,
        })
        .collect();
    replace_meshes_matching(data, &loose)
}
