use crate::error::{PackageError, Result};
use crate::package::{Export, Package};
use crate::properties::read_export_properties;

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub vertices: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub position_offset: usize,
    pub properties_end: usize,
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
        };
        assert!(mesh.replace_vertices(&[0u8; 24], &[[1.0; 3]]).is_err());
    }
}
