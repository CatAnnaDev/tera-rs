use crate::error::{PackageError, Result};
use crate::package::{Export, Package};
use crate::properties::read_export_properties;

#[derive(Clone, Debug, Default)]
pub struct Bone {
    pub name: String,
    pub name_index: i32,
    pub parent: i32,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct Skin {
    pub bones: Vec<Bone>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub position_offset: usize,
    pub properties_end: usize,
    pub material_refs: Vec<i32>,
    pub skin: Option<Skin>,
}

impl Mesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut low = [f32::MAX; 3];
        let mut high = [f32::MIN; 3];
        for vertex in &self.vertices {
            for axis in 0..3 {
                low[axis] = low[axis].min(vertex[axis]);
                high[axis] = high[axis].max(vertex[axis]);
            }
        }
        if self.vertices.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        (low, high)
    }

    pub fn to_obj(&self, name: &str) -> String {
        let mut out = String::with_capacity(self.vertices.len() * 32 + self.indices.len() * 12);
        out.push_str(&format!("o {name}\n"));
        for vertex in &self.vertices {
            out.push_str(&format!("v {} {} {}\n", vertex[0], vertex[1], vertex[2]));
        }
        for uv in &self.uvs {
            out.push_str(&format!("vt {} {}\n", uv[0], 1.0 - uv[1]));
        }
        let textured = self.uvs.len() == self.vertices.len();
        for triangle in self.indices.as_chunks::<3>().0 {
            let (a, b, c) = (triangle[0] + 1, triangle[1] + 1, triangle[2] + 1);
            if textured {
                out.push_str(&format!("f {a}/{a} {b}/{b} {c}/{c}\n"));
            } else {
                out.push_str(&format!("f {a} {b} {c}\n"));
            }
        }
        out
    }
}

#[inline]
fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

#[inline]
fn read_f32(data: &[u8], offset: usize) -> f32 {
    f32::from_bits(read_u32(data, offset))
}

