#[inline]
fn expand565(value: u16) -> [u8; 3] {
    let red = ((value >> 11) & 0x1f) as u8;
    let green = ((value >> 5) & 0x3f) as u8;
    let blue = (value & 0x1f) as u8;
    [
        (red << 3) | (red >> 2),
        (green << 2) | (green >> 4),
        (blue << 3) | (blue >> 2),
    ]
}

#[inline]
fn pack565(rgb: [u8; 3]) -> u16 {
    ((u16::from(rgb[0]) >> 3) << 11) | ((u16::from(rgb[1]) >> 2) << 5) | (u16::from(rgb[2]) >> 3)
}

#[inline]
fn lerp(a: u8, b: u8, numerator: u32, denominator: u32) -> u8 {
    ((u32::from(a) * (denominator - numerator) + u32::from(b) * numerator) / denominator) as u8
}

fn color_palette(block: &[u8], opaque: bool) -> [[u8; 4]; 4] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let a = expand565(c0);
    let b = expand565(c1);
    let mut palette = [[0u8; 4]; 4];
    palette[0] = [a[0], a[1], a[2], 255];
    palette[1] = [b[0], b[1], b[2], 255];
    if opaque || c0 > c1 {
        for channel in 0..3 {
            palette[2][channel] = lerp(a[channel], b[channel], 1, 3);
            palette[3][channel] = lerp(a[channel], b[channel], 2, 3);
        }
        palette[2][3] = 255;
        palette[3][3] = 255;
    } else {
        for channel in 0..3 {
            palette[2][channel] = lerp(a[channel], b[channel], 1, 2);
            palette[3][channel] = 0;
        }
        palette[2][3] = 255;
        palette[3][3] = 0;
    }
    palette
}

fn alpha_palette(a0: u8, a1: u8) -> [u8; 8] {
    let mut values = [0u8; 8];
    values[0] = a0;
    values[1] = a1;
    if a0 > a1 {
        for index in 2..8u32 {
            values[index as usize] = lerp(a0, a1, index - 1, 7);
        }
    } else {
        for index in 2..6u32 {
            values[index as usize] = lerp(a0, a1, index - 1, 5);
        }
        values[6] = 0;
        values[7] = 255;
    }
    values
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockFormat {
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
}

impl BlockFormat {
    pub fn block_bytes(self) -> usize {
        match self {
            Self::Bc1 | Self::Bc4 => 8,
            _ => 16,
        }
    }
}

pub fn decode_blocks(
    format: BlockFormat,
    data: &[u8],
    width: usize,
    height: usize,
) -> Option<Vec<u8>> {
    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let stride = format.block_bytes();
    let required = blocks_x
        .checked_mul(blocks_y)
        .and_then(|value| value.checked_mul(stride))?;
    let out_len = width.checked_mul(height).and_then(|value| value.checked_mul(4))?;
    if data.len() < required {
        return None;
    }
    let mut out = vec![0u8; out_len];
    for block_y in 0..blocks_y {
        for block_x in 0..blocks_x {
            let block = &data[(block_y * blocks_x + block_x) * stride..][..stride];
            let mut pixels = [[0u8; 4]; 16];
            match format {
                BlockFormat::Bc1 => {
                    let palette = color_palette(block, false);
                    let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
                    for index in 0..16 {
                        pixels[index] = palette[((indices >> (index * 2)) & 3) as usize];
                    }
                }
                BlockFormat::Bc2 => {
                    let palette = color_palette(&block[8..], true);
                    let indices = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);
                    for index in 0..16 {
                        let mut pixel = palette[((indices >> (index * 2)) & 3) as usize];
                        let nibble = block[index / 2];
                        let alpha = if index % 2 == 0 {
                            nibble & 0xf
                        } else {
                            nibble >> 4
                        };
                        pixel[3] = alpha * 17;
                        pixels[index] = pixel;
                    }
                }
                BlockFormat::Bc3 => {
                    let palette = color_palette(&block[8..], true);
                    let indices = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);
                    let alphas = alpha_palette(block[0], block[1]);
                    let bits = u64::from_le_bytes([
                        block[2], block[3], block[4], block[5], block[6], block[7], 0, 0,
                    ]);
                    for index in 0..16 {
                        let mut pixel = palette[((indices >> (index * 2)) & 3) as usize];
                        pixel[3] = alphas[((bits >> (index * 3)) & 7) as usize];
                        pixels[index] = pixel;
                    }
                }
                BlockFormat::Bc4 => {
                    let values = alpha_palette(block[0], block[1]);
                    let bits = u64::from_le_bytes([
                        block[2], block[3], block[4], block[5], block[6], block[7], 0, 0,
                    ]);
                    for index in 0..16 {
                        let value = values[((bits >> (index * 3)) & 7) as usize];
                        pixels[index] = [value, value, value, 255];
                    }
                }
                BlockFormat::Bc5 => {
                    let reds = alpha_palette(block[0], block[1]);
                    let red_bits = u64::from_le_bytes([
                        block[2], block[3], block[4], block[5], block[6], block[7], 0, 0,
                    ]);
                    let greens = alpha_palette(block[8], block[9]);
                    let green_bits = u64::from_le_bytes([
                        block[10], block[11], block[12], block[13], block[14], block[15], 0, 0,
                    ]);
                    for index in 0..16 {
                        let red = reds[((red_bits >> (index * 3)) & 7) as usize];
                        let green = greens[((green_bits >> (index * 3)) & 7) as usize];
                        pixels[index] = [red, green, reconstruct_blue(red, green), 255];
                    }
                }
            }
            for row in 0..4 {
                let y = block_y * 4 + row;
                if y >= height {
                    break;
                }
                for column in 0..4 {
                    let x = block_x * 4 + column;
                    if x >= width {
                        break;
                    }
                    let target = (y * width + x) * 4;
                    out[target..target + 4].copy_from_slice(&pixels[row * 4 + column]);
                }
            }
        }
    }
    Some(out)
}

