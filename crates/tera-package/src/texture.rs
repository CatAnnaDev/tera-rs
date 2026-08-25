use crate::error::{PackageError, Result};
use crate::package::{Export, Package};
use crate::properties::{read_export_properties, Property, PropertyValue};
use crate::reader::Reader;

pub const BULKDATA_STORE_IN_SEPARATE_FILE: u32 = 0x01;
pub const BULKDATA_COMPRESSED_ZLIB: u32 = 0x02;
pub const BULKDATA_COMPRESSED_LZO: u32 = 0x10;
pub const BULKDATA_UNUSED: u32 = 0x20;
pub const BULKDATA_STORE_ONLY_PAYLOAD: u32 = 0x40;

#[derive(Clone, Debug)]
pub struct BulkData {
    pub flags: u32,
    pub element_count: i32,
    pub size_on_disk: i32,
    pub offset_in_file: i32,
    pub field_offset: usize,
    pub payload_offset: usize,
    pub payload: Vec<u8>,
}

impl BulkData {
    pub fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let field_offset = reader.offset();
        let flags = reader.u32()?;
        let element_count = reader.i32()?;
        let size_on_disk = reader.i32()?;
        let offset_in_file = reader.i32()?;
        let stored_elsewhere = flags & (BULKDATA_STORE_IN_SEPARATE_FILE | BULKDATA_UNUSED) != 0;
        let payload_offset = reader.offset();
        let payload = if stored_elsewhere || size_on_disk <= 0 {
            Vec::new()
        } else {
            reader.take(size_on_disk as usize)?.to_vec()
        };
        Ok(Self {
            flags,
            element_count,
            size_on_disk,
            offset_in_file,
            field_offset,
            payload_offset,
            payload,
        })
    }

    pub fn is_inline(&self) -> bool {
        !self.payload.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct Mip {
    pub data: BulkData,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug)]
pub struct Texture2D {
    pub name: String,
    pub properties: Vec<Property>,
    pub format: String,
    pub width: i32,
    pub height: i32,
    pub mips: Vec<Mip>,
}

impl Texture2D {
    pub fn parse(package: &Package, export: &Export) -> Result<Self> {
        let data = package.export_data(export)?;
        let (properties, consumed) = read_export_properties(package, data)?;
        let format = property_enum(&properties, "Format").unwrap_or_default();
        let width = property_int(&properties, "SizeX").unwrap_or(0);
        let height = property_int(&properties, "SizeY").unwrap_or(0);
        let mips = find_mips(data, consumed, width, height)?;
        Ok(Self {
            name: package.name_text(export.object_name),
            properties,
            format,
            width,
            height,
            mips,
        })
    }

    pub fn replace_mips(&self, blob: &[u8], dds: &crate::dds::Dds) -> Result<(Vec<u8>, usize)> {
        self.replace_mips_with(blob, dds, None)
    }


    pub fn decode_rgba(&self) -> Result<(u32, u32, Vec<u8>)> {
        self.decode_rgba_with(None)
    }

    pub fn decode_rgba_with(&self, caches: Option<&mut Caches>) -> Result<(u32, u32, Vec<u8>)> {
        let (mip, resolved) = self.resolve_best(caches)?;
        let width = mip.width.max(0) as usize;
        let height = mip.height.max(0) as usize;
        let payload = &resolved;
        let rgba = match self.format.as_str() {
            "PF_DXT1" => crate::bc::decode_blocks(crate::bc::BlockFormat::Bc1, payload, width, height),
            "PF_DXT3" => crate::bc::decode_blocks(crate::bc::BlockFormat::Bc2, payload, width, height),
            "PF_DXT5" => crate::bc::decode_blocks(crate::bc::BlockFormat::Bc3, payload, width, height),
            "PF_BC5" | "PF_V8U8" => {
                crate::bc::decode_blocks(crate::bc::BlockFormat::Bc5, payload, width, height)
            }
            "PF_A8R8G8B8" => (payload.len() >= width * height * 4).then(|| {
                let mut out = vec![0u8; width * height * 4];
                for (target, source) in out.as_chunks_mut::<4>().0.iter_mut().zip(payload.as_chunks::<4>().0) {
                    target[0] = source[2];
                    target[1] = source[1];
                    target[2] = source[0];
                    target[3] = source[3];
                }
                out
            }),
            "PF_G8" => (payload.len() >= width * height).then(|| {
                let mut out = vec![0u8; width * height * 4];
                for (target, source) in out.as_chunks_mut::<4>().0.iter_mut().zip(payload.iter()) {
                    target.copy_from_slice(&[*source, *source, *source, 255]);
                }
                out
            }),
            other => return Err(PackageError::UnsupportedPixelFormat(other.to_string())),
        };
        let rgba = rgba.ok_or_else(|| PackageError::Truncated {
            offset: 0,
            needed: width * height * 4,
            available: payload.len(),
        })?;
        Ok((width as u32, height as u32, rgba))
    }

