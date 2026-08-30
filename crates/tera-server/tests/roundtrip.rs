use std::path::PathBuf;
use tera_protocol::value::{self, Value};
use tera_server::registry::Registry;
use tera_server::world::{self, Character};

fn registry() -> Registry {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    Registry::load(&[root.join("data/definitions")], Some(100)).expect("definitions")
}

fn sample() -> Character {
    Character {
        id: 7,
        name: "고양이 무도가".into(),
        gender: 1,
        race: 4,
        class: 10,
        level: 12,
        appearance: 0x0102_0304_0506_0708,
        appearance2: 42,
        details: vec![1, 2, 3, 4, 5, 6, 7, 8],
        shape: vec![9, 8, 7, 6],
        position: 1,
        walk_speed: 50,
        run_speed: 150,
        admin_level: 0,
        equipment: Vec::new(),
        carried: Vec::new(),
        gold: 0,
        xp: 0,
        hp: -1,
        zone: 13,
        location: [0.0; 3],
        facing: 0,
    }
}

#[test]
fn no_definition_meant_for_a_later_patch_is_ever_chosen() {
    let registry = registry();
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/definitions");
    let entries = std::fs::read_dir(&directory).expect("definitions");
    let mut checked = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|kind| kind != "def").unwrap_or(true) {
            continue;
        }
        let Ok(file) = tera_protocol::defs::read_file(&path) else {
            continue;
        };
        let Some(chosen) = registry.version(&file.name) else {
            continue;
        };
        if chosen != file.version {
            continue;
        }
        assert!(
            file.patch.admits(100),
            "{} version {} was chosen for a patch 100 client but its guard excludes it",
            file.name,
            file.version
        );
        checked += 1;
    }
    assert!(checked > 500, "only {checked} definitions were checked");
}

#[test]
fn the_patch_version_picks_the_definitions_this_client_speaks() {
    let registry = registry();
    assert_eq!(registry.version("S_LOGIN"), Some(14));
    assert_eq!(registry.version("S_GET_USER_LIST"), Some(18));
    assert_eq!(registry.version("S_PLAYER_STAT_UPDATE"), Some(14));
    assert_eq!(registry.version("S_USER_STATUS"), Some(3));
    assert_eq!(registry.version("S_CHAT"), Some(3));
    assert_eq!(registry.version("S_CAN_CREATE_USER"), Some(1));
    assert_eq!(registry.version("C_PLAYER_LOCATION"), Some(5));
}

#[test]
fn a_later_patch_picks_the_later_definitions() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let modern = Registry::load(&[root.join("data/definitions")], Some(120)).expect("definitions");
    assert_eq!(modern.version("S_LOGIN"), Some(15));
    assert_eq!(modern.version("S_GET_USER_LIST"), Some(21));
    assert_eq!(modern.version("S_PLAYER_STAT_UPDATE"), Some(17));
    assert_eq!(modern.version("S_CHAT"), Some(4));
}

#[test]
fn user_list_survives_a_round_trip() {
    let registry = registry();
    let definition = registry.get("S_GET_USER_LIST").expect("S_GET_USER_LIST");
    let character = sample();
    let packet = value::write(definition, 26457, &world::user_list(&[character.clone()]))
        .expect("write");
    let decoded = value::read(definition, &packet).expect("read");

    assert_eq!(
        decoded.get("maxCharacters").and_then(Value::as_int),
        Some(world::MAX_CHARACTERS)
    );
    let Some(Value::Array(characters)) = decoded.get("characters") else {
        panic!("characters is not an array");
    };
    assert_eq!(characters.len(), 1);
    let entry = &characters[0];
    assert_eq!(entry.get("name").and_then(Value::as_str), Some(character.name.as_str()));
    assert_eq!(entry.get("id").and_then(Value::as_uint), Some(7));
    assert_eq!(entry.get("level").and_then(Value::as_int), Some(12));
    assert_eq!(entry.get("appearance").and_then(Value::as_uint), Some(character.appearance));
    assert_eq!(entry.get("details").and_then(Value::as_bytes), Some(character.details.as_slice()));
    assert_eq!(entry.get("shape").and_then(Value::as_bytes), Some(character.shape.as_slice()));
    assert_eq!(entry.get("guildName").and_then(Value::as_str), Some(""));
}