fn reconstruct_blue(red: u8, green: u8) -> u8 {
    let x = f32::from(red) / 127.5 - 1.0;
    let y = f32::from(green) / 127.5 - 1.0;
    let z = (1.0 - x * x - y * y).max(0.0).sqrt();
    ((z + 1.0) * 127.5).clamp(0.0, 255.0) as u8
}

pub fn encode_blocks(format: BlockFormat, rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let stride = format.block_bytes();
    let mut out = vec![0u8; blocks_x * blocks_y * stride];
    for block_y in 0..blocks_y {
        for block_x in 0..blocks_x {
            let mut pixels = [[0u8; 4]; 16];
            for row in 0..4 {
                let y = (block_y * 4 + row).min(height.saturating_sub(1));
                for column in 0..4 {
                    let x = (block_x * 4 + column).min(width.saturating_sub(1));
                    let source = (y * width + x) * 4;
                    pixels[row * 4 + column].copy_from_slice(&rgba[source..source + 4]);
                }
            }
            let target = &mut out[(block_y * blocks_x + block_x) * stride..][..stride];
            match format {
                BlockFormat::Bc1 => encode_color_block(&pixels, target, false),
                BlockFormat::Bc3 => {
                    encode_alpha_block(&pixels, &mut target[..8]);
                    encode_color_block(&pixels, &mut target[8..], true);
                }
                _ => encode_color_block(&pixels, target, true),
            }
        }
    }
    out
}

