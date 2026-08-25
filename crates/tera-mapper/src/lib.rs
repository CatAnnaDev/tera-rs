use std::collections::HashMap;
use std::path::Path;

pub const PKG_MAPPER_MAGIC: u32 = 0x6a87_ac80;
pub const OBJECT_REDIRECTOR_MAGIC: u32 = 0x6a62_4e14;

#[derive(Debug, thiserror::Error)]
pub enum MapperError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unexpected end of file at offset {0}")]
    Truncated(usize),
    #[error("unexpected magic {found:#010x}, expected {expected:#010x}")]
    Magic { found: u32, expected: u32 },
    #[error("entry {index} is not valid utf-8")]
    Encoding { index: usize },
    #[error("trailing data: {0} bytes left unread")]
    Trailing(usize),
}

pub type Result<T> = std::result::Result<T, MapperError>;

struct Cursor<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn u32(&mut self) -> Result<u32> {
        let end = self.offset + 4;
        if end > self.data.len() {
            return Err(MapperError::Truncated(self.offset));
        }
        let value = u32::from_le_bytes(self.data[self.offset..end].try_into().unwrap());
        self.offset = end;
        Ok(value)
    }

    fn string(&mut self, index: usize) -> Result<String> {
        let length = self.u32()? as usize;
        let end = self.offset + length;
        if end > self.data.len() {
            return Err(MapperError::Truncated(self.offset));
        }
        let bytes = &self.data[self.offset..end];
        self.offset = end;
        String::from_utf8(bytes.to_vec()).map_err(|_| MapperError::Encoding { index })
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.offset
    }
}

fn write_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}

#[derive(Clone, Debug, Default)]
pub struct PairMapper {
    pub magic: u32,
    pub reserved: u32,
    pub entries: Vec<(String, String)>,
}

impl PairMapper {
    pub fn parse(data: &[u8], expected_magic: u32) -> Result<Self> {
        let mut cursor = Cursor::new(data);
        let magic = cursor.u32()?;
        if magic != expected_magic {
            return Err(MapperError::Magic {
                found: magic,
                expected: expected_magic,
            });
        }
        let reserved = cursor.u32()?;
        let count = cursor.u32()? as usize;
        let mut entries = Vec::with_capacity(count);
        for index in 0..count {
            let key = cursor.string(index)?;
            let value = cursor.string(index)?;
            entries.push((key, value));
        }
        if cursor.remaining() != 0 {
            return Err(MapperError::Trailing(cursor.remaining()));
        }
        Ok(Self {
            magic,
            reserved,
            entries,
        })
    }

    pub fn read(path: impl AsRef<Path>, expected_magic: u32) -> Result<Self> {
        Self::parse(&std::fs::read(path)?, expected_magic)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let capacity = 12 + self
            .entries
            .iter()
            .map(|(key, value)| key.len() + value.len() + 8)
            .sum::<usize>();
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(&self.magic.to_le_bytes());
        out.extend_from_slice(&self.reserved.to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (key, value) in &self.entries {
            write_string(&mut out, key);
            write_string(&mut out, value);
        }
        out
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.to_bytes())?;
        Ok(())
    }

    pub fn index(&self) -> HashMap<&str, &str> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }

    pub fn lookup(&self, key: &str) -> Option<&str> {
        let needle = key.to_ascii_uppercase();
        self.entries
            .iter()
            .find(|(entry, _)| entry.eq_ignore_ascii_case(&needle))
            .map(|(_, value)| value.as_str())
    }

    pub fn upsert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        match self
            .entries
            .iter_mut()
            .find(|(entry, _)| entry.eq_ignore_ascii_case(&key))
        {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    pub fn remove(&mut self, key: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(entry, _)| !entry.eq_ignore_ascii_case(key));
        before != self.entries.len()
    }
}

pub struct PkgMapper;

impl PkgMapper {
    pub fn read(path: impl AsRef<Path>) -> Result<PairMapper> {
        PairMapper::read(path, PKG_MAPPER_MAGIC)
    }
}

pub struct ObjectRedirectorMapper;

impl ObjectRedirectorMapper {
    pub fn read(path: impl AsRef<Path>) -> Result<PairMapper> {
        PairMapper::read(path, OBJECT_REDIRECTOR_MAGIC)
    }
}

#[derive(Clone, Debug, Default)]
pub struct DirCache {
    pub entries: Vec<String>,
}

impl DirCache {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(data);
        let count = cursor.u32()? as usize;
        let mut entries = Vec::with_capacity(count);
        for index in 0..count {
            entries.push(cursor.string(index)?);
        }
        if cursor.remaining() != 0 {
            return Err(MapperError::Trailing(cursor.remaining()));
        }
        Ok(Self { entries })
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        Self::parse(&std::fs::read(path)?)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.entries.iter().map(|e| e.len() + 4).sum::<usize>());
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            write_string(&mut out, entry);
        }
        out
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.to_bytes())?;
        Ok(())
    }

    pub fn package_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter_map(|entry| {
            let file = entry.rsplit(['\\', '/']).next()?;
            file.strip_suffix(".gpk").or_else(|| file.strip_suffix(".upk"))
        })
    }

    pub fn contains_package(&self, name: &str) -> bool {
        self.package_names().any(|entry| entry.eq_ignore_ascii_case(name))
    }

    pub fn push(&mut self, relative_path: impl Into<String>) {
        let value = relative_path.into();
        if !self.entries.iter().any(|entry| entry.eq_ignore_ascii_case(&value)) {
            self.entries.push(value);
        }
    }
}

pub fn split_object_path(path: &str) -> (&str, Option<&str>, &str) {
    let mut parts = path.split('.');
    let package = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();
    match rest.len() {
        0 => (package, None, ""),
        1 => (package, None, rest[0]),
        _ => (package, Some(rest[0]), rest[rest.len() - 1]),
    }
}
