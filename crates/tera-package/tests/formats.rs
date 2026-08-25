use tera_package::dds::{level_size, unreal_format_for, Dds};
use tera_package::reader::Reader;

#[test]
fn reads_ascii_and_utf16_strings() {
    let mut data = Vec::new();
    data.extend_from_slice(&5i32.to_le_bytes());
    data.extend_from_slice(b"None\0");
    data.extend_from_slice(&(-4i32).to_le_bytes());
    for unit in "héé".encode_utf16() {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0i32.to_le_bytes());

    let mut reader = Reader::new(&data);
    assert_eq!(reader.string().unwrap(), "None");
    assert_eq!(reader.string().unwrap(), "héé");
    assert_eq!(reader.string().unwrap(), "");
}

#[test]
fn refuses_truncated_reads() {
    let data = [1u8, 2, 3];
    let mut reader = Reader::new(&data);
    assert!(reader.u32().is_err());
}

#[test]
fn dds_level_sizes() {
    assert_eq!(level_size(64, 64, Some(b"DXT1"), 0).unwrap(), 2048);
    assert_eq!(level_size(64, 64, Some(b"DXT5"), 0).unwrap(), 4096);
    assert_eq!(level_size(4, 4, Some(b"DXT5"), 0).unwrap(), 16);
    assert_eq!(level_size(2, 2, Some(b"DXT1"), 0).unwrap(), 8);
    assert_eq!(level_size(16, 16, None, 32).unwrap(), 1024);
}

#[test]
fn dds_format_names() {
    assert_eq!(unreal_format_for(Some(b"DXT1"), 0).unwrap(), "PF_DXT1");
    assert_eq!(unreal_format_for(Some(b"DXT5"), 0).unwrap(), "PF_DXT5");
    assert_eq!(unreal_format_for(None, 32).unwrap(), "PF_A8R8G8B8");
    assert!(unreal_format_for(Some(b"BC7 "), 0).is_err());
}

#[test]
fn parses_a_minimal_dds() {
    let mut file = vec![0u8; 128];
    file[..4].copy_from_slice(b"DDS ");
    file[4..8].copy_from_slice(&124u32.to_le_bytes());
    file[12..16].copy_from_slice(&8u32.to_le_bytes());
    file[16..20].copy_from_slice(&8u32.to_le_bytes());
    file[28..32].copy_from_slice(&2u32.to_le_bytes());
    file[80..84].copy_from_slice(&4u32.to_le_bytes());
    file[84..88].copy_from_slice(b"DXT5");
    file.extend_from_slice(&[0xabu8; 64]);
    file.extend_from_slice(&[0xcdu8; 16]);

    let dds = Dds::parse(&file).unwrap();
    assert_eq!((dds.width, dds.height), (8, 8));
    assert_eq!(dds.format_name(), "DXT5");
    assert_eq!(dds.mips.len(), 2);
    assert_eq!(dds.mips[0].len(), 64);
    assert_eq!(dds.mips[1].len(), 16);
}
