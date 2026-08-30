use std::time::Instant;
use crate::session::{CAST_DURATION, Connection, Server, send, strike};
use anyhow::Result;
use tera_protocol::value::{Object, Value};

pub fn owns(name: &str) -> bool {
    matches!(name, "C_CANCEL_SKILL" | "C_START_SKILL" | "C_PRESS_SKILL" | "C_START_TARGETED_SKILL" | "C_START_INSTANCE_SKILL" | "C_START_COMBO_INSTANT_SKILL"
            | "C_SKILL_LEARN_LIST"
            | "C_SKILL_LEARN_REQUEST"
    )
}

pub fn handle(
    name: &str,
    request: Option<&Object>,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let (_logger, world) = (server.logger, server.world);
    let reply = |name: &str, object: Object, connection: &mut Connection| {
        send(name, object, server, connection)
    };
    match name {
        "C_CANCEL_SKILL" => {
            let Some(active) = connection.casting.take() else {
                return Ok(());
            };
            let Some(character) = world.find(connection.state.character) else {
                return Ok(());
            };
            let mut queued = None;
            connection.pending.retain(|(_, packet, object)| {
                let mine = *packet == "S_ACTION_END"
                    && object.get("id").and_then(Value::as_uint) == Some(active);
                if mine {
                    queued = Some(object.clone());
                }
                !mine
            });
            let Some(mut ending) = queued else {
                return Ok(());
            };
            let kind = request
                .and_then(|object| object.get("type"))
                .and_then(Value::as_int)
                .unwrap_or(2);
            ending.set("type", Value::Int(kind));
            ending.set("loc", Value::Vec3(connection.state.location));
            ending.set("w", Value::Int(connection.state.angle));
            ending.set("templateId", Value::Int(character.template_id()));
            reply("S_ACTION_END", ending, connection)
        },
        "C_START_SKILL" | "C_PRESS_SKILL" | "C_START_TARGETED_SKILL"
        | "C_START_INSTANCE_SKILL" | "C_START_COMBO_INSTANT_SKILL" => {
            let Some(character) = world.find(connection.state.character) else {
                return Ok(());
            };
            let Some(request) = request else {
                return Ok(());
            };
            if name == "C_PRESS_SKILL"
                && !matches!(request.get("press"), Some(Value::Bool(true)))
            {
                return Ok(());
            }
            connection.action += 1;
            let skill = request.get("skill").and_then(Value::as_uint).unwrap_or(0);
            let facing = connection.state.angle;
            let location = connection.state.location;
            let destination = match request.get("dest") {
                Some(Value::Vec3(at)) if at != &[0.0; 3] => *at,
                _ => location,
            };
            let stage = Object::new()
                .with("gameId", Value::Uint(connection.state.game_id))
                .with("loc", Value::Vec3(location))
                .with("w", Value::Int(facing))
                .with("templateId", Value::Int(character.template_id()))
                .with("skill", Value::Uint(skill))
                .with("stage", Value::Int(0))
                .with("speed", Value::Float(1.0))
                .with("projectileSpeed", Value::Float(1.0))
                .with("id", Value::Uint(connection.action))
                .with("effectScale", Value::Float(1.0))
                .with(
                    "moving",
                    Value::Bool(
                        request
                            .get("moving")
                            .map(|value| matches!(value, Value::Bool(true)))
                            .unwrap_or(false),
                    ),
                )
                .with("dest", Value::Vec3(destination))
                .with(
                    "target",
                    Value::Uint(request.get("target").and_then(Value::as_uint).unwrap_or(0)),
                )
                .with("animSeq", Value::Array(Vec::new()));
            reply("S_ACTION_STAGE", stage, connection)?;

            let ending = Object::new()
                .with("gameId", Value::Uint(connection.state.game_id))
                .with("loc", Value::Vec3(location))
                .with("w", Value::Int(facing))
                .with("templateId", Value::Int(character.template_id()))
                .with("skill", Value::Uint(skill))
                .with("type", Value::Int(0))
                .with("id", Value::Uint(connection.action));
            connection.casting = Some(connection.action);
            connection
                .pending
                .push((Instant::now() + CAST_DURATION, "S_ACTION_END", ending));

            strike(server, connection, &character, skill, Some(request))
        },
        "C_SKILL_LEARN_LIST" => reply(
            "S_SKILL_LEARN_LIST",
            Object::new().with("skills", Value::Array(Vec::new())),
            connection,
        ),
        "C_SKILL_LEARN_REQUEST" => {
            let wanted = request
                .and_then(|r| r.get("id"))
                .and_then(Value::as_uint)
                .unwrap_or(0);
            reply(
                "S_SKILL_LEARN_RESULT",
                Object::new()
                    .with("success", Value::Bool(false))
                    .with("forced", Value::Bool(false))
                    .with("oldSkill", Value::Uint(0))
                    .with("newSkill", Value::Uint(wanted)),
                connection,
            )
        }
        _ => Ok(()),
    }
}
