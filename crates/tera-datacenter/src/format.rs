use crate::error::{DataCenterError, Result};

pub const NODE_SIZE: usize = 24;
pub const ATTRIBUTE_SIZE: usize = 12;
pub const STRING_ENTRY_SIZE: usize = 16;
pub const KEY_SIZE: usize = 8;
pub const VALUE_TABLE_SEGMENTS: usize = 1024;
pub const NAME_TABLE_SEGMENTS: usize = 512;
pub const ROOT_NAME: &str = "__root__";
pub const TEXT_NAME: &str = "__value__";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Address {
    pub segment: u16,
    pub element: u16,
}

impl Address {
    pub const NONE: Address = Address {
        segment: u16::MAX,
        element: u16::MAX,
    };

    pub fn from_raw(raw: u32) -> Self {
        Self {
            segment: (raw & 0xffff) as u16,
            element: (raw >> 16) as u16,
        }
    }

    pub fn to_raw(self) -> u32 {
        u32::from(self.segment) | (u32::from(self.element) << 16)
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.segment, self.element)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub version: u32,
    pub timestamp: f64,
    pub revision: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyDefinition {
    pub name_indexes: [u16; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct SegmentSpan {
    pub offset: usize,
    pub full: u32,
    pub used: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RawNode {
    pub name_index: u16,
    pub key_flags: u8,
    pub key_index: u16,
    pub attribute_count: u16,
    pub child_count: u16,
    pub attribute_address: Address,
    pub child_address: Address,
}

impl RawNode {
    pub fn empty() -> Self {
        Self {
            name_index: 0,
            key_flags: 0,
            key_index: 0,
            attribute_count: 0,
            child_count: 0,
            attribute_address: Address::NONE,
            child_address: Address::NONE,
        }
    }

    pub fn parse(bytes: &[u8]) -> Self {
        let name_index = u16::from_le_bytes([bytes[0], bytes[1]]);
        let key = u16::from_le_bytes([bytes[2], bytes[3]]);
        Self {
            name_index,
            key_flags: (key & 0xf) as u8,
            key_index: key >> 4,
            attribute_count: u16::from_le_bytes([bytes[4], bytes[5]]),
            child_count: u16::from_le_bytes([bytes[6], bytes[7]]),
            attribute_address: Address::from_raw(u32::from_le_bytes([
                bytes[8], bytes[9], bytes[10], bytes[11],
            ])),
            child_address: Address::from_raw(u32::from_le_bytes([
                bytes[16], bytes[17], bytes[18], bytes[19],
            ])),
        }
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.name_index.to_le_bytes());
        out.extend_from_slice(&(u16::from(self.key_flags) & 0xf | self.key_index << 4).to_le_bytes());
        out.extend_from_slice(&self.attribute_count.to_le_bytes());
        out.extend_from_slice(&self.child_count.to_le_bytes());
        out.extend_from_slice(&self.attribute_address.to_raw().to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&self.child_address.to_raw().to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeCode {
    Int,
    Float,
    String,
}

impl TypeCode {
    pub fn from_bits(bits: u16) -> Option<Self> {
        match bits & 0x3 {
            1 => Some(Self::Int),
            2 => Some(Self::Float),
            3 => Some(Self::String),
            _ => None,
        }
    }

    pub fn bits(self) -> u16 {
        match self {
            Self::Int => 1,
            Self::Float => 2,
            Self::String => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RawAttribute {
    pub name_index: u16,
    pub type_info: u16,
    pub value: u32,
}

impl RawAttribute {
    pub fn empty() -> Self {
        Self {
            name_index: 0,
            type_info: 0,
            value: 0,
        }
    }

    pub fn parse(bytes: &[u8]) -> Self {
        Self {
            name_index: u16::from_le_bytes([bytes[0], bytes[1]]),
            type_info: u16::from_le_bytes([bytes[2], bytes[3]]),
            value: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.name_index.to_le_bytes());
        out.extend_from_slice(&self.type_info.to_le_bytes());
        out.extend_from_slice(&self.value.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
    }

    pub fn type_code(&self) -> Option<TypeCode> {
        TypeCode::from_bits(self.type_info)
    }

    pub fn extended_code(&self) -> u16 {
        self.type_info >> 2
    }

    pub fn is_bool(&self) -> bool {
        self.type_code() == Some(TypeCode::Int) && self.extended_code() & 1 == 1
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StringEntry {
    pub hash: u32,
    pub length: u32,
    pub index: u32,
    pub address: Address,
}

pub struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.offset
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(DataCenterError::Truncated {
                offset: self.offset,
                needed: count,
                available: self.remaining(),
            })?;
        if end > self.data.len() {
            return Err(DataCenterError::Truncated {
                offset: self.offset,
                needed: count,
                available: self.remaining(),
            });
        }
        let slice = &self.data[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    pub fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn f64(&mut self) -> Result<f64> {
        let bytes = self.take(8)?;
        Ok(f64::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn address(&mut self) -> Result<Address> {
        Ok(Address::from_raw(self.u32()?))
    }

    pub fn segmented_region(&mut self, element_size: usize) -> Result<Vec<SegmentSpan>> {
        let count = self.u32()? as usize;
        let mut segments = Vec::with_capacity(count);
        for _ in 0..count {
            let full = self.u32()?;
            let used = self.u32()?;
            let offset = self.offset;
            self.take(full as usize * element_size)?;
            segments.push(SegmentSpan { offset, full, used });
        }
        Ok(segments)
    }

    pub fn string_entries(&mut self, segment_count: usize) -> Result<Vec<StringEntry>> {
        let mut entries = Vec::new();
        for _ in 0..segment_count {
            let count = self.u32()? as usize;
            entries.reserve(count);
            for _ in 0..count {
                let bytes = self.take(STRING_ENTRY_SIZE)?;
                entries.push(StringEntry {
                    hash: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                    length: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
                    index: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
                    address: Address::from_raw(u32::from_le_bytes([
                        bytes[12], bytes[13], bytes[14], bytes[15],
                    ])),
                });
            }
        }
        Ok(entries)
    }

    pub fn address_region(&mut self) -> Result<Vec<Address>> {
        let count = self.u32()? as usize;
        let count = count.saturating_sub(1);
        let mut addresses = Vec::with_capacity(count);
        for _ in 0..count {
            addresses.push(self.address()?);
        }
        Ok(addresses)
    }
}
