use crate::error::{PackageError, Result};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        let mut value = index as u32;
        for _ in 0..8 {
            value = if value & 1 != 0 {
                0xedb8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
        }
        *entry = value;
    }
    table
}

fn crc32(data: &[u8]) -> u32 {
    let table = crc_table();
    let mut value = 0xffff_ffffu32;
    for byte in data {
        value = table[((value ^ u32::from(*byte)) & 0xff) as usize] ^ (value >> 8);
    }
    value ^ 0xffff_ffff
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    let checksum = crc32(&out[start..]);
    out.extend_from_slice(&checksum.to_be_bytes());
}

pub fn encode(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    if rgba.len() < (width as usize) * (height as usize) * 4 {
        return Err(PackageError::Truncated {
            offset: 0,
            needed: (width as usize) * (height as usize) * 4,
            available: rgba.len(),
        });
    }
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0);
        let start = row * width as usize * 4;
        raw.extend_from_slice(&rgba[start..start + width as usize * 4]);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(6));
    encoder
        .write_all(&raw)
        .map_err(|error| PackageError::Zlib {
            offset: 0,
            reason: error.to_string(),
        })?;
    let compressed = encoder.finish().map_err(|error| PackageError::Zlib {
        offset: 0,
        reason: error.to_string(),
    })?;

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);

    let mut out = Vec::with_capacity(compressed.len() + 64);
    out.extend_from_slice(&SIGNATURE);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &compressed);
    chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn decode(data: &[u8]) -> Result<Image> {
    if data.len() < 8 || data[..8] != SIGNATURE {
        return Err(PackageError::UnsupportedPixelFormat("not a png file".into()));
    }
    let mut offset = 8usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut depth = 0u8;
    let mut color_type = 0u8;
    let mut interlace = 0u8;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut transparency: Vec<u8> = Vec::new();
    let mut compressed = Vec::new();
    while offset + 8 <= data.len() {
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &data[offset + 4..offset + 8];
        let payload_start = offset + 8;
        let payload_end = payload_start + length;
        if payload_end + 4 > data.len() {
            break;
        }
        let payload = &data[payload_start..payload_end];
        match kind {
            b"IHDR" => {
                width = u32::from_be_bytes(payload[0..4].try_into().unwrap());
                height = u32::from_be_bytes(payload[4..8].try_into().unwrap());
                depth = payload[8];
                color_type = payload[9];
                interlace = payload[12];
            }
            b"PLTE" => {
                palette = payload.as_chunks::<3>().0.to_vec();
            }
            b"tRNS" => transparency = payload.to_vec(),
            b"IDAT" => compressed.extend_from_slice(payload),
            b"IEND" => break,
            _ => {}
        }
        offset = payload_end + 4;
    }
    if depth != 8 {
        return Err(PackageError::UnsupportedPixelFormat(format!(
            "png bit depth {depth}, only 8 is supported"
        )));
    }
    if interlace != 0 {
        return Err(PackageError::UnsupportedPixelFormat(
            "interlaced png is not supported".into(),
        ));
    }
    let channels = match color_type {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        other => {
            return Err(PackageError::UnsupportedPixelFormat(format!(
                "png color type {other}"
            )))
        }
    };
    let raw = inflate(&compressed)?;
    let stride = width as usize * channels;
    let mut previous = vec![0u8; stride];
    let mut current = vec![0u8; stride];
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    let mut cursor = 0usize;
    for row in 0..height as usize {
        if cursor >= raw.len() {
            break;
        }
        let filter = raw[cursor];
        cursor += 1;
        let end = (cursor + stride).min(raw.len());
        current[..end - cursor].copy_from_slice(&raw[cursor..end]);
        cursor = end;
        unfilter(filter, channels, &mut current, &previous);
        for column in 0..width as usize {
            let source = column * channels;
            let target = (row * width as usize + column) * 4;
            let pixel = match color_type {
                0 => {
                    let value = current[source];
                    [value, value, value, 255]
                }
                2 => [current[source], current[source + 1], current[source + 2], 255],
                3 => {
                    let index = current[source] as usize;
                    let entry = palette.get(index).copied().unwrap_or([0, 0, 0]);
                    let alpha = transparency.get(index).copied().unwrap_or(255);
                    [entry[0], entry[1], entry[2], alpha]
                }
                4 => {
                    let value = current[source];
                    [value, value, value, current[source + 1]]
                }
                _ => [
                    current[source],
                    current[source + 1],
                    current[source + 2],
                    current[source + 3],
                ],
            };
            rgba[target..target + 4].copy_from_slice(&pixel);
        }
        previous.copy_from_slice(&current);
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    use flate2::{Decompress, FlushDecompress, Status};
    let mut inflater = Decompress::new(true);
    let mut out = Vec::with_capacity(data.len() * 4);
    let mut buffer = vec![0u8; 1 << 18];
    let mut input = data;
    loop {
        let before = inflater.total_out();
        let status = inflater
            .decompress(input, &mut buffer, FlushDecompress::None)
            .map_err(|error| PackageError::Zlib {
                offset: 0,
                reason: error.to_string(),
            })?;
        let produced = (inflater.total_out() - before) as usize;
        out.extend_from_slice(&buffer[..produced]);
        let consumed = inflater.total_in() as usize - (data.len() - input.len());
        input = &input[consumed..];
        match status {
            Status::StreamEnd => break,
            _ if produced == 0 && consumed == 0 => break,
            _ => {}
        }
    }
    Ok(out)
}

fn unfilter(filter: u8, channels: usize, current: &mut [u8], previous: &[u8]) {
    match filter {
        1 => {
            for index in channels..current.len() {
                current[index] = current[index].wrapping_add(current[index - channels]);
            }
        }
        2 => {
            for index in 0..current.len() {
                current[index] = current[index].wrapping_add(previous[index]);
            }
        }
        3 => {
            for index in 0..current.len() {
                let left = if index >= channels {
                    u16::from(current[index - channels])
                } else {
                    0
                };
                let above = u16::from(previous[index]);
                current[index] = current[index].wrapping_add(((left + above) / 2) as u8);
            }
        }
        4 => {
            for index in 0..current.len() {
                let left = if index >= channels {
                    i16::from(current[index - channels])
                } else {
                    0
                };
                let above = i16::from(previous[index]);
                let corner = if index >= channels {
                    i16::from(previous[index - channels])
                } else {
                    0
                };
                let estimate = left + above - corner;
                let distance_left = (estimate - left).abs();
                let distance_above = (estimate - above).abs();
                let distance_corner = (estimate - corner).abs();
                let predictor =
                    if distance_left <= distance_above && distance_left <= distance_corner {
                        left
                    } else if distance_above <= distance_corner {
                        above
                    } else {
                        corner
                    };
                current[index] = current[index].wrapping_add(predictor as u8);
            }
        }
        _ => {}
    }
}
