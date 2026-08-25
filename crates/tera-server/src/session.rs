use crate::log::Log;
use crate::commands::{self, Action};
use crate::registry::Registry;
use crate::responses::{self, Context, Responses};
use crate::world::{self, World};
use anyhow::{bail, Result};
use std::collections::HashSet;
use tera_protocol::defs::{Definition, Field};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tera_protocol::handshake::{random_key, ServerHandshake, Step};
use tera_protocol::session::{LEGACY, MODERN};
use tera_protocol::value::{write as write_packet, Object, Value};
use tera_protocol::{OpcodeMap, PacketBuffer, Session};

const PING_INTERVAL: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub struct Connection {
    stream: TcpStream,
    session: Session,
    state: commands::State,
    announced: HashSet<String>,
    visible: HashSet<u64>,
    action: u64,
    casting: Option<u64>,
    pending: Vec<(Instant, &'static str, Object)>,
    range: f32,
    refreshed_at: [f32; 3],
}

impl Connection {
    pub fn send(&mut self, packet: &[u8]) -> Result<()> {
        let mut framed = packet.to_vec();
        self.session.encrypt(&mut framed);
        self.stream.write_all(&framed)?;
        Ok(())
    }
}

pub struct Server<'a> {
    pub opcodes: &'a OpcodeMap,
    pub registry: &'a Registry,
    pub logger: &'a Log,
    pub world: &'a World,
    pub worlds: &'a crate::worlds::Worlds,
    pub npcs: &'a crate::npcs::Npcs,
    pub items: &'a crate::items::Items,
    pub skills: &'a crate::skills::Skills,
    pub realm: &'a crate::realm::Realm,
    pub spawns: &'a crate::spawns::Spawns,
    pub responses: &'a Responses,
    pub auto_reply: bool,
    pub auto_reply_aliases: bool,
}

pub fn serve(mut stream: TcpStream, server: &Server<'_>, legacy: bool) -> Result<()> {
    let Server {
        opcodes,
        registry,
        logger,
        ..
    } = server;
    stream.set_nodelay(true)?;
    let mut handshake = ServerHandshake::new(random_key(), random_key());
    if legacy {
        handshake = handshake.with_constants(LEGACY);
    } else {
        handshake = handshake.with_constants(MODERN);
    }
    stream.write_all(&handshake.greeting())?;
    logger.line("sent the 4 byte greeting");

    let mut buffer = [0u8; 8192];
    let mut session = loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            bail!("client closed during the handshake");
        }
        match handshake.feed(&buffer[..read]) {
            Step::Send(reply) => {
                logger.line(format!("client key 1 received, sent server key 1 ({} bytes)", reply.len()));
                stream.write_all(&reply)?;
            }
            Step::Established(session) => {
                logger.line("client key 2 received, sending server key 2");
                stream.write_all(handshake.server_second())?;
                logger.line("session keys derived, traffic is now encrypted");
                break *session;
            }
            Step::Wait => {}
        }
    };

    let leftover = handshake.leftover();
    let mut packets = PacketBuffer::new();
    if !leftover.is_empty() {
        let mut data = leftover;
        session.decrypt(&mut data);
        packets.push(&data);
    }

    let mut connection = Connection {
        stream,
        session,
        state: commands::State {
            markup: true,
            channel: 0,
            game_id: 0,
            character: 0,
            zone: world::SPAWN_ZONE,
            location: world::SPAWN_POINT,
            angle: world::SPAWN_ANGLE,
            next_npc: 0,
        },
        announced: HashSet::new(),
        visible: HashSet::new(),
        action: 0,
        casting: None,
        pending: Vec::new(),
        range: crate::realm::DEFAULT_VISIBLE_RANGE,
        refreshed_at: [f32::MAX; 3],
    };
    connection.stream.set_read_timeout(Some(POLL_INTERVAL))?;
    let mut last_ping = Instant::now();
    let mut last_tick = Instant::now();
    loop {
        match connection.stream.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => {
                let mut data = buffer[..read].to_vec();
                connection.session.decrypt(&mut data);
                packets.push(&data);
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(error) => return Err(error.into()),
        }
        while let Some(packet) = packets.take_packet() {
            let name = opcodes
                .name(packet.opcode)
                .map(str::to_string)
                .unwrap_or_else(|| format!("UNKNOWN_{}", packet.opcode));
            logger.packet("<-", &name, packet.opcode, &packet.body);
            let mut request = None;
            if let Some(definition) = registry.get(&name) {
                match tera_protocol::value::read(definition, &packet.encode()) {
                    Ok(object) => {
                        logger.line(format!("   {}", describe(&object)));
                        request = Some(object);
                    }
                    Err(error) => logger.line(format!("   decode failed: {error}")),
                }
            }
            handle(&name, request, server, &mut connection)?;
        }
        if last_tick.elapsed() >= AI_INTERVAL {
            last_tick = Instant::now();
            think(server, &mut connection)?;
        }
        let now = Instant::now();
        while let Some(index) = connection
            .pending
            .iter()
            .position(|(due, _, _)| *due <= now)
        {
            let (_, packet, object) = connection.pending.remove(index);
            if packet == "S_ACTION_END"
                && connection.casting == object.get("id").and_then(Value::as_uint)
            {
                connection.casting = None;
            }
            send(packet, object, server, &mut connection)?;
        }
        if last_ping.elapsed() >= PING_INTERVAL {
            last_ping = Instant::now();
            send("S_PING", Object::new(), server, &mut connection)?;
        }
    }
}