    pub fn to_png(&self) -> Result<Vec<u8>> {
        self.to_png_with(None)
    }

    pub fn to_png_with(&self, caches: Option<&mut Caches>) -> Result<Vec<u8>> {
        let (width, height, rgba) = self.decode_rgba_with(caches)?;
        crate::png::encode(&rgba, width, height)
    }

    pub fn resolve_best(
        &self,
        mut caches: Option<&mut Caches>,
    ) -> Result<(&Mip, Vec<u8>)> {
        let mut candidates: Vec<&Mip> = self
            .mips
            .iter()
            .filter(|mip| {
                mip.data.is_inline()
                    || (caches.is_some() && mip.data.flags & BULKDATA_STORE_IN_SEPARATE_FILE != 0)
            })
            .collect();
        candidates.sort_by_key(|mip| std::cmp::Reverse(mip.width as i64 * mip.height as i64));
        let mut last = None;
        for mip in candidates {
            match self.mip_bytes(mip, caches.as_deref_mut()) {
                Ok(bytes) => return Ok((mip, bytes)),
                Err(error) => last = Some(error),
            }
        }
        Err(last.unwrap_or_else(|| PackageError::NoPayload(self.name.clone())))
    }

    pub fn best_mip(&self, allow_cached: bool) -> Option<&Mip> {
        self.mips
            .iter()
            .filter(|mip| {
                mip.data.is_inline()
                    || (allow_cached && mip.data.flags & BULKDATA_STORE_IN_SEPARATE_FILE != 0)
            })
            .max_by_key(|mip| mip.width as i64 * mip.height as i64)
    }

    pub fn largest_inline_mip(&self) -> Option<&Mip> {
        self.mips
            .iter()
            .filter(|mip| mip.data.is_inline())
            .max_by_key(|mip| mip.width as i64 * mip.height as i64)
    }
}

fn property_int(properties: &[Property], name: &str) -> Option<i32> {
    properties.iter().find(|p| p.name == name).and_then(|p| {
        match &p.value {
            PropertyValue::Int(value) => Some(*value),
            _ => None,
        }
    })
}

fn property_enum(properties: &[Property], name: &str) -> Option<String> {
    properties.iter().find(|p| p.name == name).and_then(|p| {
        match &p.value {
            PropertyValue::Enum(value) | PropertyValue::Name(value) => Some(value.clone()),
            PropertyValue::Byte(value) => Some(value.to_string()),
            _ => None,
        }
    })
}

fn find_mips(data: &[u8], start: usize, width: i32, height: i32) -> Result<Vec<Mip>> {
    let limit = (start + 1024).min(data.len().saturating_sub(8));
    for offset in start..=limit {
        let mut reader = Reader::at(data, offset);
        let Ok(count) = reader.i32() else { continue };
        if count <= 0 || count > 16 {
            continue;
        }
        let mut mips = Vec::with_capacity(count as usize);
        let mut valid = true;
        for index in 0..count {
            let Ok(bulk) = BulkData::read(&mut reader) else {
                valid = false;
                break;
            };
            if bulk.flags > 0xffff || bulk.size_on_disk < 0 || bulk.element_count < 0 {
                valid = false;
                break;
            }
            let (Ok(mip_width), Ok(mip_height)) = (reader.i32(), reader.i32()) else {
                valid = false;
                break;
            };
            if mip_width <= 0
                || mip_height <= 0
                || mip_width > 16384
                || mip_height > 16384
                || !(mip_width as u32).is_power_of_two()
                || !(mip_height as u32).is_power_of_two()
            {
                valid = false;
                break;
            }
            if index == 0 && width > 0 && height > 0 && (mip_width != width || mip_height != height)
            {
                valid = false;
                break;
            }
            mips.push(Mip {
                data: bulk,
                width: mip_width,
                height: mip_height,
            });
        }
        if valid && !mips.is_empty() {
            return Ok(mips);
        }
    }
    Ok(Vec::new())
}