fn half_to_f32(value: u16) -> f32 {
    let sign = (value >> 15) & 1;
    let exponent = (value >> 10) & 0x1f;
    let mantissa = value & 0x3ff;
    let magnitude = if exponent == 0 {
        f32::from(mantissa) * 2.0f32.powi(-24)
    } else if exponent == 0x1f {
        if mantissa == 0 {
            f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        (1.0 + f32::from(mantissa) / 1024.0) * 2.0f32.powi(i32::from(exponent) - 15)
    };
    if sign == 1 {
        -magnitude
    } else {
        magnitude
    }
}

fn edge_length(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn filter_spikes(vertices: &[[f32; 3]], indices: &[u32]) -> (Vec<u32>, f32) {
    let triangles: Vec<&[u32; 3]> = indices.as_chunks::<3>().0.iter().collect();
    if triangles.len() < 2 {
        return (indices.to_vec(), 1.0);
    }
    let count = vertices.len();
    let mut edges: Vec<f32> = Vec::with_capacity(triangles.len());
    let mut used: Vec<[f32; 3]> = Vec::with_capacity(triangles.len() * 3);
    for triangle in &triangles {
        if triangle.iter().all(|index| (*index as usize) < count) {
            edges.push(edge_length(
                vertices[triangle[0] as usize],
                vertices[triangle[1] as usize],
            ));
            for index in triangle.iter() {
                used.push(vertices[*index as usize]);
            }
        }
    }
    if edges.is_empty() {
        return (Vec::new(), 0.0);
    }
    edges.sort_by(f32::total_cmp);
    let median = edges[edges.len() / 2].max(1e-3);
    let mut diagonal = 0.0f32;
    for axis in 0..3 {
        let mut values: Vec<f32> = used
            .iter()
            .map(|vertex| vertex[axis])
            .filter(|value| value.is_finite())
            .collect();
        if values.is_empty() {
            continue;
        }
        values.sort_by(f32::total_cmp);
        let high = values[(((values.len() - 1) as f32) * 0.95) as usize];
        let low = values[(((values.len() - 1) as f32) * 0.05) as usize];
        diagonal += (high - low) * (high - low);
    }
    let limit = (diagonal.sqrt() * 1.5).max(median * 8.0);
    let mut kept = Vec::with_capacity(indices.len());
    for triangle in &triangles {
        if triangle.iter().any(|index| (*index as usize) >= count) {
            continue;
        }
        let a = vertices[triangle[0] as usize];
        let b = vertices[triangle[1] as usize];
        let c = vertices[triangle[2] as usize];
        if edge_length(a, b) <= limit && edge_length(b, c) <= limit && edge_length(c, a) <= limit {
            kept.extend_from_slice(&triangle[..]);
        }
    }
    let fraction = kept.len() as f32 / indices.len() as f32;
    (kept, fraction)
}

fn find_indices(data: &[u8], from: usize, end: usize, vertex_count: usize) -> Option<Vec<u32>> {
    let mut offset = from;
    while offset + 8 < end {
        let stride = read_u32(data, offset) as usize;
        let count = read_u32(data, offset + 4) as usize;
        let start = offset + 8;
        if (stride == 2 || stride == 4)
            && count >= 3
            && count.is_multiple_of(3)
            && count < (1 << 22)
            && start + count * stride <= end
        {
            let read = |index: usize| -> usize {
                if stride == 2 {
                    u16::from_le_bytes([data[start + index * 2], data[start + index * 2 + 1]])
                        as usize
                } else {
                    read_u32(data, start + index * 4) as usize
                }
            };
            if (0..count.min(256)).all(|index| read(index) < vertex_count) {
                let indices: Vec<u32> = (0..count).map(|index| read(index) as u32).collect();
                let highest = indices.iter().copied().max().unwrap_or(0) as usize;
                if highest + 1 >= vertex_count / 2 {
                    return Some(indices);
                }
            }
        }
        offset += 1;
    }
    None
}

pub fn parse_static_mesh(package: &Package<'_>, export: &Export) -> Option<Mesh> {
    let data = package.export_data(export).ok()?;
    let start = read_export_properties(package, data)
        .map(|(_, consumed)| consumed)
        .unwrap_or(0);
    parse_static_mesh_blob(data, start)
}

pub fn parse_static_mesh_blob(data: &[u8], start: usize) -> Option<Mesh> {
    let end = data.len();
    let valid_vector = |offset: usize| -> bool {
        offset + 12 <= end
            && (0..3).all(|axis| {
                let value = read_f32(data, offset + axis * 4);
                value.is_finite() && value.abs() < 1.0e5
            })
    };
    let mut offset = start;
    while offset + 12 <= end {
        let coordinate_count = read_u32(data, offset);
        let stride = read_u32(data, offset + 4) as usize;
        let count = read_u32(data, offset + 8) as usize;
        if (1..=4).contains(&coordinate_count)
            && (8..=64).contains(&stride)
            && (12..(1 << 21)).contains(&count)
        {
            if let Some(position_start) = offset.checked_sub(count * 12) {
                if position_start >= start
                    && (0..count)
                        .step_by((count / 24).max(1))
                        .all(|index| valid_vector(position_start + index * 12))
                {
                    let vertices: Vec<[f32; 3]> = (0..count)
                        .map(|index| {
                            let base = position_start + index * 12;
                            [
                                read_f32(data, base),
                                read_f32(data, base + 4),
                                read_f32(data, base + 8),
                            ]
                        })
                        .collect();
                    if let Some(indices) = find_indices(data, offset, end, count) {
                        let (kept, fraction) = filter_spikes(&vertices, &indices);
                        if fraction > 0.6 && kept.len() >= 3 {
                            let full_precision = read_u32(data, offset + 12) != 0;
                            let bulk = offset + 24;
                            let uvs = (0..count)
                                .map(|index| {
                                    let base = bulk + index * stride + 8;
                                    if full_precision && base + 8 <= end {
                                        [read_f32(data, base), read_f32(data, base + 4)]
                                    } else if base + 4 <= end {
                                        [
                                            half_to_f32(u16::from_le_bytes([
                                                data[base],
                                                data[base + 1],
                                            ])),
                                            half_to_f32(u16::from_le_bytes([
                                                data[base + 2],
                                                data[base + 3],
                                            ])),
                                        ]
                                    } else {
                                        [0.0, 0.0]
                                    }
                                })
                                .collect();
                            return Some(Mesh {
                                vertices,
                                uvs,
                                indices: kept,
                                position_offset: position_start,
                                properties_end: start,
                                material_refs: Vec::new(),
                                skin: None,
                            });
                        }
                    }
                }
            }
        }
        offset += 1;
    }
    None
}

pub fn parse_skeletal_mesh(package: &Package<'_>, export: &Export) -> Option<Mesh> {
    let data = package.export_data(export).ok()?;
    let start = read_export_properties(package, data)
        .map(|(_, consumed)| consumed)
        .unwrap_or(0);
    let mut mesh = parse_skeletal_mesh_blob(data, start)?;
    if let Some(skin) = mesh.skin.as_mut() {
        for bone in &mut skin.bones {
            if let Some(name) = package.names.get(bone.name_index.max(0) as usize) {
                bone.name = name.clone();
            }
        }
    }
    Some(mesh)
}

pub fn parse_skeletal_mesh_blob(data: &[u8], start: usize) -> Option<Mesh> {
    let end = data.len();
    let read_i32 = |offset: usize| -> i32 { read_u32(data, offset) as i32 };

    if start + 28 > end {
        return None;
    }
    let mut offset = start;
    let origin = [
        read_f32(data, offset),
        read_f32(data, offset + 4),
        read_f32(data, offset + 8),
    ];
    let extent = [
        read_f32(data, offset + 12),
        read_f32(data, offset + 16),
        read_f32(data, offset + 20),
    ];
    if origin.iter().chain(&extent).any(|value| !value.is_finite()) {
        return None;
    }
    offset += 28;

    let material_count = read_i32(offset);
    if !(0..=256).contains(&material_count) {
        return None;
    }
    offset += 4;
    let mut material_refs = Vec::with_capacity(material_count as usize);
    for _ in 0..material_count {
        if offset + 4 > end {
            return None;
        }
        material_refs.push(read_i32(offset));
        offset += 4;
    }

    offset += 24;
    if offset + 4 > end {
        return None;
    }
    let bone_count = read_i32(offset);
    if !(1..=8192).contains(&bone_count) {
        return None;
    }
    offset += 4;
    let reference_skeleton = offset;

    let mut lod_models = None;
    let mut bone_stride = 52usize;
    for stride in [52usize, 48, 56, 44, 60] {
        let after = reference_skeleton.checked_add(bone_count as usize * stride)?;
        if after + 8 > end {
            continue;
        }
        let depth = read_i32(after);
        let count = read_i32(after + 4);
        if (1..=64).contains(&depth) && (1..=16).contains(&count) {
            lod_models = Some(after + 8);
            bone_stride = stride;
            break;
        }
    }
    let lod_start = lod_models?;

    let (indices, index_end) = find_skeletal_indices(data, lod_start, end)?;
    let vertex_count = *indices.iter().max()? as usize + 1;
    let buffer = find_skeletal_vertices(data, index_end, end, vertex_count, origin, extent)?;

    let bones = parse_reference_skeleton(data, reference_skeleton, bone_count as usize, bone_stride);
    let skin = build_skin(data, index_end, &buffer, bone_count as usize, bones);

    Some(Mesh {
        vertices: buffer.vertices,
        uvs: buffer.uvs,
        indices,
        position_offset: buffer.data_start + buffer.position_in_stride,
        properties_end: start,
        material_refs,
        skin,
    })
}

struct VertexBuffer {
    vertices: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    data_start: usize,
    stride: usize,
    position_in_stride: usize,
}

struct Chunk {
    start: usize,
    len: usize,
    bone_map: Vec<u16>,
}

fn parse_reference_skeleton(data: &[u8], start: usize, count: usize, stride: usize) -> Vec<Bone> {
    let parent_offset = if stride >= 48 { 44 } else { stride.saturating_sub(8) };
    let mut bones = Vec::with_capacity(count);
    for index in 0..count {
        let base = start + index * stride;
        if base + parent_offset + 4 > data.len() {
            break;
        }
        let name_index = read_u32(data, base) as i32;
        let rotation = [
            read_f32(data, base + 12),
            read_f32(data, base + 16),
            read_f32(data, base + 20),
            read_f32(data, base + 24),
        ];
        let translation = [
            read_f32(data, base + 28),
            read_f32(data, base + 32),
            read_f32(data, base + 36),
        ];
        let parent = read_u32(data, base + parent_offset) as i32;
        bones.push(Bone {
            name: String::new(),
            name_index,
            parent,
            translation,
            rotation,
        });
    }
    bones
}

struct MapCandidate {
    offset: usize,
    end: usize,
    len: usize,
    map: Vec<u16>,
}

fn parse_chunks(data: &[u8], from: usize, to: usize, bone_count: usize, total: usize) -> Option<Vec<Chunk>> {
    let mut candidates: Vec<MapCandidate> = Vec::new();
    let mut offset = from;
    while offset + 4 <= to {
        let width = read_u32(data, offset) as usize;
        if (1..=bone_count).contains(&width) {
            let map_end = offset + 4 + width * 2;
            if map_end + 12 <= to
                && (0..width).all(|slot| {
                    (u16::from_le_bytes([data[offset + 4 + slot * 2], data[offset + 5 + slot * 2]])
                        as usize)
                        < bone_count
                })
            {
                let rigid = read_u32(data, map_end) as i64;
                let soft = read_u32(data, map_end + 4) as i64;
                let influences = read_u32(data, map_end + 8) as i64;
                let chunk_len = (rigid + soft) as usize;
                if rigid >= 0 && soft >= 0 && (1..=8).contains(&influences) && chunk_len > 0 && chunk_len <= total {
                    let map: Vec<u16> = (0..width)
                        .map(|slot| {
                            u16::from_le_bytes([data[offset + 4 + slot * 2], data[offset + 5 + slot * 2]])
                        })
                        .collect();
                    candidates.push(MapCandidate {
                        offset,
                        end: map_end + 12,
                        len: chunk_len,
                        map,
                    });
                    offset = map_end + 8;
                    continue;
                }
            }
        }
        offset += 1;
    }
    if candidates.is_empty() || candidates.len() > 64 {
        return None;
    }
    candidates.sort_by_key(|candidate| candidate.offset);
    let mut chosen = Vec::new();
    if !select_chunks(&candidates, 0, 0, 0, total, &mut chosen) {
        return None;
    }
    let mut base = 0;
    let mut chunks = Vec::with_capacity(chosen.len());
    for index in chosen {
        let candidate = &candidates[index];
        chunks.push(Chunk {
            start: base,
            len: candidate.len,
            bone_map: candidate.map.clone(),
        });
        base += candidate.len;
    }
    Some(chunks)
}

fn select_chunks(
    candidates: &[MapCandidate],
    index: usize,
    sum: usize,
    last_end: usize,
    total: usize,
    chosen: &mut Vec<usize>,
) -> bool {
    if sum == total {
        return true;
    }
    if index >= candidates.len() || sum > total {
        return false;
    }
    let candidate = &candidates[index];
    if candidate.offset >= last_end && sum + candidate.len <= total {
        chosen.push(index);
        if select_chunks(candidates, index + 1, sum + candidate.len, candidate.end, total, chosen) {
            return true;
        }
        chosen.pop();
    }
    select_chunks(candidates, index + 1, sum, last_end, total, chosen)
}

fn build_skin(data: &[u8], index_end: usize, buffer: &VertexBuffer, bone_count: usize, bones: Vec<Bone>) -> Option<Skin> {
    if bones.is_empty() {
        return None;
    }
    let count = buffer.vertices.len();
    let chunk_end = buffer.data_start.saturating_sub(8);
    let chunks = parse_chunks(data, index_end, chunk_end, bone_count, count)?;
    if buffer.position_in_stride < 8 {
        return None;
    }
    let influences = (buffer.position_in_stride - 8) / 2;
    if influences == 0 {
        return None;
    }
    let take = influences.min(4);
    let bones_at = 8;
    let weights_at = 8 + influences;
    if weights_at + take > buffer.position_in_stride {
        return None;
    }
    let mut joints = Vec::with_capacity(count);
    let mut weights = Vec::with_capacity(count);
    for index in 0..count {
        let base = buffer.data_start + index * buffer.stride;
        let chunk = chunks
            .iter()
            .find(|chunk| index >= chunk.start && index < chunk.start + chunk.len)
            .unwrap_or(&chunks[0]);
        let mut joint = [0u16; 4];
        let mut weight = [0f32; 4];
        for slot in 0..take {
            let local = data[base + bones_at + slot] as usize;
            joint[slot] = chunk.bone_map.get(local).copied().unwrap_or(0);
            weight[slot] = data[base + weights_at + slot] as f32;
        }
        let sum: f32 = weight.iter().sum();
        if sum > 0.0 {
            for value in &mut weight {
                *value /= sum;
            }
        } else {
            weight[0] = 1.0;
        }
        joints.push(joint);
        weights.push(weight);
    }
    Some(Skin { bones, joints, weights })
}

fn find_skeletal_indices(data: &[u8], from: usize, end: usize) -> Option<(Vec<u32>, usize)> {
    let mut offset = from;
    while offset + 13 <= end {
        if read_u32(data, offset) == 1 {
            let element_size = data[offset + 4] as u32;
            if (element_size == 2 || element_size == 4) && read_u32(data, offset + 5) == element_size
            {
                let count = read_u32(data, offset + 9) as usize;
                let stride = element_size as usize;
                let bytes = offset + 13;
                if count >= 3
                    && count.is_multiple_of(3)
                    && count < (1 << 24)
                    && bytes + count * stride <= end
                {
                    let indices: Vec<u32> = (0..count)
                        .map(|index| {
                            let base = bytes + index * stride;
                            if stride == 2 {
                                u16::from_le_bytes([data[base], data[base + 1]]) as u32
                            } else {
                                read_u32(data, base)
                            }
                        })
                        .collect();
                    return Some((indices, bytes + count * stride));
                }
            }
        }
        offset += 1;
    }
    None
}

fn find_skeletal_vertices(
    data: &[u8],
    from: usize,
    end: usize,
    vertex_count: usize,
    origin: [f32; 3],
    extent: [f32; 3],
) -> Option<VertexBuffer> {
    let within = |offset: usize, position_offset: usize, stride: usize| -> bool {
        (0..vertex_count)
            .step_by((vertex_count / 32).max(1))
            .all(|index| {
                let base = offset + index * stride + position_offset;
                (0..3).all(|axis| {
                    let value = read_f32(data, base + axis * 4);
                    value.is_finite()
                        && (value - origin[axis]).abs() <= extent[axis].abs() * 2.0 + 4.0
                })
            })
    };
    let mut offset = from;
    while offset + 8 <= end {
        let stride = read_u32(data, offset) as usize;
        let count = read_u32(data, offset + 4) as usize;
        if count >= vertex_count
            && count <= vertex_count + 256
            && (16..=128).contains(&stride)
        {
            let bytes = offset + 8;
            if bytes + stride * count <= end {
                for position_offset in [16usize, 24, 8, 0] {
                    if position_offset + 12 > stride {
                        continue;
                    }
                    if within(bytes, position_offset, stride) {
                        let vertices: Vec<[f32; 3]> = (0..count)
                            .map(|index| {
                                let base = bytes + index * stride + position_offset;
                                [
                                    read_f32(data, base),
                                    read_f32(data, base + 4),
                                    read_f32(data, base + 8),
                                ]
                            })
                            .collect();
                        let uv_offset = position_offset + 12;
                        let uvs: Vec<[f32; 2]> = if uv_offset + 4 <= stride {
                            (0..count)
                                .map(|index| {
                                    let base = bytes + index * stride + uv_offset;
                                    [
                                        half_to_f32(u16::from_le_bytes([data[base], data[base + 1]])),
                                        half_to_f32(u16::from_le_bytes([
                                            data[base + 2],
                                            data[base + 3],
                                        ])),
                                    ]
                                })
                                .collect()
                        } else {
                            Vec::new()
                        };
                        return Some(VertexBuffer {
                            vertices,
                            uvs,
                            data_start: bytes,
                            stride,
                            position_in_stride: position_offset,
                        });
                    }
                }
            }
        }
        offset += 1;
    }
    None
}

fn sphere_bounds(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3], f32) {
    let mut low = [f32::MAX; 3];
    let mut high = [f32::MIN; 3];
    for position in positions {
        for axis in 0..3 {
            low[axis] = low[axis].min(position[axis]);
            high[axis] = high[axis].max(position[axis]);
        }
    }
    let origin = [
        (low[0] + high[0]) / 2.0,
        (low[1] + high[1]) / 2.0,
        (low[2] + high[2]) / 2.0,
    ];
    let extent = [
        (high[0] - low[0]) / 2.0,
        (high[1] - low[1]) / 2.0,
        (high[2] - low[2]) / 2.0,
    ];
    let radius = positions
        .iter()
        .map(|position| {
            (0..3)
                .map(|axis| (position[axis] - origin[axis]).powi(2))
                .sum::<f32>()
                .sqrt()
        })
        .fold(0.0f32, f32::max);
    (origin, extent, radius)
}

impl Mesh {
    pub fn bounds_offset(&self, blob: &[u8]) -> Option<usize> {
        let (origin, extent, radius) = sphere_bounds(&self.vertices);
        let close = |left: f32, right: f32| (left - right).abs() <= right.abs().max(1.0) * 1e-3;
        let limit = self
            .position_offset
            .min(blob.len())
            .min(self.properties_end + 1024);
        (self.properties_end..limit)
            .step_by(4)
            .find(|base| {
                let end = base + 28;
                if end > blob.len() {
                    return false;
                }
                let value = |at: usize| read_f32(blob, base + at);
                (0..3).all(|axis| close(value(axis * 4), origin[axis]))
                    && (0..3).all(|axis| close(value(12 + axis * 4), extent[axis]))
                    && close(value(24), radius)
            })
    }

    pub fn collision_box_offset(&self, blob: &[u8]) -> Option<usize> {
        let (low, high) = self.bounds();
        let close = |left: f32, right: f32| (left - right).abs() <= right.abs().max(1.0) * 1e-3;
        let limit = self
            .position_offset
            .min(blob.len())
            .min(self.properties_end + 1024);
        (self.properties_end..limit).step_by(4).find(|base| {
            if base + 24 > blob.len() {
                return false;
            }
            let value = |at: usize| read_f32(blob, base + at);
            (0..3).all(|axis| close(value(axis * 4), low[axis]))
                && (0..3).all(|axis| close(value(12 + axis * 4), high[axis]))
        })
    }

    pub fn replace_vertices(&self, blob: &[u8], positions: &[[f32; 3]]) -> Result<Vec<u8>> {
        if positions.len() != self.vertices.len() {
            return Err(PackageError::UnsupportedProperty(format!(
                "mesh has {} vertices but the replacement has {}",
                self.vertices.len(),
                positions.len()
            )));
        }
        let end = self.position_offset + positions.len() * 12;
        if end > blob.len() {
            return Err(PackageError::Truncated {
                offset: self.position_offset,
                needed: end,
                available: blob.len(),
            });
        }
        let bounds_at = self.bounds_offset(blob);
        let collision_at = self.collision_box_offset(blob);
        let mut out = blob.to_vec();
        for (index, position) in positions.iter().enumerate() {
            let base = self.position_offset + index * 12;
            for (axis, value) in position.iter().enumerate() {
                out[base + axis * 4..base + axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        if let Some(base) = collision_at {
            let mut low = [f32::MAX; 3];
            let mut high = [f32::MIN; 3];
            for position in positions {
                for axis in 0..3 {
                    low[axis] = low[axis].min(position[axis]);
                    high[axis] = high[axis].max(position[axis]);
                }
            }
            for axis in 0..3 {
                let at = base + axis * 4;
                out[at..at + 4].copy_from_slice(&low[axis].to_le_bytes());
                let at = base + 12 + axis * 4;
                out[at..at + 4].copy_from_slice(&high[axis].to_le_bytes());
            }
        }
        if let Some(base) = bounds_at {
            let (origin, extent, radius) = sphere_bounds(positions);
            let mut write = |at: usize, value: f32| {
                out[base + at..base + at + 4].copy_from_slice(&value.to_le_bytes());
            };
            for axis in 0..3 {
                write(axis * 4, origin[axis]);
                write(12 + axis * 4, extent[axis]);
            }
            write(24, radius);
        }
        Ok(out)
    }
}

pub fn read_obj_vertices(text: &str) -> Vec<[f32; 3]> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_ascii_whitespace();
            if parts.next() != Some("v") {
                return None;
            }
            let mut axis = [0.0f32; 3];
            for slot in &mut axis {
                *slot = parts.next()?.parse().ok()?;
            }
            Some(axis)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_vertices_are_read_in_order() {
        let text = "o thing\nv 1 2 3\nvt 0 0\nv -1.5 0 2e1\nf 1 2 3\n";
        assert_eq!(read_obj_vertices(text), vec![[1.0, 2.0, 3.0], [-1.5, 0.0, 20.0]]);
    }

    #[test]
    fn a_malformed_vertex_line_is_skipped() {
        assert!(read_obj_vertices("v 1 2\nv a b c\n").is_empty());
    }

    #[test]
    fn bounds_cover_every_vertex() {
        let (origin, extent, radius) =
            sphere_bounds(&[[-2.0, 0.0, 0.0], [4.0, 0.0, 0.0], [1.0, 3.0, 0.0]]);
        assert_eq!(origin, [1.0, 1.5, 0.0]);
        assert_eq!(extent, [3.0, 1.5, 0.0]);
        assert!((radius - 3.354).abs() < 0.01);
    }

    #[test]
    fn a_replacement_with_the_wrong_vertex_count_is_refused() {
        let mesh = Mesh {
            vertices: vec![[0.0; 3]; 2],
            uvs: Vec::new(),
            indices: Vec::new(),
            position_offset: 0,
            properties_end: 0,
            material_refs: Vec::new(),
            skin: None,
        };
        assert!(mesh.replace_vertices(&[0u8; 24], &[[1.0; 3]]).is_err());
    }
}
