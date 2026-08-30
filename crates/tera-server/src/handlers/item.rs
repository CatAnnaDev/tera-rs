use crate::session::{Connection, LOOT_REACH, Server, send};
use crate::world;
use anyhow::Result;
use tera_protocol::value::{Object, Value};

pub fn owns(name: &str) -> bool {
    matches!(name, "C_TRY_LOOT_DROPITEM" | "C_SHOW_ITEMLIST")
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
        "C_TRY_LOOT_DROPITEM" => {
            let wanted = request.and_then(|object| object.get("gameId")).and_then(Value::as_uint);
            let taken = match wanted.filter(|id| *id != 0) {
                Some(id) => server.realm.take_drop(id),
                None => server
                    .realm
                    .drops_near(connection.state.zone, connection.state.location, LOOT_REACH)
                    .first()
                    .and_then(|dropped| server.realm.take_drop(dropped.id)),
            };
            let Some(taken) = taken else {
                return Ok(());
            };
            reply("S_DESPAWN_DROPITEM", taken.despawn_packet(), connection)?;
            let item = taken.item;
            let amount = taken.amount;
            let updated = world.update(connection.state.character, |character| {
                character.carry(item, amount);
            });
            let Some(character) = updated else {
                return Ok(());
            };
            logger.line(format!("   looted {amount} of item {item}"));
            reply(
                "S_ITEMLIST",
                character.inventory_list(connection.state.game_id),
                connection,
            )
        },
        "C_SHOW_ITEMLIST" => {
            let Some(character) = world.find(connection.state.character) else {
                return Ok(());
            };
            let container = request
                .and_then(|object| object.get("container"))
                .and_then(Value::as_int)
                .unwrap_or(world::CONTAINER_INVENTORY);
            let list = character.container_list(connection.state.game_id, container, true);
            reply("S_ITEMLIST", list, connection)
        },
        _ => Ok(()),
    }
}