#[test]
fn nested_arrays_inside_array_items_round_trip() {
    let registry = registry();
    let definition = registry.get("S_GET_USER_LIST").expect("S_GET_USER_LIST");
    let mut entry = sample().list_entry();
    entry.set(
        "customStrings",
        Value::Array(vec![
            tera_protocol::value::Object::new()
                .with("id", Value::Int(3))
                .with("string", Value::Str("first".into())),
            tera_protocol::value::Object::new()
                .with("id", Value::Int(9))
                .with("string", Value::Str("second".into())),
        ]),
    );
    let mut list = world::user_list(&[]);
    list.set("characters", Value::Array(vec![entry]));

    let packet = value::write(definition, 26457, &list).expect("write");
    let decoded = value::read(definition, &packet).expect("read");
    let Some(Value::Array(characters)) = decoded.get("characters") else {
        panic!("characters is not an array");
    };
    let Some(Value::Array(strings)) = characters[0].get("customStrings") else {
        panic!("customStrings is not an array");
    };
    assert_eq!(strings.len(), 2);
    assert_eq!(strings[0].get("id").and_then(Value::as_int), Some(3));
    assert_eq!(strings[0].get("string").and_then(Value::as_str), Some("first"));
    assert_eq!(strings[1].get("id").and_then(Value::as_int), Some(9));
    assert_eq!(strings[1].get("string").and_then(Value::as_str), Some("second"));
    assert_eq!(
        characters[0].get("name").and_then(Value::as_str),
        Some("고양이 무도가")
    );
}

#[test]
fn login_packet_round_trips() {
    let registry = registry();
    let definition = registry.get("S_LOGIN").expect("S_LOGIN");
    let character = sample();
    let packet = value::write(definition, 62054, &character.login(0x1_0000_0007, 1)).expect("write");
    let decoded = value::read(definition, &packet).expect("read");

    assert_eq!(decoded.get("templateId").and_then(Value::as_int), Some(11011));
    assert_eq!(decoded.get("gameId").and_then(Value::as_uint), Some(0x1_0000_0007));
    assert_eq!(decoded.get("name").and_then(Value::as_str), Some(character.name.as_str()));
    assert_eq!(decoded.get("shape").and_then(Value::as_bytes), Some(character.shape.as_slice()));
    assert_eq!(decoded.get("scale"), Some(&Value::Float(1.0)));
}

#[test]
fn three_levels_of_nested_arrays_round_trip() {
    let registry = registry();
    let definition = registry.get("S_ITEMLIST").expect("S_ITEMLIST");
    let passivity_set = tera_protocol::value::Object::new()
        .with("index", Value::Uint(2))
        .with("masterworkBonus", Value::Int(7))
        .with("itemLevel", Value::Float(412.0))
        .with(
            "passivities",
            Value::List(vec![Value::Int(101), Value::Int(102), Value::Int(103)]),
        );
    let item = tera_protocol::value::Object::new()
        .with("id", Value::Int(88))
        .with("dbid", Value::Uint(1234))
        .with("slot", Value::Uint(1))
        .with("amount", Value::Int(1))
        .with("enchant", Value::Int(12))
        .with("customString", Value::Str("etched".into()))
        .with("crystals", Value::List(vec![Value::Int(5), Value::Int(6)]))
        .with("passivitySets", Value::Array(vec![passivity_set]))
        .with("mergedPassivities", Value::List(vec![Value::Int(9)]));
    let list = tera_protocol::value::Object::new()
        .with("gameId", Value::Uint(0x1_0000_0007))
        .with("container", Value::Int(14))
        .with("money", Value::Int(999))
        .with("items", Value::Array(vec![item]));

    let packet = value::write(definition, 48029, &list).expect("write");
    let decoded = value::read(definition, &packet).expect("read");
    let Some(Value::Array(items)) = decoded.get("items") else {
        panic!("items is not an array");
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].get("enchant").and_then(Value::as_int), Some(12));
    assert_eq!(
        items[0].get("customString").and_then(Value::as_str),
        Some("etched")
    );
    let Some(Value::List(crystals)) = items[0].get("crystals") else {
        panic!("crystals is not a list");
    };
    assert_eq!(crystals.len(), 2);
    let Some(Value::Array(sets)) = items[0].get("passivitySets") else {
        panic!("passivitySets is not an array");
    };
    assert_eq!(sets[0].get("masterworkBonus").and_then(Value::as_int), Some(7));
    let Some(Value::List(passivities)) = sets[0].get("passivities") else {
        panic!("passivities is not a list");
    };
    assert_eq!(passivities.len(), 3);
    assert_eq!(passivities[2].as_int(), Some(103));
}

