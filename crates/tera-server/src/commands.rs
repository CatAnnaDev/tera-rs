use crate::world::{Character, World};
use crate::items::Items;
use crate::skills::Skills;
use crate::npcs::Npcs;
use crate::realm::Realm;
use crate::spawns::Spawns;
use crate::worlds::Worlds;
use tera_protocol::value::{Object, Value};

pub const PREFIX: &str = "!";

pub fn strip_markup(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut inside = false;
    for character in message.chars() {
        match character {
            '<' => inside = true,
            '>' => inside = false,
            other if !inside => out.push(other),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

pub const NOTICE_COLOUR: &str = "#F0FF89";
pub const WARNING_COLOUR: &str = "#EE1C24";

pub fn as_markup(text: &str, colour: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("<FONT color=\"{colour}\">{escaped}</FONT>")
}

pub struct State {
    pub markup: bool,
    pub channel: u64,
    pub game_id: u64,
    pub character: u32,
    pub zone: i64,
    pub location: [f32; 3],
    pub angle: i64,
    pub next_npc: u64,
    pub target: u64,
}

pub enum Action {
    Say(String),
    Warn(String),
    SayOn(u64, String),
    Send(&'static str, Object),
    Refresh,
}

pub const CHANNELS: [(u64, &str); 8] = [
    (0, "say"),
    (1, "party"),
    (2, "guild"),
    (3, "area"),
    (11, "private"),
    (21, "p-notice"),
    (25, "r-notice"),
    (27, "global"),
];

pub struct Tables<'a> {
    pub worlds: &'a Worlds,
    pub npcs: &'a Npcs,
    pub items: &'a Items,
    pub skills: &'a Skills,
    pub realm: &'a Realm,
    pub spawns: &'a Spawns,
    pub villagers: &'a crate::villagers::Villagers,
}

pub fn run(line: &str, state: &mut State, world: &World, tables: &Tables<'_>) -> Vec<Action> {
    let Tables {
        worlds,
        npcs,
        items,
        skills,
        realm,
        spawns,
        villagers,
    } = tables;
    let plain = strip_markup(line);
    let line = plain.trim();
    let line = line.strip_prefix(PREFIX).unwrap_or(line);
    let mut words = line.split_whitespace();
    let Some(name) = words.next() else {
        return Vec::new();
    };
    let arguments: Vec<&str> = words.collect();
    let Some(character) = world.find(state.character) else {
        return vec![Action::Say("no character selected".into())];
    };
    match name {
        "help" => vec![Action::Say(HELP.into())],
        "where" => vec![Action::Say(format!(
            "zone {} at {:.0} {:.0} {:.0}, facing {}",
            state.zone, state.location[0], state.location[1], state.location[2], state.angle
        ))],
        "tp" => teleport(&arguments, state),
        "zone" => zone(&arguments, state),
        "go" => go(&arguments, state, worlds),
        "worlds" => search(&arguments, worlds),
        "speed" => speed(&arguments, state, world, &character),
        "level" => level(&arguments, state, world, &character),
        "hp" | "heal" => heal(&arguments, world, &character),
        "npc" => npc(&arguments, state, npcs, realm),
        "populate" => {
            let radius = arguments
                .first()
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(6000.0);
            let placed = spawns.populate(
                realm,
                npcs,
                villagers,
                &crate::spawns::Around {
                    continent: state.zone,
                    origin: state.location,
                    radius,
                    limit: 60,
                },
            );
            vec![
                Action::Refresh,
                Action::Say(format!(
                    "placed {placed} creatures within {radius:.0} units, {} in zone {}",
                    realm.count(state.zone),
                    state.zone
                )),
            ]
        }
        "aggro" => {
            let on = !matches!(arguments.first().copied(), Some("off") | Some("0"));
            let count = realm.set_aggressive(state.zone, on);
            let nearest = realm
                .nearest(state.zone, state.location, f32::MAX)
                .map(|creature| {
                    let (dx, dy) = (
                        creature.location[0] - state.location[0],
                        creature.location[1] - state.location[1],
                    );
                    (dx * dx + dy * dy).sqrt()
                });
            let reach = match nearest {
                Some(distance) if distance <= crate::realm::NOTICE_RANGE => {
                    format!("nearest is {distance:.0} units away and will come")
                }
                Some(distance) => format!(
                    "nearest is {distance:.0} units away, too far to notice you past {:.0}",
                    crate::realm::NOTICE_RANGE
                ),
                None => "nothing is spawned here".to_string(),
            };
            vec![Action::Say(format!(
                "{count} creatures in zone {} are now {}, {reach}",
                state.zone,
                if on { "hostile" } else { "passive" }
            ))]
        }
        "clear" => {
            let removed = realm.clear(state.zone);
            vec![
                Action::Refresh,
                Action::Say(format!("removed {removed} creatures from zone {}", state.zone)),
            ]
        }
        "npcs" => npc_search(&arguments, npcs),
        "gm" => gm(&arguments, world, &character),
        "privilege" => {
            let level = arguments
                .first()
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(1);
            vec![
                Action::Send(
                    "S_ADMIN_PRIVILEGE",
                    Object::new().with("level", Value::Int(level)),
                ),
                Action::Send(
                    "S_QA_SET_ADMIN_LEVEL",
                    Object::new().with("level", Value::Int(level)),
                ),
                Action::Say(format!("admin level {level} granted, now press Alt+A")),
            ]
        }
        "relog" => vec![
            Action::Say("returning to the lobby".into()),
            Action::Send("S_PREPARE_RETURN_TO_LOBBY", Object::new().with("time", Value::Int(0))),
            Action::Send("S_RETURN_TO_LOBBY", Object::new()),
        ],
        "ch" => channel(&arguments, state),
        "markup" => {
            state.markup = !matches!(arguments.first().copied(), Some("off") | Some("0"));
            vec![Action::Say(format!(
                "markup {}",
                if state.markup { "on" } else { "off" }
            ))]
        }
        "chtest" => CHANNELS
            .iter()
            .map(|(id, label)| Action::SayOn(*id, format!("channel {id} is {label}")))
            .collect(),
        "msgtest" => message_test(state),
        "equip" => equip(&arguments, state, world, items),
        "items" => item_search(&arguments, items),
        "give" => give(&arguments, state, world, items),
        "bag" => bag(&character, items),
        "gold" => gold(&arguments, state, world, &character),
        "xp" => experience(&arguments, state, world, &character),
        "skills" => learn(&arguments, state, &character, skills, world),
        "gear" => gear(&character),
        "say" => vec![Action::Say(arguments.join(" "))],
        other => vec![Action::Warn(format!(
            "unknown command `{other}`, try {PREFIX}help"
        ))],
    }
}

const HELP: &str = "!where  !tp x y z  !go name  !worlds text  !zone id [x y z]  \
                    !speed run [walk]  !level n  \
                    !hp n  !heal  !npc name|id [here]  !npcs text  !clear  \
                    !equip slot name|id  !items text  !gear  !give name|id [count]  !bag  \
                    !gold n  !xp n  !skills [all]  \
                    !gm on|off  !privilege n  !relog  !say text  !ch n  !chtest  !msgtest  !markup on|off  --  \
                    slots: weapon body hand feet underwear head face";

fn numbers(arguments: &[&str]) -> Vec<f32> {
    arguments
        .iter()
        .filter_map(|value| value.parse::<f32>().ok())
        .collect()
}

fn teleport(arguments: &[&str], state: &mut State) -> Vec<Action> {
    let values = numbers(arguments);
    if values.len() < 3 {
        return vec![Action::Say(format!("usage: {PREFIX}tp x y z"))];
    }
    state.location = [values[0], values[1], values[2]];
    vec![
        Action::Send(
            "S_INSTANT_MOVE",
            Object::new()
                .with("gameId", Value::Uint(state.game_id))
                .with("loc", Value::Vec3(state.location))
                .with("w", Value::Int(state.angle)),
        ),
        Action::Say(format!(
            "moved to {:.0} {:.0} {:.0}",
            state.location[0], state.location[1], state.location[2]
        )),
    ]
}

fn zone(arguments: &[&str], state: &mut State) -> Vec<Action> {
    let Some(id) = arguments.first().and_then(|value| value.parse::<i64>().ok()) else {
        return vec![Action::Say(format!("usage: {PREFIX}zone id [x y z]"))];
    };
    state.zone = id;
    let values = numbers(&arguments[1..]);
    if values.len() >= 3 {
        state.location = [values[0], values[1], values[2]];
    }
    vec![
        Action::Send(
            "S_LOAD_TOPO",
            Object::new()
                .with("zone", Value::Int(state.zone))
                .with("loc", Value::Vec3(state.location))
                .with("quick", Value::Bool(false)),
        ),
        Action::Say(format!("loading zone {id}")),
    ]
}

fn go(arguments: &[&str], state: &mut State, worlds: &Worlds) -> Vec<Action> {
    let fragment = arguments.join(" ");
    if fragment.is_empty() {
        return vec![Action::Say(format!("usage: {PREFIX}go name"))];
    }
    let Some(place) = worlds.find(&fragment) else {
        return vec![Action::Warn(format!("no world matches `{fragment}`"))];
    };
    state.zone = place.continent;
    state.location = place.pos;
    vec![
        Action::Send(
            "S_LOAD_TOPO",
            Object::new()
                .with("zone", Value::Int(state.zone))
                .with("loc", Value::Vec3(state.location))
                .with("quick", Value::Bool(false)),
        ),
        Action::Say(format!(
            "{} on continent {}",
            place.name, place.continent
        )),
    ]
}

fn search(arguments: &[&str], worlds: &Worlds) -> Vec<Action> {
    let fragment = arguments.join(" ");
    let found = worlds.search(&fragment, 8);
    if found.is_empty() {
        return vec![Action::Warn(format!(
            "no world matches `{fragment}` out of {}",
            worlds.len()
        ))];
    }
    found
        .iter()
        .map(|place| Action::Say(format!("{} [{}]", place.name, place.continent)))
        .collect()
}

fn speed(
    arguments: &[&str],
    state: &State,
    world: &World,
    character: &Character,
) -> Vec<Action> {
    let values = numbers(arguments);
    if values.is_empty() {
        return vec![Action::Say(format!(
            "run {} walk {}",
            character.run_speed, character.walk_speed
        ))];
    }
    let run = values[0] as i64;
    let walk = values.get(1).map(|value| *value as i64).unwrap_or(run / 3);
    let updated = world.update(state.character, |value| {
        value.run_speed = run;
        value.walk_speed = walk;
    });
    match updated {
        Some(value) => vec![
            Action::Send("S_PLAYER_STAT_UPDATE", value.stats()),
            Action::Say(format!("run {run} walk {walk}")),
        ],
        None => vec![Action::Say("no character selected".into())],
    }
}

fn level(
    arguments: &[&str],
    state: &State,
    world: &World,
    character: &Character,
) -> Vec<Action> {
    let Some(value) = arguments.first().and_then(|value| value.parse::<i64>().ok()) else {
        return vec![Action::Say(format!("level {}", character.level))];
    };
    let level = value.clamp(1, 70);
    let updated = world.update(state.character, |character| {
        character.level = level;
        character.xp = 0;
    });
    match updated {
        Some(character) => vec![
            Action::Send("S_PLAYER_STAT_UPDATE", character.stats()),
            Action::Send(
                "S_USER_LEVELUP",
                Object::new()
                    .with("gameId", Value::Uint(state.game_id))
                    .with("level", Value::Int(level)),
            ),
            Action::Say(format!("level {level}")),
        ],
        None => vec![Action::Say("no character selected".into())],
    }
}

fn heal(arguments: &[&str], world: &World, character: &Character) -> Vec<Action> {
    let mut stats = character.stats();
    if let Some(value) = numbers(arguments).first() {
        stats.set("hp", Value::Int(*value as i64));
    }
    let _ = world;
    vec![
        Action::Send("S_PLAYER_STAT_UPDATE", stats),
        Action::Say("stats refreshed".into()),
    ]
}

fn npc(arguments: &[&str], state: &mut State, npcs: &Npcs, realm: &Realm) -> Vec<Action> {
    if arguments.is_empty() {
        return vec![Action::Say(format!(
            "usage: {PREFIX}npc name|id [huntingZone], try {PREFIX}npcs text"
        ))];
    }
    let explicit_zone = arguments.last().and_then(|value| value.parse::<i64>().ok());
    let (template, zone, label) = match arguments[0].parse::<i64>() {
        Ok(id) => {
            let known = npcs.by_id(id);
            let zone = arguments
                .get(1)
                .and_then(|value| value.parse::<i64>().ok())
                .or_else(|| known.map(|npc| npc.zone))
                .unwrap_or(state.zone);
            let label = known.map(|npc| npc.name.clone()).unwrap_or_else(|| id.to_string());
            (id, zone, label)
        }
        Err(_) => {
            let fragment = arguments.join(" ");
            let Some(found) = npcs.find(&fragment) else {
                return vec![Action::Warn(format!("no npc matches `{fragment}`"))];
            };
            (found.id, explicit_zone.unwrap_or(found.zone), found.name.clone())
        }
    };
    let location = if arguments.contains(&"here") {
        state.location
    } else {
        crate::realm::in_front_of(state.location, state.angle, 150.0)
    };
    let Some(known) = npcs.lookup(template, zone) else {
        return vec![Action::Warn(format!("no creature {template} in zone {zone}"))];
    };
    let creature = realm.spawn(state.zone, &known.spawn(location, state.angle));
    vec![
        Action::Refresh,
        Action::Say(format!(
            "{label} [{template}/{zone}] level {} shape {} as {:#x}",
            known.level, known.shape, creature.id
        )),
    ]
}

fn npc_search(arguments: &[&str], npcs: &Npcs) -> Vec<Action> {
    let fragment = arguments.join(" ");
    let found = npcs.search(&fragment, 8);
    if found.is_empty() {
        return vec![Action::Warn(format!(
            "no npc matches `{fragment}` out of {}",
            npcs.len()
        ))];
    }
    found
        .iter()
        .map(|npc| {
            Action::Say(format!(
                "{} [{}/{}] lvl{} hp{}{}",
                npc.name,
                npc.id,
                npc.zone,
                npc.level,
                npc.hp,
                if npc.boss { " boss" } else { "" }
            ))
        })
        .collect()
}

fn channel(arguments: &[&str], state: &mut State) -> Vec<Action> {
    match arguments.first().and_then(|value| value.parse::<u64>().ok()) {
        Some(id) => {
            state.channel = id;
            vec![Action::Say(format!("replies now go to channel {id}"))]
        }
        None => vec![Action::Say(format!(
            "replies go to channel {}, try {PREFIX}chtest",
            state.channel
        ))],
    }
}

fn message_test(state: &State) -> Vec<Action> {
    vec![
        Action::SayOn(state.channel, "1 S_CHAT".into()),
        Action::Send(
            "S_CUSTOM_STYLE_SYSTEM_MESSAGE",
            Object::new()
                .with("style", Value::Int(0))
                .with("message", Value::Str("2 S_CUSTOM_STYLE_SYSTEM_MESSAGE".into())),
        ),
        Action::Send(
            "S_DUNGEON_EVENT_MESSAGE",
            Object::new()
                .with("type", Value::Uint(2))
                .with("chat", Value::Bool(true))
                .with("channel", Value::Uint(state.channel))
                .with("message", Value::Str("3 S_DUNGEON_EVENT_MESSAGE in chat".into())),
        ),
        Action::Send(
            "S_DUNGEON_EVENT_MESSAGE",
            Object::new()
                .with("type", Value::Uint(2))
                .with("chat", Value::Bool(false))
                .with("channel", Value::Uint(state.channel))
                .with("message", Value::Str("4 S_DUNGEON_EVENT_MESSAGE on screen".into())),
        ),
    ]
}

fn equip(arguments: &[&str], state: &State, world: &World, items: &Items) -> Vec<Action> {
    let Some(slot) = arguments
        .first()
        .and_then(|value| crate::world::slot_by_name(value))
    else {
        return vec![Action::Say(format!(
            "usage: {PREFIX}equip slot name|id, try {PREFIX}items text"
        ))];
    };
    let rest = arguments[1..].join(" ");
    let Some(item) = rest
        .parse::<i64>()
        .ok()
        .or_else(|| items.find(&rest).map(|found| found.id))
    else {
        return vec![Action::Warn(format!("no item matches `{rest}`"))];
    };
    let updated = world.update(state.character, |character| character.equip(slot, item));
    match updated {
        Some(character) => vec![
            Action::Send(
                "S_USER_EXTERNAL_CHANGE",
                character.appearance_change(state.game_id),
            ),
            Action::Send("S_ITEMLIST", character.item_list(state.game_id)),
            match items.by_id(item) {
                Some(found) if found.is_equipment() => {
                    Action::Say(format!("slot {slot}: {}", found.describe()))
                }
                Some(found) => Action::Warn(format!(
                    "slot {slot}: {} is not equipment, nothing will show",
                    found.describe()
                )),
                None => Action::Say(format!("slot {slot} now holds item {item}")),
            },
        ],
        None => vec![Action::Warn("no character selected".into())],
    }
}

fn gold(
    arguments: &[&str],
    state: &State,
    world: &World,
    character: &Character,
) -> Vec<Action> {
    let Some(amount) = arguments.first().and_then(|value| value.parse::<i64>().ok()) else {
        return vec![Action::Say(format!("{} gold", character.gold))];
    };
    let updated = world.update(state.character, |character| character.gold = amount.max(0));
    match updated {
        Some(character) => vec![
            Action::Send("S_ITEMLIST", character.inventory_list(state.game_id)),
            Action::Say(format!("{} gold", character.gold)),
        ],
        None => vec![Action::Warn("no character selected".into())],
    }
}

fn experience(
    arguments: &[&str],
    state: &State,
    world: &World,
    character: &Character,
) -> Vec<Action> {
    let Some(amount) = arguments.first().and_then(|value| value.parse::<i64>().ok()) else {
        return vec![Action::Say(format!(
            "level {} at {} / {} xp",
            character.level,
            character.level_xp(),
            character.total_level_xp()
        ))];
    };
    let mut levelled = false;
    let updated = world.update(state.character, |character| {
        levelled = character.gain(amount);
    });
    let Some(character) = updated else {
        return vec![Action::Warn("no character selected".into())];
    };
    let mut actions = vec![Action::Send("S_PLAYER_CHANGE_EXP", character.experience(amount))];
    if levelled {
        actions.push(Action::Send(
            "S_USER_LEVELUP",
            Object::new()
                .with("gameId", Value::Uint(state.game_id))
                .with("level", Value::Int(character.level)),
        ));
        actions.push(Action::Send("S_PLAYER_STAT_UPDATE", character.stats()));
    }
    actions.push(Action::Say(format!(
        "level {} at {} / {} xp",
        character.level,
        character.level_xp(),
        character.total_level_xp()
    )));
    actions
}

fn learn(
    arguments: &[&str],
    state: &State,
    character: &Character,
    skills: &Skills,
    world: &World,
) -> Vec<Action> {
    let include_common = matches!(arguments.first().copied(), Some("all"));
    let learned = skills.for_class(character.class, include_common);
    if learned.is_empty() {
        return vec![Action::Warn(format!(
            "no skills known for class {}",
            character.class
        ))];
    }
    let ids: Vec<i64> = learned.iter().map(|skill| skill.id).collect();
    world.learn(state.character, &ids);
    let list = ids
        .iter()
        .map(|id| {
            Object::new()
                .with("id", Value::Uint(*id as u64))
                .with("active", Value::Bool(true))
        })
        .collect();
    vec![
        Action::Send("S_SKILL_LIST", Object::new().with("skills", Value::Array(list))),
        Action::Say(format!(
            "{} skills granted for {}, saved",
            ids.len(),
            crate::skills::class_name(character.class).unwrap_or("this class")
        )),
    ]
}

fn give(arguments: &[&str], state: &State, world: &World, items: &Items) -> Vec<Action> {
    if arguments.is_empty() {
        return vec![Action::Say(format!("usage: {PREFIX}give name|id [count]"))];
    }
    let count = arguments
        .last()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0 && arguments.len() > 1);
    let wanted = match count {
        Some(_) => arguments[..arguments.len() - 1].join(" "),
        None => arguments.join(" "),
    };
    let amount = count.unwrap_or(1);
    let Some(found) = wanted
        .parse::<i64>()
        .ok()
        .and_then(|id| items.by_id(id))
        .or_else(|| items.find(&wanted))
    else {
        return vec![Action::Warn(format!("no item matches `{wanted}`"))];
    };
    let id = found.id;
    let updated = world.update(state.character, |character| {
        character.carry(id, amount);
    });
    match updated {
        Some(character) => vec![
            Action::Send("S_ITEMLIST", character.inventory_list(state.game_id)),
            Action::Say(format!("{} x{amount}", found.describe())),
        ],
        None => vec![Action::Warn("no character selected".into())],
    }
}

fn bag(character: &Character, items: &Items) -> Vec<Action> {
    if character.carried.is_empty() {
        return vec![Action::Say("the bag is empty".into())];
    }
    character
        .carried
        .iter()
        .take(8)
        .map(|held| {
            Action::Say(format!(
                "slot {} {} x{}",
                held.slot,
                items
                    .by_id(held.item)
                    .map(|item| item.name.clone())
                    .unwrap_or_else(|| held.item.to_string()),
                held.amount
            ))
        })
        .collect()
}

fn item_search(arguments: &[&str], items: &Items) -> Vec<Action> {
    let fragment = arguments.join(" ");
    let found = items.search(&fragment, 8);
    if found.is_empty() {
        return vec![Action::Warn(format!(
            "no item matches `{fragment}` out of {}",
            items.len()
        ))];
    }
    found
        .iter()
        .map(|item| Action::Say(item.describe()))
        .collect()
}

fn gear(character: &Character) -> Vec<Action> {
    if character.equipment.is_empty() {
        return vec![Action::Say("nothing equipped".into())];
    }
    let worn: Vec<String> = character
        .equipment
        .iter()
        .map(|entry| format!("{}:{}", entry.slot, entry.item))
        .collect();
    vec![Action::Say(worn.join("  "))]
}

fn gm(arguments: &[&str], world: &World, character: &Character) -> Vec<Action> {
    let on = !matches!(arguments.first().copied(), Some("off") | Some("0"));
    let updated = world.update(character.id, |value| {
        value.admin_level = i64::from(on);
    });
    match updated {
        Some(_) => vec![Action::Say(format!(
            "gm {}, relog to apply",
            if on { "on" } else { "off" }
        ))],
        None => vec![Action::Say("no character selected".into())],
    }
}

#[cfg(test)]
mod tests {
    use super::{as_markup, strip_markup, NOTICE_COLOUR};

    #[test]
    fn the_clients_font_wrapper_is_removed() {
        assert_eq!(strip_markup("<FONT>!help</FONT>"), "!help");
        assert_eq!(strip_markup("<FONT>!tp 1 2 3</FONT>"), "!tp 1 2 3");
        assert_eq!(strip_markup("plain"), "plain");
    }

    #[test]
    fn entities_are_decoded() {
        assert_eq!(strip_markup("<FONT>a &lt; b &amp; c</FONT>"), "a < b & c");
    }

    #[test]
    fn replies_are_wrapped_and_escaped() {
        assert_eq!(as_markup("hi", "#FFFFFF"), "<FONT color=\"#FFFFFF\">hi</FONT>");
        assert_eq!(
            as_markup("a<b", NOTICE_COLOUR),
            "<FONT color=\"#F0FF89\">a&lt;b</FONT>"
        );
    }
}