pub fn export_dds(texture: &Texture2D) -> Result<Vec<u8>> {
    export_dds_with(texture, None)
}

pub fn export_dds_with(texture: &Texture2D, caches: Option<&mut Caches>) -> Result<Vec<u8>> {
    let (mip, payload) = texture.resolve_best(caches)?;
    let (four_cc, bits_per_pixel, masks) = pixel_format(&texture.format)?;
    let mut out = Vec::with_capacity(128 + payload.len());
    out.extend_from_slice(b"DDS ");
    out.extend_from_slice(&124u32.to_le_bytes());
    let compressed = four_cc.is_some();
    let flags: u32 = 0x1 | 0x2 | 0x4 | 0x1000 | if compressed { 0x80000 } else { 0x8 };
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(mip.height as u32).to_le_bytes());
    out.extend_from_slice(&(mip.width as u32).to_le_bytes());
    let pitch = if compressed {
        payload.len() as u32
    } else {
        (mip.width as u32 * bits_per_pixel).div_ceil(8)
    };
    out.extend_from_slice(&pitch.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 44]);
    out.extend_from_slice(&32u32.to_le_bytes());
    match four_cc {
        Some(code) => {
            out.extend_from_slice(&0x4u32.to_le_bytes());
            out.extend_from_slice(code);
            out.extend_from_slice(&[0u8; 20]);
        }
        None => {
            let (has_alpha, red, green, blue, alpha) = masks;
            out.extend_from_slice(&(if has_alpha { 0x41u32 } else { 0x40u32 }).to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&bits_per_pixel.to_le_bytes());
            out.extend_from_slice(&red.to_le_bytes());
            out.extend_from_slice(&green.to_le_bytes());
            out.extend_from_slice(&blue.to_le_bytes());
            out.extend_from_slice(&alpha.to_le_bytes());
        }
    }
    out.extend_from_slice(&0x1000u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    out.extend_from_slice(&payload);
    Ok(out)
}

type PixelSpec = (Option<&'static [u8; 4]>, u32, (bool, u32, u32, u32, u32));

fn pixel_format(format: &str) -> Result<PixelSpec> {
    Ok(match format {
        "PF_DXT1" => (Some(b"DXT1"), 4, (false, 0, 0, 0, 0)),
        "PF_DXT3" => (Some(b"DXT3"), 8, (true, 0, 0, 0, 0)),
        "PF_DXT5" => (Some(b"DXT5"), 8, (true, 0, 0, 0, 0)),
        "PF_A8R8G8B8" => (
            None,
            32,
            (true, 0x00ff0000, 0x0000ff00, 0x000000ff, 0xff000000),
        ),
        "PF_G8" => (None, 8, (false, 0xff, 0, 0, 0)),
        other => return Err(PackageError::UnsupportedPixelFormat(other.to_string())),
    })
}

#[derive(Default)]
pub struct Caches {
    entries: std::collections::HashMap<String, Vec<u8>>,
    root: std::path::PathBuf,
}

impl Caches {
    pub fn at(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            entries: std::collections::HashMap::new(),
            root: root.into(),
        }
    }

    pub fn get(&mut self, name: &str) -> Option<&[u8]> {
        if !self.entries.contains_key(name) {
            let path = self.root.join(format!("{name}.tfc"));
            let bytes = std::fs::read(path).ok()?;
            self.entries.insert(name.to_string(), bytes);
        }
        self.entries.get(name).map(Vec::as_slice)
    }
}

impl Texture2D {
    pub fn cache_name(&self) -> Option<String> {
        self.properties
            .iter()
            .find(|property| property.name == "TextureFileCacheName")
            .and_then(|property| match &property.value {
                crate::properties::PropertyValue::Name(name) => Some(name.clone()),
                _ => None,
            })
    }

