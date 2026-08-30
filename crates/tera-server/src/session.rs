use crate::log::Log;
use crate::commands::{self, Action};
use crate::registry::Registry;
use crate::responses::{self, Context, Responses};
use crate::world::{self, World};
use anyhow::{bail, Result};
use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tera_protocol::handshake::{random_key, ServerHandshake, Step};
use tera_protocol::session::{LEGACY, MODERN};
use tera_protocol::value::{write as write_packet, Object, Value};
use tera_protocol::{OpcodeMap, PacketBuffer, Session};

pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(10);
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub struct Connection {
    pub(crate) stream: TcpStream,
    pub(crate) session: Session,
    pub(crate) state: commands::State,
    pub(crate) announced: HashSet<String>,
    pub(crate) cooling: std::collections::HashMap<(u64, i64), Instant>,
    pub(crate) visible: HashSet<u64>,
    pub(crate) action: u64,
    pub(crate) casting: Option<u64>,
    pub(crate) pending: Vec<(Instant, &'static str, Object)>,
    pub(crate) range: f32,
    pub(crate) refreshed_at: [f32; 3],
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
    pub attacks: &'a crate::npcskills::Attacks,
    pub villagers: &'a crate::villagers::Villagers,
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
            target: 0,
        },
        announced: HashSet::new(),
        cooling: std::collections::HashMap::new(),
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

pub(crate) fn describe(object: &Object) -> String {
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

pub(crate) fn send(
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
    let (_logger, _world) = (server.logger, server.world);
    let _reply = |name: &str, object: Object, connection: &mut Connection| {
        send(name, object, server, connection)
    };
    if crate::handlers::dispatch(name, request, server, connection)? {
        return Ok(());
    }
    fallback(name, request, server, connection)
}

pub(crate) fn apply(line: &str, server: &Server<'_>, connection: &mut Connection) -> Result<()> {
    server.logger.line(format!("   command {line}"));
    let tables = commands::Tables {
        villagers: server.villagers,
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

pub(crate) const EVENT_MESSAGE_TYPE: u64 = 2;

pub(crate) fn say(
    channel: u64,
    text: String,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    event_message(channel, text, commands::NOTICE_COLOUR, server, connection)
}

pub(crate) fn warn(
    channel: u64,
    text: String,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    event_message(channel, text, commands::WARNING_COLOUR, server, connection)
}

pub(crate) fn event_message(
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

pub(crate) fn say_as_chat(
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

pub(crate) fn fallback(
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
    if connection.announced.insert(name.to_string()) {
        logger.line("   no handler for this request, nothing sent");
    }
    Ok(())
}

pub(crate) const REFRESH_DISTANCE: f32 = 200.0;
pub(crate) const CAST_DURATION: Duration = Duration::from_millis(900);
pub(crate) const REACH: f32 = 250.0;
pub(crate) const CORPSE_LINGER: Duration = Duration::from_secs(8);
pub(crate) const LOOT_REACH: f32 = 200.0;
pub(crate) const AI_INTERVAL: Duration = Duration::from_millis(700);
pub(crate) const KEEP_DISTANCE: f32 = 90.0;
pub(crate) const STROLL_STRIDE: f32 = 60.0;

pub(crate) fn think(server: &Server<'_>, connection: &mut Connection) -> Result<()> {
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
    let now = Instant::now();
    let hunting: Vec<u64> = hunters.iter().map(|creature| creature.id).collect();
    for creature in hunters {
        let gap = distance(creature.location, here);
        let reach = server
            .attacks
            .longest_reach(creature.hunting_zone, creature.template)
            .unwrap_or(KEEP_DISTANCE);
        if gap <= reach {
            let ready = |skill: i64| {
                connection
                    .cooling
                    .get(&(creature.id, skill))
                    .map(|until| now >= *until)
                    .unwrap_or(true)
            };
            let chosen = server
                .attacks
                .choose(creature.hunting_zone, creature.template, gap, ready)
                .cloned();
            if let Some(attack) = &chosen {
                let wait = Duration::from_millis(attack.cool_time.max(0) as u64)
                    .max(AI_INTERVAL);
                connection.cooling.insert((creature.id, attack.id), now + wait);
            }
            maul(server, connection, &creature, chosen.as_ref())?;
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
            creature.walk_packet(destination, facing, creature.run_speed),
            server,
            connection,
        )?;
        server.realm.move_to(creature.id, destination, facing);
    }
    stroll(server, connection, now, &hunting)
}

pub(crate) const STROLLERS_PER_TICK: usize = 6;

fn stroll(
    server: &Server<'_>,
    connection: &mut Connection,
    now: Instant,
    hunting: &[u64],
) -> Result<()> {
    let here = connection.state.location;
    let sight = crate::realm::DEFAULT_VISIBLE_RANGE;
    let sight_squared = sight * sight;
    let wandered = server.realm.stroll(
        connection.state.zone,
        now,
        STROLL_STRIDE,
        STROLLERS_PER_TICK,
        |creature| {
            hunting.contains(&creature.id)
                || crate::realm::distance_squared(creature.location, here) > sight_squared
        },
    );
    for (creature, destination, facing) in wandered {
        send(
            "S_NPC_LOCATION",
            creature.walk_packet(destination, facing, creature.walk_speed),
            server,
            connection,
        )?;
    }
    Ok(())
}

pub(crate) fn maul(
    server: &Server<'_>,
    connection: &mut Connection,
    creature: &crate::realm::Creature,
    attack: Option<&crate::npcskills::Attack>,
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

    let used = attack
        .map(|attack| {
            tera_protocol::value::SkillId::creature(attack.id as u32, creature.hunting_zone as u16)
                .raw()
        })
        .unwrap_or(0);
    send(
        "S_EACH_SKILL_RESULT",
        Object::new()
            .with("source", Value::Uint(creature.id))
            .with("owner", Value::Uint(0))
            .with("target", Value::Uint(connection.state.game_id))
            .with("templateId", Value::Int(creature.template))
            .with("skill", Value::Uint(used))
            .with("originSkill", Value::Uint(used))
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

pub(crate) fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt()
}


pub(crate) fn strike(
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


pub(crate) fn moved_far_enough(connection: &mut Connection) -> bool {
    let (here, last) = (connection.state.location, connection.refreshed_at);
    let (dx, dy, dz) = (here[0] - last[0], here[1] - last[1], here[2] - last[2]);
    if dx * dx + dy * dy + dz * dz < REFRESH_DISTANCE * REFRESH_DISTANCE {
        return false;
    }
    connection.refreshed_at = here;
    true
}

pub(crate) const POPULATE_RADIUS: f32 = 6000.0;
pub(crate) const POPULATE_LIMIT: usize = 60;

pub(crate) fn populate_around(server: &Server<'_>, connection: &mut Connection) {
    if connection.state.character == 0 {
        return;
    }
    let placed = server.spawns.populate(
        server.realm,
        server.npcs,
        server.villagers,
        &crate::spawns::Around {
            continent: connection.state.zone,
            origin: connection.state.location,
            radius: POPULATE_RADIUS,
            limit: POPULATE_LIMIT,
        },
    );
    if placed > 0 {
        server.logger.line(format!(
            "   the world filled in: {placed} creatures placed within {POPULATE_RADIUS:.0} units, {} alive in zone {}",
            server.realm.count(connection.state.zone),
            connection.state.zone
        ));
    }
}

pub(crate) fn refresh_visibility(server: &Server<'_>, connection: &mut Connection) -> Result<()> {
    if connection.state.character == 0 {
        return Ok(());
    }
    populate_around(server, connection);
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

pub(crate) const GOLD_ITEM: i64 = 88;
pub(crate) const HEALING_ITEM: i64 = 1;

pub(crate) fn remember(world: &World, connection: &Connection) {
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






