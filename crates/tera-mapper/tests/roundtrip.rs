use tera_mapper::{DirCache, PairMapper, PKG_MAPPER_MAGIC};

#[test]
fn pair_mapper_round_trip() {
    let mut mapper = PairMapper {
        magic: PKG_MAPPER_MAGIC,
        reserved: 0,
        entries: vec![
            ("ENGINE_MI_SHADERS.MI.M_SHADER_SIMPLE".into(), "abc_def_0.MI.M_Shader_Simple".into()),
            ("ICON_STATUS.TEX.FURIOUSBLOWS".into(), "Icon_Status.Tex.FuriousBlows".into()),
        ],
    };
    mapper.upsert("NEW.OBJECT", "mod_pack.Tex.New");
    let bytes = mapper.to_bytes();
    let parsed = PairMapper::parse(&bytes, PKG_MAPPER_MAGIC).unwrap();
    assert_eq!(parsed.entries.len(), 3);
    assert_eq!(parsed.lookup("new.object"), Some("mod_pack.Tex.New"));
    assert_eq!(parsed.to_bytes(), bytes);
    assert!(mapper.remove("NEW.OBJECT"));
    assert_eq!(mapper.entries.len(), 2);
}

#[test]
fn rejects_wrong_magic() {
    let mapper = PairMapper {
        magic: 0x1234_5678,
        reserved: 0,
        entries: Vec::new(),
    };
    assert!(PairMapper::parse(&mapper.to_bytes(), PKG_MAPPER_MAGIC).is_err());
}

#[test]
fn dir_cache_round_trip() {
    let mut cache = DirCache::default();
    cache.push("CookedPC\\S1UI_Login.gpk");
    cache.push("CookedPC\\Art_Data\\Packages\\S1UI\\Icon_Status.gpk");
    cache.push("CookedPC\\S1UI_Login.gpk");
    assert_eq!(cache.entries.len(), 2);
    let bytes = cache.to_bytes();
    let parsed = DirCache::parse(&bytes).unwrap();
    assert_eq!(parsed.entries, cache.entries);
    assert!(parsed.contains_package("Icon_Status"));
    assert!(!parsed.contains_package("Missing"));
    let names: Vec<&str> = parsed.package_names().collect();
    assert_eq!(names, vec!["S1UI_Login", "Icon_Status"]);
}

#[test]
fn splits_object_paths() {
    use tera_mapper::split_object_path;
    assert_eq!(
        split_object_path("Icon_Status.Tex.FuriousBlows"),
        ("Icon_Status", Some("Tex"), "FuriousBlows")
    );
    assert_eq!(split_object_path("Package.Object"), ("Package", None, "Object"));
    assert_eq!(split_object_path("Package"), ("Package", None, ""));
}
