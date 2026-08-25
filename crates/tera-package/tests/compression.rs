const COMPRESSED: &str = "/Users/anna/Library/Application Support/CrossOver/Bottles/Tera/drive_c/Games/TERA Europe Classic/S1Game/CookedPC/ffd585e3_247.gpk";

fn first_material(data: &[u8]) -> Option<(String, String)> {
    let material = tera_package::materials(data).into_iter().next()?;
    let parameter = material
        .parameters
        .iter()
        .find(|parameter| parameter.kind == tera_package::material::Kind::Vector)?;
    Some((material.path, parameter.name.clone()))
}

#[test]
fn editing_a_compressed_package_decompresses_it_and_keeps_it_readable() {
    let Ok(data) = std::fs::read(COMPRESSED) else { return };
    let Some((path, parameter)) = first_material(&data) else { return };

    let edited = tera_package::set_parameters(
        &data,
        &path,
        &[(parameter.clone(), "0.25,0.5,0.75,1".to_string())],
    )
    .expect("set a colour");

    assert!(
        edited.bytes.len() > data.len(),
        "an LZO package can only be rewritten uncompressed, so it must grow"
    );

    let after = tera_package::materials(&edited.bytes);
    let changed = after
        .iter()
        .find(|material| material.path == path)
        .and_then(|material| {
            material
                .parameters
                .iter()
                .find(|entry| entry.name == parameter)
        })
        .expect("the parameter is still there");
    assert_eq!(changed.value, "0.25,0.5,0.75,1");
}

#[test]
fn every_export_survives_a_recompressed_rebuild() {
    let Ok(data) = std::fs::read(COMPRESSED) else { return };
    let Some((path, parameter)) = first_material(&data) else { return };
    let edited = tera_package::set_parameters(
        &data,
        &path,
        &[(parameter, "1,1,1,1".to_string())],
    )
    .expect("set a colour");

    let names = |bytes: &[u8]| {
        let mut out = Vec::new();
        for package in tera_package::Bundle::new(bytes) {
            let Ok(package) = package else { break };
            for (index, export) in package.exports.iter().enumerate() {
                out.push((package.export_path(index), package.export_class(export)));
            }
        }
        out
    };
    assert_eq!(names(&data), names(&edited.bytes));
}
