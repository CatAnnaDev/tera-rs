use std::collections::HashMap;
use tera_protocol::{defs, value};

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .filter_map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

fn definition(name: &str, version: u32) -> Option<defs::Definition> {
    let path = format!("../../data/definitions/{name}.{version}.def");
    defs::read_file(&path).ok().map(|file| file.definition)
}

fn captured(name: &str) -> Option<Vec<u8>> {
    let mut opcodes = HashMap::new();
    for line in std::fs::read_to_string("../../data/opcodes/protocol.376012.map")
        .ok()?
        .lines()
    {
        let mut parts = line.split_whitespace();
        if let (Some(key), Some(value)) = (parts.next(), parts.next()) {
            if let Ok(number) = value.parse::<u32>() {
                opcodes.insert(key.to_string(), number);
            }
        }
    }
    let wanted = *opcodes.get(name)?;
    for line in std::fs::read_to_string("../../captures/capture.jsonl").ok()?.lines() {
        let Some(at) = line.find("\"opcode\":") else { continue };
        let rest = &line[at + 9..];
        let end = rest.find(',')?;
        if rest[..end].trim().parse::<u32>().ok()? != wanted {
            continue;
        }
        let at = line.find("\"hex\":\"")? + 7;
        let rest = &line[at..];
        let end = rest.find('"')?;
        return Some(hex(&rest[..end]));
    }
    None
}

#[test]
fn the_chat_settings_packet_round_trips() {
    let Some(body) = captured("C_SAVE_CLIENT_CHAT_OPTION_SETTING") else {
        return;
    };
    let definition =
        definition("C_SAVE_CLIENT_CHAT_OPTION_SETTING", 1).expect("definition present");
    let mut packet = vec![0u8; 4];
    packet.extend_from_slice(&body);
    let size = u16::try_from(packet.len()).expect("fits");
    packet[0..2].copy_from_slice(&size.to_le_bytes());

    let parsed = value::read(&definition, &packet).expect("parses the captured bytes");
    let written = value::write(&definition, 0, &parsed).expect("serialises");
    assert_eq!(&written[4..], &body[..], "chat settings did not survive the round trip");
}

#[test]
fn packets_written_from_captures_round_trip() {
    let cases: HashMap<&str, &str> = [
        ("C_REQUEST_GUILD_PERK_LIST", ""),
        ("C_RQ_SKILL_POLISHING_EXP_INFO", ""),
        ("C_RQ_SKILL_POLISHING_LIST", ""),
        ("C_REQUEST_SERVANT_ADVENTURE_LIST", "01"),
        ("C_REQUEST_SERVANT_INFO_LIST", "010000000100000001"),
        ("C_SET_SERVANT_SEQUENCE", "0100080008000000ffffffffffffffffffffffff"),
        ("C_UPDATE_CONTENTS_PLAYTIME", "0100080008000000010000005206000000000000"),
    ]
    .into_iter()
    .collect();

    for (name, body) in cases {
        let Some(definition) = definition(name, 1) else {
            panic!("{name}.1.def is missing");
        };
        let body = hex(body);
        let mut packet = vec![0u8; 4];
        packet.extend_from_slice(&body);
        let size = u16::try_from(packet.len()).expect("packet fits");
        packet[0..2].copy_from_slice(&size.to_le_bytes());

        let parsed = value::read(&definition, &packet)
            .unwrap_or_else(|error| panic!("{name} did not parse its captured bytes: {error}"));
        let written = value::write(&definition, 0, &parsed)
            .unwrap_or_else(|error| panic!("{name} did not serialise: {error}"));
        assert_eq!(
            &written[4..],
            &body[..],
            "{name} did not come back byte for byte"
        );
    }
}
