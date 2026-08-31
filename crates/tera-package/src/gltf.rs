use crate::mesh::{Mesh, Skin};
use serde_json::json;

const MAGIC_GLTF: u32 = 0x4654_6C67;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

#[derive(Clone, Default)]
pub struct MaterialInput {
    pub diffuse: Option<(u32, u32, Vec<u8>)>,
    pub normal: Option<(u32, u32, Vec<u8>)>,
}

pub fn write_glb(mesh: &Mesh, name: &str, texture: Option<(u32, u32, &[u8])>) -> Vec<u8> {
    let material = MaterialInput {
        diffuse: texture.map(|(width, height, bytes)| (width, height, bytes.to_vec())),
        normal: None,
    };
    write_glb_multi(mesh, name, &[material])
}

pub fn write_glb_multi(mesh: &Mesh, name: &str, materials: &[MaterialInput]) -> Vec<u8> {
    let skinned = matches!(&mesh.skin, Some(skin)
        if !skin.bones.is_empty() && skin.joints.len() == mesh.vertices.len() && !mesh.vertices.is_empty());
    build_glb(mesh, name, materials, skinned)
}

fn z_up_to_y_up(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

const ROOT_ROTATION: [f32; 4] = [-0.707_106_77, 0.0, 0.0, 0.707_106_77];

fn build_glb(mesh: &Mesh, name: &str, materials: &[MaterialInput], skinned: bool) -> Vec<u8> {
    let positions: Vec<[f32; 3]> = if skinned {
        mesh.vertices.clone()
    } else {
        mesh.vertices.iter().map(|v| z_up_to_y_up(*v)).collect()
    };
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

    let mut attributes = json!({ "POSITION": position_accessor, "NORMAL": normal_accessor });
    if let Some(accessor) = uv_accessor {
        attributes["TEXCOORD_0"] = json!(accessor);
    }
    if skinned {
        let skin = mesh.skin.as_ref().unwrap();
        attributes["JOINTS_0"] = json!(push_joints(&mut bin, &mut views, &mut accessors, &skin.joints));
        attributes["WEIGHTS_0"] = json!(push_weights(&mut bin, &mut views, &mut accessors, &skin.weights));
    }

    let index_view = push_index_view(&mut bin, &mut views, &mesh.indices);
    let sections: Vec<(u16, u32, u32)> = if mesh.sections.is_empty() {
        vec![(0, 0, mesh.indices.len() as u32)]
    } else {
        mesh.sections
            .iter()
            .map(|section| (section.material, section.index_start, section.index_count))
            .collect()
    };

    let mut images: Vec<serde_json::Value> = Vec::new();
    let mut textures: Vec<serde_json::Value> = Vec::new();
    let mut materials_json: Vec<serde_json::Value> = Vec::new();
    for material in materials {
        let mut entry = json!({
            "name": name,
            "pbrMetallicRoughness": { "metallicFactor": 0.0, "roughnessFactor": 1.0 },
            "doubleSided": true,
        });
        if has_uv {
            if let Some((_, _, png)) = &material.diffuse {
                let view = push_bytes(&mut bin, &mut views, png);
                let image = images.len();
                images.push(json!({ "bufferView": view, "mimeType": "image/png" }));
                let texture = textures.len();
                textures.push(json!({ "source": image, "sampler": 0 }));
                entry["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": texture });
            }
            if let Some((_, _, png)) = &material.normal {
                let view = push_bytes(&mut bin, &mut views, png);
                let image = images.len();
                images.push(json!({ "bufferView": view, "mimeType": "image/png" }));
                let texture = textures.len();
                textures.push(json!({ "source": image, "sampler": 0 }));
                entry["normalTexture"] = json!({ "index": texture });
            }
        }
        materials_json.push(entry);
    }
    if materials_json.is_empty() {
        materials_json.push(json!({
            "name": name,
            "pbrMetallicRoughness": { "metallicFactor": 0.0, "roughnessFactor": 1.0 },
            "doubleSided": true,
        }));
    }

    let primitives: Vec<serde_json::Value> = sections
        .iter()
        .map(|(material, start, len)| {
            let accessor = push_index_accessor(&mut accessors, index_view, *start, *len);
            let material = (*material as usize).min(materials_json.len() - 1);
            json!({ "attributes": attributes, "indices": accessor, "material": material })
        })
        .collect();

    let mut nodes: Vec<serde_json::Value> = Vec::new();
    let scene_root;
    let mut skins_json: Vec<serde_json::Value> = Vec::new();
    if skinned {
        let skin = mesh.skin.as_ref().unwrap();
        let inverse_binds = inverse_bind_matrices(skin);
        let inverse_accessor = push_matrices(&mut bin, &mut views, &mut accessors, &inverse_binds);
        let bone_count = skin.bones.len();
        let mesh_node = bone_count;
        let root_node = bone_count + 1;
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); bone_count];
        let mut roots: Vec<usize> = Vec::new();
        for (index, bone) in skin.bones.iter().enumerate() {
            let parent = bone.parent;
            if parent < 0 || parent as usize >= bone_count || parent as usize == index {
                roots.push(index);
            } else {
                children[parent as usize].push(index);
            }
        }
        for (index, bone) in skin.bones.iter().enumerate() {
            let mut node = json!({ "translation": bone.translation, "rotation": normalize_quat(bone.rotation) });
            if !bone.name.is_empty() {
                node["name"] = json!(bone.name);
            }
            if !children[index].is_empty() {
                node["children"] = json!(children[index]);
            }
            nodes.push(node);
        }
        nodes.push(json!({ "mesh": 0, "skin": 0, "name": name }));
        let mut root_children = vec![mesh_node];
        root_children.extend(roots);
        nodes.push(json!({ "rotation": ROOT_ROTATION, "children": root_children, "name": "root" }));
        let joints: Vec<usize> = (0..bone_count).collect();
        skins_json.push(json!({ "joints": joints, "inverseBindMatrices": inverse_accessor, "skeleton": root_node }));
        scene_root = root_node;
    } else {
        nodes.push(json!({ "mesh": 0, "name": name }));
        scene_root = 0;
    }

    let mut samplers: Vec<serde_json::Value> = Vec::new();
    if !textures.is_empty() {
        samplers.push(json!({ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }));
    }

    let mut doc = json!({
        "asset": { "version": "2.0", "generator": "tera-package" },
        "scene": 0,
        "scenes": [{ "nodes": [scene_root] }],
        "nodes": nodes,
        "meshes": [{ "name": name, "primitives": primitives }],
        "materials": materials_json,
        "buffers": [{ "byteLength": bin.len() }],
        "bufferViews": views,
        "accessors": accessors,
    });
    if !skins_json.is_empty() {
        doc["skins"] = json!(skins_json);
    }
    if !images.is_empty() {
        doc["images"] = json!(images);
        doc["textures"] = json!(textures);
        doc["samplers"] = json!(samplers);
    }

    assemble(doc, bin)
}

