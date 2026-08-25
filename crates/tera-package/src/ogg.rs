#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Info {
    pub channels: u8,
    pub sample_rate: u32,
    pub samples: u64,
}

impl Info {
    pub fn duration(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples as f32 / self.sample_rate as f32
    }
}

fn page_length(bytes: &[u8], at: usize) -> Option<usize> {
    let segments = *bytes.get(at + 26)? as usize;
    let table = bytes.get(at + 27..at + 27 + segments)?;
    Some(27 + segments + table.iter().map(|size| *size as usize).sum::<usize>())
}

pub fn info(bytes: &[u8]) -> Option<Info> {
    if !bytes.starts_with(b"OggS") {
        return None;
    }
    let first_body = 27 + *bytes.get(26)? as usize;
    let identification = bytes.get(first_body..)?;
    if !identification.starts_with(&[1]) || !identification[1..].starts_with(b"vorbis") {
        return None;
    }
    let channels = *identification.get(11)?;
    let sample_rate = u32::from_le_bytes(identification.get(12..16)?.try_into().ok()?);

    let mut at = 0usize;
    let mut samples = 0u64;
    while bytes.get(at..at + 4) == Some(b"OggS") {
        let Some(granule) = bytes
            .get(at + 6..at + 14)
            .and_then(|slice| slice.try_into().ok())
            .map(u64::from_le_bytes)
        else {
            break;
        };
        let Some(length) = page_length(bytes, at) else {
            break;
        };
        if at + length > bytes.len() {
            break;
        }
        if granule != u64::MAX {
            samples = granule;
        }
        at += length;
    }
    Some(Info {
        channels,
        sample_rate,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(header_type: u8, granule: u64, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"OggS");
        out.push(0);
        out.push(header_type);
        out.extend_from_slice(&granule.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.push(1);
        out.push(body.len() as u8);
        out.extend_from_slice(body);
        out
    }

    fn identification(channels: u8, sample_rate: u32) -> Vec<u8> {
        let mut body = vec![1];
        body.extend_from_slice(b"vorbis");
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(channels);
        body.extend_from_slice(&sample_rate.to_le_bytes());
        body.extend_from_slice(&[0u8; 12]);
        body
    }

    #[test]
    fn it_reads_channels_and_rate_from_the_first_page() {
        let stream = page(2, 0, &identification(2, 44100));
        let info = info(&stream).expect("vorbis");
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 44100);
    }

    #[test]
    fn duration_comes_from_the_last_granule_position() {
        let mut stream = page(2, 0, &identification(1, 22050));
        stream.extend_from_slice(&page(4, 44100, &[0u8; 8]));
        let info = info(&stream).expect("vorbis");
        assert_eq!(info.samples, 44100);
        assert!((info.duration() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_page_that_finishes_no_packet_does_not_become_the_sample_count() {
        let mut stream = page(2, 0, &identification(2, 44100));
        stream.extend_from_slice(&page(0, 88200, &[0u8; 8]));
        stream.extend_from_slice(&page(0, u64::MAX, &[0u8; 8]));
        let info = info(&stream).expect("vorbis");
        assert_eq!(info.samples, 88200);
        assert!((info.duration() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_truncated_last_page_keeps_the_samples_counted_so_far() {
        let mut stream = page(2, 0, &identification(1, 22050));
        stream.extend_from_slice(&page(0, 44100, &[0u8; 8]));
        let full = stream.len();
        stream.extend_from_slice(&page(4, 66150, &[0u8; 8]));
        stream.truncate(full + 20);
        let info = info(&stream).expect("vorbis");
        assert_eq!(info.samples, 44100);
    }

    #[test]
    fn it_refuses_anything_that_is_not_ogg() {
        assert_eq!(info(b"RIFFWAVEfmt "), None);
    }
}
