use crate::error::{PackageError, Result};
use crate::package::{Bundle, Package};
use crate::properties::{read_export_properties, Property};
use std::collections::BTreeMap;

pub struct Found {
    pub path: String,
    pub class: String,
    pub properties: Vec<Property>,
}

pub struct Edited {
    pub bytes: Vec<u8>,
    pub objects: Vec<String>,
}

#[derive(Clone)]
pub struct Needle {
    text: String,
    strict: bool,
}

impl Needle {
    pub fn strict(text: &str) -> Self {
        Self {
            text: text.to_ascii_lowercase(),
            strict: true,
        }
    }

    pub fn loose(text: &str) -> Self {
        Self {
            text: text.to_ascii_lowercase(),
            strict: false,
        }
    }

    pub fn matches(&self, path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        if self.text.contains('.') {
            return lower == self.text || lower.ends_with(&self.text);
        }
        if self.strict {
            return lower.rsplit('.').next() == Some(self.text.as_str());
        }
        lower.contains(&self.text)
    }
}

fn matches(package: &Package<'_>, export_index: usize, needle: &Needle) -> Option<String> {
    let path = package.export_path(export_index);
    needle.matches(&path).then_some(path)
}

pub fn find(data: &[u8], object: &str) -> Result<Vec<Found>> {
    let strict = find_with(data, &Needle::strict(object))?;
    if strict.is_empty() {
        return find_with(data, &Needle::loose(object));
    }
    Ok(strict)
}

fn find_with(data: &[u8], needle: &Needle) -> Result<Vec<Found>> {
    let mut found = Vec::new();
    for package in Bundle::new(data) {
        let Ok(package) = package else {
            break;
        };
        for (export_index, export) in package.exports.iter().enumerate() {
            let Some(path) = matches(&package, export_index, needle) else {
                continue;
            };
            let Ok(blob) = package.export_data(export) else {
                continue;
            };
            let Ok((properties, _)) = read_export_properties(&package, blob) else {
                continue;
            };
            found.push(Found {
                path,
                class: package.export_class(export),
                properties,
            });
        }
    }
    Ok(found)
}

fn patch(
    package: &Package<'_>,
    needle: &Needle,
    set: &[(String, String)],
) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut overrides: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    let mut objects = Vec::new();
    for (export_index, export) in package.exports.iter().enumerate() {
        let Some(path) = matches(package, export_index, needle) else {
            continue;
        };
        let base = export.serial_offset.max(0) as usize;
        let mut patched = package.export_data(export)?.to_vec();
        let payloads_from = read_export_properties(package, &patched)
            .map(|(_, consumed)| consumed)
            .unwrap_or(0);
        let mut edits = 0;
        for (name, literal) in set {
            let (properties, _) = read_export_properties(package, &patched)?;
            let property = properties
                .iter()
                .find(|property| property.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| {
                    PackageError::UnsupportedProperty(format!("{path} has no `{name}`"))
                })?
                .clone();
            if property.resizes() {
                let next = property.rewrite(package, &patched, literal)?;
                let delta = next.len() as i64 - patched.len() as i64;
                let previous = std::mem::replace(&mut patched, next);
                crate::writer::shift_bulk_offsets(
                    &previous,
                    &mut patched,
                    base,
                    property.offset,
                    delta,
                    payloads_from,
                );
            } else {
                property.set(&mut patched, literal)?;
            }
            edits += 1;
        }
        if edits > 0 {
            overrides.insert(export_index, patched);
            objects.push(path);
        }
    }
    if overrides.is_empty() {
        return Ok(None);
    }
    Ok(Some((crate::writer::rebuild(package, &overrides)?, objects)))
}

pub fn set_properties(data: &[u8], object: &str, set: &[(String, String)]) -> Result<Edited> {
    match set_properties_with(data, &Needle::strict(object), set) {
        Err(PackageError::NoSuchObject(_)) => {
            set_properties_with(data, &Needle::loose(object), set)
        }
        other => other,
    }
}

fn set_properties_with(data: &[u8], needle: &Needle, set: &[(String, String)]) -> Result<Edited> {
    let mut out = Vec::with_capacity(data.len());
    let mut objects = Vec::new();
    let mut bundle = Bundle::new(data);
    let mut start = 0usize;
    while let Some(package) = bundle.next() {
        let end = bundle.offset().min(data.len());
        let Ok(package) = package else {
            break;
        };
        match patch(&package, needle, set)? {
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
        return Err(PackageError::NoSuchObject(needle.text.clone()));
    }
    Ok(Edited { bytes: out, objects })
}

#[cfg(test)]
mod tests {
    use super::Needle;

    #[test]
    fn a_bare_name_matches_only_the_last_segment() {
        let needle = Needle::strict("AnimSequence_45_1");
        assert!(needle.matches("pack.Anim.AnimSequence_45_1"));
        assert!(!needle.matches("pack.Anim.AnimSequence_45_1.Compression_91"));
    }

    #[test]
    fn a_loose_needle_still_matches_anywhere() {
        let needle = Needle::loose("AnimSequence_45_1");
        assert!(needle.matches("pack.Anim.AnimSequence_45_1.Compression_91"));
    }

    #[test]
    fn a_dotted_needle_matches_a_path_suffix() {
        let needle = Needle::strict("Tex.TestTex00_Norm");
        assert!(needle.matches("pack.S1_MI.Tex.TestTex00_Norm"));
        assert!(!needle.matches("pack.S1_MI.Tex.TestTex00_Diff"));
    }

    #[test]
    fn matching_ignores_case() {
        assert!(Needle::strict("charwindow_i1").matches("pack.CharWindow_I1"));
    }
}
