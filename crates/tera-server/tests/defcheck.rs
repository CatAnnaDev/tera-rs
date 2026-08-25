#[test]
fn every_definition_parses() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/definitions");
    let mut broken = Vec::new();
    for entry in std::fs::read_dir(&root).unwrap().flatten() {
        let path = entry.path();
        if path.extension().map(|value| value != "def").unwrap_or(true) {
            continue;
        }
        if let Err(error) = tera_protocol::defs::read_file(&path) {
            broken.push(format!("{}: {error}", path.file_name().unwrap().to_string_lossy()));
        }
    }
    broken.sort();
    assert!(broken.is_empty(), "unreadable definitions:\n{}", broken.join("\n"));
}