fn describe(object: &Object) -> String {
    object
        .fields
        .iter()
        .map(|(name, value)| match value {
            Value::Str(text) => format!("{name}=\"{text}\""),
            Value::Bytes(bytes) => format!("{name}=<{} bytes>", bytes.len()),
            Value::Array(items) => format!("{name}=[{} items]", items.len()),
            Value::List(items) => format!("{name}=[{} values]", items.len()),
            Value::Object(_) => format!("{name}={{..}}"),
            Value::Bool(flag) => format!("{name}={flag}"),
            Value::Int(number) => format!("{name}={number}"),
            Value::Uint(number) => format!("{name}={number}"),
            Value::Float(number) => format!("{name}={number}"),
            Value::Vec3(vector) => format!("{name}=({:.1},{:.1},{:.1})", vector[0], vector[1], vector[2]),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn send(
    name: &str,
    object: Object,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let (logger, registry) = (server.logger, server.registry);
    let Some(opcode) = server.opcodes.code(name) else {
        logger.line(format!("   no opcode for {name}, skipped"));
        return Ok(());
    };
    let Some(definition) = registry.get(name) else {
        logger.line(format!("   no definition for {name}, skipped"));
        return Ok(());
    };
    let bytes = write_packet(definition, opcode, &object)?;
    logger.packet("->", name, opcode, &bytes[4..]);
    connection.send(&bytes)
}

fn handle(
    name: &str,
    request: Option<Object>,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let request = request.as_ref();
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
        }
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
        }
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
        }
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
        }
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
        }
        "C_EXIT" => {
            remember(world, connection);
            reply(
            "S_EXIT",
                Object::new()
                    .with("category", Value::Int(0))
                    .with("code", Value::Int(0)),
                connection,
            )
        }
        "C_REQUEST_GAMESTAT_PING" => {
            reply("S_RESPONSE_GAMESTAT_PONG", Object::new(), connection)
        }
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
        }
        "C_PONG" => Ok(()),
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
        }
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
        }
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
        }
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
        }
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
        }
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
        }
        "C_SET_VISIBLE_RANGE" => {
            if let Some(range) = request
                .and_then(|object| object.get("range"))
                .and_then(Value::as_uint)
            {
                connection.range = range as f32;
                logger.line(format!("   visible range {range}"));
            }
            Ok(())
        }
        "C_ADMIN" => {
            let line = request
                .and_then(|object| object.get("command"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            apply(&line, server, connection)
        }
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
        }
        _ => fallback(name, request, server, connection),
    }
}

