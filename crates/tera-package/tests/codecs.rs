use tera_package::author::{build_texture_package, TextureSpec};
use tera_package::bc::{decode_blocks, encode_blocks, BlockFormat};
use tera_package::{png, Package, Texture2D};

fn gradient(width: usize, height: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            rgba[offset] = (x * 255 / width.max(1)) as u8;
            rgba[offset + 1] = (y * 255 / height.max(1)) as u8;
            rgba[offset + 2] = 128;
            rgba[offset + 3] = if (x / 8 + y / 8) % 2 == 0 { 255 } else { 32 };
        }
    }
    rgba
}

#[test]
fn png_round_trip_is_exact() {
    let rgba = gradient(64, 32);
    let encoded = png::encode(&rgba, 64, 32).unwrap();
    let decoded = png::decode(&encoded).unwrap();
    assert_eq!(decoded.width, 64);
    assert_eq!(decoded.height, 32);
    assert_eq!(decoded.rgba, rgba);
}

#[test]
fn bc3_round_trip_stays_close() {
    let rgba = gradient(64, 64);
    let blocks = encode_blocks(BlockFormat::Bc3, &rgba, 64, 64);
    assert_eq!(blocks.len(), 16 * 16 * 16);
    let decoded = decode_blocks(BlockFormat::Bc3, &blocks, 64, 64).unwrap();
    let error: f64 = rgba
        .iter()
        .zip(&decoded)
        .map(|(source, result)| {
            let delta = f64::from(*source) - f64::from(*result);
            delta * delta
        })
        .sum::<f64>()
        / rgba.len() as f64;
    assert!(error.sqrt() < 8.0, "rmse too high: {}", error.sqrt());
}

#[test]
fn bc1_round_trip_keeps_opaque_pixels() {
    let mut rgba = gradient(32, 32);
    for pixel in rgba.as_chunks_mut::<4>().0.iter_mut() {
        pixel[3] = 255;
    }
    let blocks = encode_blocks(BlockFormat::Bc1, &rgba, 32, 32);
    assert_eq!(blocks.len(), 8 * 8 * 8);
    let decoded = decode_blocks(BlockFormat::Bc1, &blocks, 32, 32).unwrap();
    assert!(decoded.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255));
}

#[test]
fn authored_package_parses_back() {
    let rgba = gradient(64, 64);
    let payload = encode_blocks(BlockFormat::Bc3, &rgba, 64, 64);
    let mut spec = TextureSpec::new("test_pack", "Test_Tex");
    spec.width = 64;
    spec.height = 64;
    spec.format = "PF_DXT5".into();
    spec.source_path = "test.png".into();
    spec.mips = vec![payload.clone()];

    let bytes = build_texture_package(&spec).unwrap();
    let package = Package::parse(&bytes, 0).unwrap();
    assert_eq!(package.summary.version, 897);
    assert_eq!(package.summary.licensee, 17);
    assert_eq!(package.exports.len(), 3);
    assert_eq!(package.imports.len(), 5);
    assert_eq!(package.package_name(), "test_pack");
    assert_eq!(package.span, bytes.len());

    let export_index = package
        .exports
        .iter()
        .position(|export| package.export_class(export) == "Texture2D")
        .unwrap();
    let export = &package.exports[export_index];
    assert_eq!(package.export_path(export_index), "test_pack.Test_Tex");

    let texture = Texture2D::parse(&package, export).unwrap();
    assert_eq!(texture.format, "PF_DXT5");
    assert_eq!((texture.width, texture.height), (64, 64));
    assert_eq!(texture.mips.len(), 1);
    assert_eq!(texture.mips[0].data.payload, payload);

    let (width, height, decoded) = texture.decode_rgba().unwrap();
    assert_eq!((width, height), (64, 64));
    assert_eq!(decoded.len(), 64 * 64 * 4);
}

#[test]
fn authored_package_survives_a_rebuild() {
    let rgba = gradient(32, 32);
    let mut spec = TextureSpec::new("rebuild_pack", "Rebuilt_Tex");
    spec.width = 32;
    spec.height = 32;
    spec.format = "PF_DXT1".into();
    spec.mips = vec![encode_blocks(BlockFormat::Bc1, &rgba, 32, 32)];
    let bytes = build_texture_package(&spec).unwrap();

    let package = Package::parse(&bytes, 0).unwrap();
    let rebuilt = tera_package::rebuild(&package, &std::collections::BTreeMap::new()).unwrap();
    assert_eq!(rebuilt, bytes);
}