    pub fn mip_size(&self, mip: &Mip) -> Result<usize> {
        let (four_cc, bits_per_pixel, _) = pixel_format(&self.format)?;
        crate::dds::level_size(
            mip.width.max(0) as u32,
            mip.height.max(0) as u32,
            four_cc.map(|code| code.to_owned()).as_ref(),
            bits_per_pixel,
        )
    }

    pub fn mip_bytes(&self, mip: &Mip, caches: Option<&mut Caches>) -> Result<Vec<u8>> {
        let expected = self.mip_size(mip)?;
        let stored = if mip.data.flags & BULKDATA_STORE_IN_SEPARATE_FILE != 0 {
            let name = self
                .cache_name()
                .ok_or_else(|| PackageError::NoPayload(self.name.clone()))?;
            let caches = caches.ok_or_else(|| PackageError::NoPayload(self.name.clone()))?;
            let cache = caches
                .get(&name)
                .ok_or_else(|| PackageError::NoSuchObject(format!("{name}.tfc")))?;
            let start = mip.data.offset_in_file.max(0) as usize;
            let end = start + mip.data.size_on_disk.max(0) as usize;
            if end > cache.len() {
                return Err(PackageError::Truncated {
                    offset: start,
                    needed: end - start,
                    available: cache.len().saturating_sub(start),
                });
            }
            cache
                .get(start..end)
                .ok_or(PackageError::Truncated {
                    offset: start,
                    needed: end - start,
                    available: cache.len().saturating_sub(start),
                })?
                .to_vec()
        } else {
            mip.data.payload.clone()
        };
        inflate_mip(&stored, mip.data.flags, expected)
    }
}

fn inflate_mip(stored: &[u8], flags: u32, expected: usize) -> Result<Vec<u8>> {
    let compressed = flags & (BULKDATA_COMPRESSED_LZO | BULKDATA_COMPRESSED_ZLIB);
    if compressed == 0 || stored.len() == expected {
        return Ok(stored.to_vec());
    }
    let mut out = vec![0u8; expected];
    let method = if flags & BULKDATA_COMPRESSED_LZO != 0 {
        crate::summary::COMPRESS_LZO
    } else {
        crate::summary::COMPRESS_ZLIB
    };
    if stored.len() >= 4 && u32::from_le_bytes(stored[..4].try_into().unwrap()) == crate::summary::PACKAGE_MAGIC {
        let chunk = crate::summary::CompressedChunk {
            uncompressed_offset: 0,
            uncompressed_size: expected as i32,
            compressed_offset: 0,
            compressed_size: stored.len() as i32,
        };
        crate::decompress::decompress_chunk(stored, &chunk, method, &mut out)?;
        return Ok(out);
    }
    if method == crate::summary::COMPRESS_LZO {
        lzo::decompress_into(stored, &mut out).map_err(|error| PackageError::Lzo {
            offset: 0,
            reason: format!("{error:?}"),
        })?;
    } else {
        let mut inflater = flate2::Decompress::new(true);
        inflater
            .decompress(stored, &mut out, flate2::FlushDecompress::Finish)
            .map_err(|error| PackageError::Zlib {
                offset: 0,
                reason: error.to_string(),
            })?;
    }
    Ok(out)
}

struct MipWrite {
    flags: u32,
    element_count: i32,
    size: i32,
    offset: i32,
    payload: Option<Vec<u8>>,
    separate: bool,
}

pub struct CacheAppender {
    root: std::path::PathBuf,
    lengths: std::collections::BTreeMap<String, u64>,
    appended: std::collections::BTreeMap<String, Vec<u8>>,
}

