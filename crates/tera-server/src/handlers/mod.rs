pub mod account;
pub mod system;
pub mod zone;
pub mod skill;
pub mod item;
pub mod silence;
pub mod social;

use super::session::{Connection, Server};
use anyhow::Result;
use tera_protocol::value::Object;

pub fn dispatch(
    name: &str,
    request: Option<&Object>,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<bool> {
    if account::owns(name) {
        account::handle(name, request, server, connection)?;
        return Ok(true);
    }
    if system::owns(name) {
        system::handle(name, request, server, connection)?;
        return Ok(true);
    }
    if zone::owns(name) {
        zone::handle(name, request, server, connection)?;
        return Ok(true);
    }
    if skill::owns(name) {
        skill::handle(name, request, server, connection)?;
        return Ok(true);
    }
    if item::owns(name) {
        item::handle(name, request, server, connection)?;
        return Ok(true);
    }
    if social::owns(name) {
        social::handle(name, request, server, connection)?;
        return Ok(true);
    }
    if silence::owns(name) {
        silence::handle(name, request, server, connection)?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_owner(name: &str) -> Vec<&'static str> {
        let mut owners = Vec::new();
        if account::owns(name) { owners.push("account") }
        if system::owns(name) { owners.push("system") }
        if zone::owns(name) { owners.push("zone") }
        if skill::owns(name) { owners.push("skill") }
        if item::owns(name) { owners.push("item") }
        if social::owns(name) { owners.push("social") }
        if silence::owns(name) { owners.push("silence") }
        owners
    }

    const EVERY_PACKET: [&str; 36] = [
        "C_CHECK_VERSION", "C_LOGIN_ARBITER", "C_GET_USER_LIST", "C_CAN_CREATE_USER",
        "C_CHECK_USERNAME", "C_CREATE_USER", "C_SELECT_USER", "C_RETURN_TO_LOBBY", "C_EXIT",
        "C_REQUEST_GAMESTAT_PING", "C_PONG",
        "C_LOAD_TOPO_FIN", "C_PLAYER_LOCATION", "C_SET_VISIBLE_RANGE", "C_REVIVE_NOW",
        "C_VISIT_NEW_SECTION", "C_SET_TARGET_INFO",
        "C_CANCEL_SKILL", "C_START_SKILL", "C_PRESS_SKILL", "C_SKILL_LEARN_LIST",
        "C_SKILL_LEARN_REQUEST",
        "C_TRY_LOOT_DROPITEM", "C_SHOW_ITEMLIST",
        "C_ADMIN", "C_CHAT", "C_GUARD_PK_POLICY",
        "C_NPC_CONTACT", "C_HARDWARE_INFO", "C_REQUEST_GUILD_PERK_LIST",
        "C_CAN_LOCKON_TARGET", "C_REQUEST_USABLE_CHARACTER_NAME",
        "C_DELETE_PARCEL", "C_WHISPER",
        "C_DIALOG_EVENT", "C_SELECT_CHANNEL",
    ];

    #[test]
    fn no_packet_is_claimed_by_two_subsystems() {
        for name in EVERY_PACKET {
            let owners = every_owner(name);
            assert!(
                owners.len() <= 1,
                "{name} is claimed by {owners:?}; the dispatch order would decide silently"
            );
        }
    }

    #[test]
    fn every_packet_we_know_about_has_an_owner() {
        for name in EVERY_PACKET {
            assert!(
                !every_owner(name).is_empty(),
                "{name} would fall through to no handler at all"
            );
        }
    }

    #[test]
    fn every_deliberate_silence_says_why() {
        for entry in silence::DELIBERATE {
            assert!(
                entry.why.len() > 20,
                "{} is silent without a stated reason",
                entry.packet
            );
            assert!(
                !silence::owns(entry.packet) || silence::reason(entry.packet).is_some(),
                "{} is listed but its reason cannot be looked up",
                entry.packet
            );
        }
    }

    #[test]
    fn a_silenced_packet_is_never_also_answered() {
        for entry in silence::DELIBERATE {
            let owners = every_owner(entry.packet);
            assert_eq!(
                owners,
                vec!["silence"],
                "{} is both answered and silenced",
                entry.packet
            );
        }
    }
}

#[cfg(test)]
mod scripted {
    use std::collections::BTreeMap;

    const POLICY: [(&str, &str, &str); 1] = [(
        "C_SIMPLE_TIP_REPEAT_CHECK",
        "hide",
        "a preference the server is entitled to decide, not a claim about game state",
    )];

    fn allowed(request: &str, field: &str) -> bool {
        POLICY
            .iter()
            .any(|(packet, name, _)| *packet == request && *name == field)
    }

    fn table() -> BTreeMap<String, serde_json::Value> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/responses.json");
        let text = std::fs::read_to_string(path).expect("responses.json");
        serde_json::from_str(&text).expect("responses.json parses")
    }

    #[test]
    fn no_scripted_reply_asserts_a_state_the_server_cannot_back() {
        let mut lying = Vec::new();
        for (request, replies) in table() {
            let Some(list) = replies.as_array() else { continue };
            for reply in list {
                let Some(fields) = reply.get("fields").and_then(|f| f.as_object()) else {
                    continue;
                };
                for (name, value) in fields {
                    let echoed = value.as_str().map(|t| t.starts_with('$')).unwrap_or(false);
                    let empty = match value {
                        serde_json::Value::Bool(flag) => !flag,
                        serde_json::Value::Number(number) => number.as_f64() == Some(0.0),
                        serde_json::Value::String(text) => text.is_empty(),
                        serde_json::Value::Array(items) => items.is_empty(),
                        serde_json::Value::Null => true,
                        _ => false,
                    };
                    if !echoed && !empty && !allowed(&request, name) {
                        lying.push(format!("{request} claims {name} = {value}"));
                    }
                }
            }
        }
        assert!(
            lying.is_empty(),
            "scripted replies may only echo the request or answer empty, but: {lying:#?}"
        );
    }

    #[test]
    fn nothing_is_both_scripted_and_deliberately_silent() {
        for request in table().keys() {
            assert!(
                !super::silence::owns(request),
                "{request} is scripted and silenced at the same time"
            );
        }
    }
}
