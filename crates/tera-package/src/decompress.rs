use crate::error::{PackageError, Result};
use crate::reader::Reader;
use crate::summary::{CompressedChunk, COMPRESS_LZO, COMPRESS_ZLIB, PACKAGE_MAGIC};
use flate2::Decompress;
use flate2::FlushDecompress;

pub fn decompress_chunk(
    file: &[u8],
    chunk: &CompressedChunk,
    flags: u32,
    out: &mut [u8],
) -> Result<()> {
    let base = chunk.compressed_offset as usize;
    let mut reader = Reader::at(file, base);
    let magic = reader.u32()?;
    if magic != PACKAGE_MAGIC {
        return Err(PackageError::BadChunkMagic(base));
    }
    let block_size = reader.u32()? as usize;
    let _total_compressed = reader.i32()?;
    let total_uncompressed = reader.i32()?;
    if total_uncompressed < 0 || total_uncompressed as usize > out.len() {
        return Err(PackageError::Truncated {
            offset: base,
            needed: total_uncompressed.max(0) as usize,
            available: out.len(),
        });
    }
    let block_count = (total_uncompressed as usize).div_ceil(block_size.max(1));
    let mut blocks = Vec::with_capacity(block_count.min(out.len() + 1));
    for _ in 0..block_count {
        let compressed = reader.i32()?;
        let uncompressed = reader.i32()?;
        if compressed < 0 || uncompressed < 0 {
            return Err(PackageError::BadChunkMagic(base));
        }
        blocks.push((compressed as usize, uncompressed as usize));
    }
    let mut written = 0usize;
    for (compressed, uncompressed) in blocks {
        let offset = reader.offset();
        let source = reader.take(compressed)?;
        let end = written.saturating_add(uncompressed);
        if end > out.len() {
            return Err(PackageError::Truncated {
                offset,
                needed: end,
                available: out.len(),
            });
        }
        let target = &mut out[written..end];
        if compressed == uncompressed {
            target.copy_from_slice(source);
        } else if flags & COMPRESS_LZO != 0 {
            lzo::decompress_into(source, target).map_err(|error| PackageError::Lzo {
                offset,
                reason: format!("{error:?}"),
            })?;
        } else if flags & COMPRESS_ZLIB != 0 {
            let mut inflater = Decompress::new(true);
            inflater
                .decompress(source, target, FlushDecompress::Finish)
                .map_err(|error| PackageError::Zlib {
                    offset,
                    reason: error.to_string(),
                })?;
        } else {
            return Err(PackageError::UnsupportedCompression(flags));
        }
        written = end;
    }
    Ok(())
}
