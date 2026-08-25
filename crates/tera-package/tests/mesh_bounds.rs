use tera_package::{mesh, Bundle};

const ROOT: &str = "/Users/anna/Library/Application Support/CrossOver/Bottles/Tera/drive_c/Games/TERA Europe Classic/S1Game/CookedPC";

#[test]
fn static_mesh_bounds_are_located_in_real_packages() {
    let Ok(entries) = std::fs::read_dir(ROOT) else { return };
    let mut all: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|kind| kind == "gpk").unwrap_or(false))
        .collect();
    all.sort();
    let names: Vec<std::path::PathBuf> = all.into_iter().step_by(7).collect();
    let mut found = 0usize;
    let mut missed = 0usize;
    let mut collision = 0usize;
    for path in names.iter().take(900) {
        let Ok(data) = std::fs::read(path) else { continue };
        for package in Bundle::new(&data) {
            let Ok(package) = package else { break };
            for (index, export) in package.exports.iter().enumerate() {
                if package.export_class(export) != "StaticMesh" {
                    continue;
                }
                let Some(parsed) = mesh::parse_static_mesh(&package, export) else { continue };
                let Ok(blob) = package.export_data(export) else { continue };
                match parsed.bounds_offset(blob) {
                    Some(_) => found += 1,
                    None => missed += 1,
                }
                if parsed.collision_box_offset(blob).is_some() {
                    collision += 1;
                }
                let _ = index;
            }
        }
        if found + missed > 3000 {
            break;
        }
    }
    println!(
        "bounds located for {found} of {}, collision box for {collision}",
        found + missed
    );
    assert!(found + missed > 0, "no static meshes in the sample");
    assert!(
        found * 100 / (found + missed) > 90,
        "bounds located for only {found} of {}",
        found + missed
    );
}
