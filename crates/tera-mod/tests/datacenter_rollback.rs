use std::path::{Path, PathBuf};
use tera_datacenter::build::{BuildValue, Builder};
use tera_mod::apply::Install;
use tera_mod::manifest::{Change, Manifest};

fn fixture(path: &Path) {
    let mut builder = Builder::new();
    let sheet = builder.name_index("ItemData");
    let item = builder.name_index("Item");
    let id = builder.name_index("id");
    let root = builder.root;
    let group = builder.add_node(sheet);
    builder.attach(root, group);
    for number in 1..=8i32 {
        let node = builder.add_node(item);
        builder.attach(group, node);
        builder.node_mut(node).attributes.push((id, BuildValue::Int(number)));
    }
    let image = builder.pack().expect("pack");
    let keyiv = tera_crypto::known_keys()[0].keyiv();
    let wrapped = tera_datacenter::wrap(&image, &keyiv, 6).expect("wrap");
    std::fs::write(path, wrapped).expect("write");
}

fn edit(select: &str, value: &str) -> Change {
    Change::DataCenter {
        select: select.to_string(),
        set: [("id".to_string(), value.to_string())].into_iter().collect(),
        remove: Vec::new(),
    }
}

struct Tree(PathBuf);

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tree(label: &str) -> Tree {
    let base = std::env::temp_dir().join(format!("tera-mod-dc-{label}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("game")).expect("mkdir");
    Tree(base)
}

#[test]
fn a_selector_that_matches_nothing_leaves_the_data_center_untouched() {
    let tree = tree("nomatch");
    let data_center = tree.0.join("game/DataCenter.dat");
    fixture(&data_center);
    let before = std::fs::read(&data_center).expect("read");

    let install = Install::new(tree.0.join("game"), tree.0.join("store"));
    let manifest = Manifest {
        name: "ghost".into(),
        version: "1.0".into(),
        author: String::new(),
        description: String::new(),
        changes: vec![
            edit("/ItemData/Item[@id=\"404\"]", "7"),
            edit("/ItemData/Item[@id=\"1\"]", "9"),
        ],
    };

    let failure = install
        .apply(&manifest, &data_center, &tree.0)
        .expect_err("a selector matching nothing must fail");
    assert!(format!("{failure:#}").contains("matched nothing"));

    assert_eq!(
        std::fs::read(&data_center).expect("read"),
        before,
        "the data center was left modified after a failed apply"
    );
    assert!(!install.is_applied("ghost"));
}

#[test]
fn a_data_center_edit_reverts_byte_for_byte() {
    let tree = tree("roundtrip");
    let data_center = tree.0.join("game/DataCenter.dat");
    fixture(&data_center);
    let before = std::fs::read(&data_center).expect("read");

    let install = Install::new(tree.0.join("game"), tree.0.join("store"));
    let manifest = Manifest {
        name: "shift".into(),
        version: "1.0".into(),
        author: String::new(),
        description: String::new(),
        changes: vec![edit("/ItemData/Item[@id=\"1\"]", "42")],
    };

    install.apply(&manifest, &data_center, &tree.0).expect("apply");
    assert_ne!(std::fs::read(&data_center).expect("read"), before);
    tera_datacenter::DataCenter::open(&data_center).expect("the edited file still opens");

    install.revert("shift").expect("revert");
    assert_eq!(std::fs::read(&data_center).expect("read"), before);
}

#[test]
fn a_data_center_outside_the_game_root_is_still_backed_up() {
    let tree = tree("outside");
    let data_center = tree.0.join("elsewhere/DataCenter.dat");
    std::fs::create_dir_all(data_center.parent().unwrap()).expect("mkdir");
    fixture(&data_center);
    let before = std::fs::read(&data_center).expect("read");

    let install = Install::new(tree.0.join("game"), tree.0.join("store"));
    let manifest = Manifest {
        name: "outside".into(),
        version: "1.0".into(),
        author: String::new(),
        description: String::new(),
        changes: vec![edit("/ItemData/Item[@id=\"1\"]", "5")],
    };

    let receipt = install
        .apply(&manifest, &data_center, &tree.0)
        .expect("apply");
    let backup = receipt.applied[0].backup.clone().expect("a real backup");
    assert_ne!(backup, data_center, "the backup collapsed onto the target");
    assert_eq!(std::fs::read(&backup).expect("read backup"), before);

    install.revert("outside").expect("revert");
    assert_eq!(std::fs::read(&data_center).expect("read"), before);
}

#[test]
fn a_manifest_may_not_reach_outside_the_game_folder() {
    use tera_mod::manifest::Change;
    let escaping = |change: Change| Manifest {
        name: "escape".into(),
        version: "1.0".into(),
        author: String::new(),
        description: String::new(),
        changes: vec![change],
    };

    assert!(escaping(Change::File {
        source: "a".into(),
        target: "/etc/passwd".into(),
    })
    .validate()
    .is_err());

    assert!(escaping(Change::RemoveFile {
        target: "../../../important".into(),
    })
    .validate()
    .is_err());

    assert!(escaping(Change::Texture {
        package: "/absolute.gpk".into(),
        object: "x".into(),
        source: "art.png".into(),
    })
    .validate()
    .is_err());

    assert!(escaping(Change::File {
        source: "art/a.png".into(),
        target: "S1Game/Config/a.ini".into(),
    })
    .validate()
    .is_ok());
}

#[test]
fn a_mod_name_must_be_a_plain_filename() {
    for bad in ["", "..", "../../etc", "a/b", "a\\b"] {
        assert!(
            tera_mod::manifest::safe_name(bad).is_err(),
            "`{bad}` should be refused"
        );
    }
    assert!(tera_mod::manifest::safe_name("my-mod 1.0_final").is_ok());
}