fn push_index_view(bin: &mut Vec<u8>, views: &mut Vec<serde_json::Value>, indices: &[u32]) -> usize {
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let offset = bin.len();
    for value in indices {
        bin.extend_from_slice(&value.to_le_bytes());
    }
    let view = views.len();
    views.push(json!({ "buffer": 0, "byteOffset": offset, "byteLength": indices.len() * 4, "target": 34963 }));
    view
}

fn push_index_accessor(accessors: &mut Vec<serde_json::Value>, view: usize, start: u32, len: u32) -> usize {
    let index = accessors.len();
    accessors.push(json!({
        "bufferView": view,
        "byteOffset": start as usize * 4,
        "componentType": 5125,
        "count": len,
        "type": "SCALAR",
    }));
    index
}

fn assemble(doc: serde_json::Value, mut bin: Vec<u8>) -> Vec<u8> {
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

fn normalize_quat(q: [f32; 4]) -> [f32; 4] {
    let length = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if length > 1e-8 {
        [q[0] / length, q[1] / length, q[2] / length, q[3] / length]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

fn quat_to_mat3(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [x, y, z, w] = normalize_quat(q);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

fn mat3_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = a[row][0] * b[0][col] + a[row][1] * b[1][col] + a[row][2] * b[2][col];
        }
    }
    out
}

fn mat3_apply(m: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn inverse_bind_matrices(skin: &Skin) -> Vec<[f32; 16]> {
    let count = skin.bones.len();
    let mut global_rotation = vec![[[0.0f32; 3]; 3]; count];
    let mut global_translation = vec![[0.0f32; 3]; count];
    for (index, bone) in skin.bones.iter().enumerate() {
        let local_rotation = quat_to_mat3(bone.rotation);
        let local_translation = bone.translation;
        let parent = bone.parent;
        if parent >= 0 && (parent as usize) < index {
            let parent = parent as usize;
            global_rotation[index] = mat3_mul(global_rotation[parent], local_rotation);
            let rotated = mat3_apply(global_rotation[parent], local_translation);
            global_translation[index] = [
                rotated[0] + global_translation[parent][0],
                rotated[1] + global_translation[parent][1],
                rotated[2] + global_translation[parent][2],
            ];
        } else {
            global_rotation[index] = local_rotation;
            global_translation[index] = local_translation;
        }
    }
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let rotation = global_rotation[index];
        let translation = global_translation[index];
        let inverse_rotation = [
            [rotation[0][0], rotation[1][0], rotation[2][0]],
            [rotation[0][1], rotation[1][1], rotation[2][1]],
            [rotation[0][2], rotation[1][2], rotation[2][2]],
        ];
        let inverse_translation = mat3_apply(inverse_rotation, translation);
        out.push([
            inverse_rotation[0][0],
            inverse_rotation[1][0],
            inverse_rotation[2][0],
            0.0,
            inverse_rotation[0][1],
            inverse_rotation[1][1],
            inverse_rotation[2][1],
            0.0,
            inverse_rotation[0][2],
            inverse_rotation[1][2],
            inverse_rotation[2][2],
            0.0,
            -inverse_translation[0],
            -inverse_translation[1],
            -inverse_translation[2],
            1.0,
        ]);
    }
    out
}

fn push_joints(
    bin: &mut Vec<u8>,
    views: &mut Vec<serde_json::Value>,
    accessors: &mut Vec<serde_json::Value>,
    data: &[[u16; 4]],
) -> usize {
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let offset = bin.len();
    for joint in data {
        for value in joint {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    let view = views.len();
    views.push(json!({ "buffer": 0, "byteOffset": offset, "byteLength": data.len() * 8, "target": 34962 }));
    let index = accessors.len();
    accessors.push(json!({
        "bufferView": view,
        "componentType": 5123,
        "count": data.len(),
        "type": "VEC4",
    }));
    index
}

fn push_weights(
    bin: &mut Vec<u8>,
    views: &mut Vec<serde_json::Value>,
    accessors: &mut Vec<serde_json::Value>,
    data: &[[f32; 4]],
) -> usize {
    let offset = bin.len();
    for weight in data {
        for value in weight {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    let view = views.len();
    views.push(json!({ "buffer": 0, "byteOffset": offset, "byteLength": data.len() * 16, "target": 34962 }));
    let index = accessors.len();
    accessors.push(json!({
        "bufferView": view,
        "componentType": 5126,
        "count": data.len(),
        "type": "VEC4",
    }));
    index
}

fn push_matrices(
    bin: &mut Vec<u8>,
    views: &mut Vec<serde_json::Value>,
    accessors: &mut Vec<serde_json::Value>,
    data: &[[f32; 16]],
) -> usize {
    let offset = bin.len();
    for matrix in data {
        for value in matrix {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    let view = views.len();
    views.push(json!({ "buffer": 0, "byteOffset": offset, "byteLength": data.len() * 64 }));
    let index = accessors.len();
    accessors.push(json!({
        "bufferView": view,
        "componentType": 5126,
        "count": data.len(),
        "type": "MAT4",
    }));
    index
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
