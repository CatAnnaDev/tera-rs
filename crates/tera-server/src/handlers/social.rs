use crate::commands;
use crate::session::{Connection, Server, apply, fallback, send};
use anyhow::Result;
use tera_protocol::value::{Object, Value};

pub fn owns(name: &str) -> bool {
    matches!(name, "C_ADMIN" | "C_CHAT" | "C_GUARD_PK_POLICY")
}

pub fn handle(
    name: &str,
    request: Option<&Object>,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let (_logger, _world) = (server.logger, server.world);
    let reply = |name: &str, object: Object, connection: &mut Connection| {
        send(name, object, server, connection)
    };
    match name {
        "C_ADMIN" => {
            let line = request
                .and_then(|object| object.get("command"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            apply(&line, server, connection)
        },
        "C_CHAT" => {
            let message = request
                .and_then(|object| object.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(id) = request
                .and_then(|object| object.get("channel"))
                .and_then(Value::as_uint)
            {
                connection.state.channel = id;
            }
            if commands::strip_markup(message)
                .trim_start()
                .starts_with(commands::PREFIX)
            {
                let line = message.to_string();
                return apply(&line, server, connection);
            }
            fallback(name, request, server, connection)
        },
        "C_GUARD_PK_POLICY" => reply(
            "S_GUARD_PK_POLICY",
            Object::new().with("unk", Value::Uint(0)),
            connection,
        ),
        _ => Ok(()),
    }
}