fn apply(line: &str, server: &Server<'_>, connection: &mut Connection) -> Result<()> {
    server.logger.line(format!("   command {line}"));
    let tables = commands::Tables {
        worlds: server.worlds,
        npcs: server.npcs,
        items: server.items,
        skills: server.skills,
        realm: server.realm,
        spawns: server.spawns,
    };
    for action in commands::run(line, &mut connection.state, server.world, &tables) {
        match action {
            Action::Say(text) => {
                let channel = connection.state.channel;
                say(channel, text, server, connection)?;
            }
            Action::Warn(text) => {
                let channel = connection.state.channel;
                warn(channel, text, server, connection)?;
            }
            Action::SayOn(channel, text) => say_as_chat(channel, text, server, connection)?,
            Action::Send(packet, object) => {
                send(packet, object, server, connection)?;
            }
            Action::Refresh => {
                connection.refreshed_at = connection.state.location;
                refresh_visibility(server, connection)?;
            }
        }
    }
    Ok(())
}

const EVENT_MESSAGE_TYPE: u64 = 2;

fn say(
    channel: u64,
    text: String,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    event_message(channel, text, commands::NOTICE_COLOUR, server, connection)
}

fn warn(
    channel: u64,
    text: String,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    event_message(channel, text, commands::WARNING_COLOUR, server, connection)
}

fn event_message(
    channel: u64,
    text: String,
    colour: &str,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let message = if connection.state.markup {
        commands::as_markup(&text, colour)
    } else {
        text
    };
    let object = Object::new()
        .with("type", Value::Uint(EVENT_MESSAGE_TYPE))
        .with("chat", Value::Bool(true))
        .with("channel", Value::Uint(channel))
        .with("message", Value::Str(message));
    send("S_DUNGEON_EVENT_MESSAGE", object, server, connection)
}

fn say_as_chat(
    channel: u64,
    text: String,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let object = Object::new()
        .with("channel", Value::Uint(channel))
        .with("gameId", Value::Uint(connection.state.game_id))
        .with("isWorldEventTarget", Value::Bool(false))
        .with("gm", Value::Bool(true))
        .with("founder", Value::Bool(false))
        .with("name", Value::Str("server".into()))
        .with("message", Value::Str(commands::as_markup(&text, commands::NOTICE_COLOUR)));
    send("S_CHAT", object, server, connection)
}

fn fallback(
    name: &str,
    request: Option<&Object>,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    let (logger, registry, world) = (server.logger, server.registry, server.world);
    let replies = server.responses.get(name);
    if !replies.is_empty() {
        let context = Context {
            game_id: connection.state.game_id,
            character: world.find(connection.state.character),
            zone: world::SPAWN_ZONE,
            location: world::SPAWN_POINT,
            angle: world::SPAWN_ANGLE,
            uptime_ms: logger.uptime_ms(),
            request: request.cloned(),
        };
        for reply in replies {
            let Some(definition) = registry.get(&reply.packet) else {
                logger.line(format!("   no definition for {}, skipped", reply.packet));
                continue;
            };
            let object = responses::object_from_json(definition, &reply.fields, &context);
            send(&reply.packet, object, server, connection)?;
        }
        return Ok(());
    }
    if server.auto_reply {
        for candidate in response_names(name, server.auto_reply_aliases) {
            if let (Some(_), Some(definition)) =
                (server.opcodes.code(&candidate), registry.get(&candidate))
            {
                let echoed = echo(definition, request);
                if connection.announced.insert(candidate.clone()) {
                    logger.line(format!(
                        "   auto-reply {candidate}, echoing {} field(s)",
                        echoed.fields.len()
                    ));
                }
                return send(&candidate, echoed, server, connection);
            }
        }
        if connection.announced.insert(name.to_string()) {
            logger.line("   no answer known for this request");
        }
    }
    Ok(())
}

const REFRESH_DISTANCE: f32 = 200.0;
const CAST_DURATION: Duration = Duration::from_millis(900);
const REACH: f32 = 250.0;
const CORPSE_LINGER: Duration = Duration::from_secs(8);
const LOOT_REACH: f32 = 200.0;
const AI_INTERVAL: Duration = Duration::from_millis(700);
const KEEP_DISTANCE: f32 = 90.0;

