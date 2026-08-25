use tera_protocol::defs::{self, Field, Primitive};
use tera_protocol::value::{read, write, Object, Value};

fn definition(path: &str) -> defs::Definition {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/definitions");
    let file = defs::read_file(format!("{root}/{path}")).expect("definition");
    file.definition
}

#[test]
fn parses_a_fixed_definition() {
    let def = definition("S_LOGIN_ARBITER.3.def");
    let names: Vec<&str> = def.fields.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "success",
            "loginQueue",
            "status",
            "unk",
            "language",
            "pvpDisabled",
            "unk1",
            "unk2"
        ]
    );
    assert!(matches!(
        def.find("status"),
        Some(Field::Value(Primitive::Uint32))
    ));
}

#[test]
fn puts_implicit_references_first() {
    let def = definition("C_LOGIN_ARBITER.2.def");
    let names: Vec<&str> = def.fields.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "name",
            "ticket",
            "unk1",
            "unk2",
            "language",
            "patchVersion",
            "name",
            "ticket"
        ]
    );
    assert!(matches!(def.fields[0].1, Field::RefString(_)));
    assert!(matches!(def.fields[1].1, Field::RefBytes(_)));
}

#[test]
fn fixed_packet_round_trips() {
    let def = definition("S_LOGIN_ARBITER.3.def");
    let object = Object::new()
        .with("success", Value::Bool(true))
        .with("loginQueue", Value::Bool(false))
        .with("status", Value::Uint(0))
        .with("unk", Value::Uint(0))
        .with("language", Value::Uint(6))
        .with("pvpDisabled", Value::Bool(false))
        .with("unk1", Value::Uint(0))
        .with("unk2", Value::Uint(65));
    let bytes = write(&def, 12345, &object).expect("write");
    assert_eq!(bytes.len(), 4 + 19);
    assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]) as usize, bytes.len());
    assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 12345);

    let back = read(&def, &bytes).expect("read");
    assert_eq!(back.get("success"), Some(&Value::Bool(true)));
    assert_eq!(back.get("language"), Some(&Value::Uint(6)));
    assert_eq!(back.get("unk2"), Some(&Value::Uint(65)));
}

#[test]
fn string_and_bytes_round_trip() {
    let def = definition("C_LOGIN_ARBITER.2.def");
    let object = Object::new()
        .with("unk1", Value::Int(0))
        .with("unk2", Value::Uint(0))
        .with("language", Value::Uint(6))
        .with("patchVersion", Value::Int(387463))
        .with("name", Value::Str("35171".into()))
        .with("ticket", Value::Bytes(vec![7, 8, 9, 10]));
    let bytes = write(&def, 40075, &object).expect("write");
    let back = read(&def, &bytes).expect("read");
    assert_eq!(back.get("name"), Some(&Value::Str("35171".into())));
    assert_eq!(back.get("ticket"), Some(&Value::Bytes(vec![7, 8, 9, 10])));
    assert_eq!(back.get("patchVersion"), Some(&Value::Int(387463)));
}

#[test]
fn vectors_and_angles_round_trip() {
    let def = definition("S_SPAWN_ME.3.def");
    let object = Object::new()
        .with("gameId", Value::Uint(0x1122_3344_5566_7788))
        .with("loc", Value::Vec3([1.5, -2.25, 300.0]))
        .with("w", Value::Int(-1234))
        .with("alive", Value::Bool(true))
        .with("isLord", Value::Bool(false));
    let bytes = write(&def, 64411, &object).expect("write");
    assert_eq!(bytes.len(), 4 + 24);
    let back = read(&def, &bytes).expect("read");
    assert_eq!(back.get("gameId"), Some(&Value::Uint(0x1122_3344_5566_7788)));
    assert_eq!(back.get("loc"), Some(&Value::Vec3([1.5, -2.25, 300.0])));
    assert_eq!(back.get("w"), Some(&Value::Int(-1234)));
}

#[test]
fn explicit_reference_definition_parses() {
    let def = definition("C_CHECK_VERSION.1.def");
    let names: Vec<&str> = def.fields.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["version", "version"]);
    assert!(matches!(def.fields[0].1, Field::RefArray(_)));
    assert!(matches!(def.fields[1].1, Field::Array { .. }));
}

#[test]
fn arrays_of_objects_round_trip() {
    let def = definition("C_CHECK_VERSION.1.def");
    let items = vec![
        Object::new()
            .with("index", Value::Int(0))
            .with("value", Value::Int(387463)),
        Object::new()
            .with("index", Value::Int(1))
            .with("value", Value::Int(387463)),
    ];
    let object = Object::new().with("version", Value::Array(items.clone()));
    let bytes = write(&def, 19900, &object).expect("write");
    let back = read(&def, &bytes).expect("read");
    let Some(Value::Array(parsed)) = back.get("version") else {
        panic!("expected an array, got {:?}", back.get("version"));
    };
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].get("value"), Some(&Value::Int(387463)));
    assert_eq!(parsed[1].get("index"), Some(&Value::Int(1)));
}
