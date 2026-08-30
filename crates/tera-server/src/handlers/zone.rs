use crate::session::{Connection, Server, moved_far_enough, refresh_visibility, send};
use crate::world;
use anyhow::Result;
use tera_protocol::value::{Object, Value};

pub fn owns(name: &str) -> bool {
    matches!(
        name,
        "C_LOAD_TOPO_FIN"
            | "C_PLAYER_LOCATION"
            | "C_SET_VISIBLE_RANGE"
            | "C_REVIVE_NOW"
            | "C_VISIT_NEW_SECTION"
            | "C_SET_TARGET_INFO"
            | "C_CAN_LOCKON_TARGET"
    )
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
        "C_LOAD_TOPO_FIN" => {
            reply(
                "S_SPAWN_ME",
                Object::new()
                    .with("gameId", Value::Uint(connection.state.game_id))
                    .with("loc", Value::Vec3(connection.state.location))
                    .with("w", Value::Int(connection.state.angle))
                    .with("alive", Value::Bool(true))
                    .with("isLord", Value::Bool(false)),
                connection,
            )?;
            if let Some(character) = world.find(connection.state.character) {
                reply("S_PLAYER_STAT_UPDATE", character.stats(), connection)?;
                let data = character.inventory_data(connection.state.game_id);
                reply("S_INVEN_USERDATA", data, connection)?;
                let inventory = character.inventory_list(connection.state.game_id);
                reply("S_ITEMLIST", inventory, connection)?;
                let learned = world.learned(character.id);
                if !learned.is_empty() {
                    let list = learned
                        .iter()
                        .map(|skill| {
                            Object::new()
                                .with("id", Value::Uint(*skill as u64))
                                .with("active", Value::Bool(true))
                        })
                        .collect();
                    reply(
                        "S_SKILL_LIST",
                        Object::new().with("skills", Value::Array(list)),
                        connection,
                    )?;
                }
                if !character.equipment.is_empty() {
                    let items = character.item_list(connection.state.game_id);
                    reply("S_ITEMLIST", items, connection)?;
                }
            }
            reply(
                "S_USER_STATUS",
                Object::new()
                    .with("gameId", Value::Uint(connection.state.game_id))
                    .with("status", Value::Int(0))
                    .with("bySkill", Value::Bool(false)),
                connection,
            )?;
            connection.visible.clear();
            connection.refreshed_at = [f32::MAX; 3];
            refresh_visibility(server, connection)
        },
        "C_REVIVE_NOW" => {
            let Some(character) = world.update(connection.state.character, |character| {
                character.revive();
            }) else {
                return Ok(());
            };
            connection.state.zone = world::SPAWN_ZONE;
            connection.state.location = world::SPAWN_POINT;
            connection.state.angle = world::SPAWN_ANGLE;
            logger.line(format!("   {} revived", character.name));
            reply(
                "S_CREATURE_LIFE",
                character.life_packet(connection.state.game_id, connection.state.location),
                connection,
            )?;
            reply(
                "S_INSTANT_MOVE",
                Object::new()
                    .with("gameId", Value::Uint(connection.state.game_id))
                    .with("loc", Value::Vec3(connection.state.location))
                    .with("w", Value::Int(connection.state.angle)),
                connection,
            )?;
            reply("S_PLAYER_STAT_UPDATE", character.stats(), connection)
        },
        "C_PLAYER_LOCATION" => {
            if let Some(object) = request {
                if let Some(Value::Vec3(location)) = object.get("loc") {
                    connection.state.location = *location;
                }
                if let Some(angle) = object.get("w").and_then(Value::as_int) {
                    connection.state.angle = angle;
                }
            }
            if moved_far_enough(connection) {
                return refresh_visibility(server, connection);
            }
            Ok(())
        },
        "C_SET_VISIBLE_RANGE" => {
            if let Some(range) = request
                .and_then(|object| object.get("range"))
                .and_then(Value::as_uint)
            {
                connection.range = range as f32;
                logger.line(format!("   visible range {range}"));
            }
            Ok(())
        },
        "C_VISIT_NEW_SECTION" => {
            let field = |key: &str| request.and_then(|r| r.get(key)).and_then(Value::as_int).unwrap_or(0);
            let (map, guard, section) = (field("mapId"), field("guardId"), field("unk"));
            let first = world.visit_section(connection.state.character, map, guard, section);
            reply(
                "S_VISIT_NEW_SECTION",
                Object::new()
                    .with("mapId", Value::Int(map))
                    .with("guardId", Value::Int(guard))
                    .with("sectionId", Value::Int(section))
                    .with("isFirstVisit", Value::Bool(first)),
                connection,
            )
        }
        "C_SET_TARGET_INFO" => {
            let target = request
                .and_then(|r| r.get("target"))
                .and_then(Value::as_uint)
                .unwrap_or(0);
            connection.state.target = target;
            let Some((zone, creature)) = server.realm.find(target) else {
                return Ok(());
            };
            if zone != connection.state.zone || !creature.alive() {
                return Ok(());
            }
            let share = match creature.max_hp {
                0 => 0.0,
                max => creature.hp.max(0) as f64 / max as f64,
            };
            reply(
                "S_TARGET_INFO",
                Object::new()
                    .with("target", Value::Uint(target))
                    .with("hpPercentage", Value::Float(share))
                    .with("level", Value::Int(creature.level))
                    .with("itemLevel", Value::Float(0.0))
                    .with("stPercentage", Value::Float(0.0)),
                connection,
            )
        }
        "C_CAN_LOCKON_TARGET" => {
            let field = |key: &str| request.and_then(|r| r.get(key)).and_then(Value::as_uint).unwrap_or(0);
            let (target, skill) = (field("target"), field("skill"));
            let reachable = server
                .realm
                .find(target)
                .map(|(zone, creature)| zone == connection.state.zone && creature.alive())
                .unwrap_or(false);
            reply(
                "S_CAN_LOCKON_TARGET",
                Object::new()
                    .with("target", Value::Uint(target))
                    .with("skill", Value::Uint(skill))
                    .with("success", Value::Bool(reachable)),
                connection,
            )
        }
        _ => Ok(()),
    }
}
