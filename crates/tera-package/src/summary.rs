use crate::error::{PackageError, Result};
use crate::reader::Reader;

pub const PACKAGE_MAGIC: u32 = 0x9e2a_83c1;
pub const COMPRESS_ZLIB: u32 = 0x01;
pub const COMPRESS_LZO: u32 = 0x02;
pub const COMPRESS_LZX: u32 = 0x04;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Text {
    pub value: String,
    terminated: bool,
    wide: bool,
}

impl std::fmt::Display for Text {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.value)
    }
}

impl From<&str> for Text {
    fn from(value: &str) -> Self {
        Self {
            value: value.to_string(),
            terminated: true,
            wide: !value.is_ascii(),
        }
    }
}

impl Text {
    pub fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let length = reader.i32()?;
        match length {
            0 => Ok(Self::default()),
            length if length > 0 => {
                let bytes = reader.take(length as usize)?;
                let terminated = bytes.last() == Some(&0);
                let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
                Ok(Self {
                    value: bytes[..end].iter().map(|byte| *byte as char).collect(),
                    terminated,
                    wide: false,
                })
            }
            length => {
                let count = length
                    .checked_neg()
                    .ok_or(PackageError::BadStringLength(length))? as usize;
                let bytes = reader.take(count * 2)?;
                let (pairs, _) = bytes.as_chunks::<2>();
                let units: Vec<u16> = pairs.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
                let terminated = units.last() == Some(&0);
                let end = units.iter().position(|unit| *unit == 0).unwrap_or(units.len());
                Ok(Self {
                    value: String::from_utf16_lossy(&units[..end]),
                    terminated,
                    wide: true,
                })
            }
        }
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        if self.value.is_empty() && !self.terminated {
            out.extend_from_slice(&0i32.to_le_bytes());
            return;
        }
        let extra = usize::from(self.terminated);
        if self.wide {
            let units: Vec<u16> = self.value.encode_utf16().collect();
            out.extend_from_slice(&(-((units.len() + extra) as i32)).to_le_bytes());
            for unit in units {
                out.extend_from_slice(&unit.to_le_bytes());
            }
            if self.terminated {
                out.extend_from_slice(&0u16.to_le_bytes());
            }
        } else {
            out.extend_from_slice(&((self.value.len() + extra) as i32).to_le_bytes());
            out.extend_from_slice(self.value.as_bytes());
            if self.terminated {
                out.push(0);
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CompressedChunk {
    pub uncompressed_offset: i32,
    pub uncompressed_size: i32,
    pub compressed_offset: i32,
    pub compressed_size: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Generation {
    pub export_count: i32,
    pub name_count: i32,
    pub net_object_count: i32,
}

#[derive(Clone, Debug)]
pub struct Summary {
    pub version: u16,
    pub licensee: u16,
    pub total_header_size: i32,
    pub folder_name: Text,
    pub package_flags: u32,
    pub name_count: i32,
    pub name_offset: i32,
    pub export_count: i32,
    pub export_offset: i32,
    pub import_count: i32,
    pub import_offset: i32,
    pub depends_offset: i32,
    pub import_export_guids_offset: i32,
    pub thumbnail_table_offset: i32,
    pub guid: [u8; 16],
    pub generations: Vec<Generation>,
    pub engine_version: u32,
    pub cooker_version: u32,
    pub compression_flags: u32,
    pub compressed_chunks: Vec<CompressedChunk>,
    pub package_source: u32,
    pub additional_packages_to_cook: Vec<Text>,
    pub header_end: usize,
}

impl Summary {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(data);
        let magic = reader.u32()?;
        if magic != PACKAGE_MAGIC {
            return Err(PackageError::BadMagic(magic));
        }
        let version = reader.u16()?;
        let licensee = reader.u16()?;
        if version < 600 {
            return Err(PackageError::UnsupportedVersion { version, licensee });
        }
        let total_header_size = reader.i32()?;
        let folder_name = Text::read(&mut reader)?;
        let package_flags = reader.u32()?;
        let name_count = reader.i32()?;
        let name_offset = reader.i32()?;
        let export_count = reader.i32()?;
        let export_offset = reader.i32()?;
        let import_count = reader.i32()?;
        let import_offset = reader.i32()?;
        let depends_offset = reader.i32()?;
        let import_export_guids_offset = reader.i32()?;
        let _import_guids_count = reader.i32()?;
        let _export_guids_count = reader.i32()?;
        let thumbnail_table_offset = reader.i32()?;
        let guid = reader.guid()?;
        let generations = reader.array(|reader| {
            Ok(Generation {
                export_count: reader.i32()?,
                name_count: reader.i32()?,
                net_object_count: reader.i32()?,
            })
        })?;
        let engine_version = reader.u32()?;
        let cooker_version = reader.u32()?;
        let compression_flags = reader.u32()?;
        let compressed_chunks = reader.array(|reader| {
            Ok(CompressedChunk {
                uncompressed_offset: reader.i32()?,
                uncompressed_size: reader.i32()?,
                compressed_offset: reader.i32()?,
                compressed_size: reader.i32()?,
            })
        })?;
        let package_source = reader.u32()?;
        let additional_packages_to_cook = reader.array(Text::read)?;
        let _texture_allocations = reader.i32()?;
        Ok(Self {
            version,
            licensee,
            total_header_size,
            folder_name,
            package_flags,
            name_count,
            name_offset,
            export_count,
            export_offset,
            import_count,
            import_offset,
            depends_offset,
            import_export_guids_offset,
            thumbnail_table_offset,
            guid,
            generations,
            engine_version,
            cooker_version,
            compression_flags,
            compressed_chunks,
            package_source,
            additional_packages_to_cook,
            header_end: reader.offset(),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(&PACKAGE_MAGIC.to_le_bytes());
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.licensee.to_le_bytes());
        out.extend_from_slice(&self.total_header_size.to_le_bytes());
        self.folder_name.write(&mut out);
        out.extend_from_slice(&self.package_flags.to_le_bytes());
        out.extend_from_slice(&self.name_count.to_le_bytes());
        out.extend_from_slice(&self.name_offset.to_le_bytes());
        out.extend_from_slice(&self.export_count.to_le_bytes());
        out.extend_from_slice(&self.export_offset.to_le_bytes());
        out.extend_from_slice(&self.import_count.to_le_bytes());
        out.extend_from_slice(&self.import_offset.to_le_bytes());
        out.extend_from_slice(&self.depends_offset.to_le_bytes());
        out.extend_from_slice(&self.import_export_guids_offset.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&self.thumbnail_table_offset.to_le_bytes());
        out.extend_from_slice(&self.guid);
        out.extend_from_slice(&(self.generations.len() as i32).to_le_bytes());
        for generation in &self.generations {
            out.extend_from_slice(&generation.export_count.to_le_bytes());
            out.extend_from_slice(&generation.name_count.to_le_bytes());
            out.extend_from_slice(&generation.net_object_count.to_le_bytes());
        }
        out.extend_from_slice(&self.engine_version.to_le_bytes());
        out.extend_from_slice(&self.cooker_version.to_le_bytes());
        out.extend_from_slice(&self.compression_flags.to_le_bytes());
        out.extend_from_slice(&(self.compressed_chunks.len() as i32).to_le_bytes());
        for chunk in &self.compressed_chunks {
            out.extend_from_slice(&chunk.uncompressed_offset.to_le_bytes());
            out.extend_from_slice(&chunk.uncompressed_size.to_le_bytes());
            out.extend_from_slice(&chunk.compressed_offset.to_le_bytes());
            out.extend_from_slice(&chunk.compressed_size.to_le_bytes());
        }
        out.extend_from_slice(&self.package_source.to_le_bytes());
        out.extend_from_slice(&(self.additional_packages_to_cook.len() as i32).to_le_bytes());
        for package in &self.additional_packages_to_cook {
            package.write(&mut out);
        }
        out.extend_from_slice(&0i32.to_le_bytes());
        out
    }

    pub fn is_compressed(&self) -> bool {
        self.compression_flags != 0 && !self.compressed_chunks.is_empty()
    }

    pub fn compressed_end(&self) -> usize {
        self.compressed_chunks
            .iter()
            .map(|chunk| (chunk.compressed_offset + chunk.compressed_size) as usize)
            .max()
            .unwrap_or(0)
    }

    pub fn uncompressed_size(&self) -> usize {
        self.compressed_chunks
            .iter()
            .map(|chunk| (chunk.uncompressed_offset + chunk.uncompressed_size) as usize)
            .max()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(stored: &[u8]) -> Vec<u8> {
        let mut reader = Reader::new(stored);
        let text = Text::read(&mut reader).unwrap();
        let mut out = Vec::new();
        text.write(&mut out);
        out
    }

    #[test]
    fn a_string_without_its_terminator_is_written_back_the_same_length() {
        let mut stored = 3i32.to_le_bytes().to_vec();
        stored.extend_from_slice(b"abc");
        assert_eq!(round_trip(&stored), stored);
    }

    #[test]
    fn a_terminated_string_keeps_its_terminator() {
        let mut stored = 4i32.to_le_bytes().to_vec();
        stored.extend_from_slice(b"abc\0");
        assert_eq!(round_trip(&stored), stored);
    }

    #[test]
    fn a_wide_string_stays_wide() {
        let mut stored = (-4i32).to_le_bytes().to_vec();
        for unit in "ab\u{e9}\0".encode_utf16() {
            stored.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(round_trip(&stored), stored);
    }

    #[test]
    fn an_empty_string_stays_a_bare_zero() {
        assert_eq!(round_trip(&0i32.to_le_bytes()), 0i32.to_le_bytes());
    }

    #[test]
    fn the_folder_name_that_broke_texture_modding_round_trips() {
        let name = b"MOD:c7a706fb_5c7e778e_1c59.CharacterWindow_dup";
        let mut stored = (name.len() as i32).to_le_bytes().to_vec();
        stored.extend_from_slice(name);
        assert_eq!(round_trip(&stored).len(), stored.len());
        assert_eq!(round_trip(&stored), stored);
    }
}