fn block_error(
    pixels: &[[u8; 4]; 16],
    c0: u16,
    c1: u16,
    four_colors: bool,
    opaque: bool,
) -> f32 {
    let mut header = [0u8; 4];
    header[..2].copy_from_slice(&c0.to_le_bytes());
    header[2..4].copy_from_slice(&c1.to_le_bytes());
    let palette = color_palette(&header, four_colors);
    let mut total = 0.0f32;
    for pixel in pixels {
        if !opaque && pixel[3] < 128 {
            continue;
        }
        let mut best = f32::MAX;
        for (candidate, entry) in palette.iter().enumerate() {
            if !four_colors && candidate == 3 {
                continue;
            }
            let error: f32 = (0..3)
                .map(|channel| {
                    let delta = f32::from(entry[channel]) - f32::from(pixel[channel]);
                    delta * delta
                })
                .sum();
            best = best.min(error);
        }
        total += best;
    }
    total
}

fn perturb(value: u16, channel: usize, delta: i32) -> Option<u16> {
    let (shift, mask) = match channel {
        0 => (11u32, 0x1fu16),
        1 => (5, 0x3f),
        _ => (0, 0x1f),
    };
    let current = ((value >> shift) & mask) as i32;
    let next = current + delta;
    if next < 0 || next > mask as i32 {
        return None;
    }
    Some((value & !(mask << shift)) | ((next as u16) << shift))
}

fn refine_endpoints(
    pixels: &[[u8; 4]; 16],
    start: (u16, u16),
    four_colors: bool,
    opaque: bool,
) -> (u16, u16) {
    let mut best = start;
    let mut best_error = block_error(pixels, best.0, best.1, four_colors, opaque);
    for _ in 0..4 {
        let mut improved = false;
        for endpoint in 0..2 {
            for channel in 0..3 {
                for delta in [-1i32, 1] {
                    let candidate = if endpoint == 0 {
                        perturb(best.0, channel, delta).map(|value| (value, best.1))
                    } else {
                        perturb(best.1, channel, delta).map(|value| (best.0, value))
                    };
                    let Some((c0, c1)) = candidate else { continue };
                    if four_colors && c0 <= c1 {
                        continue;
                    }
                    if !four_colors && c0 > c1 {
                        continue;
                    }
                    let error = block_error(pixels, c0, c1, four_colors, opaque);
                    if error < best_error - 1e-3 {
                        best_error = error;
                        best = (c0, c1);
                        improved = true;
                    }
                }
            }
        }
        if !improved {
            break;
        }
    }
    best
}

fn principal_axis(points: &[[f32; 3]]) -> [f32; 3] {
    let count = points.len().max(1) as f32;
    let mut mean = [0.0f32; 3];
    for point in points {
        for channel in 0..3 {
            mean[channel] += point[channel];
        }
    }
    for value in mean.iter_mut() {
        *value /= count;
    }
    let mut covariance = [[0.0f32; 3]; 3];
    for point in points {
        let centered = [
            point[0] - mean[0],
            point[1] - mean[1],
            point[2] - mean[2],
        ];
        for row in 0..3 {
            for column in 0..3 {
                covariance[row][column] += centered[row] * centered[column];
            }
        }
    }
    let mut axis = [1.0f32, 1.0, 1.0];
    for _ in 0..8 {
        let mut next = [0.0f32; 3];
        for row in 0..3 {
            for column in 0..3 {
                next[row] += covariance[row][column] * axis[column];
            }
        }
        let length = (next[0] * next[0] + next[1] * next[1] + next[2] * next[2]).sqrt();
        if length < 1e-6 {
            return [1.0, 1.0, 1.0];
        }
        axis = [next[0] / length, next[1] / length, next[2] / length];
    }
    axis
}

fn quantize_endpoints(low: [f32; 3], high: [f32; 3]) -> (u16, u16) {
    let clamp = |value: f32| value.clamp(0.0, 255.0).round() as u8;
    (
        pack565([clamp(high[0]), clamp(high[1]), clamp(high[2])]),
        pack565([clamp(low[0]), clamp(low[1]), clamp(low[2])]),
    )
}

fn palette_weights(opaque: bool) -> [f32; 4] {
    if opaque {
        [0.0, 1.0, 1.0 / 3.0, 2.0 / 3.0]
    } else {
        [0.0, 1.0, 0.5, 0.0]
    }
}

