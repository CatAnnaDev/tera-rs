#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};
use tera_hook::{Action, Hooks, Plugin};
use tera_protocol::Value;
use tera_world::{EntityKind, Vec3, World};

const TRACKED: &[&str] = &[
    "S_LOGIN",
    "S_SPAWN_ME",
    "S_LOAD_TOPO",
    "S_PLAYER_STAT_UPDATE",
    "S_SPAWN_NPC",
    "S_SPAWN_USER",
    "S_NPC_LOCATION",
    "S_USER_LOCATION",
    "S_CREATURE_LIFE",
    "S_DESPAWN_NPC",
    "S_DESPAWN_USER",
    "S_ITEMLIST",
    "C_PLAYER_LOCATION",
];

const MOVE_LOG_DISTANCE: f32 = 150.0;

struct State {
    world: World,
    last_move: Vec3,
}

struct Radar {
    state: Arc<Mutex<State>>,
}

impl Default for Radar {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                world: World::new(),
                last_move: Vec3::default(),
            })),
        }
    }
}

fn uint(event_object: &tera_protocol::Object, key: &str) -> u64 {
    event_object.get(key).and_then(Value::as_uint).unwrap_or(0)
}

fn log_event(packet: &str, object: &tera_protocol::Object, state: &mut State) {
    match packet {
        "S_LOGIN" => println!(
            "[radar] login: {} niv {}",
            object.get("name").and_then(Value::as_str).unwrap_or("?"),
            uint(object, "level")
        ),
        "S_LOAD_TOPO" => println!(
            "[radar] zone -> {}",
            object.get("zone").and_then(Value::as_int).unwrap_or(0)
        ),
        "S_SPAWN_NPC" => println!(
            "[radar] npc+ template {} (hz {}), {} pv, {} visibles",
            uint(object, "templateId"),
            uint(object, "huntingZoneId"),
            object.get("maxHp").and_then(Value::as_int).unwrap_or(0),
            state.world.npc_count()
        ),
        "S_SPAWN_USER" => println!(
            "[radar] joueur+ playerId {} ({} autour)",
            uint(object, "playerId"),
            state.world.user_count()
        ),
        "S_DESPAWN_NPC" => println!("[radar] npc- {}", uint(object, "gameId")),
        "S_DESPAWN_USER" => println!("[radar] joueur- {}", uint(object, "gameId")),
        "C_PLAYER_LOCATION" => {
            let position = state.world.player.location;
            if position.distance_to(state.last_move) >= MOVE_LOG_DISTANCE {
                state.last_move = position;
                let target = state
                    .world
                    .nearest_npc()
                    .map(|npc| format!(", npc le plus proche a {:.0} u", npc.location.distance_to(position)))
                    .unwrap_or_default();
                println!(
                    "[radar] moi -> ({:.0}, {:.0}, {:.0}){}",
                    position.x, position.y, position.z, target
                );
            }
        }
        _ => {}
    }
}

impl Plugin for Radar {
    fn name(&self) -> &'static str {
        "radar"
    }

    fn setup(&mut self, hooks: &mut Hooks) {
        println!("[radar] charge, suit le game-state (commandes: /where /mobs /who)");
        for &packet in TRACKED {
            let state = Arc::clone(&self.state);
            hooks.on(packet, 0, 50, move |event| {
                if let Some(object) = event.object() {
                    if let Ok(mut state) = state.lock() {
                        state.world.apply(packet, object);
                        log_event(packet, object, &mut state);
                    }
                }
                Action::Pass
            });
        }

        let state = Arc::clone(&self.state);
        hooks.command("where", move |command| {
            let Ok(state) = state.lock() else {
                return;
            };
            let player = &state.world.player;
            command.reply(&format!(
                "zone {} — vie {}/{} mana {}/{} — pos {:.0}, {:.0}, {:.0}",
                player.zone,
                player.hp,
                player.max_hp,
                player.mp,
                player.max_mp,
                player.location.x,
                player.location.y,
                player.location.z
            ));
        });

        let state = Arc::clone(&self.state);
        hooks.command("mobs", move |command| {
            let Ok(state) = state.lock() else {
                return;
            };
            let world = &state.world;
            match world.nearest_npc() {
                Some(npc) => command.reply(&format!(
                    "{} npc visibles — plus proche template {} (hz {}) a {:.0} u, {} pv max",
                    world.npc_count(),
                    npc.template_id,
                    npc.hunting_zone,
                    npc.location.distance_to(world.player.location),
                    npc.max_hp
                )),
                None => command.reply(&format!("{} npc visibles", world.npc_count())),
            }
        });

        let state = Arc::clone(&self.state);
        hooks.command("who", move |command| {
            let Ok(state) = state.lock() else {
                return;
            };
            let mut players: Vec<u32> = state
                .world
                .entities
                .values()
                .filter(|entity| entity.kind == EntityKind::User)
                .map(|entity| entity.player_id)
                .collect();
            players.sort_unstable();
            command.reply(&format!("{} joueurs autour: {:?}", players.len(), players));
        });
    }
}

tera_hook::export_mod!(Radar::default());
