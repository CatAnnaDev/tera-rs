use crate::error::{PackageError, Result};
use crate::package::{Export, Package};
use crate::properties::{read_export_properties, Property, PropertyValue};
use crate::reader::Reader;
use crate::texture::BulkData;

#[derive(Clone, Debug)]
pub struct SoundNodeWave {
    pub name: String,
    pub properties: Vec<Property>,
    pub duration: f32,
    pub channels: i32,
    pub sample_rate: i32,
    pub ogg: Option<Vec<u8>>,
    pub span: Option<BulkSpan>,
}

#[derive(Clone, Copy, Debug)]
pub struct BulkSpan {
    pub field_offset: usize,
    pub payload_offset: usize,
    pub size: usize,
    pub flags: u32,
}

impl SoundNodeWave {
    pub fn parse(package: &Package<'_>, export: &Export) -> Result<Self> {
        let data = package.export_data(export)?;
        let (properties, consumed) = read_export_properties(package, data)?;
        let base = export.serial_offset.max(0) as usize;
        let spans = inline_bulk_spans(data, base, consumed);
        let span = spans
            .into_iter()
            .find(|span| data[span.payload_offset..].starts_with(b"OggS"));
        let ogg = span.map(|span| data[span.payload_offset..span.payload_offset + span.size].to_vec());
        Ok(Self {
            name: package.name_text(export.object_name),
            duration: float_property(&properties, "Duration").unwrap_or_default(),
            channels: int_property(&properties, "NumChannels").unwrap_or_default(),
            sample_rate: int_property(&properties, "SampleRate").unwrap_or_default(),
            properties,
            ogg,
            span,
        })
    }

    pub fn payload(&self) -> Result<&[u8]> {
        self.ogg
            .as_deref()
            .ok_or_else(|| PackageError::NoPayload(self.name.clone()))
    }
}

pub fn inline_bulk_payloads(blob: &[u8], base: usize, start: usize) -> Vec<Vec<u8>> {
    inline_bulk_spans(blob, base, start)
        .into_iter()
        .map(|span| blob[span.payload_offset..span.payload_offset + span.size].to_vec())
        .collect()
}

pub fn inline_bulk_spans(blob: &[u8], base: usize, start: usize) -> Vec<BulkSpan> {
    let mut spans = Vec::new();
    let mut position = start;
    while position + 16 <= blob.len() {
        let mut reader = Reader::at(blob, position);
        let Ok(bulk) = BulkData::read(&mut reader) else {
            break;
        };
        let payload_start = position + 16;
        if bulk.size_on_disk > 0
            && bulk.offset_in_file >= 0
            && bulk.offset_in_file as usize == base + payload_start
            && payload_start + bulk.size_on_disk as usize <= blob.len()
        {
            let end = payload_start + bulk.size_on_disk as usize;
            spans.push(BulkSpan {
                field_offset: position,
                payload_offset: payload_start,
                size: bulk.size_on_disk as usize,
                flags: bulk.flags,
            });
            position = end;
            continue;
        }
        if bulk.size_on_disk == 0 && bulk.element_count == 0 {
            position += 16;
            continue;
        }
        position += 4;
    }
    spans
}

fn int_property(properties: &[Property], name: &str) -> Option<i32> {
    properties
        .iter()
        .find(|property| property.name == name)
        .and_then(|property| match &property.value {
            PropertyValue::Int(value) => Some(*value),
            _ => None,
        })
}

fn float_property(properties: &[Property], name: &str) -> Option<f32> {
    properties
        .iter()
        .find(|property| property.name == name)
        .and_then(|property| match &property.value {
            PropertyValue::Float(value) => Some(*value),
            _ => None,
        })
}

impl SoundNodeWave {
    pub fn replace(&self, blob: &[u8], base: usize, ogg: &[u8]) -> Result<Vec<u8>> {
        let span = self
            .span
            .ok_or_else(|| PackageError::NoPayload(self.name.clone()))?;
        let info = crate::ogg::info(ogg)
            .ok_or_else(|| PackageError::UnsupportedProperty("not an ogg vorbis stream".into()))?;
        let mut out = Vec::with_capacity(blob.len() + ogg.len());
        out.extend_from_slice(&blob[..span.field_offset]);
        out.extend_from_slice(&span.flags.to_le_bytes());
        out.extend_from_slice(&(ogg.len() as i32).to_le_bytes());
        out.extend_from_slice(&(ogg.len() as i32).to_le_bytes());
        out.extend_from_slice(&((base + span.payload_offset) as i32).to_le_bytes());
        out.extend_from_slice(ogg);
        out.extend_from_slice(&blob[span.payload_offset + span.size..]);
        for property in &self.properties {
            let literal = match property.name.as_str() {
                "Duration" => info.duration().to_string(),
                "NumChannels" => info.channels.to_string(),
                "SampleRate" => info.sample_rate.to_string(),
                _ => continue,
            };
            property.set(&mut out, &literal)?;
        }
        Ok(out)
    }
}