fn think(server: &Server<'_>, connection: &mut Connection) -> Result<()> {
    if connection.state.character == 0 {
        return Ok(());
    }
    let here = connection.state.location;
    let hunters = server
        .realm
        .near(connection.state.zone, here, crate::realm::NOTICE_RANGE)
        .into_iter()
        .filter(|creature| creature.alive() && creature.aggressive)
        .take(4)
        .collect::<Vec<_>>();
    for creature in hunters {
        let gap = distance(creature.location, here);
        if gap <= KEEP_DISTANCE {
            maul(server, connection, &creature)?;
            continue;
        }
        let facing = crate::realm::bearing(creature.location, here);
        let stride = (creature.run_speed as f32 * 0.7).max(30.0);
        let destination = crate::realm::step_towards(creature.location, here, stride);
        send(
            "S_NPC_STATUS",
            creature.status_packet(connection.state.game_id),
            server,
            connection,
        )?;
        send(
            "S_NPC_LOCATION",
            creature.walk_packet(destination, facing),
            server,
            connection,
        )?;
        server.realm.move_to(creature.id, destination, facing);
    }
    Ok(())
}

fn maul(
    server: &Server<'_>,
    connection: &mut Connection,
    creature: &crate::realm::Creature,
) -> Result<()> {
    let Some(character) = server.world.find(connection.state.character) else {
        return Ok(());
    };
    if !character.alive() {
        return Ok(());
    }
    let damage = (5 + creature.level * 4).max(1);
    let mut died = false;
    let updated = server.world.update(character.id, |character| {
        died = character.wound(damage);
    });
    let Some(character) = updated else {
        return Ok(());
    };

    send(
        "S_EACH_SKILL_RESULT",
        Object::new()
            .with("source", Value::Uint(creature.id))
            .with("owner", Value::Uint(0))
            .with("target", Value::Uint(connection.state.game_id))
            .with("templateId", Value::Int(creature.template))
            .with("value", Value::Int(damage))
            .with("type", Value::Int(1))
            .with("crit", Value::Bool(false))
            .with("damageType", Value::Int(1)),
        server,
        connection,
    )?;
    send(
        "S_CREATURE_CHANGE_HP",
        Object::new()
            .with("curHp", Value::Uint(character.health() as u64))
            .with("maxHp", Value::Uint(character.max_hp() as u64))
            .with("diff", Value::Int(-damage))
            .with("type", Value::Uint(0))
            .with("target", Value::Uint(connection.state.game_id))
            .with("source", Value::Uint(creature.id))
            .with("crit", Value::Uint(0))
            .with("abnormId", Value::Uint(0)),
        server,
        connection,
    )?;
    send("S_PLAYER_STAT_UPDATE", character.stats(), server, connection)?;

    if died {
        server.logger.line(format!(
            "   {} was killed by {:#x}",
            character.name, creature.id
        ));
        send(
            "S_CREATURE_LIFE",
            character.life_packet(connection.state.game_id, connection.state.location),
            server,
            connection,
        )?;
        send(
            "S_SHOW_REVIVE_UI",
            Object::new()
                .with("unk", Value::Int(0))
                .with("name", Value::Str(character.name.clone())),
            server,
            connection,
        )?;
    }
    Ok(())
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}


