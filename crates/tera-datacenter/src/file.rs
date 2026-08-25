use crate::error::{DataCenterError, Result};
use crate::format::*;
use flate2::{Decompress, FlushDecompress, Status};
use std::borrow::Cow;
use std::path::Path;
use tera_crypto::{decrypt_in_place, known_keys, KeyIv, ZlibOracle};

pub struct StringTable {
    pub data_segments: Vec<SegmentSpan>,
    pub entries: Vec<StringEntry>,
    pub addresses: Vec<Address>,
}

pub struct DataCenter {
    data: Vec<u8>,
    pub header: Header,
    pub keys: Vec<KeyDefinition>,
    pub attribute_segments: Vec<SegmentSpan>,
    pub node_segments: Vec<SegmentSpan>,
    pub values: StringTable,
    pub names: StringTable,
    name_strings: Vec<Box<str>>,
    pub keyiv: Option<KeyIv>,
}

impl DataCenter {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut bytes = std::fs::read(path.as_ref())?;
        let keyiv = detect_key(&bytes).ok_or(DataCenterError::UnknownKey)?;
        decrypt_in_place(&keyiv, &mut bytes);
        let mut dc = Self::from_decrypted(&bytes)?;
        dc.keyiv = Some(keyiv);
        Ok(dc)
    }

    pub fn open_with_key(path: impl AsRef<Path>, keyiv: &KeyIv) -> Result<Self> {
        let mut bytes = std::fs::read(path.as_ref())?;
        decrypt_in_place(keyiv, &mut bytes);
        let mut dc = Self::from_decrypted(&bytes)?;
        dc.keyiv = Some(*keyiv);
        Ok(dc)
    }

    pub fn open_plain(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = std::fs::read(path.as_ref())?;
        Self::from_inflated(bytes)
    }

    pub fn from_decrypted(bytes: &[u8]) -> Result<Self> {
        Self::from_inflated(inflate(bytes)?)
    }

    pub fn from_inflated(data: Vec<u8>) -> Result<Self> {
        let mut reader = Reader::new(&data);
        let version = reader.u32()?;
        if version != 6 {
            return Err(DataCenterError::UnsupportedVersion(version));
        }
        let timestamp = reader.f64()?;
        let revision = reader.u32()?;
        reader.i16()?;
        reader.i16()?;
        reader.i32()?;
        reader.i32()?;
        reader.i32()?;
        let header = Header {
            version,
            timestamp,
            revision,
        };

        let key_count = reader.u32()? as usize;
        let mut keys = Vec::with_capacity(key_count);
        for _ in 0..key_count {
            let bytes = reader.take(KEY_SIZE)?;
            keys.push(KeyDefinition {
                name_indexes: [
                    u16::from_le_bytes([bytes[0], bytes[1]]),
                    u16::from_le_bytes([bytes[2], bytes[3]]),
                    u16::from_le_bytes([bytes[4], bytes[5]]),
                    u16::from_le_bytes([bytes[6], bytes[7]]),
                ],
            });
        }

        let attribute_segments = reader.segmented_region(ATTRIBUTE_SIZE)?;
        let node_segments = reader.segmented_region(NODE_SIZE)?;
        let values = read_string_table(&mut reader, VALUE_TABLE_SEGMENTS)?;
        let names = read_string_table(&mut reader, NAME_TABLE_SEGMENTS)?;

        let mut dc = Self {
            data,
            header,
            keys,
            attribute_segments,
            node_segments,
            values,
            names,
            name_strings: Vec::new(),
            keyiv: None,
        };
        dc.name_strings = dc.build_name_cache()?;
        Ok(dc)
    }

    fn build_name_cache(&self) -> Result<Vec<Box<str>>> {
        self.names
            .addresses
            .iter()
            .map(|address| {
                self.read_string(&self.names, *address, "names")
                    .map(|text| text.into_owned().into_boxed_str())
            })
            .collect()
    }

    pub fn raw_len(&self) -> usize {
        self.data.len()
    }

    pub fn node_count(&self) -> u64 {
        self.node_segments.iter().map(|s| u64::from(s.used)).sum()
    }

    pub fn attribute_count(&self) -> u64 {
        self.attribute_segments
            .iter()
            .map(|s| u64::from(s.used))
            .sum()
    }

    pub fn name(&self, index: u16) -> Result<&str> {
        if index == 0 {
            return Err(DataCenterError::BadNameIndex(index));
        }
        self.name_strings
            .get(usize::from(index) - 1)
            .map(|name| &**name)
            .ok_or(DataCenterError::BadNameIndex(index))
    }

    pub fn names_iter(&self) -> impl Iterator<Item = &str> {
        self.name_strings.iter().map(|name| &**name)
    }

    pub fn raw_node(&self, address: Address) -> Result<RawNode> {
        let segment = self
            .node_segments
            .get(usize::from(address.segment))
            .ok_or(DataCenterError::BadAddress {
                region: "nodes",
                segment: address.segment,
                element: address.element,
            })?;
        if u32::from(address.element) >= segment.full {
            return Err(DataCenterError::BadAddress {
                region: "nodes",
                segment: address.segment,
                element: address.element,
            });
        }
        let offset = segment.offset + usize::from(address.element) * NODE_SIZE;
        Ok(RawNode::parse(&self.data[offset..offset + NODE_SIZE]))
    }

    pub fn raw_attribute(&self, address: Address, index: u16) -> Result<RawAttribute> {
        let element = usize::from(address.element) + usize::from(index);
        let segment = self
            .attribute_segments
            .get(usize::from(address.segment))
            .ok_or(DataCenterError::BadAddress {
                region: "attributes",
                segment: address.segment,
                element: address.element,
            })?;
        if element >= segment.full as usize {
            return Err(DataCenterError::BadAddress {
                region: "attributes",
                segment: address.segment,
                element: address.element,
            });
        }
        let offset = segment.offset + element * ATTRIBUTE_SIZE;
        Ok(RawAttribute::parse(&self.data[offset..offset + ATTRIBUTE_SIZE]))
    }

    pub fn value_units(&self, address: Address) -> Result<&[u8]> {
        self.string_units(&self.values, address, "values")
    }

    pub fn value_string(&self, address: Address) -> Result<Cow<'_, str>> {
        self.read_string(&self.values, address, "values")
    }

    fn string_units(
        &self,
        table: &StringTable,
        address: Address,
        region: &'static str,
    ) -> Result<&[u8]> {
        let segment = table
            .data_segments
            .get(usize::from(address.segment))
            .ok_or(DataCenterError::BadAddress {
                region,
                segment: address.segment,
                element: address.element,
            })?;
        let start_unit = usize::from(address.element);
        if start_unit >= segment.full as usize {
            return Err(DataCenterError::BadAddress {
                region,
                segment: address.segment,
                element: address.element,
            });
        }
        let start = segment.offset + start_unit * 2;
        let end = segment.offset + segment.full as usize * 2;
        let window = &self.data[start..end];
        let mut length = window.len() / 2;
        for index in 0..window.len() / 2 {
            if window[index * 2] == 0 && window[index * 2 + 1] == 0 {
                length = index;
                break;
            }
        }
        Ok(&window[..length * 2])
    }

    fn read_string(
        &self,
        table: &StringTable,
        address: Address,
        region: &'static str,
    ) -> Result<Cow<'_, str>> {
        let units = self.string_units(table, address, region)?;
        Ok(Cow::Owned(decode_utf16(units)))
    }

    pub fn string_equals(&self, address: Address, needle: &str) -> bool {
        let Ok(units) = self.value_units(address) else {
            return false;
        };
        let mut expected = needle.encode_utf16();
        let (pairs, _) = units.as_chunks::<2>();
        for pair in pairs {
            let unit = u16::from_le_bytes(*pair);
            match expected.next() {
                Some(other) if other == unit => {}
                _ => return false,
            }
        }
        expected.next().is_none()
    }

    pub fn root(&self) -> Result<crate::node::Node<'_>> {
        let address = Address {
            segment: 0,
            element: 0,
        };
        crate::node::Node::new(self, address)
    }
}