#[test]
fn every_level_field_carries_the_characters_level() {
    let registry = registry();
    let definition = registry
        .get("S_PLAYER_STAT_UPDATE")
        .expect("S_PLAYER_STAT_UPDATE");
    let mut character = sample();
    character.level = 65;
    let packet = value::write(definition, 51405, &character.stats()).expect("write");
    let decoded = value::read(definition, &packet).expect("read");

    assert_eq!(decoded.get("level").and_then(Value::as_int), Some(65));
    assert_eq!(decoded.get("conditionLevel").and_then(Value::as_int), Some(65));
    assert_eq!(decoded.get("trueLevel").and_then(Value::as_int), Some(65));
    assert_eq!(decoded.get("maxHp").and_then(Value::as_int), Some(33000));
    assert_eq!(decoded.get("runSpeed").and_then(Value::as_int), Some(150));
}

#[test]
fn equipment_reaches_the_login_and_appearance_packets() {
    let registry = registry();
    let mut character = sample();
    character.equip(tera_server::world::SLOT_WEAPON, 10007);

    let login = registry.get("S_LOGIN").expect("S_LOGIN");
    let packet = value::write(login, 62054, &character.login(1, 1)).expect("write");
    let decoded = value::read(login, &packet).expect("read");
    assert_eq!(decoded.get("weapon").and_then(Value::as_int), Some(10007));
    assert_eq!(decoded.get("weaponModel").and_then(Value::as_int), Some(10007));

    let change = registry
        .get("S_USER_EXTERNAL_CHANGE")
        .expect("S_USER_EXTERNAL_CHANGE");
    let packet = value::write(change, 58700, &character.appearance_change(1)).expect("write");
    let decoded = value::read(change, &packet).expect("read");
    assert_eq!(decoded.get("weapon").and_then(Value::as_int), Some(10007));
    assert_eq!(decoded.get("weaponModel").and_then(Value::as_int), Some(10007));
}

#[test]
fn the_contract_request_decodes_the_bytes_the_client_really_sent() {
    let registry = registry();
    let definition = registry
        .get("C_REQUEST_CONTRACT")
        .expect("C_REQUEST_CONTRACT");
    let body: [u8; 32] = [
        0x01, 0x00, 0x00, 0x00, 0x40, 0xbc, 0x03, 0x00, 0x22, 0x00, 0x24, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00,
    ];
    let mut packet = Vec::with_capacity(36);
    packet.extend_from_slice(&36u16.to_le_bytes());
    packet.extend_from_slice(&34899u16.to_le_bytes());
    packet.extend_from_slice(&body);

    let decoded = value::read(definition, &packet).expect("read");
    assert_eq!(decoded.get("type").and_then(Value::as_int), Some(1));
    assert_eq!(decoded.get("unk1").and_then(Value::as_int), Some(244800));
    assert_eq!(decoded.get("unk2").and_then(Value::as_int), Some(8));
    assert_eq!(decoded.get("name").and_then(Value::as_str), Some(""));
    assert_eq!(decoded.get("data").and_then(Value::as_bytes), Some(&[][..]));
}

