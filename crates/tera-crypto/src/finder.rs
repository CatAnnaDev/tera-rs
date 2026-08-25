use crate::{Cfb128, KeyIv, BLOCK_LEN, KEY_LEN};
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;
use flate2::{Decompress, FlushDecompress, Status};
use rayon::prelude::*;

pub const ORACLE_PREFIX_LEN: usize = 4096;

#[derive(Clone)]
pub struct ZlibOracle {
    pub prefix: Vec<u8>,
    encrypted_len: u64,
}

impl ZlibOracle {
    pub fn new(encrypted_prefix: &[u8], encrypted_len: u64) -> Self {
        let take = encrypted_prefix.len().min(ORACLE_PREFIX_LEN);
        Self {
            prefix: encrypted_prefix[..take].to_vec(),
            encrypted_len,
        }
    }

    #[inline]
    pub fn quick_test(&self, keyiv: &KeyIv) -> bool {
        self.probe(&keyiv.key).test(&keyiv.iv)
    }

    pub fn probe(&self, key: &[u8; KEY_LEN]) -> KeyProbe<'_> {
        KeyProbe {
            oracle: self,
            cipher: Aes128::new(GenericArray::from_slice(key)),
        }
    }

    #[inline]
    fn accepts(&self, head: &[u8; BLOCK_LEN]) -> bool {
        let uncompressed = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        if u64::from(uncompressed) < self.encrypted_len || uncompressed > 0x8000_0000 {
            return false;
        }
        valid_zlib_header(head[4], head[5])
    }

    pub fn verify(&self, keyiv: &KeyIv) -> bool {
        if !self.quick_test(keyiv) {
            return false;
        }
        let mut plain = self.prefix.clone();
        Cfb128::new(keyiv).decrypt(&mut plain);
        if plain.len() <= 4 {
            return false;
        }
        let mut inflate = Decompress::new(true);
        let mut out = vec![0u8; 1 << 16];
        match inflate.decompress(&plain[4..], &mut out, FlushDecompress::None) {
            Ok(Status::Ok | Status::StreamEnd | Status::BufError) => inflate.total_out() > 0,
            _ => false,
        }
    }
}

pub struct KeyProbe<'a> {
    oracle: &'a ZlibOracle,
    cipher: Aes128,
}

impl KeyProbe<'_> {
    #[inline]
    pub fn test(&self, iv: &[u8; BLOCK_LEN]) -> bool {
        let mut mask = GenericArray::from(*iv);
        self.cipher.encrypt_block(&mut mask);
        let mut head = [0u8; BLOCK_LEN];
        for index in 0..BLOCK_LEN.min(self.oracle.prefix.len()) {
            head[index] = self.oracle.prefix[index] ^ mask[index];
        }
        self.oracle.accepts(&head)
    }
}

#[inline]
fn valid_zlib_header(cmf: u8, flg: u8) -> bool {
    if cmf & 0x0f != 8 || cmf >> 4 > 7 {
        return false;
    }
    (u16::from(cmf) << 8 | u16::from(flg)) % 31 == 0
}

#[derive(Clone, Copy, Debug)]
pub enum ScanMode {
    Adjacent,
    Window(usize),
    Exhaustive,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub key_offset: usize,
    pub iv_offset: usize,
    pub keyiv: KeyIv,
    pub verified: bool,
}

pub fn scan_bytes(
    haystack: &[u8],
    oracle: &ZlibOracle,
    mode: ScanMode,
    alignment: usize,
) -> Vec<Candidate> {
    if haystack.len() < KEY_LEN + BLOCK_LEN {
        return Vec::new();
    }
    let alignment = alignment.max(1);
    let last_key = haystack.len() - KEY_LEN;
    match mode {
        ScanMode::Adjacent => scan_offsets(haystack, oracle, alignment, last_key, |offset, len| {
            AdjacentIter {
                offset,
                len,
                step: 0,
            }
        }),
        ScanMode::Window(radius) => {
            scan_offsets(haystack, oracle, alignment, last_key, move |offset, len| {
                WindowIter::new(offset, len, radius, alignment)
            })
        }
        ScanMode::Exhaustive => scan_bytes_exhaustive(haystack, oracle, alignment),
    }
}

fn scan_offsets<F, I>(
    haystack: &[u8],
    oracle: &ZlibOracle,
    alignment: usize,
    last_key: usize,
    make_iter: F,
) -> Vec<Candidate>
where
    F: Fn(usize, usize) -> I + Sync + Send,
    I: Iterator<Item = usize>,
{
    let steps = last_key / alignment + 1;
    (0..steps)
        .into_par_iter()
        .map(move |step| step * alignment)
        .flat_map_iter(|key_offset| {
            let key: [u8; KEY_LEN] = haystack[key_offset..key_offset + KEY_LEN]
                .try_into()
                .unwrap();
            let probe = oracle.probe(&key);
            make_iter(key_offset, haystack.len())
                .filter_map(|iv_offset| {
                    let iv: [u8; BLOCK_LEN] =
                        haystack.get(iv_offset..iv_offset + BLOCK_LEN)?.try_into().ok()?;
                    if !probe.test(&iv) {
                        return None;
                    }
                    let keyiv = KeyIv::new(key, iv);
                    Some(Candidate {
                        key_offset,
                        iv_offset,
                        verified: oracle.verify(&keyiv),
                        keyiv,
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn scan_bytes_exhaustive(
    haystack: &[u8],
    oracle: &ZlibOracle,
    alignment: usize,
) -> Vec<Candidate> {
    let alignment = alignment.max(1);
    let last = haystack.len().saturating_sub(BLOCK_LEN);
    let steps = last / alignment + 1;
    (0..steps)
        .into_par_iter()
        .map(move |step| step * alignment)
        .flat_map_iter(|key_offset| {
            let key: [u8; KEY_LEN] = haystack[key_offset..key_offset + KEY_LEN]
                .try_into()
                .unwrap();
            let probe = oracle.probe(&key);
            (0..steps)
                .map(move |step| step * alignment)
                .filter_map(|iv_offset| {
                    let iv: [u8; BLOCK_LEN] =
                        haystack.get(iv_offset..iv_offset + BLOCK_LEN)?.try_into().ok()?;
                    if !probe.test(&iv) {
                        return None;
                    }
                    let keyiv = KeyIv::new(key, iv);
                    Some(Candidate {
                        key_offset,
                        iv_offset,
                        verified: oracle.verify(&keyiv),
                        keyiv,
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

struct AdjacentIter {
    offset: usize,
    len: usize,
    step: u8,
}

impl Iterator for AdjacentIter {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        loop {
            let step = self.step;
            self.step += 1;
            match step {
                0 => {
                    let candidate = self.offset + KEY_LEN;
                    if candidate + BLOCK_LEN <= self.len {
                        return Some(candidate);
                    }
                }
                1 => {
                    if self.offset >= BLOCK_LEN {
                        return Some(self.offset - BLOCK_LEN);
                    }
                }
                _ => return None,
            }
        }
    }
}

struct WindowIter {
    current: usize,
    end: usize,
    step: usize,
}

impl WindowIter {
    fn new(offset: usize, len: usize, radius: usize, alignment: usize) -> Self {
        let start = offset.saturating_sub(radius);
        let end = (offset + radius).min(len.saturating_sub(BLOCK_LEN));
        let step = alignment.max(1);
        Self {
            current: start - start % step,
            end,
            step,
        }
    }
}

impl Iterator for WindowIter {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if self.current > self.end {
            return None;
        }
        let value = self.current;
        self.current += self.step;
        Some(value)
    }
}