fn read_string_table(reader: &mut Reader<'_>, segment_count: usize) -> Result<StringTable> {
    let data_segments = reader.segmented_region(2)?;
    let entries = reader.string_entries(segment_count)?;
    let addresses = reader.address_region()?;
    Ok(StringTable {
        data_segments,
        entries,
        addresses,
    })
}

pub fn decode_utf16(units: &[u8]) -> String {
    let (pairs, _) = units.as_chunks::<2>();
    let iter = pairs.iter().map(|pair| u16::from_le_bytes(*pair));
    char::decode_utf16(iter)
        .map(|result| result.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

pub fn deflate(image: &[u8], level: u32) -> Result<Vec<u8>> {
    use flate2::{Compress, Compression, FlushCompress, Status};
    let mut out = Vec::with_capacity(image.len() / 3 + 1024);
    out.extend_from_slice(&(image.len() as u32).to_le_bytes());
    let mut compressor = Compress::new(Compression::new(level), true);
    let mut buffer = vec![0u8; 1 << 20];
    let mut input = image;
    loop {
        let before_in = compressor.total_in();
        let before_out = compressor.total_out();
        let finishing = input.is_empty();
        let flush = match finishing {
            true => FlushCompress::Finish,
            false => FlushCompress::None,
        };
        let status = compressor
            .compress(input, &mut buffer, flush)
            .map_err(|error| DataCenterError::Inflate(error.to_string()))?;
        let consumed = (compressor.total_in() - before_in) as usize;
        let produced = (compressor.total_out() - before_out) as usize;
        out.extend_from_slice(&buffer[..produced]);
        input = &input[consumed..];
        match status {
            Status::StreamEnd => break,
            _ if finishing && produced == 0 => {
                return Err(DataCenterError::Inflate(
                    "deflate stalled before the stream ended".into(),
                ))
            }
            _ => {}
        }
    }
    Ok(out)
}

pub fn wrap(image: &[u8], keyiv: &KeyIv, level: u32) -> Result<Vec<u8>> {
    let mut payload = deflate(image, level)?;
    tera_crypto::encrypt_in_place(keyiv, &mut payload);
    Ok(payload)
}

pub fn detect_key(encrypted: &[u8]) -> Option<KeyIv> {
    let oracle = ZlibOracle::new(
        &encrypted[..encrypted.len().min(tera_crypto::ORACLE_PREFIX_LEN)],
        encrypted.len() as u64,
    );
    known_keys()
        .iter()
        .map(|known| known.keyiv())
        .find(|keyiv| oracle.verify(keyiv))
}

pub fn inflate(decrypted: &[u8]) -> Result<Vec<u8>> {
    if decrypted.len() < 6 {
        return Err(DataCenterError::Truncated {
            offset: 0,
            needed: 6,
            available: decrypted.len(),
        });
    }
    let declared = u32::from_le_bytes([decrypted[0], decrypted[1], decrypted[2], decrypted[3]]);
    let mut out = Vec::with_capacity(declared as usize);
    let mut inflater = Decompress::new(true);
    let mut input = &decrypted[4..];
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let before_in = inflater.total_in();
        let status = inflater
            .decompress(input, &mut buffer, FlushDecompress::None)
            .map_err(|error| DataCenterError::Inflate(error.to_string()))?;
        let consumed = (inflater.total_in() - before_in) as usize;
        let produced = inflater.total_out() as usize - out.len();
        out.extend_from_slice(&buffer[..produced]);
        input = &input[consumed..];
        match status {
            Status::StreamEnd => break,
            Status::BufError if consumed == 0 && produced == 0 => break,
            _ => {}
        }
        if input.is_empty() && produced == 0 {
            break;
        }
    }
    if out.len() != declared as usize {
        return Err(DataCenterError::SizeMismatch {
            declared,
            actual: out.len(),
        });
    }
    Ok(out)
}
