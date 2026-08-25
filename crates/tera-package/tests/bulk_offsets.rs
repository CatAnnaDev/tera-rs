use tera_package::{Bundle, Texture2D};

const GAME: &str = "/Users/anna/Library/Application Support/CrossOver/Bottles/Tera/drive_c/Games/TERA Europe Classic/S1Game/CookedPC/c7a706fb_2.gpk";
const OBJECT: &str = "CharacterWindow_I101";

fn inline_offsets_agree(data: &[u8]) -> usize {
    let mut checked = 0usize;
    for package in Bundle::new(data) {
        let Ok(package) = package else { break };
        for (index, export) in package.exports.iter().enumerate() {
            if package.export_class(export) != "Texture2D" {
                continue;
            }
            if !package.export_path(index).contains(OBJECT) {
                continue;
            }
            let texture = Texture2D::parse(&package, export).expect("texture");
            let base = export.serial_offset.max(0) as usize;
            for (level, mip) in texture.mips.iter().enumerate() {
                if !mip.data.is_inline() {
                    continue;
                }
                assert_eq!(
                    mip.data.offset_in_file as usize,
                    base + mip.data.payload_offset,
                    "mip {level} points at {} but its payload sits at {}",
                    mip.data.offset_in_file,
                    base + mip.data.payload_offset
                );
                checked += 1;
            }
        }
    }
    checked
}

#[test]
fn a_resized_property_leaves_bulk_offsets_pointing_at_their_payloads() {
    let Ok(data) = std::fs::read(GAME) else { return };
    assert!(inline_offsets_agree(&data) > 0, "fixture has inline mips");

    let longer = "C:\\a\\much\\longer\\source\\path\\than\\the\\original\\one\\here.tga";
    let edited = tera_package::set_properties(
        &data,
        OBJECT,
        &[("SourceFilePath".to_string(), longer.to_string())],
    )
    .expect("set a longer string");
    assert!(inline_offsets_agree(&edited.bytes) > 0);

    let shorter = "s.tga";
    let edited = tera_package::set_properties(
        &data,
        OBJECT,
        &[("SourceFilePath".to_string(), shorter.to_string())],
    )
    .expect("set a shorter string");
    assert!(inline_offsets_agree(&edited.bytes) > 0);
}
