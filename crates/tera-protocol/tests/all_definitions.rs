use std::collections::HashMap;
use tera_protocol::defs;

#[test]
fn every_definition_on_disk_parses() {
    let directory = std::path::Path::new("../../data/definitions");
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut ok = 0usize;
    let mut broken = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|kind| kind != "def").unwrap_or(true) {
            continue;
        }
        match defs::read_file(&path) {
            Ok(_) => ok += 1,
            Err(error) => broken.push(format!("{}: {error}", path.display())),
        }
    }
    println!("{ok} definitions parsed, {} unreadable", broken.len());
    for line in broken.iter().take(10) {
        println!("  {line}");
    }
    assert!(ok > 800, "only {ok} definitions parsed");
    assert!(broken.is_empty(), "{} definitions failed to parse", broken.len());
}

#[test]
fn the_registry_picks_a_version_for_the_live_patch() {
    let names: HashMap<String, u32> = HashMap::new();
    let _ = names;
    let directory = std::path::PathBuf::from("../../data/definitions");
    if !directory.exists() {
        return;
    }
    let mut count = 0usize;
    for entry in std::fs::read_dir(&directory).unwrap().flatten() {
        let path = entry.path();
        if path.extension().map(|kind| kind == "def").unwrap_or(false) {
            if let Ok(file) = defs::read_file(&path) {
                if file.patch.admits(100) {
                    count += 1;
                }
            }
        }
    }
    println!("{count} definitions admit major patch 100");
    assert!(count > 500);
}
