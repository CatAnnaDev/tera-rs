use crate::mesh::Mesh;
use serde_json::json;

const MAGIC_GLTF: u32 = 0x4654_6C67;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

pub fn write_glb(mesh: &Mesh, name: &str, texture: Option<(u32, u32, &[u8])>) -> Vec<u8> {
    let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| z_up_to_y_up(*v)).collect();
    let count = positions.len();
    let normals = compute_normals(&positions, &mesh.indices);
    let has_uv = mesh.uvs.len() == count && count > 0;

    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<serde_json::Value> = Vec::new();
    let mut accessors: Vec<serde_json::Value> = Vec::new();

    let (min, max) = bounds_of(&positions);
    let position_accessor = push_vec3(&mut bin, &mut views, &mut accessors, &positions, Some((min, max)));
    let normal_accessor = push_vec3(&mut bin, &mut views, &mut accessors, &normals, None);
    let uv_accessor = if has_uv {
        Some(push_vec2(&mut bin, &mut views, &mut accessors, &mesh.uvs))
    } else {
        None
    };
    let index_accessor = push_indices(&mut bin, &mut views, &mut accessors, &mesh.indices);

    let mut attributes = json!({
        "POSITION": position_accessor,
        "NORMAL": normal_accessor,
    });
    if let Some(accessor) = uv_accessor {
        attributes["TEXCOORD_0"] = json!(accessor);
    }

    let mut images = Vec::new();
    let mut textures = Vec::new();
    let mut samplers = Vec::new();
    let mut material = json!({
        "name": name,
        "pbrMetallicRoughness": { "metallicFactor": 0.0, "roughnessFactor": 1.0 },
        "doubleSided": true,
    });
    if let (Some((width, height, png)), true) = (texture, has_uv) {
        let view = push_bytes(&mut bin, &mut views, png);
        images.push(json!({ "bufferView": view, "mimeType": "image/png" }));
        samplers.push(json!({ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }));
        textures.push(json!({ "source": 0, "sampler": 0 }));
        material["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": 0 });
        let _ = (width, height);
    }

    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "tera-package" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0, "name": name }],
        "meshes": [{
            "name": name,
            "primitives": [{ "attributes": attributes, "indices": index_accessor, "material": 0 }],
        }],
        "materials": [material],
        "buffers": [{ "byteLength": bin.len() }],
        "bufferViews": views,
        "accessors": accessors,
    });
    if !images.is_empty() {
        doc["images"] = json!(images);
        doc["textures"] = json!(textures);
        doc["samplers"] = json!(samplers);
    }

    let mut json_bytes = serde_json::to_vec(&doc).unwrap_or_default();
    pad(&mut json_bytes, 0x20);
    pad(&mut bin, 0x00);

    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&MAGIC_GLTF.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
    out.extend_from_slice(&bin);
    out
}

fn z_up_to_y_up(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

fn bounds_of(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut low = [f32::MAX; 3];
    let mut high = [f32::MIN; 3];
    for position in positions {
        for axis in 0..3 {
            low[axis] = low[axis].min(position[axis]);
            high[axis] = high[axis].max(position[axis]);
        }
    }
    if positions.is_empty() {
        return ([0.0; 3], [0.0; 3]);
    }
    (low, high)
}

fn compute_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32; 3]; positions.len()];
    for triangle in indices.chunks_exact(3) {
        let (a, b, c) = (
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        );
        if a >= positions.len() || b >= positions.len() || c >= positions.len() {
            continue;
        }
        let edge1 = sub(positions[b], positions[a]);
        let edge2 = sub(positions[c], positions[a]);
        let face = cross(edge1, edge2);
        for &index in &[a, b, c] {
            normals[index][0] += face[0];
            normals[index][1] += face[1];
            normals[index][2] += face[2];
        }
    }
    for normal in &mut normals {
        let length = (normal[0].powi(2) + normal[1].powi(2) + normal[2].powi(2)).sqrt();
        if length > 1e-8 {
            normal[0] /= length;
            normal[1] /= length;
            normal[2] /= length;
        } else {
            *normal = [0.0, 1.0, 0.0];
        }
    }
    normals
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn push_vec3(
    bin: &mut Vec<u8>,
    views: &mut Vec<serde_json::Value>,
    accessors: &mut Vec<serde_json::Value>,
    data: &[[f32; 3]],
    minmax: Option<([f32; 3], [f32; 3])>,
) -> usize {
    let offset = bin.len();
    for value in data {
        for component in value {
            bin.extend_from_slice(&component.to_le_bytes());
        }
    }
    let view = views.len();
    views.push(json!({ "buffer": 0, "byteOffset": offset, "byteLength": data.len() * 12, "target": 34962 }));
    let mut accessor = json!({
        "bufferView": view,
        "componentType": 5126,
        "count": data.len(),
        "type": "VEC3",
    });
    if let Some((min, max)) = minmax {
        accessor["min"] = json!(min);
        accessor["max"] = json!(max);
    }
    let index = accessors.len();
    accessors.push(accessor);
    index
}

fn push_vec2(
    bin: &mut Vec<u8>,
    views: &mut Vec<serde_json::Value>,
    accessors: &mut Vec<serde_json::Value>,
    data: &[[f32; 2]],
) -> usize {
    let offset = bin.len();
    for value in data {
        bin.extend_from_slice(&value[0].to_le_bytes());
        bin.extend_from_slice(&(1.0 - value[1]).to_le_bytes());
    }
    let view = views.len();
    views.push(json!({ "buffer": 0, "byteOffset": offset, "byteLength": data.len() * 8, "target": 34962 }));
    let index = accessors.len();
    accessors.push(json!({
        "bufferView": view,
        "componentType": 5126,
        "count": data.len(),
        "type": "VEC2",
    }));
    index
}

fn push_indices(
    bin: &mut Vec<u8>,
    views: &mut Vec<serde_json::Value>,
    accessors: &mut Vec<serde_json::Value>,
    data: &[u32],
) -> usize {
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let offset = bin.len();
    for value in data {
        bin.extend_from_slice(&value.to_le_bytes());
    }
    let view = views.len();
    views.push(json!({ "buffer": 0, "byteOffset": offset, "byteLength": data.len() * 4, "target": 34963 }));
    let index = accessors.len();
    accessors.push(json!({
        "bufferView": view,
        "componentType": 5125,
        "count": data.len(),
        "type": "SCALAR",
    }));
    index
}

fn push_bytes(bin: &mut Vec<u8>, views: &mut Vec<serde_json::Value>, data: &[u8]) -> usize {
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let offset = bin.len();
    bin.extend_from_slice(data);
    let view = views.len();
    views.push(json!({ "buffer": 0, "byteOffset": offset, "byteLength": data.len() }));
    view
}

fn pad(bytes: &mut Vec<u8>, filler: u8) {
    while bytes.len() % 4 != 0 {
        bytes.push(filler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Mesh;

    #[test]
    fn glb_is_well_formed() {
        let mesh = Mesh {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let glb = write_glb(&mesh, "tri", None);
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(glb[4..8].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
            glb.len()
        );
        assert_eq!(glb.len() % 4, 0);
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(&glb[16..20], b"JSON");
        let doc: serde_json::Value =
            serde_json::from_slice(&glb[20..20 + json_len]).expect("valid json");
        assert!(doc["meshes"].is_array());
        assert_eq!(doc["accessors"].as_array().unwrap().len(), 4);
        assert_eq!(doc["asset"]["version"], "2.0");
        let bin_start = 20 + json_len;
        assert_eq!(&glb[bin_start + 4..bin_start + 8], b"BIN\0");
    }
}