impl CacheAppender {
    pub fn at(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            root: root.into(),
            lengths: std::collections::BTreeMap::new(),
            appended: std::collections::BTreeMap::new(),
        }
    }

    pub fn path(&self, name: &str) -> std::path::PathBuf {
        self.root.join(format!("{name}.tfc"))
    }

    pub fn original_length(&mut self, name: &str) -> Result<u64> {
        if let Some(length) = self.lengths.get(name) {
            return Ok(*length);
        }
        let length = std::fs::metadata(self.path(name))?.len();
        self.lengths.insert(name.to_string(), length);
        Ok(length)
    }

    pub fn reserve(&mut self, name: &str, data: &[u8]) -> Result<i32> {
        let base = self.original_length(name)?;
        let pending = self.appended.entry(name.to_string()).or_default();
        let offset = base + pending.len() as u64;
        pending.extend_from_slice(data);
        i32::try_from(offset).map_err(|_| {
            PackageError::UnsupportedProperty(format!("{name}.tfc is too large to extend"))
        })
    }

    pub fn touched(&self) -> impl Iterator<Item = (&String, u64)> {
        self.appended
            .keys()
            .map(|name| (name, self.lengths.get(name).copied().unwrap_or_default()))
    }

    pub fn flush(&self) -> Result<()> {
        use std::io::Write;
        for (name, data) in &self.appended {
            let mut file = std::fs::OpenOptions::new().append(true).open(self.path(name))?;
            file.write_all(data)?;
        }
        Ok(())
    }
}

impl Texture2D {
    pub fn replace_mips_with(
        &self,
        blob: &[u8],
        dds: &crate::dds::Dds,
        mut appender: Option<&mut CacheAppender>,
    ) -> Result<(Vec<u8>, usize)> {
        let wanted = crate::dds::unreal_format_for(dds.four_cc.as_ref(), dds.bits_per_pixel)?;
        if !self.format.is_empty() && self.format != wanted {
            return Err(PackageError::UnsupportedPixelFormat(format!(
                "texture is {} but the dds is {wanted}",
                self.format
            )));
        }
        if let Some(mip) = self.mips.first() {
            if dds.width as i32 != mip.width || dds.height as i32 != mip.height {
                return Err(PackageError::UnsupportedPixelFormat(format!(
                    "texture is {}x{} but the dds is {}x{}",
                    mip.width, mip.height, dds.width, dds.height
                )));
            }
        }
        let cache = self.cache_name();
        let mut plan: Vec<MipWrite> = Vec::with_capacity(self.mips.len());
        let mut replaced = 0usize;
        let mut resized = false;
        for (index, mip) in self.mips.iter().enumerate() {
            let separate = mip.data.flags & BULKDATA_STORE_IN_SEPARATE_FILE != 0;
            let mut write = MipWrite {
                flags: mip.data.flags,
                element_count: mip.data.element_count,
                size: mip.data.size_on_disk,
                offset: mip.data.offset_in_file,
                payload: None,
                separate,
            };
            match (dds.mips.get(index), separate) {
                (Some(source), true) => {
                    if let (Some(appender), Some(name)) = (appender.as_deref_mut(), cache.as_deref())
                    {
                        write.offset = appender.reserve(name, source)?;
                        write.flags &= !(BULKDATA_COMPRESSED_LZO | BULKDATA_COMPRESSED_ZLIB);
                        write.size = source.len() as i32;
                        write.element_count = write.size;
                        replaced += 1;
                    }
                }
                (Some(source), false) => {
                    if mip.data.is_inline() {
                        resized |= source.len() != mip.data.size_on_disk.max(0) as usize;
                        write.flags &= !(BULKDATA_COMPRESSED_LZO | BULKDATA_COMPRESSED_ZLIB);
                        write.size = source.len() as i32;
                        write.element_count = write.size;
                        write.payload = Some(source.clone());
                        replaced += 1;
                    }
                }
                _ => {}
            }
            plan.push(write);
        }
        if replaced == 0 {
            return Err(PackageError::NoPayload(self.name.clone()));
        }
        if resized {
            return Ok((self.rebuild_mip_table(blob, &plan)?, replaced));
        }
        let mut out = blob.to_vec();
        for (mip, write) in self.mips.iter().zip(&plan) {
            let at = mip.data.field_offset;
            if at + 16 > out.len() {
                continue;
            }
            out[at..at + 4].copy_from_slice(&write.flags.to_le_bytes());
            out[at + 4..at + 8].copy_from_slice(&write.element_count.to_le_bytes());
            out[at + 8..at + 12].copy_from_slice(&write.size.to_le_bytes());
            out[at + 12..at + 16].copy_from_slice(&write.offset.to_le_bytes());
            if let Some(payload) = &write.payload {
                let start = mip.data.payload_offset;
                let end = start + payload.len();
                if end <= out.len() {
                    out[start..end].copy_from_slice(payload);
                }
            }
        }
        Ok((out, replaced))
    }}

