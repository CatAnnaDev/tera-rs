pub mod sysmsg;

use std::collections::HashMap;
use tera_protocol::{Object, Value};

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn distance_to(self, other: Vec3) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntityKind {
    Npc,
    User,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub game_id: u64,
    pub kind: EntityKind,
    pub template_id: u32,
    pub hunting_zone: u16,
    pub player_id: u32,
    pub level: i32,
    pub max_hp: i64,
    pub location: Vec3,
    pub heading: i32,
    pub alive: bool,
    pub villager: bool,
}

impl Entity {
    pub fn is_boss_candidate(&self) -> bool {
        self.kind == EntityKind::Npc && self.max_hp >= 1_000_000
    }
}

#[derive(Clone, Default, Debug)]
pub struct Player {
    pub game_id: u64,
    pub player_id: u32,
    pub name: String,
    pub template_id: i32,
    pub level: u16,
    pub hp: i64,
    pub max_hp: i64,
    pub mp: i32,
    pub max_mp: i32,
    pub location: Vec3,
    pub zone: i32,
}

#[derive(Default)]
pub struct World {
    pub player: Player,
    pub entities: HashMap<u64, Entity>,
    pub inventory_slots: usize,
    pub money: i64,
    pub spawns: u64,
    pub despawns: u64,
    pub applied: u64,
}

fn uint(object: &Object, key: &str) -> u64 {
    object.get(key).and_then(Value::as_uint).unwrap_or(0)
}

fn int(object: &Object, key: &str) -> i64 {
    object.get(key).and_then(Value::as_int).unwrap_or(0)
}

fn text(object: &Object, key: &str) -> String {
    object.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn location(object: &Object, key: &str) -> Vec3 {
    match object.get(key) {
        Some(Value::Vec3(components)) => Vec3 {
            x: components[0],
            y: components[1],
            z: components[2],
        },
        _ => Vec3::default(),
    }
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, name: &str, object: &Object) {
        self.applied += 1;
        match name {
            "S_LOGIN" => {
                self.player.game_id = uint(object, "gameId");
                self.player.player_id = uint(object, "playerId") as u32;
                self.player.name = text(object, "name");
                self.player.template_id = int(object, "templateId") as i32;
                self.player.level = uint(object, "level") as u16;
                self.entities.clear();
            }
            "S_SPAWN_ME" => {
                self.player.game_id = uint(object, "gameId");
                self.player.location = location(object, "loc");
            }
            "S_LOAD_TOPO" => {
                self.player.zone = int(object, "zone") as i32;
                self.player.location = location(object, "loc");
                self.entities.clear();
            }
            "S_PLAYER_STAT_UPDATE" => {
                self.player.hp = int(object, "hp");
                self.player.max_hp = int(object, "maxHp");
                self.player.mp = int(object, "mp") as i32;
                self.player.max_mp = int(object, "maxMp") as i32;
                self.player.level = uint(object, "level") as u16;
            }
            "S_SPAWN_NPC" => {
                let game_id = uint(object, "gameId");
                self.entities.insert(
                    game_id,
                    Entity {
                        game_id,
                        kind: EntityKind::Npc,
                        template_id: uint(object, "templateId") as u32,
                        hunting_zone: uint(object, "huntingZoneId") as u16,
                        player_id: 0,
                        level: int(object, "level") as i32,
                        max_hp: int(object, "maxHp"),
                        location: location(object, "loc"),
                        heading: int(object, "w") as i32,
                        alive: int(object, "status") != 4,
                        villager: uint(object, "villager") != 0,
                    },
                );
                self.spawns += 1;
            }
            "S_SPAWN_USER" => {
                let game_id = uint(object, "gameId");
                self.entities.insert(
                    game_id,
                    Entity {
                        game_id,
                        kind: EntityKind::User,
                        template_id: 0,
                        hunting_zone: 0,
                        player_id: uint(object, "playerId") as u32,
                        level: 0,
                        max_hp: 0,
                        location: location(object, "loc"),
                        heading: int(object, "w") as i32,
                        alive: true,
                        villager: false,
                    },
                );
                self.spawns += 1;
            }
            "S_NPC_LOCATION" | "S_USER_LOCATION" => {
                if let Some(entity) = self.entities.get_mut(&uint(object, "gameId")) {
                    entity.location = location(object, "loc");
                    entity.heading = int(object, "w") as i32;
                }
            }
            "S_CREATURE_LIFE" => {
                let alive = uint(object, "alive") != 0;
                if let Some(entity) = self.entities.get_mut(&uint(object, "gameId")) {
                    entity.alive = alive;
                }
            }
            "S_DESPAWN_NPC" | "S_DESPAWN_USER" => {
                if self.entities.remove(&uint(object, "gameId")).is_some() {
                    self.despawns += 1;
                }
            }
            "S_ITEMLIST" => {
                if let Some(Value::Array(items)) = object.get("items") {
                    self.inventory_slots = items.len();
                }
                self.money = int(object, "money");
            }
            _ => {}
        }
    }

    pub fn npc_count(&self) -> usize {
        self.entities
            .values()
            .filter(|entity| entity.kind == EntityKind::Npc)
            .count()
    }

    pub fn user_count(&self) -> usize {
        self.entities
            .values()
            .filter(|entity| entity.kind == EntityKind::User)
            .count()
    }

    pub fn nearest_npc(&self) -> Option<&Entity> {
        self.entities
            .values()
            .filter(|entity| entity.kind == EntityKind::Npc && entity.alive)
            .min_by(|a, b| {
                let da = a.location.distance_to(self.player.location);
                let db = b.location.distance_to(self.player.location);
                da.total_cmp(&db)
            })
    }
}
