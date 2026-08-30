use crate::session::{send, Connection, Server};
use anyhow::Result;
use tera_protocol::value::Object;

pub fn owns(name: &str) -> bool {
    matches!(name, "C_REQUEST_GAMESTAT_PING" | "C_PONG")
}

pub fn handle(
    name: &str,
    _request: Option<&Object>,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let (_logger, _world) = (server.logger, server.world);
    let reply = |name: &str, object: Object, connection: &mut Connection| {
        send(name, object, server, connection)
    };
    match name {
        "C_REQUEST_GAMESTAT_PING" => {
            reply("S_RESPONSE_GAMESTAT_PONG", Object::new(), connection)
        },
        "C_PONG" => Ok(()),
        _ => Ok(()),
    }
}
