use crate::session::{Connection, Server, remember, send};
use crate::world;
use anyhow::Result;
use tera_protocol::value::{Object, Value};

pub fn owns(name: &str) -> bool {
    matches!(name, "C_CHECK_VERSION" | "C_LOGIN_ARBITER" | "C_GET_USER_LIST" | "C_CAN_CREATE_USER" | "C_CHECK_USERNAME" | "C_REQUEST_USABLE_CHARACTER_NAME" | "C_CREATE_USER" | "C_SELECT_USER" | "C_RETURN_TO_LOBBY" | "C_EXIT")
}

pub fn handle(
    name: &str,
    request: Option<&Object>,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let (logger, world) = (server.logger, server.world);
    let reply = |name: &str, object: Object, connection: &mut Connection| {
        send(name, object, server, connection)
    };
    match name {
        "C_CHECK_VERSION" => reply(
            "S_CHECK_VERSION",
            Object::new().with("ok", Value::Bool(true)),
            connection,
        ),
        "C_LOGIN_ARBITER" => {
            let language = request
                .and_then(|object| object.get("language"))
                .and_then(Value::as_uint)
                .unwrap_or(6);
            reply(
                "S_LOGIN_ARBITER",
                Object::new()
                    .with("success", Value::Bool(true))
                    .with("loginQueue", Value::Bool(false))
                    .with("status", Value::Uint(0))
                    .with("unk", Value::Uint(0))
                    .with("language", Value::Uint(language))
                    .with("pvpDisabled", Value::Bool(false))
                    .with("unk1", Value::Uint(0))
                    .with("unk2", Value::Uint(0)),
                connection,
            )
        },
        "C_GET_USER_LIST" => reply(
            "S_GET_USER_LIST",
            world::user_list(&world.characters()),
            connection,
        ),
        "C_CAN_CREATE_USER" => reply(
            "S_CAN_CREATE_USER",
            Object::new().with("ok", Value::Bool(!world.is_full())),
            connection,
        ),
        "C_CHECK_USERNAME" => {
            let taken = request
                .and_then(|object| object.get("name"))
                .and_then(Value::as_str)
                .map(|name| world.name_taken(name))
                .unwrap_or(true);
            reply(
                "S_CHECK_USERNAME",
                Object::new().with("result", Value::Uint(u64::from(!taken))),
                connection,
            )
        },
        "C_CREATE_USER" => {
            let created = request.map(|object| world.create(object));
            match &created {
                Some(character) => logger.line(format!(
                    "   created {} (id {}, template {})",
                    character.name,
                    character.id,
                    character.template_id()
                )),
                None => logger.line("   C_CREATE_USER could not be decoded"),
            }
            reply(
                "S_CREATE_USER",
                Object::new().with("success", Value::Bool(created.is_some())),
                connection,
            )
        },
        "C_SELECT_USER" => {
            let id = request
                .and_then(|object| object.get("id"))
                .and_then(Value::as_uint)
                .unwrap_or(0) as u32;
            let Some(character) = world.find(id) else {
                logger.line(format!("   no character with id {id}"));
                return Ok(());
            };
            connection.state.game_id = 0x1_0000_0000 | u64::from(character.id);
            connection.state.character = character.id;
            let known = character.zone != 0;
            connection.state.zone = if known { character.zone } else { world::SPAWN_ZONE };
            connection.state.location = if known {
                character.location
            } else {
                world::SPAWN_POINT
            };
            connection.state.angle = if known {
                character.facing
            } else {
                world::SPAWN_ANGLE
            };
            logger.line(format!("   entering the world as {}", character.name));
            reply("S_LOGIN", character.login(connection.state.game_id, 1), connection)?;
            reply(
                "S_LOAD_TOPO",
                Object::new()
                    .with("zone", Value::Int(connection.state.zone))
                    .with("loc", Value::Vec3(connection.state.location))
                    .with("quick", Value::Bool(false)),
                connection,
            )?;
            reply(
                "S_LOAD_HINT",
                Object::new().with("unk1", Value::Uint(0)),
                connection,
            )
        },
        "C_RETURN_TO_LOBBY" => {
            remember(world, connection);
            connection.state.game_id = 0;
            connection.state.character = 0;
            reply(
                "S_PREPARE_RETURN_TO_LOBBY",
                Object::new().with("time", Value::Int(0)),
                connection,
            )?;
            reply("S_RETURN_TO_LOBBY", Object::new(), connection)
        },
        "C_EXIT" => {
            remember(world, connection);
            reply(
            "S_EXIT",
                Object::new()
                    .with("category", Value::Int(0))
                    .with("code", Value::Int(0)),
                connection,
            )
        },
        "C_REQUEST_USABLE_CHARACTER_NAME" => {
            let wanted = request
                .and_then(|r| r.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let usable = !wanted.trim().is_empty() && !world.name_taken(&wanted);
            reply(
                "S_RESULT_USABLE_CHARACTER_NAME",
                Object::new().with("ok", Value::Bool(usable)),
                connection,
            )
        }
        _ => Ok(()),
    }
}
