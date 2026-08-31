use std::collections::HashMap;
use tera_package::mesh::Mesh;
use tera_package::Animation;

type Mat3 = [[f32; 3]; 3];

fn quat_to_mat3(q: [f32; 4]) -> Mat3 {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let (x, y, z, w) = if len > 1e-8 {
        (q[0] / len, q[1] / len, q[2] / len, q[3] / len)
    } else {
        (0.0, 0.0, 0.0, 1.0)
    };
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
        [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
        [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

fn mat3_mul(a: Mat3, b: Mat3) -> Mat3 {
    let mut out = [[0.0f32; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            out[row][col] = a[row][0] * b[0][col] + a[row][1] * b[1][col] + a[row][2] * b[2][col];
        }
    }
    out
}

fn mat3_apply(m: Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn transpose(m: Mat3) -> Mat3 {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}

fn sample_vec3(keys: &[[f32; 3]], time: f32, duration: f32) -> [f32; 3] {
    if keys.len() == 1 {
        return keys[0];
    }
    let last = (keys.len() - 1) as f32;
    let position = if duration > 0.0 { (time / duration).clamp(0.0, 1.0) * last } else { 0.0 };
    let index = position.floor() as usize;
    let next = (index + 1).min(keys.len() - 1);
    let fraction = position - index as f32;
    let a = keys[index];
    let b = keys[next];
    [
        a[0] + (b[0] - a[0]) * fraction,
        a[1] + (b[1] - a[1]) * fraction,
        a[2] + (b[2] - a[2]) * fraction,
    ]
}

fn sample_quat(keys: &[[f32; 4]], time: f32, duration: f32) -> [f32; 4] {
    if keys.len() == 1 {
        return keys[0];
    }
    let last = (keys.len() - 1) as f32;
    let position = if duration > 0.0 { (time / duration).clamp(0.0, 1.0) * last } else { 0.0 };
    let index = position.floor() as usize;
    let next = (index + 1).min(keys.len() - 1);
    let fraction = position - index as f32;
    let a = keys[index];
    let mut b = keys[next];
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if dot < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
    }
    let mut result = [
        a[0] + (b[0] - a[0]) * fraction,
        a[1] + (b[1] - a[1]) * fraction,
        a[2] + (b[2] - a[2]) * fraction,
        a[3] + (b[3] - a[3]) * fraction,
    ];
    let len = (result[0] * result[0] + result[1] * result[1] + result[2] * result[2] + result[3] * result[3]).sqrt();
    if len > 1e-8 {
        for value in &mut result {
            *value /= len;
        }
    } else {
        result = [0.0, 0.0, 0.0, 1.0];
    }
    result
}

pub fn pose_vertices(mesh: &Mesh, animation: &Animation, time: f32) -> Option<Vec<[f32; 3]>> {
    let skin = mesh.skin.as_ref()?;
    let count = skin.bones.len();
    if count == 0 || skin.joints.len() != mesh.vertices.len() {
        return None;
    }

    let mut local_translation: Vec<[f32; 3]> = skin.bones.iter().map(|bone| bone.translation).collect();
    let mut local_rotation: Vec<[f32; 4]> = skin.bones.iter().map(|bone| bone.rotation).collect();

    let index_of: HashMap<&str, usize> = skin
        .bones
        .iter()
        .enumerate()
        .map(|(index, bone)| (bone.name.as_str(), index))
        .collect();
    for track in &animation.tracks {
        if let Some(&index) = index_of.get(track.bone.as_str()) {
            if !track.translations.is_empty() {
                local_translation[index] = sample_vec3(&track.translations, time, animation.duration);
            }
            if !track.rotations.is_empty() {
                local_rotation[index] = sample_quat(&track.rotations, time, animation.duration);
            }
        }
    }

    let bind = global_transforms(skin, &default_translation(skin), &default_rotation(skin));
    let posed = global_transforms(skin, &local_translation, &local_rotation);

    let mut skin_rotation = vec![[[0.0f32; 3]; 3]; count];
    let mut skin_translation = vec![[0.0f32; 3]; count];
    for bone in 0..count {
        let inverse_bind_rotation = transpose(bind[bone].0);
        let inverse_bind_translation = {
            let t = mat3_apply(inverse_bind_rotation, bind[bone].1);
            [-t[0], -t[1], -t[2]]
        };
        skin_rotation[bone] = mat3_mul(posed[bone].0, inverse_bind_rotation);
        let rotated = mat3_apply(posed[bone].0, inverse_bind_translation);
        skin_translation[bone] = [
            rotated[0] + posed[bone].1[0],
            rotated[1] + posed[bone].1[1],
            rotated[2] + posed[bone].1[2],
        ];
    }

    let mut out = Vec::with_capacity(mesh.vertices.len());
    for (vertex, (joints, weights)) in mesh.vertices.iter().zip(skin.joints.iter().zip(skin.weights.iter())) {
        let mut accumulated = [0.0f32; 3];
        for slot in 0..4 {
            let weight = weights[slot];
            if weight == 0.0 {
                continue;
            }
            let bone = joints[slot] as usize;
            if bone >= count {
                continue;
            }
            let rotated = mat3_apply(skin_rotation[bone], *vertex);
            accumulated[0] += weight * (rotated[0] + skin_translation[bone][0]);
            accumulated[1] += weight * (rotated[1] + skin_translation[bone][1]);
            accumulated[2] += weight * (rotated[2] + skin_translation[bone][2]);
        }
        out.push(accumulated);
    }
    Some(out)
}

fn default_translation(skin: &tera_package::mesh::Skin) -> Vec<[f32; 3]> {
    skin.bones.iter().map(|bone| bone.translation).collect()
}

fn default_rotation(skin: &tera_package::mesh::Skin) -> Vec<[f32; 4]> {
    skin.bones.iter().map(|bone| bone.rotation).collect()
}

fn global_transforms(
    skin: &tera_package::mesh::Skin,
    translation: &[[f32; 3]],
    rotation: &[[f32; 4]],
) -> Vec<(Mat3, [f32; 3])> {
    let count = skin.bones.len();
    let mut global = vec![([[0.0f32; 3]; 3], [0.0f32; 3]); count];
    for index in 0..count {
        let local_rotation = quat_to_mat3(rotation[index]);
        let local_translation = translation[index];
        let parent = skin.bones[index].parent;
        if parent >= 0 && (parent as usize) < index {
            let parent = parent as usize;
            let (parent_rotation, parent_translation) = global[parent];
            let combined = mat3_mul(parent_rotation, local_rotation);
            let rotated = mat3_apply(parent_rotation, local_translation);
            global[index] = (
                combined,
                [
                    rotated[0] + parent_translation[0],
                    rotated[1] + parent_translation[1],
                    rotated[2] + parent_translation[2],
                ],
            );
        } else {
            global[index] = (local_rotation, local_translation);
        }
    }
    global
}
