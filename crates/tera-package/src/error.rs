#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not an unreal package: magic {0:#010x}")]
    BadMagic(u32),
    #[error("unsupported package version {version}.{licensee}")]
    UnsupportedVersion { version: u16, licensee: u16 },
    #[error("truncated at offset {offset} (needed {needed}, {available} available)")]
    Truncated {
        offset: usize,
        needed: usize,
        available: usize,
    },
    #[error("invalid string length {0}")]
    BadStringLength(i32),
    #[error("invalid name index {0}")]
    BadNameIndex(i32),
    #[error("chunk header magic mismatch at {0}")]
    BadChunkMagic(usize),
    #[error("lzo decompression failed at offset {offset}: {reason}")]
    Lzo { offset: usize, reason: String },
    #[error("zlib decompression failed at offset {offset}: {reason}")]
    Zlib { offset: usize, reason: String },
    #[error("unsupported compression flags {0:#x}")]
    UnsupportedCompression(u32),
    #[error("object index {0} out of range")]
    BadObjectIndex(i32),
    #[error("unsupported pixel format `{0}`")]
    UnsupportedPixelFormat(String),
    #[error("object `{0}` has no exportable payload")]
    NoPayload(String),
    #[error("object `{0}` not found")]
    NoSuchObject(String),
    #[error("{0}")]
    UnsupportedProperty(String),
}

pub type Result<T> = std::result::Result<T, PackageError>;
