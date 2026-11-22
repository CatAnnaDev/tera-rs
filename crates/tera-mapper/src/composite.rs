use crate::{MapperError, Result};
use std::path::Path;

pub const COMPOSITE_KEY1: [usize; 16] = [12, 6, 9, 4, 3, 14, 1, 10, 13, 2, 7, 15, 0, 8, 5, 11];
pub const COMPOSITE_KEY2: &[u8] = b"GeneratePackageMapper";
pub const COMPOSITE_MOD_PREFIX: &str = "OmiMapper: ";

fn permute(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    let mut offset = 0usize;
    while offset + 16 <= data.len() {
        for i in 0..16 {
            out[offset + i] = data[offset + COMPOSITE_KEY1[i]];
        }
        offset += 16;
    }
    out
}

fn swap_pairs(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    let size = out.len();
    if size < 2 {
        return out;
    }
    let mut a = 1usize;
    let mut b = size - 1;
    let mut rounds = (size / 2 + 1) / 2;
    while rounds > 0 && a < b {
        out.swap(a, b);
        a += 2;
        b -= 2;
        rounds -= 1;
    }
    out
}

fn xor_key(data: &[u8]) -> Vec<u8> {
    let mut out = data.to_vec();
    for (i, byte) in out.iter_mut().enumerate() {
        *byte ^= COMPOSITE_KEY2[i % COMPOSITE_KEY2.len()];
    }
    out
}

pub fn composite_decrypt(data: &[u8]) -> Vec<u8> {
    xor_key(&swap_pairs(&permute(data)))
}

pub fn composite_encrypt(data: &[u8]) -> Vec<u8> {
    permute(&swap_pairs(&xor_key(data)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeEntry {
    pub object_path: String,
    pub composite_name: String,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeGroup {
    pub filename: String,
    pub entries: Vec<CompositeEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompositeMapper {
    pub groups: Vec<CompositeGroup>,
}

impl CompositeMapper {
    pub fn parse_text(text: &str) -> Result<Self> {
        let mut groups = Vec::new();
        for raw in text.split('!') {
            if raw.is_empty() {
                continue;
            }
            let (filename, body) = raw
                .split_once('?')
                .ok_or(MapperError::Encoding { index: groups.len() })?;
            let mut entries = Vec::new();
            for chunk in body.split('|') {
                if chunk.is_empty() {
                    continue;
                }
                let fields: Vec<&str> = chunk.split(',').collect();
                if fields.len() < 4 {
                    return Err(MapperError::Encoding { index: entries.len() });
                }
                entries.push(CompositeEntry {
                    object_path: fields[0].trim().to_string(),
                    composite_name: fields[1].trim().to_string(),
                    offset: fields[2].trim().parse().unwrap_or(0),
                    size: fields[3].trim().parse().unwrap_or(0),
                });
            }
            groups.push(CompositeGroup {
                filename: filename.to_string(),
                entries,
            });
        }
        Ok(Self { groups })
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for group in &self.groups {
            out.push_str(&group.filename);
            out.push('?');
            for entry in &group.entries {
                out.push_str(&entry.object_path);
                out.push(',');
                out.push_str(&entry.composite_name);
                out.push(',');
                out.push_str(&entry.offset.to_string());
                out.push(',');
                out.push_str(&entry.size.to_string());
                out.push(',');
                out.push('|');
            }
            out.push('!');
        }
        out
    }

    pub fn decrypt_parse(data: &[u8]) -> Result<Self> {
        let text = composite_decrypt(data);
        let text = String::from_utf8_lossy(&text);
        Self::parse_text(&text)
    }

    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::decrypt_parse(&data)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        composite_encrypt(self.to_text().as_bytes())
    }

    pub fn write(&self, path: impl AsRef<Path>) -> Result<()> {
        std::fs::write(path, self.to_bytes())?;
        Ok(())
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &CompositeEntry)> {
        self.groups
            .iter()
            .flat_map(|group| group.entries.iter().map(move |entry| (group.filename.as_str(), entry)))
    }

    pub fn len(&self) -> usize {
        self.groups.iter().map(|group| group.entries.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn find_composite(&self, composite_name: &str) -> Option<(&str, &CompositeEntry)> {
        self.entries()
            .find(|(_, entry)| entry.composite_name == composite_name)
    }

    pub fn remove_composite(&mut self, composite_name: &str) -> bool {
        let mut removed = false;
        for group in &mut self.groups {
            let before = group.entries.len();
            group.entries.retain(|entry| entry.composite_name != composite_name);
            removed |= group.entries.len() != before;
        }
        self.groups.retain(|group| !group.entries.is_empty());
        removed
    }

    pub fn upsert(&mut self, filename: &str, entry: CompositeEntry) {
        self.remove_composite(&entry.composite_name);
        if let Some(group) = self.groups.iter_mut().find(|group| group.filename == filename) {
            group.entries.push(entry);
        } else {
            self.groups.push(CompositeGroup {
                filename: filename.to_string(),
                entries: vec![entry],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crypto_round_trips() {
        let text = b"aaaa_1?aaaa_bbbb_1.PS.Thing_dup,aaaa_bbbb_1,0,128,|!cccc_2?cccc_dddd_2.Tex.Other_dup,cccc_dddd_2,0,64,|!";
        let encrypted = composite_encrypt(text);
        assert_eq!(composite_decrypt(&encrypted), text.to_vec());
    }

    #[test]
    fn key1_is_an_involution() {
        for i in 0..16 {
            assert_eq!(COMPOSITE_KEY1[COMPOSITE_KEY1[i]], i);
        }
    }

    #[test]
    fn text_round_trips() {
        let text = "aaaa_1?aaaa_bbbb_1.PS.Thing_dup,aaaa_bbbb_1,0,128,|!cccc_2?cccc_dddd_2.Tex.Other_dup,cccc_dddd_2,16,64,|!";
        let mapper = CompositeMapper::parse_text(text).unwrap();
        assert_eq!(mapper.len(), 2);
        assert_eq!(mapper.to_text(), text);
    }

    #[test]
    fn upsert_and_remove() {
        let mut mapper = CompositeMapper::parse_text(
            "shard_1?shard_a_1.PS.A_dup,shard_a_1,10,20,|!",
        )
        .unwrap();
        mapper.upsert(
            "mymod_300",
            CompositeEntry {
                object_path: "shard_a_1.PS.A_dup".into(),
                composite_name: "shard_a_1".into(),
                offset: 0,
                size: 999,
            },
        );
        let (filename, entry) = mapper.find_composite("shard_a_1").unwrap();
        assert_eq!(filename, "mymod_300");
        assert_eq!(entry.size, 999);
        assert!(mapper.remove_composite("shard_a_1"));
        assert!(mapper.is_empty());
    }
}