#[test]
fn a_character_survives_a_round_trip_through_the_database() {
    let file = std::env::temp_dir().join("tera-world-test.db");
    let _ = std::fs::remove_file(&file);
    let world = tera_server::world::World::open(&file, "35171").expect("world");

    let request = tera_protocol::value::Object::new()
        .with("name", Value::Str("Meow".into()))
        .with("gender", Value::Int(1))
        .with("race", Value::Int(4))
        .with("class", Value::Int(6))
        .with("appearance", Value::Uint(1154504389566053))
        .with("appearance2", Value::Uint(100))
        .with("details", Value::Bytes(vec![1, 2, 3]))
        .with("shape", Value::Bytes(vec![9, 8]));
    let created = world.create(&request);
    assert_eq!(created.name, "Meow");
    assert_eq!(created.class, 6);
    assert!(world.name_taken("meow"));

    world.update(created.id, |character| {
        character.level = 65;
        character.gold = 100_000;
        character.equip(tera_server::world::SLOT_WEAPON, 10007);
    });
    world.learn(created.id, &[10100, 10101]);
    world.remember(created.id, 13, [1.0, 2.0, 3.0], 42);

    drop(world);
    let reopened = tera_server::world::World::open(&file, "35171").expect("world");
    let loaded = reopened.find(created.id).expect("character");
    assert_eq!(loaded.level, 65);
    assert_eq!(loaded.gold, 100_000);
    assert_eq!(loaded.worn(tera_server::world::SLOT_WEAPON), 10007);
    assert_eq!(loaded.zone, 13);
    assert_eq!(loaded.location, [1.0, 2.0, 3.0]);
    assert_eq!(loaded.facing, 42);
    assert_eq!(loaded.details, vec![1, 2, 3]);
    let mut learned = reopened.learned(created.id);
    learned.sort_unstable();
    assert_eq!(learned, vec![10100, 10101]);
    assert_eq!(reopened.characters().len(), 1);
    let _ = std::fs::remove_file(&file);
}

#[test]
fn experience_accumulates_and_levels_up() {
    use tera_server::world::{xp_for_level, MAX_LEVEL};
    let mut character = sample();
    character.level = 1;
    character.xp = 0;
    assert_eq!(character.total_level_xp(), xp_for_level(1));

    assert!(!character.gain(xp_for_level(1) - 1));
    assert_eq!(character.level, 1);
    assert!(character.gain(1));
    assert_eq!(character.level, 2);
    assert_eq!(character.level_xp(), 0);

    character.level = MAX_LEVEL;
    character.xp = 0;
    assert!(!character.gain(xp_for_level(MAX_LEVEL) * 10));
    assert_eq!(character.level, MAX_LEVEL);
    assert_eq!(character.level_xp(), character.total_level_xp());
}

#[test]
fn the_login_packet_carries_a_usable_experience_bar() {
    let registry = registry();
    let definition = registry.get("S_LOGIN").expect("S_LOGIN");
    let mut character = sample();
    character.level = 12;
    character.xp = 4321;

    let packet = value::write(definition, 62054, &character.login(1, 1)).expect("write");
    let decoded = value::read(definition, &packet).expect("read");
    let level_xp = decoded.get("levelXp").and_then(Value::as_int).expect("levelXp");
    let total = decoded
        .get("totalLevelXp")
        .and_then(Value::as_int)
        .expect("totalLevelXp");
    assert_eq!(level_xp, 4321);
    assert!(total > 0, "a zero denominator is what broke the profile");
    assert!(level_xp < total);
    assert!(decoded.get("totalXp").and_then(Value::as_int).unwrap() > level_xp);
}

#[test]
fn the_combat_packets_all_encode_for_this_client() {
    let registry = registry();
    for name in [
        "S_ACTION_STAGE",
        "S_ACTION_END",
        "S_EACH_SKILL_RESULT",
        "S_CREATURE_LIFE",
        "S_PLAYER_CHANGE_EXP",
        "S_USER_LEVELUP",
        "S_DESPAWN_NPC",
    ] {
        let definition = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} has no definition at patch 100"));
        value::write(definition, 1, &tera_protocol::value::Object::new())
            .unwrap_or_else(|error| panic!("{name} does not encode: {error}"));
    }
    assert_eq!(registry.version("S_EACH_SKILL_RESULT"), Some(14));
}

#[test]
fn a_kill_is_worth_more_experience_than_a_weaker_one() {
    use tera_server::world::xp_for_kill;
    assert!(xp_for_kill(20, 20) > xp_for_kill(10, 20));
    assert!(xp_for_kill(30, 20) > xp_for_kill(20, 20));
    assert!(xp_for_kill(1, 70) >= 1, "a kill is never worth nothing");
    assert!(xp_for_kill(70, 1) > xp_for_kill(70, 70));
}

