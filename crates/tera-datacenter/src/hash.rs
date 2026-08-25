static HASH_TABLE: [u32; 256] = build_table();

const POLYNOMIAL: u32 = 0x04c1_1db7;

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut value = (index as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 0x8000_0000 != 0 {
                (value << 1) ^ POLYNOMIAL
            } else {
                value << 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

#[inline]
fn feed(hash: u32, unit: u16) -> u32 {
    let bytes = unit.to_le_bytes();
    let hash = HASH_TABLE[((hash ^ u32::from(bytes[0])) & 0xff) as usize] ^ (hash >> 8);
    HASH_TABLE[((hash ^ u32::from(bytes[1])) & 0xff) as usize] ^ (hash >> 8)
}

pub fn string_hash_units(units: impl Iterator<Item = u16>) -> u32 {
    units.fold(0u32, feed)
}

pub fn string_hash(value: &str) -> u32 {
    string_hash_units(value.encode_utf16())
}

pub fn value_hash_units(units: impl Iterator<Item = u16>) -> u16 {
    (units.map(fold_case).fold(0u32, feed) & 0x3fff) as u16
}

pub fn value_hash(value: &str) -> u16 {
    value_hash_units(value.encode_utf16())
}

fn fold_case(unit: u16) -> u16 {
    match unit {
        0x9c => 0x8c,
        0xff => 0x9f,
        0x151 => 0x150,
        0xf0 | 0xf7 => unit,
        0x61..=0x7a | 0xe0..=0xfe => unit - 0x20,
        _ => unit,
    }
}

pub fn table_segment_index(hash: u32, segment_count: u32) -> u32 {
    (hash ^ (hash >> 16)) % segment_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_reference() {
        assert_eq!(HASH_TABLE[0], 0x0000_0000);
        assert_eq!(HASH_TABLE[1], 0x04c1_1db7);
        assert_eq!(HASH_TABLE[2], 0x0982_3b6e);
        assert_eq!(HASH_TABLE[3], 0x0d43_26d9);
        assert_eq!(HASH_TABLE[255], 0xb1f7_40b4);
    }
}
