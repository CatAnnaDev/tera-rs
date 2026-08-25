use crate::error::{PackageError, Result};

#[derive(Clone, Debug)]
pub struct Dds {
    pub width: u32,
    pub height: u32,
    pub four_cc: Option<[u8; 4]>,
    pub bits_per_pixel: u32,
    pub mips: Vec<Vec<u8>>,
}

impl Dds {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 128 || &bytes[..4] != b"DDS " {
            return Err(PackageError::UnsupportedPixelFormat("not a dds file".into()));
        }
        let read = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let height = read(12);
        let width = read(16);
        let mip_count = read(28).max(1);
        let pixel_flags = read(80);
        let four_cc = if pixel_flags & 0x4 != 0 {
            Some(bytes[84..88].try_into().unwrap())
        } else {
            None
        };
        let bits_per_pixel = if four_cc.is_some() { 0 } else { read(88) };
        let mut mips = Vec::with_capacity(mip_count as usize);
        let mut offset = 128usize;
        let mut level_width = width;
        let mut level_height = height;
        for _ in 0..mip_count {
            let size = level_size(level_width, level_height, four_cc.as_ref(), bits_per_pixel)?;
            if offset + size > bytes.len() {
                break;
            }
            mips.push(bytes[offset..offset + size].to_vec());
            offset += size;
            level_width = (level_width / 2).max(1);
            level_height = (level_height / 2).max(1);
        }
        Ok(Self {
            width,
            height,
            four_cc,
            bits_per_pixel,
            mips,
        })
    }

    pub fn format_name(&self) -> String {
        match &self.four_cc {
            Some(code) => String::from_utf8_lossy(code).to_string(),
            None => format!("{}bpp", self.bits_per_pixel),
        }
    }
}

pub fn level_size(
    width: u32,
    height: u32,
    four_cc: Option<&[u8; 4]>,
    bits_per_pixel: u32,
) -> Result<usize> {
    match four_cc {
        Some(b"DXT1") => Ok((width.div_ceil(4) * height.div_ceil(4) * 8) as usize),
        Some(b"DXT3") | Some(b"DXT5") => Ok((width.div_ceil(4) * height.div_ceil(4) * 16) as usize),
        Some(other) => Err(PackageError::UnsupportedPixelFormat(
            String::from_utf8_lossy(other).to_string(),
        )),
        None => Ok((width * height * bits_per_pixel).div_ceil(8) as usize),
    }
}

pub fn unreal_format_for(four_cc: Option<&[u8; 4]>, bits_per_pixel: u32) -> Result<&'static str> {
    Ok(match four_cc {
        Some(b"DXT1") => "PF_DXT1",
        Some(b"DXT3") => "PF_DXT3",
        Some(b"DXT5") => "PF_DXT5",
        None if bits_per_pixel == 32 => "PF_A8R8G8B8",
        None if bits_per_pixel == 8 => "PF_G8",
        Some(other) => {
            return Err(PackageError::UnsupportedPixelFormat(
                String::from_utf8_lossy(other).to_string(),
            ))
        }
        None => {
            return Err(PackageError::UnsupportedPixelFormat(format!(
                "{bits_per_pixel}bpp"
            )))
        }
    })
}
