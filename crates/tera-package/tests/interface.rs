const UI: &str = "/Users/anna/Library/Application Support/CrossOver/Bottles/Tera/drive_c/Games/TERA Europe Classic/S1Game/CookedPC/c7a706fb_6.gpk";

#[test]
fn a_scaleform_movie_survives_being_replaced_by_a_larger_one() {
    let Ok(data) = std::fs::read(UI) else { return };
    let movies = tera_package::movies(&data);
    assert!(!movies.is_empty(), "the fixture has no GFxMovieInfo");
    let original = &movies[0];
    assert_eq!(original.kind(), "gfx");

    let mut bigger = original.data.clone();
    bigger.extend_from_slice(&[0x5a; 512]);
    let object = original.path.rsplit('.').next().unwrap().to_string();

    let edited = tera_package::replace_movie(&data, &object, &bigger).expect("replace");
    let after = tera_package::movies(&edited.bytes);
    assert_eq!(after.len(), movies.len(), "a movie went missing");
    assert_eq!(after[0].data, bigger, "the movie did not come back byte for byte");
    assert_eq!(
        after[0].source_file, original.source_file,
        "the neighbouring properties were disturbed"
    );
}

#[test]
fn a_smaller_movie_also_round_trips() {
    let Ok(data) = std::fs::read(UI) else { return };
    let movies = tera_package::movies(&data);
    let Some(original) = movies.first() else { return };
    let smaller = original.data[..original.data.len() / 2].to_vec();
    let object = original.path.rsplit('.').next().unwrap().to_string();

    let edited = tera_package::replace_movie(&data, &object, &smaller).expect("replace");
    let after = tera_package::movies(&edited.bytes);
    assert_eq!(after[0].data, smaller);
}

#[test]
fn every_other_texture_in_the_package_is_untouched() {
    let Ok(data) = std::fs::read(UI) else { return };
    let before = tera_package::movies(&data);
    let Some(original) = before.first() else { return };
    let object = original.path.rsplit('.').next().unwrap().to_string();
    let mut bigger = original.data.clone();
    bigger.extend_from_slice(&[1u8; 64]);
    let edited = tera_package::replace_movie(&data, &object, &bigger).expect("replace");

    let read_textures = |bytes: &[u8]| {
        let mut out = Vec::new();
        for package in tera_package::Bundle::new(bytes) {
            let Ok(package) = package else { break };
            for export in package.exports.iter() {
                if package.export_class(export) != "Texture2D" {
                    continue;
                }
                if let Ok(texture) = tera_package::Texture2D::parse(&package, export) {
                    if let Ok((width, height, pixels)) = texture.decode_rgba() {
                        out.push((width, height, pixels.len(), pixels.first().copied()));
                    }
                }
            }
        }
        out
    };
    let stock = read_textures(&data);
    assert!(!stock.is_empty(), "the fixture has no textures");
    assert_eq!(stock, read_textures(&edited.bytes));
}
