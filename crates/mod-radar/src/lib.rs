#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};
use tera_hook::{Action, Hooks, Plugin};
use tera_world::{EntityKind, World};

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
];

struct Radar {
    world: Arc<Mutex<World>>,
}

impl Default for Radar {
    fn default() -> Self {
        Self {
            world: Arc::new(Mutex::new(World::new())),
        }
    }
}

impl Plugin for Radar {
    fn name(&self) -> &'static str {
        "radar"
    }

    fn setup(&mut self, hooks: &mut Hooks) {
        for &packet in TRACKED {
            let world = Arc::clone(&self.world);
            hooks.on(packet, 0, 50, move |event| {
                if let Some(object) = event.object() {
                    if let Ok(mut world) = world.lock() {
                        world.apply(packet, object);
                    }
                }
                Action::Pass
            });
        }

        let world = Arc::clone(&self.world);
        hooks.command("where", move |command| {
            let Ok(world) = world.lock() else {
                return;
            };
            let player = &world.player;
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

        let world = Arc::clone(&self.world);
        hooks.command("mobs", move |command| {
            let Ok(world) = world.lock() else {
                return;
            };
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

        let world = Arc::clone(&self.world);
        hooks.command("who", move |command| {
            let Ok(world) = world.lock() else {
                return;
            };
            let mut players: Vec<u32> = world
                .entities
                .values()
                .filter(|entity| entity.kind == EntityKind::User)
                .map(|entity| entity.player_id)
                .collect();
            players.sort_unstable();
            command.reply(&format!(
                "{} joueurs autour: {:?}",
                players.len(),
                players
            ));
        });
    }
}

tera_hook::export_mod!(Radar::default());
