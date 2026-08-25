mod finder;
mod known;

pub use finder::{
    scan_bytes, scan_bytes_exhaustive, Candidate, ScanMode, ZlibOracle, ORACLE_PREFIX_LEN,
};
pub use known::{known_keys, KnownKey};

use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;

pub const BLOCK_LEN: usize = 16;
pub const KEY_LEN: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("key must be {KEY_LEN} bytes, got {0}")]
    KeyLength(usize),
    #[error("iv must be {BLOCK_LEN} bytes, got {0}")]
    IvLength(usize),
    #[error("invalid hex string")]
    Hex,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyIv {
    pub key: [u8; KEY_LEN],
    pub iv: [u8; BLOCK_LEN],
}

impl KeyIv {
    pub fn new(key: [u8; KEY_LEN], iv: [u8; BLOCK_LEN]) -> Self {
        Self { key, iv }
    }

    pub fn from_slices(key: &[u8], iv: &[u8]) -> Result<Self, CryptoError> {
        let key: [u8; KEY_LEN] = key
            .try_into()
            .map_err(|_| CryptoError::KeyLength(key.len()))?;
        let iv: [u8; BLOCK_LEN] = iv.try_into().map_err(|_| CryptoError::IvLength(iv.len()))?;
        Ok(Self { key, iv })
    }

    pub fn from_hex(key: &str, iv: &str) -> Result<Self, CryptoError> {
        Self::from_slices(&parse_hex(key)?, &parse_hex(iv)?)
    }

    pub fn key_hex(&self) -> String {
        to_hex(&self.key)
    }

    pub fn iv_hex(&self) -> String {
        to_hex(&self.iv)
    }
}

impl std::fmt::Debug for KeyIv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KeyIv({}, {})", self.key_hex(), self.iv_hex())
    }
}

pub fn parse_hex(text: &str) -> Result<Vec<u8>, CryptoError> {
    let text = text.trim();
    let cleaned: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err(CryptoError::Hex);
    }
    (0..cleaned.len() / 2)
        .map(|i| u8::from_str_radix(&cleaned[i * 2..i * 2 + 2], 16).map_err(|_| CryptoError::Hex))
        .collect()
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0xf) as u32, 16).unwrap());
    }
    out.to_uppercase()
}

pub struct Cfb128 {
    cipher: Aes128,
    feedback: [u8; BLOCK_LEN],
}

impl Cfb128 {
    pub fn new(keyiv: &KeyIv) -> Self {
        Self {
            cipher: Aes128::new(GenericArray::from_slice(&keyiv.key)),
            feedback: keyiv.iv,
        }
    }

    pub fn decrypt(&mut self, data: &mut [u8]) {
        for chunk in data.chunks_mut(BLOCK_LEN) {
            let mut mask = GenericArray::from(self.feedback);
            self.cipher.encrypt_block(&mut mask);
            let len = chunk.len();
            self.feedback[..len].copy_from_slice(&chunk[..len]);
            for (byte, key_byte) in chunk.iter_mut().zip(mask.iter()) {
                *byte ^= key_byte;
            }
        }
    }

    pub fn encrypt(&mut self, data: &mut [u8]) {
        for chunk in data.chunks_mut(BLOCK_LEN) {
            let mut mask = GenericArray::from(self.feedback);
            self.cipher.encrypt_block(&mut mask);
            for (byte, key_byte) in chunk.iter_mut().zip(mask.iter()) {
                *byte ^= key_byte;
            }
            let len = chunk.len();
            self.feedback[..len].copy_from_slice(&chunk[..len]);
        }
    }
}

pub fn decrypt_in_place(keyiv: &KeyIv, data: &mut [u8]) {
    Cfb128::new(keyiv).decrypt(data);
}

pub fn encrypt_in_place(keyiv: &KeyIv, data: &mut [u8]) {
    Cfb128::new(keyiv).encrypt(data);
}

pub fn decrypt_first_block(keyiv: &KeyIv, ciphertext: &[u8]) -> [u8; BLOCK_LEN] {
    let cipher = Aes128::new(GenericArray::from_slice(&keyiv.key));
    let mut mask = GenericArray::from(keyiv.iv);
    cipher.encrypt_block(&mut mask);
    let mut out = [0u8; BLOCK_LEN];
    let len = ciphertext.len().min(BLOCK_LEN);
    out[..len].copy_from_slice(&ciphertext[..len]);
    for index in 0..len {
        out[index] ^= mask[index];
    }
    out
}
