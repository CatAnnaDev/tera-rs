use crate::error::{PackageError, Result};
use crate::package::Package;
use std::collections::BTreeMap;

pub fn rebuild(package: &Package<'_>, overrides: &BTreeMap<usize, Vec<u8>>) -> Result<Vec<u8>> {
    rebuild_image(package, overrides)
}

fn rebuild_image(package: &Package<'_>, overrides: &BTreeMap<usize, Vec<u8>>) -> Result<Vec<u8>> {
    let mut summary = package.summary.clone();
    summary.compression_flags = 0;
    summary.compressed_chunks.clear();
    let header = summary.to_bytes();
    let name_offset = summary.name_offset.max(0) as usize;
    if header.len() > name_offset {
        return Err(PackageError::Truncated {
            offset: 0,
            needed: header.len(),
            available: name_offset,
        });
    }

    let mut out = package.image().to_vec();
    out[..header.len()].copy_from_slice(&header);
    for byte in &mut out[header.len()..name_offset] {
        *byte = 0;
    }

    let export_table = summary.export_offset.max(0) as usize;
    for (index, blob) in overrides {
        let export = package
            .exports
            .get(*index)
            .ok_or(PackageError::BadObjectIndex(*index as i32))?;
        let old_offset = export.serial_offset.max(0) as usize;
        let old_size = export.serial_size.max(0) as usize;
        let entry = *package
            .export_entry_offsets
            .get(*index)
            .ok_or(PackageError::BadObjectIndex(*index as i32))?;
        if entry + 40 > out.len() || entry < export_table {
            return Err(PackageError::BadObjectIndex(*index as i32));
        }
        if blob.len() == old_size {
            out[old_offset..old_offset + old_size].copy_from_slice(blob);
            continue;
        }
        let new_offset = out.len();
        let mut moved = blob.clone();
        let payloads_from = crate::properties::read_export_properties(package, &moved)
            .map(|(_, consumed)| consumed)
            .unwrap_or(0);
        patch_bulk_offsets(&mut moved, old_offset, new_offset, payloads_from);
        out.extend_from_slice(&moved);
        out[entry + 32..entry + 36].copy_from_slice(&(moved.len() as i32).to_le_bytes());
        out[entry + 36..entry + 40].copy_from_slice(&(new_offset as i32).to_le_bytes());
    }
    Ok(out)
}

const NAME_FLAGS: u64 = 0x0007_0010_0000_0000;

pub struct Addition {
    pub entry: Vec<u8>,
    pub data: Vec<u8>,
}

