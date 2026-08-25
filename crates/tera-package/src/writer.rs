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
