use crate::package::Package;
use crate::texture::{Caches, Texture2D};
use std::path::Path;

pub const DIFFUSE_KEYWORDS: [&str; 3] = ["diff", "base", "color"];
const NON_BASE: [&str; 6] = ["norm", "spec", "mask", "cstm", "emis", "opac"];

pub fn texture_stem(mesh_leaf: &str) -> String {
    let lower = mesh_leaf.to_ascii_lowercase();
    let base = match lower.find("_skel") {
        Some(position) => lower[..position].to_string(),
        None => lower,
    };
    let base = base.trim_end_matches("_dup").to_string();
    match base.rsplit_once('_') {
        Some((head, tail)) if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) => {
            head.to_string()
        }
        _ => base,
    }
}

pub fn mesh_texture_index(package: &Package, mesh_leaf: &str, keywords: &[&str]) -> Option<usize> {
    let stem = texture_stem(mesh_leaf);
    if stem.len() < 3 {
        return None;
    }
    let mut best: Option<(usize, i32)> = None;
    for (index, export) in package.exports.iter().enumerate() {
        if package.export_class(export) != "Texture2D" {
            continue;
        }
        let path = package.export_path(index);
        let leaf = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !leaf.contains(&stem) {
            continue;
        }
        let mut score = 0;
        for (rank, keyword) in keywords.iter().enumerate() {
            if leaf.contains(keyword) {
                score = 100 - rank as i32;
                break;
            }
        }
        if score == 0 {
            if NON_BASE.iter().any(|keyword| leaf.contains(keyword)) {
                continue;
            }
            score = 10;
        }
        if leaf.contains("lod") {
            score -= 20;
        }
        if best.map_or(true, |(_, current)| score > current) {
            best = Some((index, score));
        }
    }
    best.map(|(index, _)| index)
}

pub fn decode_texture_at(
    package: &Package,
    index: usize,
    cooked: &Path,
) -> Option<(u32, u32, Vec<u8>)> {
    let export = package.exports.get(index)?;
    let texture = Texture2D::parse(package, export).ok()?;
    let mut caches = Caches::at(cooked);
    texture.decode_rgba_with(Some(&mut caches)).ok()
}

pub fn mesh_diffuse_rgba(
    package: &Package,
    mesh_leaf: &str,
    cooked: &Path,
) -> Option<(u32, u32, Vec<u8>)> {
    let index = mesh_texture_index(package, mesh_leaf, &DIFFUSE_KEYWORDS)?;
    decode_texture_at(package, index, cooked)
}

use crate::gltf::MaterialInput;
use crate::material::{export_parameters, Kind};
use crate::mesh::Mesh;
use crate::png;

const DIFFUSE_PARAMS: [&str; 4] = ["DiffuseMap", "Diffuse", "BaseMap", "BaseColor"];
const NORMAL_PARAMS: [&str; 3] = ["NormalMap", "Normal", "BumpMap"];

pub fn mesh_material_inputs(package: &Package, mesh: &Mesh, cooked: &Path) -> Vec<MaterialInput> {
    mesh.material_refs
        .iter()
        .map(|reference| resolve_material(package, *reference, cooked))
        .collect()
}

fn resolve_material(package: &Package, reference: i32, cooked: &Path) -> MaterialInput {
    let mut input = MaterialInput::default();
    if reference <= 0 {
        return input;
    }
    let Some(export) = package.exports.get((reference - 1) as usize) else {
        return input;
    };
    let parameters = export_parameters(package, export);
    if let Some(leaf) = parameter_texture(&parameters, &DIFFUSE_PARAMS) {
        input.diffuse = decode_by_leaf(package, &leaf, cooked);
    }
    if let Some(leaf) = parameter_texture(&parameters, &NORMAL_PARAMS) {
        input.normal = decode_by_leaf(package, &leaf, cooked);
    }
    input
}

fn parameter_texture(parameters: &[crate::material::Parameter], names: &[&str]) -> Option<String> {
    for wanted in names {
        if let Some(parameter) = parameters
            .iter()
            .find(|parameter| parameter.kind == Kind::Texture && parameter.name.eq_ignore_ascii_case(wanted))
        {
            let leaf = parameter.value.rsplit('.').next().unwrap_or("");
            if !leaf.is_empty() && !leaf.starts_with('<') {
                return Some(leaf.to_string());
            }
        }
    }
    None
}

fn decode_by_leaf(package: &Package, leaf: &str, cooked: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let target = leaf.to_ascii_lowercase();
    let index = package.exports.iter().enumerate().find(|(position, export)| {
        package.export_class(export) == "Texture2D"
            && package
                .export_path(*position)
                .rsplit('.')
                .next()
                .map_or(false, |candidate| candidate.eq_ignore_ascii_case(&target))
    })?;
    let (width, height, rgba) = decode_texture_at(package, index.0, cooked)?;
    let png = png::encode(&rgba, width, height).ok()?;
    Some((width, height, png))
}
