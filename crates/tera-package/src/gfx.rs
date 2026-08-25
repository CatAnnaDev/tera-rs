use crate::error::{PackageError, Result};
use crate::objects::Needle;
use crate::package::{Bundle, Package};
use crate::properties::{read_export_properties, PropertyValue};
use std::collections::BTreeMap;

pub struct Movie {
    pub path: String,
    pub source_file: String,
    pub data: Vec<u8>,
}

impl Movie {
    pub fn kind(&self) -> &'static str {
        match self.data.get(..3) {
            Some(b"GFX") => "gfx",
            Some(b"CFX") => "gfx compressed",
            Some(b"FWS") => "swf",
            Some(b"CWS") => "swf zlib",
            Some(b"ZWS") => "swf lzma",
            _ => "unknown",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self.kind() {
            "swf" | "swf zlib" | "swf lzma" => "swf",
            _ => "gfx",
        }
    }
}

fn raw_data(package: &Package<'_>, blob: &[u8]) -> Option<(String, Vec<u8>)> {
    let (properties, _) = read_export_properties(package, blob).ok()?;
    let source = properties
        .iter()
        .find(|property| property.name == "SourceFile")
        .and_then(|property| match &property.value {
            PropertyValue::Str(text) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let data = properties
        .iter()
        .find(|property| property.name == "RawData")
        .and_then(|property| match &property.value {
            PropertyValue::Array { raw, .. } => Some(raw.clone()),
            _ => None,
        })?;
    Some((source, data))
}

pub fn movies(data: &[u8]) -> Vec<Movie> {
    let mut found = Vec::new();
    for package in Bundle::new(data) {
        let Ok(package) = package else { break };
        for (index, export) in package.exports.iter().enumerate() {
            if package.export_class(export) != "GFxMovieInfo" {
                continue;
            }
            let Ok(blob) = package.export_data(export) else {
                continue;
            };
            if let Some((source_file, data)) = raw_data(&package, blob) {
                found.push(Movie {
                    path: package.export_path(index),
                    source_file,
                    data,
                });
            }
        }
    }
    found
}

fn patch(
    package: &Package<'_>,
    needle: &Needle,
    movie: &[u8],
) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut overrides: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut replaced = Vec::new();
    for (index, export) in package.exports.iter().enumerate() {
        if package.export_class(export) != "GFxMovieInfo" {
            continue;
        }
        let path = package.export_path(index);
        if !needle.matches(&path) {
            continue;
        }
        let blob = package.export_data(export)?;
        let base = export.serial_offset.max(0) as usize;
        let (properties, payloads_from) = read_export_properties(package, blob)?;
        let property = properties
            .iter()
            .find(|property| property.name == "RawData")
            .ok_or_else(|| PackageError::UnsupportedProperty(format!("{path} has no RawData")))?;
        let mut payload = (movie.len() as i32).to_le_bytes().to_vec();
        payload.extend_from_slice(movie);
        let patched = property.rewrite_payload(package, blob, &payload)?;
        let delta = patched.len() as i64 - blob.len() as i64;
        let mut patched = patched;
        crate::writer::shift_bulk_offsets(
            blob,
            &mut patched,
            base,
            property.offset,
            delta,
            payloads_from,
        );
        overrides.insert(index, patched);
        replaced.push(path);
    }
    if overrides.is_empty() {
        return Ok(None);
    }
    Ok(Some((crate::writer::rebuild(package, &overrides)?, replaced)))
}

fn replace_matching(data: &[u8], needle: &Needle, movie: &[u8]) -> Result<crate::Replaced> {
    let mut out = Vec::with_capacity(data.len());
    let mut replaced = Vec::new();
    let mut bundle = Bundle::new(data);
    let mut start = 0usize;
    while let Some(package) = bundle.next() {
        let end = bundle.offset().min(data.len());
        let Ok(package) = package else { break };
        match patch(&package, needle, movie)? {
            Some((bytes, paths)) => {
                out.extend_from_slice(&bytes);
                replaced.extend(paths);
            }
            None => out.extend_from_slice(&data[start..end]),
        }
        start = end;
    }
    out.extend_from_slice(&data[start.min(data.len())..]);
    if replaced.is_empty() {
        return Err(PackageError::NoSuchObject("no matching GFxMovieInfo".into()));
    }
    Ok(crate::Replaced {
        bytes: out,
        textures: replaced,
    })
}

pub fn replace_movie(data: &[u8], object: &str, movie: &[u8]) -> Result<crate::Replaced> {
    match replace_matching(data, &Needle::strict(object), movie) {
        Err(PackageError::NoSuchObject(_)) => {}
        other => return other,
    }
    replace_matching(data, &Needle::loose(object), movie)
}
