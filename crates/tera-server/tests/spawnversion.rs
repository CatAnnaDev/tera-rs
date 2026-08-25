#[test]
fn the_reconstructed_spawn_packet_is_selected_and_shorter() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let registry =
        tera_server::registry::Registry::load(&[root.join("data/definitions")], Some(100))
            .expect("definitions");
    assert_eq!(
        registry.version("S_SPAWN_NPC"),
        Some(11),
        "patch 100 must not use the version that patch 101 introduced"
    );
    let modern =
        tera_server::registry::Registry::load(&[root.join("data/definitions")], Some(110))
            .expect("definitions");
    assert_eq!(modern.version("S_SPAWN_NPC"), Some(12));

    let creature = tera_server::realm::Realm::default().spawn(
        13,
        &tera_server::realm::Spawn {
            template: 7001,
            hunting_zone: 13,
            ..Default::default()
        },
    );
    let old = tera_protocol::value::write(
        registry.get("S_SPAWN_NPC").unwrap(),
        53112,
        &creature.spawn_packet(),
    )
    .expect("write");
    let new = tera_protocol::value::write(
        modern.get("S_SPAWN_NPC").unwrap(),
        53112,
        &creature.spawn_packet(),
    )
    .expect("write");
    assert_eq!(
        new.len() - old.len(),
        20,
        "level, maxHp and enrageThreshold are exactly the 20 bytes patch 101 added"
    );
}
