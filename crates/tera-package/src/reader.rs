use crate::error::{PackageError, Result};

pub struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub fn at(data: &'a [u8], offset: usize) -> Self {
        Self { data, offset }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn seek(&mut self, offset: usize) {
        self.offset = offset;
    }

    pub fn is_empty(&self) -> bool {
        self.offset >= self.data.len()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }

    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self.offset.checked_add(count).ok_or(PackageError::Truncated {
            offset: self.offset,
            needed: count,
            available: self.remaining(),
        })?;
        if end > self.data.len() {
            return Err(PackageError::Truncated {
                offset: self.offset,
                needed: count,
                available: self.remaining(),
            });
        }
        let slice = &self.data[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> Result<u64> {
        let bytes = self.take(8)?;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn guid(&mut self) -> Result<[u8; 16]> {
        Ok(self.take(16)?.try_into().unwrap())
    }

    pub fn string(&mut self) -> Result<String> {
        let length = self.i32()?;
        match length {
            0 => Ok(String::new()),
            length if length > 0 => {
                let bytes = self.take(length as usize)?;
                let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
                Ok(bytes[..end].iter().map(|byte| *byte as char).collect())
            }
            length => {
                let count = length
                    .checked_neg()
                    .ok_or(PackageError::BadStringLength(length))? as usize;
                let bytes = self.take(count * 2)?;
                let (pairs, _) = bytes.as_chunks::<2>();
                let units: Vec<u16> = pairs
                    .iter()
                    .map(|pair| u16::from_le_bytes(*pair))
                    .take_while(|unit| *unit != 0)
                    .collect();
                Ok(String::from_utf16_lossy(&units))
            }
        }
    }

    pub fn array<T>(&mut self, mut read: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let count = self.i32()?;
        if count < 0 {
            return Err(PackageError::BadStringLength(count));
        }
        let mut out = Vec::with_capacity(count.min(1 << 20) as usize);
        for _ in 0..count {
            out.push(read(self)?);
        }
        Ok(out)
    }
}