impl Texture2D {
    fn mip_table_start(&self) -> Option<usize> {
        self.mips.first().map(|mip| mip.data.field_offset - 4)
    }

    fn mip_table_end(&self) -> Option<usize> {
        self.mips
            .last()
            .map(|mip| mip.data.payload_offset + mip.data.payload.len() + 8)
    }

    fn inline_base(&self) -> Option<i32> {
        self.mips
            .iter()
            .find(|mip| mip.data.is_inline())
            .map(|mip| mip.data.offset_in_file - mip.data.payload_offset as i32)
    }

    fn rebuild_mip_table(&self, blob: &[u8], plan: &[MipWrite]) -> Result<Vec<u8>> {
        let (Some(start), Some(end), Some(base)) =
            (self.mip_table_start(), self.mip_table_end(), self.inline_base())
        else {
            return Err(PackageError::NoPayload(self.name.clone()));
        };
        if end > blob.len() {
            return Err(PackageError::Truncated {
                offset: start,
                needed: end,
                available: blob.len(),
            });
        }
        let mut out = Vec::with_capacity(blob.len());
        out.extend_from_slice(&blob[..start]);
        out.extend_from_slice(&(self.mips.len() as i32).to_le_bytes());
        for (mip, write) in self.mips.iter().zip(plan) {
            let payload: &[u8] = match &write.payload {
                Some(bytes) => bytes,
                None => &mip.data.payload,
            };
            let offset = if write.separate {
                write.offset
            } else {
                base + (out.len() + 16) as i32
            };
            out.extend_from_slice(&write.flags.to_le_bytes());
            out.extend_from_slice(&write.element_count.to_le_bytes());
            out.extend_from_slice(&write.size.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            if !write.separate {
                out.extend_from_slice(payload);
            }
            out.extend_from_slice(&mip.width.to_le_bytes());
            out.extend_from_slice(&mip.height.to_le_bytes());
        }
        out.extend_from_slice(&blob[end..]);
        Ok(out)
    }}

#[cfg(test)]
mod tests {
    use super::*;

    fn dxt5(width: u32, height: u32) -> Vec<u8> {
        vec![0x7f; (width.div_ceil(4) * height.div_ceil(4) * 16) as usize]
    }

    #[test]
    fn an_uncompressed_payload_passes_straight_through() {
        let stored = dxt5(8, 8);
        let out = inflate_mip(&stored, 0, stored.len()).unwrap();
        assert_eq!(out, stored);
    }

    #[test]
    fn a_payload_already_at_full_size_is_not_decompressed() {
        let stored = dxt5(8, 8);
        let out = inflate_mip(&stored, BULKDATA_COMPRESSED_LZO, stored.len()).unwrap();
        assert_eq!(out, stored);
    }

    #[test]
    fn a_zlib_payload_round_trips() {
        let raw = dxt5(16, 16);
        let mut deflater = flate2::Compress::new(flate2::Compression::default(), true);
        let mut packed = Vec::with_capacity(raw.len());
        deflater
            .compress_vec(&raw, &mut packed, flate2::FlushCompress::Finish)
            .unwrap();
        assert!(packed.len() < raw.len());
        let out = inflate_mip(&packed, BULKDATA_COMPRESSED_ZLIB, raw.len()).unwrap();
        assert_eq!(out, raw);
    }

    #[test]
    fn a_cache_appender_hands_out_offsets_that_do_not_overlap() {
        let directory = std::env::temp_dir().join("tera-cache-appender");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("Cache.tfc"), vec![0u8; 100]).unwrap();

        let mut appender = CacheAppender::at(&directory);
        assert_eq!(appender.reserve("Cache", &[1, 2, 3, 4]).unwrap(), 100);
        assert_eq!(appender.reserve("Cache", &[5, 6]).unwrap(), 104);
        assert_eq!(appender.touched().collect::<Vec<_>>().len(), 1);

        appender.flush().unwrap();
        let after = std::fs::read(directory.join("Cache.tfc")).unwrap();
        assert_eq!(after.len(), 106);
        assert_eq!(&after[100..], &[1, 2, 3, 4, 5, 6]);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