#[test]
fn a_skill_result_survives_the_reference_layout() {
    let registry = registry();
    let definition = registry.get("S_EACH_SKILL_RESULT").expect("S_EACH_SKILL_RESULT");
    let object = tera_protocol::value::Object::new()
        .with("source", Value::Uint(0x1_0000_0001))
        .with("target", Value::Uint(0x2_0000_0007))
        .with("templateId", Value::Int(11007))
        .with("skill", Value::Uint(tera_protocol::value::SkillId::player(1842).raw()))
        .with("value", Value::Int(423))
        .with("type", Value::Int(1))
        .with("damageType", Value::Int(1));
    let packet = value::write(definition, 1, &object).expect("write");
    let decoded = value::read(definition, &packet).expect("read");
    assert_eq!(decoded.get("target").and_then(Value::as_uint), Some(0x2_0000_0007));
    assert_eq!(decoded.get("value").and_then(Value::as_int), Some(423));
    assert_eq!(decoded.get("damageType").and_then(Value::as_int), Some(1));
}

#[test]
fn carried_items_stack_fill_slots_and_persist() {
    let file = std::env::temp_dir().join("tera-bag-test.db");
    let _ = std::fs::remove_file(&file);
    let world = tera_server::world::World::open(&file, "35171").expect("world");
    let request = tera_protocol::value::Object::new()
        .with("name", Value::Str("Bagger".into()))
        .with("class", Value::Int(6));
    let created = world.create(&request);

    world.update(created.id, |character| {
        assert_eq!(character.carry(1000, 3), 0, "the first item takes slot 0");
        assert_eq!(character.carry(2000, 1), 1, "a different item takes the next slot");
        assert_eq!(character.carry(1000, 2), 0, "the same item stacks in place");
    });

    drop(world);
    let reopened = tera_server::world::World::open(&file, "35171").expect("world");
    let loaded = reopened.find(created.id).expect("character");
    assert_eq!(loaded.carried.len(), 2);
    let stack = loaded
        .carried
        .iter()
        .find(|held| held.item == 1000)
        .expect("the stack");
    assert_eq!(stack.amount, 5, "three plus two");
    assert_eq!(stack.slot, 0);
    let _ = std::fs::remove_file(&file);
}

#[test]
fn the_inventory_packet_carries_the_bag_and_the_gold() {
    let registry = registry();
    let definition = registry.get("S_ITEMLIST").expect("S_ITEMLIST");
    let mut character = sample();
    character.gold = 500_000;
    character.carry(10007, 2);
    character.carry(1, 40);

    let packet = value::write(definition, 48029, &character.inventory_list(1)).expect("write");
    let decoded = value::read(definition, &packet).expect("read");
    assert_eq!(decoded.get("money").and_then(Value::as_int), Some(500_000));
    assert_eq!(decoded.get("container").and_then(Value::as_int), Some(0));
    let Some(Value::Array(items)) = decoded.get("items") else {
        panic!("items is not an array");
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].get("id").and_then(Value::as_int), Some(10007));
    assert_eq!(items[0].get("amount").and_then(Value::as_int), Some(2));
    assert_eq!(items[1].get("slot").and_then(Value::as_uint), Some(1));
}

#[test]
fn a_repeated_object_block_keeps_its_place_in_the_stream() {
    let registry = registry();
    let definition = registry.get("S_EACH_SKILL_RESULT").expect("S_EACH_SKILL_RESULT");
    let reaction = tera_protocol::value::Object::new()
        .with("enable", Value::Bool(true))
        .with("push", Value::Bool(true))
        .with("stage", Value::Int(3))
        .with("id", Value::Uint(77))
        .with("animSeq", Value::Array(Vec::new()));
    let object = tera_protocol::value::Object::new()
        .with("target", Value::Uint(0x2_0000_0009))
        .with("value", Value::Int(1234))
        .with("damageType", Value::Int(2))
        .with("reaction", Value::Object(reaction));

    let packet = value::write(definition, 1, &object).expect("write");
    let decoded = value::read(definition, &packet).expect("read");

    assert_eq!(
        decoded.get("damageType").and_then(Value::as_int),
        Some(2),
        "damageType sits between the two reaction blocks and must not drift"
    );
    let Some(Value::Object(back)) = decoded.get("reaction") else {
        panic!("reaction is not an object");
    };
    assert_eq!(back.get("enable"), Some(&Value::Bool(true)));
    assert_eq!(back.get("stage").and_then(Value::as_int), Some(3));
    assert_eq!(back.get("id").and_then(Value::as_uint), Some(77));
    assert_eq!(decoded.get("value").and_then(Value::as_int), Some(1234));
}

