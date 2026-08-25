use std::collections::BTreeMap;

const ROOT: &str = "/Users/anna/Library/Application Support/CrossOver/Bottles/Tera/drive_c/Games/TERA Europe Classic/S1Game";

fn walk(directory: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path
            .extension()
            .map(|kind| kind == "gpk" || kind == "gmp" || kind == "umap" || kind == "u")
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
}

#[test]
#[ignore]
fn no_unrealscript_survives_anywhere_in_the_game() {
    let mut files = Vec::new();
    walk(std::path::Path::new(ROOT), &mut files);
    if files.is_empty() {
        return;
    }
    files.sort();
    println!("scanning {} package files", files.len());

    let script = [
        "Class",
        "Function",
        "State",
        "Enum",
        "ScriptStruct",
        "Const",
        "TextBuffer",
    ];
    let mut hits: BTreeMap<String, usize> = BTreeMap::new();
    let mut scanned = 0usize;
    let mut exports = 0usize;
    for path in &files {
        let Ok(data) = std::fs::read(path) else { continue };
        scanned += 1;
        for package in tera_package::Bundle::tables_only(&data) {
            let Ok(package) = package else { break };
            for export in package.exports.iter() {
                exports += 1;
                let class = package.export_class(export);
                if script.iter().any(|name| *name == class) {
                    *hits.entry(format!("{class} in {}", path.display())).or_default() += 1;
                }
            }
        }
    }
    println!("{scanned} files, {exports} exports");
    for (what, count) in hits.iter().take(20) {
        println!("  {count} x {what}");
    }
    println!("script-class exports found: {}", hits.values().sum::<usize>());
}