fn write_fstring(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&((value.len() + 1) as i32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    out.push(0);
}

pub fn reserialize(
    package: &Package<'_>,
    data_overrides: &BTreeMap<usize, Vec<u8>>,
    new_names: &[String],
    additions: &[Addition],
) -> Result<Vec<u8>> {
    let image = package.image();
    let summary = &package.summary;
    let header_end = summary.header_end;
    let name_off = summary.name_offset.max(0) as usize;
    let import_off = summary.import_offset.max(0) as usize;
    let export_off = summary.export_offset.max(0) as usize;
    let total = summary.total_header_size.max(0) as usize;
    let depends_off = summary.depends_offset.max(0) as usize;
    let export_entries_end = if depends_off > 0 { depends_off } else { total };

    let region = |start: usize, end: usize| -> Result<&[u8]> {
        image.get(start..end).ok_or(PackageError::Truncated {
            offset: start,
            needed: end.saturating_sub(start),
            available: image.len().saturating_sub(start),
        })
    };

    let gap = region(header_end, name_off)?.to_vec();

    let mut names_block = region(name_off, import_off)?.to_vec();
    for name in new_names {
        write_fstring(&mut names_block, name);
        names_block.extend_from_slice(&NAME_FLAGS.to_le_bytes());
    }

    let imports_block = region(import_off, export_off)?.to_vec();

    let mut exports_block = Vec::new();
    let mut entry_pos = Vec::with_capacity(package.exports.len() + additions.len());
    for index in 0..package.exports.len() {
        let start = package.export_entry_offsets[index];
        let end = package
            .export_entry_offsets
            .get(index + 1)
            .copied()
            .unwrap_or(export_entries_end);
        entry_pos.push(exports_block.len());
        exports_block.extend_from_slice(region(start, end)?);
    }
    for addition in additions {
        entry_pos.push(exports_block.len());
        exports_block.extend_from_slice(&addition.entry);
    }

    let new_name_count = summary.name_count + new_names.len() as i32;
    let new_export_count = summary.export_count + additions.len() as i32;
    let depends_block = vec![0u8; 4 * new_export_count.max(0) as usize];

    let mut summary = summary.clone();
    summary.compression_flags = 0;
    summary.compressed_chunks.clear();
    summary.name_count = new_name_count;
    summary.export_count = new_export_count;
    summary.thumbnail_table_offset = 0;
    if let Some(generation) = summary.generations.last_mut() {
        generation.export_count = new_export_count;
        generation.name_count = new_name_count;
    }

    let header_len = summary.to_bytes().len();
    let name_offset = header_len + gap.len();
    let import_offset = name_offset + names_block.len();
    let export_offset = import_offset + imports_block.len();
    let depends_offset = export_offset + exports_block.len();
    let total_header_size = depends_offset + depends_block.len();
    summary.name_offset = name_offset as i32;
    summary.import_offset = import_offset as i32;
    summary.export_offset = export_offset as i32;
    summary.depends_offset = depends_offset as i32;
    summary.import_export_guids_offset = total_header_size as i32;
    summary.total_header_size = total_header_size as i32;

    let header = summary.to_bytes();
    if header.len() != header_len {
        return Err(PackageError::Truncated {
            offset: 0,
            needed: header_len,
            available: header.len(),
        });
    }

    let mut out = Vec::with_capacity(total_header_size + image.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(&gap);
    out.extend_from_slice(&names_block);
    out.extend_from_slice(&imports_block);
    out.extend_from_slice(&exports_block);
    out.extend_from_slice(&depends_block);

    let export_count = package.exports.len();
    for index in 0..new_export_count.max(0) as usize {
        let (mut data, old_offset) = if index < export_count {
            let export = &package.exports[index];
            let data = match data_overrides.get(&index) {
                Some(blob) => blob.clone(),
                None => package.export_data(export)?.to_vec(),
            };
            (data, export.serial_offset.max(0) as usize)
        } else {
            (additions[index - export_count].data.clone(), 0)
        };
        let new_offset = out.len();
        let payloads_from = crate::properties::read_export_properties(package, &data)
            .map(|(_, consumed)| consumed)
            .unwrap_or(0);
        patch_bulk_offsets(&mut data, old_offset, new_offset, payloads_from);
        let entry = export_offset + entry_pos[index];
        out[entry + 32..entry + 36].copy_from_slice(&(data.len() as i32).to_le_bytes());
        out[entry + 36..entry + 40].copy_from_slice(&(new_offset as i32).to_le_bytes());
        out.extend_from_slice(&data);
    }

    Ok(out)
}

fn patch_bulk_offsets(blob: &mut [u8], old_base: usize, new_base: usize, from: usize) {
    if old_base == new_base || blob.len() < 16 {
        return;
    }
    let mut position = from.min(blob.len());
    while position + 16 <= blob.len() {
        let size = i32::from_le_bytes(blob[position + 8..position + 12].try_into().unwrap());
        let offset = i32::from_le_bytes(blob[position + 12..position + 16].try_into().unwrap());
        let payload_start = position + 16;
        if size > 0
            && offset >= 0
            && offset as usize == old_base + payload_start
            && payload_start + size as usize <= blob.len()
        {
            let patched = (new_base + payload_start) as i32;
            blob[position + 12..position + 16].copy_from_slice(&patched.to_le_bytes());
            position = payload_start + size as usize;
            continue;
        }
        position += 1;
    }
}

pub fn shift_bulk_offsets(
    original: &[u8],
    patched: &mut [u8],
    base: usize,
    splice_at: usize,
    delta: i64,
    from: usize,
) {
    if delta == 0 {
        return;
    }
    let mut position = from.min(original.len());
    while position + 16 <= original.len() {
        let size = i32::from_le_bytes(original[position + 8..position + 12].try_into().unwrap());
        let offset = i32::from_le_bytes(original[position + 12..position + 16].try_into().unwrap());
        let payload_start = position + 16;
        if size > 0
            && offset >= 0
            && offset as usize == base + payload_start
            && payload_start + size as usize <= original.len()
        {
            if position >= splice_at {
                let moved = (position as i64 + delta) as usize;
                if moved + 16 <= patched.len() {
                    let shifted = (offset as i64 + delta) as i32;
                    patched[moved + 12..moved + 16].copy_from_slice(&shifted.to_le_bytes());
                }
            }
            position = payload_start + size as usize;
            continue;
        }
        position += 1;
    }
}