fn encode_color_block(pixels: &[[u8; 4]; 16], out: &mut [u8], opaque: bool) {
    let mut opaque_points: Vec<[f32; 3]> = Vec::with_capacity(16);
    let mut transparent = false;
    for pixel in pixels {
        if !opaque && pixel[3] < 128 {
            transparent = true;
            continue;
        }
        opaque_points.push([
            f32::from(pixel[0]),
            f32::from(pixel[1]),
            f32::from(pixel[2]),
        ]);
    }
    if opaque_points.is_empty() {
        out[..8].fill(0);
        if !opaque {
            out[4..8].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        }
        return;
    }
    let four_colors = opaque || !transparent;
    let axis = principal_axis(&opaque_points);
    let mut projections: Vec<f32> = opaque_points
        .iter()
        .map(|point| point[0] * axis[0] + point[1] * axis[1] + point[2] * axis[2])
        .collect();
    projections.sort_by(f32::total_cmp);
    let minimum = projections[0];
    let maximum = projections[projections.len() - 1];
    let mean = {
        let count = opaque_points.len() as f32;
        let mut sum = [0.0f32; 3];
        for point in &opaque_points {
            for channel in 0..3 {
                sum[channel] += point[channel];
            }
        }
        [sum[0] / count, sum[1] / count, sum[2] / count]
    };
    let center = mean[0] * axis[0] + mean[1] * axis[1] + mean[2] * axis[2];
    let mut high = [
        mean[0] + axis[0] * (maximum - center),
        mean[1] + axis[1] * (maximum - center),
        mean[2] + axis[2] * (maximum - center),
    ];
    let mut low = [
        mean[0] + axis[0] * (minimum - center),
        mean[1] + axis[1] * (minimum - center),
        mean[2] + axis[2] * (minimum - center),
    ];

    let weights = palette_weights(four_colors);
    let mut indices = [0u8; 16];
    for _ in 0..3 {
        let (mut c0, mut c1) = quantize_endpoints(low, high);
        if four_colors {
            if c0 < c1 {
                std::mem::swap(&mut c0, &mut c1);
            }
            if c0 == c1 && c0 > 0 {
                c1 = c0 - 1;
            }
        } else if c0 > c1 {
            std::mem::swap(&mut c0, &mut c1);
        }
        let mut header = [0u8; 4];
        header[..2].copy_from_slice(&c0.to_le_bytes());
        header[2..4].copy_from_slice(&c1.to_le_bytes());
        let palette = color_palette(&header, four_colors);
        for (position, pixel) in pixels.iter().enumerate() {
            if !opaque && pixel[3] < 128 {
                indices[position] = 3;
                continue;
            }
            let mut best = 0usize;
            let mut best_error = f32::MAX;
            for (candidate, entry) in palette.iter().enumerate() {
                if !four_colors && candidate == 3 {
                    continue;
                }
                let error: f32 = (0..3)
                    .map(|channel| {
                        let delta = f32::from(entry[channel]) - f32::from(pixel[channel]);
                        delta * delta
                    })
                    .sum();
                if error < best_error {
                    best_error = error;
                    best = candidate;
                }
            }
            indices[position] = best as u8;
        }

        let mut sum_aa = 0.0f32;
        let mut sum_ab = 0.0f32;
        let mut sum_bb = 0.0f32;
        let mut sum_ap = [0.0f32; 3];
        let mut sum_bp = [0.0f32; 3];
        for (position, pixel) in pixels.iter().enumerate() {
            if !opaque && pixel[3] < 128 {
                continue;
            }
            let weight = weights[indices[position] as usize];
            let inverse = 1.0 - weight;
            sum_aa += inverse * inverse;
            sum_ab += inverse * weight;
            sum_bb += weight * weight;
            for channel in 0..3 {
                sum_ap[channel] += inverse * f32::from(pixel[channel]);
                sum_bp[channel] += weight * f32::from(pixel[channel]);
            }
        }
        let determinant = sum_aa * sum_bb - sum_ab * sum_ab;
        if determinant.abs() < 1e-4 {
            break;
        }
        for channel in 0..3 {
            high[channel] = (sum_bb * sum_ap[channel] - sum_ab * sum_bp[channel]) / determinant;
            low[channel] = (sum_aa * sum_bp[channel] - sum_ab * sum_ap[channel]) / determinant;
        }
    }

    let (mut c0, mut c1) = quantize_endpoints(low, high);
    if four_colors {
        if c0 < c1 {
            std::mem::swap(&mut c0, &mut c1);
        }
        if c0 == c1 && c0 > 0 {
            c1 = c0 - 1;
        }
    } else if c0 > c1 {
        std::mem::swap(&mut c0, &mut c1);
    }
    let mut best_pair = (c0, c1);
    let mut best_error = block_error(pixels, c0, c1, four_colors, opaque);
    let center = [
        (low[0] + high[0]) * 0.5,
        (low[1] + high[1]) * 0.5,
        (low[2] + high[2]) * 0.5,
    ];
    for stretch in [1.06f32, 1.15, 1.3, 1.5] {
        let expand = |value: f32, middle: f32| middle + (value - middle) * stretch;
        let candidate_high = [
            expand(high[0], center[0]),
            expand(high[1], center[1]),
            expand(high[2], center[2]),
        ];
        let candidate_low = [
            expand(low[0], center[0]),
            expand(low[1], center[1]),
            expand(low[2], center[2]),
        ];
        let (mut a, mut b) = quantize_endpoints(candidate_low, candidate_high);
        if four_colors {
            if a < b {
                std::mem::swap(&mut a, &mut b);
            }
            if a == b {
                continue;
            }
        } else if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        let error = block_error(pixels, a, b, four_colors, opaque);
        if error < best_error {
            best_error = error;
            best_pair = (a, b);
        }
    }
    let refined = refine_endpoints(pixels, best_pair, four_colors, opaque);
    c0 = refined.0;
    c1 = refined.1;
    let mut header = [0u8; 4];
    header[..2].copy_from_slice(&c0.to_le_bytes());
    header[2..4].copy_from_slice(&c1.to_le_bytes());
    let palette = color_palette(&header, four_colors);
    let mut packed = 0u32;
    for (position, pixel) in pixels.iter().enumerate() {
        let selected = if !opaque && pixel[3] < 128 {
            3usize
        } else {
            let mut best = 0usize;
            let mut best_error = f32::MAX;
            for (candidate, entry) in palette.iter().enumerate() {
                if !four_colors && candidate == 3 {
                    continue;
                }
                let error: f32 = (0..3)
                    .map(|channel| {
                        let delta = f32::from(entry[channel]) - f32::from(pixel[channel]);
                        delta * delta
                    })
                    .sum();
                if error < best_error {
                    best_error = error;
                    best = candidate;
                }
            }
            best
        };
        packed |= (selected as u32) << (position * 2);
    }
    out[..4].copy_from_slice(&header);
    out[4..8].copy_from_slice(&packed.to_le_bytes());
}

