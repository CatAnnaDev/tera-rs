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

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

pub fn mesh_texture_index(package: &Package, mesh_leaf: &str, keywords: &[&str]) -> Option<usize> {
    let target = mesh_leaf.to_ascii_lowercase();
    let stem = texture_stem(mesh_leaf);
    if target.len() < 3 {
        return None;
    }
    let mut best: Option<(usize, i32)> = None;
    for (index, export) in package.exports.iter().enumerate() {
        if package.export_class(export) != "Texture2D" {
            continue;
        }
        let path = package.export_path(index);
        let leaf = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let prefix = common_prefix_len(&leaf, &target);
        let related = prefix >= 5 || (stem.len() >= 3 && leaf.contains(&stem));
        if !related {
            continue;
        }
        let mut keyword_score = 0;
        for (rank, keyword) in keywords.iter().enumerate() {
            if leaf.contains(keyword) {
                keyword_score = 100 - rank as i32;
                break;
            }
        }
        if keyword_score == 0 {
            if NON_BASE.iter().any(|keyword| leaf.contains(keyword)) {
                continue;
            }
            keyword_score = 10;
        }
        let mut score = keyword_score * 1000 + prefix as i32;
        if leaf.contains("lod") {
            score -= 5000;
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
const EMISSIVE_PARAMS: [&str; 4] = ["EmissiveMap", "Emissive", "EmissiveColor", "GlowMap"];
const SPECULAR_PARAMS: [&str; 3] = ["SpecularMap", "Specular", "SpecMap"];

pub fn mesh_material_inputs(package: &Package, mesh: &Mesh, cooked: &Path) -> Vec<MaterialInput> {
    mesh.material_refs
        .iter()
        .map(|reference| resolve_material(package, *reference, cooked))
        .collect()
}

pub fn mesh_materials_or_diffuse(
    package: &Package,
    mesh: &Mesh,
    mesh_leaf: &str,
    cooked: &Path,
) -> Vec<MaterialInput> {
    let mut materials = mesh_material_inputs(package, mesh, cooked);
    let fallback = mesh_material_by_name(package, mesh_leaf, cooked);
    if materials.is_empty() {
        return vec![fallback];
    }
    for material in &mut materials {
        if material.diffuse.is_none() {
            material.diffuse = fallback.diffuse.clone();
            material.alpha_mask = fallback.alpha_mask;
        }
        if material.normal.is_none() {
            material.normal = fallback.normal.clone();
        }
        if material.emissive.is_none() {
            material.emissive = fallback.emissive.clone();
        }
        if material.specular.is_none() {
            material.specular = fallback.specular.clone();
        }
    }
    materials
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
        if let Some((width, height, rgba)) = decode_rgba_by_leaf(package, &leaf, cooked) {
            input.alpha_mask = has_alpha(&rgba);
            if let Ok(png) = png::encode(&rgba, width, height) {
                input.diffuse = Some((width, height, png));
            }
        }
    }
    if let Some(leaf) = parameter_texture(&parameters, &NORMAL_PARAMS) {
        if let Some((width, height, rgba)) = decode_rgba_by_leaf(package, &leaf, cooked) {
            if let Ok(png) = png::encode(&rgba, width, height) {
                input.normal = Some((width, height, png));
            }
        }
    }
    if let Some(leaf) = parameter_texture(&parameters, &EMISSIVE_PARAMS) {
        if let Some((width, height, rgba)) = decode_rgba_by_leaf(package, &leaf, cooked) {
            if let Ok(png) = png::encode(&rgba, width, height) {
                input.emissive = Some((width, height, png));
            }
        }
    }
    if let Some(leaf) = parameter_texture(&parameters, &SPECULAR_PARAMS) {
        if let Some((width, height, rgba)) = decode_rgba_by_leaf(package, &leaf, cooked) {
            if let Ok(png) = png::encode(&rgba, width, height) {
                input.specular = Some((width, height, png));
            }
        }
    }
    input
}

fn encode_map(package: &Package, index: usize, cooked: &Path) -> Option<(u32, u32, bool, Vec<u8>)> {
    let (width, height, rgba) = decode_texture_at(package, index, cooked)?;
    let alpha = has_alpha(&rgba);
    let png = png::encode(&rgba, width, height).ok()?;
    Some((width, height, alpha, png))
}

pub fn mesh_material_by_name(package: &Package, mesh_leaf: &str, cooked: &Path) -> MaterialInput {
    let mut input = MaterialInput::default();
    let stem = texture_stem(mesh_leaf);
    if stem.len() < 3 {
        return input;
    }
    let pick = |keywords: &[&str]| -> Option<usize> { mesh_texture_index(package, mesh_leaf, keywords) };
    if let Some(i) = pick(&DIFFUSE_KEYWORDS) {
        if let Some((w, h, alpha, png)) = encode_map(package, i, cooked) {
            input.diffuse = Some((w, h, png));
            input.alpha_mask = alpha;
        }
    }
    if let Some(i) = pick(&["norm", "normal", "bump"]) {
        if let Some((w, h, _, png)) = encode_map(package, i, cooked) {
            input.normal = Some((w, h, png));
        }
    }
    if let Some(i) = pick(&["emis", "glow", "emissive"]) {
        if let Some((w, h, _, png)) = encode_map(package, i, cooked) {
            input.emissive = Some((w, h, png));
        }
    }
    if let Some(i) = pick(&["spec", "specular"]) {
        if let Some((w, h, _, png)) = encode_map(package, i, cooked) {
            input.specular = Some((w, h, png));
        }
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

fn decode_rgba_by_leaf(package: &Package, leaf: &str, cooked: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let target = leaf.to_ascii_lowercase();
    let index = package.exports.iter().enumerate().find(|(position, export)| {
        package.export_class(export) == "Texture2D"
            && package
                .export_path(*position)
                .rsplit('.')
                .next()
                .map_or(false, |candidate| candidate.eq_ignore_ascii_case(&target))
    })?;
    decode_texture_at(package, index.0, cooked)
}

fn has_alpha(rgba: &[u8]) -> bool {
    let pixels = rgba.len() / 4;
    if pixels == 0 {
        return false;
    }
    let transparent = rgba.chunks_exact(4).filter(|pixel| pixel[3] < 100).count();
    transparent * 100 >= pixels * 3
}