fn strike(
    server: &Server<'_>,
    connection: &mut Connection,
    character: &crate::world::Character,
    skill: u64,
    request: Option<&Object>,
) -> Result<()> {
    let asked = request
        .and_then(|object| object.get("target"))
        .and_then(Value::as_uint)
        .or_else(|| {
            request
                .and_then(|object| object.get("targets"))
                .and_then(|value| match value {
                    Value::Array(items) => items.first(),
                    _ => None,
                })
                .and_then(|first| first.get("gameId"))
                .and_then(Value::as_uint)
        });
    let target = match asked.filter(|id| *id != 0) {
        Some(id) => server.realm.find(id).map(|(_, creature)| creature),
        None => server
            .realm
            .nearest(connection.state.zone, connection.state.location, REACH),
    };
    let Some(target) = target.filter(|creature| creature.alive()) else {
        return Ok(());
    };

    let damage = character.attack();
    let Some(hit) = server.realm.damage(target.id, damage) else {
        return Ok(());
    };

    let result = Object::new()
        .with("source", Value::Uint(connection.state.game_id))
        .with("owner", Value::Uint(0))
        .with("target", Value::Uint(hit.id))
        .with("templateId", Value::Int(character.template_id()))
        .with("skill", Value::Uint(skill))
        .with("originSkill", Value::Uint(skill))
        .with("id", Value::Uint(connection.action))
        .with("value", Value::Int(damage))
        .with("type", Value::Int(1))
        .with("crit", Value::Bool(false))
        .with("damageType", Value::Int(1));
    send("S_EACH_SKILL_RESULT", result, server, connection)?;
    send("S_SHOW_HP", hit.health_packet(), server, connection)?;
    send(
        "S_CREATURE_CHANGE_HP",
        hit.change_packet(damage, connection.state.game_id),
        server,
        connection,
    )?;
    send(
        "S_CREATURE_LIFE",
        server.realm.life_packet(&hit),
        server,
        connection,
    )?;

    if hit.alive() {
        return Ok(());
    }

    server.logger.line(format!(
        "   killed {:#x} template {} level {}",
        hit.id, hit.template, hit.level
    ));
    connection.visible.remove(&hit.id);
    connection.pending.push((
        Instant::now() + CORPSE_LINGER,
        "S_DESPAWN_NPC",
        hit.despawn_packet(true),
    ));
    for (item, amount) in loot_for(server, &hit) {
        let dropped = server.realm.drop_item(
            connection.state.zone,
            item,
            amount,
            hit.location,
            connection.state.game_id,
        );
        send(
            "S_SPAWN_DROPITEM",
            dropped.spawn_packet(&character.name),
            server,
            connection,
        )?;
    }

    let earned = crate::world::xp_for_kill(hit.level, character.level);
    let mut levelled = false;
    let updated = server.world.update(character.id, |character| {
        levelled = character.gain(earned);
    });
    let Some(character) = updated else {
        return Ok(());
    };
    let mut change = character.experience(earned);
    change.set("monsterGameId", Value::Uint(hit.id));
    send("S_PLAYER_CHANGE_EXP", change, server, connection)?;
    if levelled {
        send(
            "S_USER_LEVELUP",
            Object::new()
                .with("gameId", Value::Uint(connection.state.game_id))
                .with("level", Value::Int(character.level)),
            server,
            connection,
        )?;
        send("S_PLAYER_STAT_UPDATE", character.stats(), server, connection)?;
    }
    Ok(())
}


fn moved_far_enough(connection: &mut Connection) -> bool {
    let (here, last) = (connection.state.location, connection.refreshed_at);
    let (dx, dy, dz) = (here[0] - last[0], here[1] - last[1], here[2] - last[2]);
    if dx * dx + dy * dy + dz * dz < REFRESH_DISTANCE * REFRESH_DISTANCE {
        return false;
    }
    connection.refreshed_at = here;
    true
}

fn refresh_visibility(server: &Server<'_>, connection: &mut Connection) -> Result<()> {
    if connection.state.character == 0 {
        return Ok(());
    }
    let near = server
        .realm
        .near(connection.state.zone, connection.state.location, connection.range);
    let present: HashSet<u64> = near.iter().map(|creature| creature.id).collect();
    let gone: Vec<u64> = connection
        .visible
        .difference(&present)
        .copied()
        .collect();
    for creature in &near {
        if connection.visible.insert(creature.id) {
            send("S_SPAWN_NPC", creature.spawn_packet(), server, connection)?;
        }
    }
    for id in gone {
        connection.visible.remove(&id);
        if let Some((_, creature)) = server.realm.find(id) {
            send("S_DESPAWN_NPC", creature.despawn_packet(false), server, connection)?;
        }
    }
    Ok(())
}

