#[derive(Debug, thiserror::Error)]
pub enum DataCenterError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("crypto error: {0}")]
    Crypto(#[from] tera_crypto::CryptoError),
    #[error("no known key decrypts this file; run `tera keyfind` to recover one")]
    UnknownKey,
    #[error("truncated data at offset {offset} (needed {needed} bytes, {available} available)")]
    Truncated {
        offset: usize,
        needed: usize,
        available: usize,
    },
    #[error("unsupported data center version {0}")]
    UnsupportedVersion(u32),
    #[error("inflate failed: {0}")]
    Inflate(String),
    #[error("declared uncompressed size {declared} does not match inflated size {actual}")]
    SizeMismatch { declared: u32, actual: usize },
    #[error("invalid address {segment}:{element} in {region}")]
    BadAddress {
        region: &'static str,
        segment: u16,
        element: u16,
    },
    #[error("invalid name index {0}")]
    BadNameIndex(u16),
    #[error("query error: {0}")]
    Query(String),
}

pub type Result<T> = std::result::Result<T, DataCenterError>;
