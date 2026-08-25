use tera_crypto::{
    decrypt_in_place, encrypt_in_place, known_keys, parse_hex, scan_bytes, to_hex, KeyIv, ScanMode,
    ZlibOracle,
};

fn sample_key() -> KeyIv {
    KeyIv::from_hex(
        "000102030405060708090a0b0c0d0e0f",
        "101112131415161718191a1b1c1d1e1f",
    )
    .unwrap()
}

#[test]
fn hex_round_trip() {
    let bytes = parse_hex("00 ff 10 A0").unwrap();
    assert_eq!(bytes, vec![0x00, 0xff, 0x10, 0xa0]);
    assert_eq!(to_hex(&bytes), "00FF10A0");
}

#[test]
fn cfb_round_trip() {
    let keyiv = sample_key();
    let original: Vec<u8> = (0..1000u32).map(|index| (index % 251) as u8).collect();
    let mut data = original.clone();
    encrypt_in_place(&keyiv, &mut data);
    assert_ne!(data, original);
    decrypt_in_place(&keyiv, &mut data);
    assert_eq!(data, original);
}

#[test]
fn built_in_keys_parse() {
    assert!(!known_keys().is_empty());
    for known in known_keys() {
        let keyiv = known.keyiv();
        assert_eq!(keyiv.key_hex(), known.key_hex.to_uppercase());
        assert_eq!(keyiv.iv_hex(), known.iv_hex.to_uppercase());
    }
}

#[test]
fn finder_recovers_adjacent_key() {
    let keyiv = sample_key();
    let payload = build_encrypted_payload(&keyiv);
    let oracle = ZlibOracle::new(&payload, payload.len() as u64);
    assert!(oracle.verify(&keyiv));

    let mut haystack = vec![0x41u8; 4096];
    haystack[1024..1040].copy_from_slice(&keyiv.key);
    haystack[1040..1056].copy_from_slice(&keyiv.iv);
    let found = scan_bytes(&haystack, &oracle, ScanMode::Adjacent, 1);
    assert!(found
        .iter()
        .any(|candidate| candidate.verified && candidate.keyiv.key == keyiv.key));
}

#[test]
fn finder_recovers_key_in_window() {
    let keyiv = sample_key();
    let payload = build_encrypted_payload(&keyiv);
    let oracle = ZlibOracle::new(&payload, payload.len() as u64);
    let mut haystack = vec![0x7fu8; 8192];
    haystack[100..116].copy_from_slice(&keyiv.key);
    haystack[400..416].copy_from_slice(&keyiv.iv);
    let found = scan_bytes(&haystack, &oracle, ScanMode::Window(512), 1);
    assert!(found.iter().any(|candidate| candidate.verified));
    let none = scan_bytes(&haystack, &oracle, ScanMode::Window(16), 1);
    assert!(none.iter().all(|candidate| !candidate.verified));
}

fn build_encrypted_payload(keyiv: &KeyIv) -> Vec<u8> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    let image: Vec<u8> = (0..8192u32).map(|index| (index % 97) as u8).collect();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&image).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut payload = Vec::with_capacity(compressed.len() + 4);
    payload.extend_from_slice(&(image.len() as u32).to_le_bytes());
    payload.extend_from_slice(&compressed);
    encrypt_in_place(keyiv, &mut payload);
    payload
}