#[test]
fn the_cast_request_decodes_the_bytes_the_client_really_sent() {
    let registry = registry();
    let definition = registry.get("C_START_SKILL").expect("C_START_SKILL");
    assert_eq!(registry.version("C_START_SKILL"), Some(8));
    let body: [u8; 54] = [
        0x02, 0x00, 0x00, 0x00, 0x96, 0x87, 0x08, 0x00, 0x84, 0xbf, 0x02, 0x10, 0x00, 0x00, 0x00,
        0x00, 0x80, 0x5f, 0xba, 0x4b, 0x82, 0x47, 0x0a, 0xf6, 0x8d, 0xc7, 0xaf, 0xb2, 0x56, 0xc5,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let mut packet = Vec::with_capacity(58);
    packet.extend_from_slice(&58u16.to_le_bytes());
    packet.extend_from_slice(&50600u16.to_le_bytes());
    packet.extend_from_slice(&body);

    let decoded = value::read(definition, &packet).expect("read");
    let Some(Value::Vec3(location)) = decoded.get("loc") else {
        panic!("loc is not a vector");
    };
    assert!((location[0] - 66711.5).abs() < 1.0, "x was {}", location[0]);
    assert!((location[1] + 72684.1).abs() < 1.0, "y was {}", location[1]);
    assert!((location[2] + 3435.2).abs() < 1.0, "z was {}", location[2]);
    assert_eq!(decoded.get("counter").and_then(Value::as_uint), Some(2));
    assert_eq!(decoded.get("unk").and_then(Value::as_uint), Some(1));
    assert_eq!(decoded.get("target").and_then(Value::as_uint), Some(0));

    let raw = decoded.get("skill").and_then(Value::as_uint).expect("skill");
    let skill = tera_protocol::value::SkillId::from_raw(raw);
    assert_eq!(skill.id, 180100, "the packed id is a real priest skill");
    assert_eq!(skill.kind, 1, "a player skill has type 1");
    assert!(!skill.npc);
}

#[test]
fn a_wounded_creature_reports_its_health_honestly() {
    let realm = tera_server::realm::Realm::default();
    let creature = realm.spawn(
        13,
        &tera_server::realm::Spawn {
            template: 1,
            max_hp: 88,
            level: 3,
            ..Default::default()
        },
    );
    let hurt = realm.damage(creature.id, 30).expect("hurt");

    let show = hurt.health_packet();
    assert_eq!(show.get("curHp").and_then(Value::as_int), Some(58));
    assert_eq!(show.get("maxHp").and_then(Value::as_int), Some(88));

    let change = hurt.change_packet(30, 0x1_0000_0001);
    assert_eq!(change.get("diff").and_then(Value::as_int), Some(-30));
    assert_eq!(change.get("target").and_then(Value::as_uint), Some(hurt.id));
    assert_eq!(change.get("source").and_then(Value::as_uint), Some(0x1_0000_0001));

    let registry = registry();
    for (name, object) in [("S_SHOW_HP", show), ("S_CREATURE_CHANGE_HP", change)] {
        let definition = registry.get(name).unwrap_or_else(|| panic!("{name}"));
        value::write(definition, 1, &object).unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

#[test]
fn the_spawn_packet_does_not_claim_the_creature_is_a_vehicle() {
    let realm = tera_server::realm::Realm::default();
    let creature = realm.spawn(13, &tera_server::realm::Spawn { template: 1, ..Default::default() });
    let packet = creature.spawn_packet();
    assert_eq!(packet.get("repairable"), Some(&Value::Bool(false)));
    assert_eq!(packet.get("visible"), Some(&Value::Bool(true)));
}

#[test]
fn a_drop_can_be_spawned_taken_once_and_never_twice() {
    let realm = tera_server::realm::Realm::default();
    let dropped = realm.drop_item(13, 88, 30, [10.0, 20.0, -30.0], 0x1_0000_0001);
    assert!(dropped.id > 0x2_ffff_ffff, "drops live in their own id space");
    assert_eq!(realm.drops_near(13, [0.0; 3], 100.0).len(), 1);
    assert_eq!(realm.drops_near(2000, [0.0; 3], 100.0).len(), 0, "another zone is unrelated");
    assert_eq!(realm.drops_near(13, [5000.0, 0.0, 0.0], 100.0).len(), 0, "too far");

    assert!(realm.take_drop(dropped.id).is_some());
    assert!(realm.take_drop(dropped.id).is_none(), "a drop is taken once");
    assert!(realm.drops_near(13, [0.0; 3], 100.0).is_empty());
}

#[test]
fn the_drop_packets_encode_for_this_client() {
    let registry = registry();
    let realm = tera_server::realm::Realm::default();
    let dropped = realm.drop_item(13, 88, 30, [10.0, 20.0, -30.0], 0x1_0000_0001);

    let definition = registry.get("S_SPAWN_DROPITEM").expect("S_SPAWN_DROPITEM");
    let packet = value::write(definition, 1, &dropped.spawn_packet("meow")).expect("write");
    let decoded = value::read(definition, &packet).expect("read");
    assert_eq!(decoded.get("item").and_then(Value::as_int), Some(88));
    assert_eq!(decoded.get("amount").and_then(Value::as_int), Some(30));
    assert_eq!(decoded.get("ownerName").and_then(Value::as_str), Some("meow"));
    assert_eq!(decoded.get("gameId").and_then(Value::as_uint), Some(dropped.id));

    let definition = registry.get("S_DESPAWN_DROPITEM").expect("S_DESPAWN_DROPITEM");
    value::write(definition, 1, &dropped.despawn_packet()).expect("write");
}

#[test]
fn a_character_can_be_wounded_killed_and_revived() {
    let mut character = sample();
    character.level = 10;
    assert_eq!(character.max_hp(), 5500);
    assert_eq!(character.health(), 5500, "an unset hp means full health");
    assert!(character.alive());

    assert!(!character.wound(500));
    assert_eq!(character.health(), 5000);
    assert!(character.alive());

    assert!(character.wound(99_999), "an overwhelming blow kills");
    assert_eq!(character.health(), 0);
    assert!(!character.alive());
    assert!(!character.wound(100), "the dead do not die twice");

    character.revive();
    assert_eq!(character.health(), 5500);
    assert!(character.alive());
}

#[test]
fn health_survives_a_restart() {
    let file = std::env::temp_dir().join("tera-health-test.db");
    let _ = std::fs::remove_file(&file);
    let world = tera_server::world::World::open(&file, "35171").expect("world");
    let created = world.create(
        &tera_protocol::value::Object::new().with("name", Value::Str("Wounded".into())),
    );
    world.update(created.id, |character| {
        character.level = 10;
        character.wound(1234);
    });

    drop(world);
    let reopened = tera_server::world::World::open(&file, "35171").expect("world");
    let loaded = reopened.find(created.id).expect("character");
    assert_eq!(loaded.health(), 5500 - 1234);
    assert!(loaded.alive());
    let _ = std::fs::remove_file(&file);
}

#[test]
fn the_stat_packet_reports_current_health_not_full_health() {
    let registry = registry();
    let definition = registry.get("S_PLAYER_STAT_UPDATE").expect("S_PLAYER_STAT_UPDATE");
    let mut character = sample();
    character.level = 10;
    character.wound(2000);

    let packet = value::write(definition, 1, &character.stats()).expect("write");
    let decoded = value::read(definition, &packet).expect("read");
    assert_eq!(decoded.get("hp").and_then(Value::as_int), Some(3500));
    assert_eq!(decoded.get("maxHp").and_then(Value::as_int), Some(5500));
}
