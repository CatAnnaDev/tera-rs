use tera_index::{contains_ignore_case, Arena, Index, IndexData, ObjectEntry, PackageEntry};

fn sample() -> IndexData {
    let mut data = IndexData::default();
    let file = data.arena.push("CookedPC/Art_Data/Packages/S1UI/Icon_Status.gpk");
    data.files.push(file);
    let name = data.arena.push("Icon_Status");
    data.packages.push(PackageEntry {
        name,
        file: 0,
        offset: 0,
        span: 198_509,
        exports: 311,
    });
    let class = data.arena.push("Texture2D");
    data.classes.push(class);
    for label in ["Tex.FuriousBlows_Tex", "Tex.Accelration_Tex"] {
        let name = data.arena.push(label);
        data.objects.push(ObjectEntry {
            package: 0,
            name,
            class: 0,
            export: data.objects.len() as u32,
        });
    }
    data
}

#[test]
fn writes_and_maps_back() {
    let data = sample();
    let directory = std::env::temp_dir().join("tera-index-test");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("sample.idx");
    data.write(&path).unwrap();

    let index = Index::open(&path).unwrap();
    assert_eq!(index.file_count(), 1);
    assert_eq!(index.package_count(), 1);
    assert_eq!(index.object_count(), 2);
    assert_eq!(index.class_count(), 1);
    assert_eq!(index.package_name(0), "Icon_Status");
    assert_eq!(index.object_class(0), "Texture2D");
    assert_eq!(index.object_name(0), "Tex.FuriousBlows_Tex");
    assert_eq!(index.object_full_path(1), "Icon_Status.Tex.Accelration_Tex");
    assert_eq!(index.package(0).span, 198_509);

    let hits = index.search_objects("furious", 10, None);
    assert_eq!(hits, vec![0]);
    let filtered = index.search_objects("tex", 10, Some("Texture2D"));
    assert_eq!(filtered.len(), 2);
    let missing = index.search_objects("tex", 10, Some("StaticMesh"));
    assert!(missing.is_empty());
    let packages = index.search_packages("icon", 10);
    assert_eq!(packages, vec![0]);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn case_insensitive_search_matches_substrings() {
    assert!(contains_ignore_case("FuriousBlows_Tex", "furious"));
    assert!(contains_ignore_case("FuriousBlows_Tex", "blows_tex"));
    assert!(!contains_ignore_case("FuriousBlows_Tex", "furiousx"));
    assert!(contains_ignore_case("abc", ""));
    assert!(!contains_ignore_case("ab", "abc"));
}

#[test]
fn arena_spans_are_stable() {
    let mut arena = Arena::default();
    let first = arena.push("hello");
    let second = arena.push("world");
    assert_eq!(first.offset, 0);
    assert_eq!(first.length, 5);
    assert_eq!(second.offset, 5);
    assert_eq!(&arena.bytes[..], b"helloworld");
}