fn alpha_error(pixels: &[[u8; 4]; 16], high: u8, low: u8) -> i32 {
    let values = alpha_palette(high, low);
    let mut total = 0i32;
    for pixel in pixels {
        let mut best = i32::MAX;
        for value in values.iter() {
            let delta = i32::from(*value) - i32::from(pixel[3]);
            best = best.min(delta * delta);
        }
        total += best;
    }
    total
}

fn encode_alpha_block(pixels: &[[u8; 4]; 16], out: &mut [u8]) {
    let mut low = 255u8;
    let mut high = 0u8;
    for pixel in pixels {
        low = low.min(pixel[3]);
        high = high.max(pixel[3]);
    }
    if low == high {
        out[0] = low;
        out[1] = low;
        out[2..8].fill(0);
        return;
    }
    let mut best = (high, low);
    let mut best_error = alpha_error(pixels, high, low);

    let mut first = f32::from(high);
    let mut second = f32::from(low);
    let mut indices = [0u8; 16];
    for _ in 0..3 {
        let candidate_high = first.clamp(0.0, 255.0).round() as u8;
        let candidate_low = second.clamp(0.0, 255.0).round() as u8;
        if candidate_high > candidate_low {
            let error = alpha_error(pixels, candidate_high, candidate_low);
            if error < best_error {
                best_error = error;
                best = (candidate_high, candidate_low);
            }
        }
        let values = alpha_palette(best.0, best.1);
        for (position, pixel) in pixels.iter().enumerate() {
            let mut chosen = 0usize;
            let mut chosen_error = i32::MAX;
            for (candidate, value) in values.iter().enumerate() {
                let delta = i32::from(*value) - i32::from(pixel[3]);
                let error = delta * delta;
                if error < chosen_error {
                    chosen_error = error;
                    chosen = candidate;
                }
            }
            indices[position] = chosen as u8;
        }
        let weight_of = |index: u8| -> f32 {
            match index {
                0 => 0.0,
                1 => 1.0,
                other => (other as f32 - 1.0) / 7.0,
            }
        };
        let mut sum_aa = 0.0f32;
        let mut sum_ab = 0.0f32;
        let mut sum_bb = 0.0f32;
        let mut sum_ap = 0.0f32;
        let mut sum_bp = 0.0f32;
        for (position, pixel) in pixels.iter().enumerate() {
            let weight = weight_of(indices[position]);
            let inverse = 1.0 - weight;
            sum_aa += inverse * inverse;
            sum_ab += inverse * weight;
            sum_bb += weight * weight;
            sum_ap += inverse * f32::from(pixel[3]);
            sum_bp += weight * f32::from(pixel[3]);
        }
        let determinant = sum_aa * sum_bb - sum_ab * sum_ab;
        if determinant.abs() < 1e-4 {
            break;
        }
        first = (sum_bb * sum_ap - sum_ab * sum_bp) / determinant;
        second = (sum_aa * sum_bp - sum_ab * sum_ap) / determinant;
    }

    for spread in [1i32, 2, 4, 8, 16, 32] {
        let candidate = (
            (i32::from(high) + spread).min(255) as u8,
            (i32::from(low) - spread).max(0) as u8,
        );
        if candidate.0 > candidate.1 {
            let error = alpha_error(pixels, candidate.0, candidate.1);
            if error < best_error {
                best_error = error;
                best = candidate;
            }
        }
    }

    for _ in 0..6 {
        let mut improved = false;
        for endpoint in 0..2 {
            for delta in [-1i32, 1, -4, 4] {
                let mut candidate = best;
                let value = if endpoint == 0 {
                    &mut candidate.0
                } else {
                    &mut candidate.1
                };
                let next = i32::from(*value) + delta;
                if !(0..=255).contains(&next) {
                    continue;
                }
                *value = next as u8;
                if candidate.0 <= candidate.1 {
                    continue;
                }
                let error = alpha_error(pixels, candidate.0, candidate.1);
                if error < best_error {
                    best_error = error;
                    best = candidate;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }

    let values = alpha_palette(best.0, best.1);
    out[0] = best.0;
    out[1] = best.1;
    let mut bits = 0u64;
    for (position, pixel) in pixels.iter().enumerate() {
        let mut chosen = 0usize;
        let mut chosen_error = i32::MAX;
        for (candidate, value) in values.iter().enumerate() {
            let delta = i32::from(*value) - i32::from(pixel[3]);
            let error = delta * delta;
            if error < chosen_error {
                chosen_error = error;
                chosen = candidate;
            }
        }
        bits |= (chosen as u64) << (position * 3);
    }
    out[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
}