fn loot_for(server: &Server<'_>, creature: &crate::realm::Creature) -> Vec<(i64, i64)> {
    let seed = creature.id ^ (creature.template as u64) ^ (creature.level as u64) << 8;
    let mut loot = vec![(GOLD_ITEM, (creature.level * 3).max(1))];
    if seed.is_multiple_of(3) {
        if let Some(item) = server
            .items
            .by_id(HEALING_ITEM)
            .map(|item| item.id)
            .or(Some(HEALING_ITEM))
        {
            loot.push((item, 1));
        }
    }
    loot
}

const GOLD_ITEM: i64 = 88;
const HEALING_ITEM: i64 = 1;

fn remember(world: &World, connection: &Connection) {
    if connection.state.character == 0 {
        return;
    }
    world.remember(
        connection.state.character,
        connection.state.zone,
        connection.state.location,
        connection.state.angle,
    );
}

fn echo(definition: &Definition, request: Option<&Object>) -> Object {
    let mut object = Object::new();
    let Some(request) = request else {
        return object;
    };
    for (name, field) in &definition.fields {
        if matches!(
            field,
            Field::RefArray(_) | Field::RefBytes(_) | Field::RefString(_)
        ) {
            continue;
        }
        if let Some(value) = request.get(name) {
            object.set(name.clone(), value.clone());
        }
    }
    object
}

const NAMING: [(&str, &[&str]); 8] = [
    (
        "REQUEST_",
        &["", "REQUEST_", "RESULT_", "RESPONSE_", "REPLY_", "SHOW_"],
    ),
    ("RQ_", &["RP_", ""]),
    ("GET_", &["GET_", ""]),
    ("SHOW_", &["SHOW_", ""]),
    ("VIEW_", &["VIEW_", ""]),
    ("REGISTER_", &["RESULT_", ""]),
    ("ASK_", &["", "ANSWER_"]),
    ("CHECK_", &["", "RESULT_"]),
];

fn response_names(request: &str, aliases: bool) -> Vec<String> {
    let Some(body) = request.strip_prefix("C_") else {
        return Vec::new();
    };
    let mut names = vec![format!("S_{body}")];
    if !aliases {
        return names;
    }
    for (client, servers) in NAMING {
        let Some(core) = body.strip_prefix(client) else {
            continue;
        };
        for server in servers {
            names.push(format!("S_{server}{core}"));
        }
        break;
    }
    for fallback in ["RESPONSE_", "RESULT_", "REPLY_"] {
        names.push(format!("S_{fallback}{body}"));
    }
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::response_names;

    #[test]
    fn the_exact_name_is_tried_first() {
        let names = response_names("C_CHAT", true);
        assert_eq!(names[0], "S_CHAT");
    }

    #[test]
    fn a_request_prefix_offers_the_bare_name_too() {
        let names = response_names("C_REQUEST_GUILD_INFO", true);
        assert!(names.contains(&"S_REQUEST_GUILD_INFO".to_string()));
        assert!(names.contains(&"S_GUILD_INFO".to_string()));
        assert!(names.contains(&"S_RESPONSE_GUILD_INFO".to_string()));
    }

    #[test]
    fn other_prefixes_are_covered() {
        assert!(response_names("C_GET_GUILD_HISTORY", true).contains(&"S_GUILD_HISTORY".to_string()));
        assert!(response_names("C_SHOW_ITEMLIST", true).contains(&"S_ITEMLIST".to_string()));
    }

    #[test]
    fn a_request_answers_with_a_reply() {
        assert!(response_names("C_RQ_SKILL_POLISHING_LIST", true)
            .contains(&"S_RP_SKILL_POLISHING_LIST".to_string()));
        assert!(response_names("C_ASK_INTERACTIVE", true).contains(&"S_ANSWER_INTERACTIVE".to_string()));
        assert!(response_names("C_REGISTER_PARTY_INFO", true)
            .contains(&"S_RESULT_PARTY_INFO".to_string()));
    }

    #[test]
    fn a_server_packet_has_no_response_names() {
        assert!(response_names("S_CHAT", true).is_empty());
        assert_eq!(response_names("C_CHAT", false), vec!["S_CHAT".to_string()]);
    }
}
