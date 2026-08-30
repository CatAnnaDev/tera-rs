use crate::db::Database;
use rusqlite::params;
use std::path::Path;
use std::sync::Mutex;
use tera_protocol::value::{Object, Value};

pub const SPAWN_PLACE: &str = "StartZone_ATW_P_x46y52_Center";
pub const SPAWN_ZONE: i64 = 13;
pub const SPAWN_POINT: [f32; 3] = [53760.0, -84480.0, -4435.0];
pub const SPAWN_ANGLE: i64 = -32768;
pub const MAX_CHARACTERS: i64 = 8;
pub const MAX_LEVEL: i64 = 70;

pub fn xp_for_level(level: i64) -> i64 {
    let level = level.clamp(1, MAX_LEVEL);
    1000 * level * level
}

pub fn xp_for_kill(creature_level: i64, player_level: i64) -> i64 {
    let base = 20 + creature_level * 12;
    let gap = (creature_level - player_level).clamp(-10, 10);
    let scaled = base + base * gap / 20;
    scaled.max(1)
}

pub fn xp_before_level(level: i64) -> i64 {
    (1..level.clamp(1, MAX_LEVEL)).map(xp_for_level).sum()
}
pub const WALK_SPEED: i64 = 50;
pub const RUN_SPEED: i64 = 150;

#[derive(Clone, Default)]
pub struct Character {
    pub id: u32,
    pub name: String,
    pub gender: i64,
    pub race: i64,
    pub class: i64,
    pub level: i64,
    pub appearance: u64,
    pub appearance2: u64,
    pub details: Vec<u8>,
    pub shape: Vec<u8>,
    pub position: i64,
    pub walk_speed: i64,
    pub run_speed: i64,
    pub admin_level: i64,
    pub equipment: Vec<Equipped>,
    pub carried: Vec<Carried>,
    pub gold: i64,
    pub xp: i64,
    pub hp: i64,
    pub zone: i64,
    pub location: [f32; 3],
    pub facing: i64,
}

#[derive(Clone)]
pub struct Equipped {
    pub slot: u64,
    pub item: i64,
}

#[derive(Clone)]
pub struct Carried {
    pub slot: u64,
    pub item: i64,
    pub amount: i64,
}

pub const CONTAINER_INVENTORY: i64 = 0;
pub const CONTAINER_EQUIPMENT: i64 = 14;
const INVENTORY_SLOTS: i64 = 40;
const EQUIPMENT_SLOTS: i64 = 24;

pub const SLOT_WEAPON: u64 = 1;
pub const SLOT_BODY: u64 = 3;
pub const SLOT_HAND: u64 = 4;
pub const SLOT_FEET: u64 = 5;
pub const SLOT_UNDERWEAR: u64 = 11;
pub const SLOT_HEAD: u64 = 12;
pub const SLOT_FACE: u64 = 13;

pub fn slot_by_name(name: &str) -> Option<u64> {
    Some(match name {
        "weapon" => SLOT_WEAPON,
        "body" => SLOT_BODY,
        "hand" | "hands" => SLOT_HAND,
        "feet" => SLOT_FEET,
        "underwear" => SLOT_UNDERWEAR,
        "head" => SLOT_HEAD,
        "face" => SLOT_FACE,
        other => other.parse().ok()?,
    })
}

impl Character {
    pub fn template_id(&self) -> i64 {
        10000 + (1 + self.race * 2 + self.gender) * 100 + (1 + self.class)
    }

    pub fn list_entry(&self) -> Object {
        Object::new()
            .with("id", Value::Uint(u64::from(self.id)))
            .with("gender", Value::Int(if self.gender == 1 { 1 } else { 2 }))
            .with("race", Value::Int(self.race))
            .with("class", Value::Int(self.class))
            .with("level", Value::Int(self.level))
            .with("hp", Value::Int(1000))
            .with("mp", Value::Int(1000))
            .with("worldId", Value::Int(1))
            .with("guardId", Value::Int(2))
            .with("sectionId", Value::Int(9))
            .with("lastLogoutTime", Value::Int(0))
            .with("appearance", Value::Uint(self.appearance))
            .with("appearance2", Value::Int(self.appearance2 as i64))
            .with("weapon", Value::Int(self.worn(SLOT_WEAPON)))
            .with("body", Value::Int(self.worn(SLOT_BODY)))
            .with("hand", Value::Int(self.worn(SLOT_HAND)))
            .with("feet", Value::Int(self.worn(SLOT_FEET)))
            .with("underwear", Value::Int(self.worn(SLOT_UNDERWEAR)))
            .with("head", Value::Int(self.worn(SLOT_HEAD)))
            .with("face", Value::Int(self.worn(SLOT_FACE)))
            .with("weaponModel", Value::Int(self.worn(SLOT_WEAPON)))
            .with("bodyModel", Value::Uint(self.worn(SLOT_BODY) as u64))
            .with("handModel", Value::Uint(self.worn(SLOT_HAND) as u64))
            .with("feetModel", Value::Uint(self.worn(SLOT_FEET) as u64))
            .with("position", Value::Int(self.position))
            .with("adminLevel", Value::Int(self.admin_level))
            .with("laurel", Value::Int(-1))
            .with("banRemainSec", Value::Int(0))
            .with("styleHeadScale", Value::Float(1.0))
            .with("styleFaceScale", Value::Float(1.0))
            .with("styleBackScale", Value::Float(1.0))
            .with("customStrings", Value::Array(Vec::new()))
            .with("name", Value::Str(self.name.clone()))
            .with("details", Value::Bytes(self.details.clone()))
            .with("shape", Value::Bytes(self.shape.clone()))
            .with("guildName", Value::Str(String::new()))
    }

    pub fn level_xp(&self) -> i64 {
        self.xp.clamp(0, self.total_level_xp())
    }

    pub fn total_level_xp(&self) -> i64 {
        xp_for_level(self.level)
    }

    pub fn total_xp(&self) -> i64 {
        xp_before_level(self.level) + self.level_xp()
    }

    pub fn gain(&mut self, amount: i64) -> bool {
        self.xp = (self.xp + amount).max(0);
        let mut levelled = false;
        while self.level < MAX_LEVEL && self.xp >= self.total_level_xp() {
            self.xp -= self.total_level_xp();
            self.level += 1;
            levelled = true;
        }
        if self.level >= MAX_LEVEL {
            self.xp = self.xp.min(self.total_level_xp());
        }
        levelled
    }

    pub fn max_hp(&self) -> i64 {
        500 + self.level * 500
    }

    pub fn health(&self) -> i64 {
        if self.hp < 0 {
            self.max_hp()
        } else {
            self.hp.min(self.max_hp())
        }
    }

    pub fn alive(&self) -> bool {
        self.health() > 0
    }

    pub fn wound(&mut self, amount: i64) -> bool {
        let was_alive = self.alive();
        self.hp = (self.health() - amount).max(0);
        was_alive && self.hp == 0
    }

    pub fn revive(&mut self) {
        self.hp = self.max_hp();
    }

    pub fn life_packet(&self, game_id: u64, location: [f32; 3]) -> Object {
        Object::new()
            .with("gameId", Value::Uint(game_id))
            .with("loc", Value::Vec3(location))
            .with("alive", Value::Bool(self.alive()))
            .with("inShuttle", Value::Bool(false))
            .with("resItem", Value::Bool(false))
            .with("resPassive", Value::Bool(false))
    }

    pub fn attack(&self) -> i64 {
        10 + self.level * 6
    }

    pub fn worn(&self, slot: u64) -> i64 {
        self.equipment
            .iter()
            .find(|entry| entry.slot == slot)
            .map(|entry| entry.item)
            .unwrap_or(0)
    }

    pub fn carry(&mut self, item: i64, amount: i64) -> u64 {
        if let Some(stack) = self.carried.iter_mut().find(|entry| entry.item == item) {
            stack.amount += amount;
            return stack.slot;
        }
        let slot = (0..INVENTORY_SLOTS as u64)
            .find(|slot| !self.carried.iter().any(|entry| entry.slot == *slot))
            .unwrap_or(0);
        self.carried.push(Carried { slot, item, amount });
        slot
    }

    pub fn equip(&mut self, slot: u64, item: i64) {
        self.equipment.retain(|entry| entry.slot != slot);
        if item != 0 {
            self.equipment.push(Equipped { slot, item });
        }
    }

    pub fn container_list(&self, game_id: u64, container: i64, requested: bool) -> Object {
        let mut list = if container == CONTAINER_EQUIPMENT {
            self.item_list(game_id)
        } else {
            self.inventory_list(game_id)
        };
        list.set("container", Value::Int(container));
        list.set("requested", Value::Bool(requested));
        list.set("open", Value::Bool(requested));
        list
    }

    pub fn inventory_data(&self, game_id: u64) -> Object {
        Object::new()
            .with("gameId", Value::Uint(game_id))
            .with("itemLevelInventory", Value::Float(0.0))
            .with("itemLevel", Value::Float(0.0))
            .with("tcat", Value::Int(0))
            .with("brokerUseTcat", Value::Int(0))
            .with("vipToken", Value::Int(0))
            .with("boughtInventoryExpansions", Value::Int(0))
    }

    pub fn item_list(&self, game_id: u64) -> Object {
        let items = self
            .equipment
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                Object::new()
                    .with("id", Value::Int(entry.item))
                    .with("dbid", Value::Uint(index as u64 + 1))
                    .with("ownerId", Value::Uint(u64::from(self.id)))
                    .with("slot", Value::Uint(entry.slot))
                    .with("amount", Value::Int(1))
                    .with("dyeSecRemaining", Value::Int(-1))
                    .with("customString", Value::Str(String::new()))
                    .with("crystals", Value::List(Vec::new()))
                    .with("passivitySets", Value::Array(Vec::new()))
                    .with("mergedPassivities", Value::List(Vec::new()))
            })
            .collect();
        Object::new()
            .with("gameId", Value::Uint(game_id))
            .with("container", Value::Int(CONTAINER_EQUIPMENT))
            .with("pocket", Value::Int(0))
            .with("numPockets", Value::Int(1))
            .with("size", Value::Int(EQUIPMENT_SLOTS))
            .with("money", Value::Int(0))
            .with("lootPriority", Value::Int(0))
            .with("open", Value::Bool(false))
            .with("requested", Value::Bool(false))
            .with("first", Value::Bool(true))
            .with("more", Value::Bool(false))
            .with("lastInBatch", Value::Bool(true))
            .with("items", Value::Array(items))
    }

    pub fn experience(&self, gained: i64) -> Object {
        Object::new()
            .with("gainedXp", Value::Int(gained))
            .with("totalXp", Value::Int(self.total_xp()))
            .with("levelXp", Value::Int(self.level_xp()))
            .with("totalLevelXp", Value::Int(self.total_level_xp()))
            .with("monsterGameId", Value::Uint(0))
            .with("xpBonusPercent", Value::Float(0.0))
            .with("dropBonusPercent", Value::Float(0.0))
    }

    pub fn appearance_change(&self, game_id: u64) -> Object {
        Object::new()
            .with("gameId", Value::Uint(game_id))
            .with("weapon", Value::Int(self.worn(SLOT_WEAPON)))
            .with("body", Value::Int(self.worn(SLOT_BODY)))
            .with("hand", Value::Int(self.worn(SLOT_HAND)))
            .with("feet", Value::Int(self.worn(SLOT_FEET)))
            .with("underwear", Value::Int(self.worn(SLOT_UNDERWEAR)))
            .with("head", Value::Int(self.worn(SLOT_HEAD)))
            .with("face", Value::Int(self.worn(SLOT_FACE)))
            .with("weaponModel", Value::Int(self.worn(SLOT_WEAPON)))
            .with("bodyModel", Value::Int(self.worn(SLOT_BODY)))
            .with("handModel", Value::Int(self.worn(SLOT_HAND)))
            .with("feetModel", Value::Int(self.worn(SLOT_FEET)))
            .with("styleHeadScale", Value::Float(1.0))
            .with("styleFaceScale", Value::Float(1.0))
            .with("styleBackScale", Value::Float(1.0))
    }

    pub fn inventory_list(&self, game_id: u64) -> Object {
        Object::new()
            .with("gameId", Value::Uint(game_id))
            .with("container", Value::Int(CONTAINER_INVENTORY))
            .with("pocket", Value::Int(0))
            .with("numPockets", Value::Int(1))
            .with("size", Value::Int(INVENTORY_SLOTS))
            .with("money", Value::Int(self.gold))
            .with("lootPriority", Value::Int(0))
            .with("open", Value::Bool(false))
            .with("requested", Value::Bool(false))
            .with("first", Value::Bool(true))
            .with("more", Value::Bool(false))
            .with("lastInBatch", Value::Bool(true))
            .with(
                "items",
                Value::Array(
                    self.carried
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| self.stack(index, entry))
                        .collect(),
                ),
            )
    }

    fn stack(&self, index: usize, entry: &Carried) -> Object {
        Object::new()
            .with("id", Value::Int(entry.item))
            .with("dbid", Value::Uint(0x1000 + index as u64))
            .with("ownerId", Value::Uint(u64::from(self.id)))
            .with("slot", Value::Uint(entry.slot))
            .with("amount", Value::Int(entry.amount))
            .with("dyeSecRemaining", Value::Int(-1))
            .with("customString", Value::Str(String::new()))
            .with("crystals", Value::List(Vec::new()))
            .with("passivitySets", Value::Array(Vec::new()))
            .with("mergedPassivities", Value::List(Vec::new()))
    }

    pub fn stats(&self) -> Object {
        let max_hp = self.max_hp();
        let max_mp = 200 + self.level * 100;
        Object::new()
            .with("hp", Value::Int(self.health()))
            .with("maxHp", Value::Int(max_hp))
            .with("mp", Value::Int(max_mp))
            .with("maxMp", Value::Int(max_mp))
            .with("power", Value::Int(10))
            .with("endurance", Value::Int(10))
            .with("critRate", Value::Float(1.0))
            .with("critResist", Value::Float(1.0))
            .with("critPower", Value::Float(1.0))
            .with("critPowerPhysical", Value::Float(1.0))
            .with("critPowerMagical", Value::Float(1.0))
            .with("attackSpeed", Value::Int(100))
            .with("runSpeed", Value::Int(self.run_speed))
            .with("walkSpeed", Value::Int(self.walk_speed))
            .with("attackMin", Value::Int(10))
            .with("attackMax", Value::Int(20))
            .with("attackPhysicalMin", Value::Int(10))
            .with("attackPhysicalMax", Value::Int(20))
            .with("level", Value::Uint(self.level as u64))
            .with("conditionLevel", Value::Uint(self.level as u64))
            .with("trueLevel", Value::Int(self.level))
    }

    pub fn login(&self, game_id: u64, server_id: u64) -> Object {
        Object::new()
            .with("templateId", Value::Int(self.template_id()))
            .with("gameId", Value::Uint(game_id))
            .with("serverId", Value::Uint(server_id))
            .with("playerId", Value::Uint(u64::from(self.id)))
            .with("actionMode", Value::Int(0))
            .with("alive", Value::Bool(true))
            .with("status", Value::Int(0))
            .with("walkSpeed", Value::Int(self.walk_speed))
            .with("runSpeed", Value::Int(self.run_speed))
            .with("appearance", Value::Uint(self.appearance))
            .with("visible", Value::Bool(true))
            .with("isSecondCharacter", Value::Bool(false))
            .with("level", Value::Uint(self.level as u64))
            .with("totalXp", Value::Int(self.total_xp()))
            .with("levelXp", Value::Int(self.level_xp()))
            .with("totalLevelXp", Value::Int(self.total_level_xp()))
            .with("serverTime", Value::Int(0))
            .with("isPkServer", Value::Bool(false))
            .with("showFace", Value::Bool(true))
            .with("showStyle", Value::Bool(true))
            .with("appearance2", Value::Int(self.appearance2 as i64))
            .with("scale", Value::Float(1.0))
            .with("weapon", Value::Int(self.worn(SLOT_WEAPON)))
            .with("body", Value::Int(self.worn(SLOT_BODY)))
            .with("hand", Value::Int(self.worn(SLOT_HAND)))
            .with("feet", Value::Int(self.worn(SLOT_FEET)))
            .with("underwear", Value::Int(self.worn(SLOT_UNDERWEAR)))
            .with("head", Value::Int(self.worn(SLOT_HEAD)))
            .with("face", Value::Int(self.worn(SLOT_FACE)))
            .with("weaponModel", Value::Int(self.worn(SLOT_WEAPON)))
            .with("bodyModel", Value::Int(self.worn(SLOT_BODY)))
            .with("handModel", Value::Int(self.worn(SLOT_HAND)))
            .with("feetModel", Value::Int(self.worn(SLOT_FEET)))
            .with("servants", Value::Array(Vec::new()))
            .with("name", Value::Str(self.name.clone()))
            .with("details", Value::Bytes(self.details.clone()))
            .with("shape", Value::Bytes(self.shape.clone()))
    }
}

pub struct World {
    database: Mutex<Database>,
    account: i64,
}

impl Default for World {
    fn default() -> Self {
        let database = Database::memory().expect("in-memory database");
        let account = database.account("local").expect("account");
        Self {
            database: Mutex::new(database),
            account,
        }
    }
}

impl World {
    pub fn open(path: &Path, account: &str) -> anyhow::Result<Self> {
        let database = Database::open(path)?;
        let account = database.account(account)?;
        Ok(Self {
            database: Mutex::new(database),
            account,
        })
    }

    pub fn remember(&self, id: u32, zone: i64, location: [f32; 3], facing: i64) {
        let database = self.database.lock().expect("world");
        let _ = database.connection().execute(
            "UPDATE characters SET zone=?2, x=?3, y=?4, z=?5, w=?6 WHERE id=?1",
            params![id, zone, location[0], location[1], location[2], facing],
        );
    }

    pub fn characters(&self) -> Vec<Character> {
        self.query("SELECT * FROM characters WHERE account = ?1 ORDER BY slot", params![self.account])
    }

    pub fn find(&self, id: u32) -> Option<Character> {
        self.query("SELECT * FROM characters WHERE id = ?1", params![id])
            .into_iter()
            .next()
    }

    pub fn is_full(&self) -> bool {
        let database = self.database.lock().expect("world");
        database
            .character_count(self.account)
            .map(|count| count >= MAX_CHARACTERS)
            .unwrap_or(true)
    }

    pub fn name_taken(&self, name: &str) -> bool {
        let database = self.database.lock().expect("world");
        database.name_taken(name).unwrap_or(true)
    }

    fn query(&self, sql: &str, arguments: impl rusqlite::Params) -> Vec<Character> {
        let database = self.database.lock().expect("world");
        let connection = database.connection();
        let Ok(mut statement) = connection.prepare(sql) else {
            return Vec::new();
        };
        let Ok(rows) = statement.query_map(arguments, read_character) else {
            return Vec::new();
        };
        let mut found: Vec<Character> = rows.flatten().collect();
        for character in &mut found {
            character.equipment = read_equipment(connection, character.id);
            character.carried = read_inventory(connection, character.id);
        }
        found
    }

    pub fn create(&self, request: &Object) -> Character {
        let name = request
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Nameless");
        let database = self.database.lock().expect("world");
        let connection = database.connection();
        let slot: i64 = connection
            .query_row(
                "SELECT count(*) + 1 FROM characters WHERE account = ?1",
                params![self.account],
                |row| row.get(0),
            )
            .unwrap_or(1);
        let empty: Vec<u8> = Vec::new();
        let _ = connection.execute(
            "INSERT INTO characters
             (account, name, gender, race, class, appearance, appearance2, details, shape, slot,
              zone, x, y, z, w)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            params![
                self.account,
                name,
                request.get("gender").and_then(Value::as_int).unwrap_or(0),
                request.get("race").and_then(Value::as_int).unwrap_or(0),
                request.get("class").and_then(Value::as_int).unwrap_or(0),
                request.get("appearance").and_then(Value::as_uint).unwrap_or(0) as i64,
                request.get("appearance2").and_then(Value::as_uint).unwrap_or(0) as i64,
                request.get("details").and_then(Value::as_bytes).unwrap_or(&empty),
                request.get("shape").and_then(Value::as_bytes).unwrap_or(&empty),
                slot,
                SPAWN_ZONE,
                SPAWN_POINT[0],
                SPAWN_POINT[1],
                SPAWN_POINT[2],
                SPAWN_ANGLE,
            ],
        );
        let id = connection.last_insert_rowid() as u32;
        drop(database);
        self.find(id).unwrap_or_default()
    }

    pub fn update(&self, id: u32, change: impl FnOnce(&mut Character)) -> Option<Character> {
        let mut character = self.find(id)?;
        change(&mut character);
        let database = self.database.lock().expect("world");
        let connection = database.connection();
        let _ = connection.execute(
            "UPDATE characters SET level=?2, xp=?3, gold=?4, walk_speed=?5, run_speed=?6,
             admin_level=?7, zone=?8, x=?9, y=?10, z=?11, w=?12, hp=?13 WHERE id=?1",
            params![
                character.id,
                character.level,
                character.xp,
                character.gold,
                character.walk_speed,
                character.run_speed,
                character.admin_level,
                character.zone,
                character.location[0],
                character.location[1],
                character.location[2],
                character.facing,
                character.hp,
            ],
        );
        let _ = connection.execute(
            "DELETE FROM equipment WHERE character = ?1",
            params![character.id],
        );
        let _ = connection.execute(
            "DELETE FROM inventory WHERE character = ?1",
            params![character.id],
        );
        for held in &character.carried {
            let _ = connection.execute(
                "INSERT OR REPLACE INTO inventory (character, slot, item, amount) VALUES (?1,?2,?3,?4)",
                params![character.id, held.slot as i64, held.item, held.amount],
            );
        }
        for worn in &character.equipment {
            let _ = connection.execute(
                "INSERT OR REPLACE INTO equipment (character, slot, item) VALUES (?1,?2,?3)",
                params![character.id, worn.slot as i64, worn.item],
            );
        }
        Some(character)
    }

    pub fn learn(&self, id: u32, skills: &[i64]) -> usize {
        let database = self.database.lock().expect("world");
        let connection = database.connection();
        let mut stored = 0;
        for skill in skills {
            if connection
                .execute(
                    "INSERT OR IGNORE INTO learned_skills (character, skill) VALUES (?1,?2)",
                    params![id, skill],
                )
                .is_ok()
            {
                stored += 1;
            }
        }
        stored
    }

    pub fn learned(&self, id: u32) -> Vec<i64> {
        let database = self.database.lock().expect("world");
        let connection = database.connection();
        let Ok(mut statement) =
            connection.prepare("SELECT skill FROM learned_skills WHERE character = ?1")
        else {
            return Vec::new();
        };
        let Ok(rows) = statement.query_map(params![id], |row| row.get(0)) else {
            return Vec::new();
        };
        rows.flatten().collect()
    }
}

fn read_character(row: &rusqlite::Row<'_>) -> rusqlite::Result<Character> {
    Ok(Character {
        id: row.get::<_, i64>("id")? as u32,
        name: row.get("name")?,
        gender: row.get("gender")?,
        race: row.get("race")?,
        class: row.get("class")?,
        level: row.get("level")?,
        xp: row.get("xp")?,
        gold: row.get("gold")?,
        appearance: row.get::<_, i64>("appearance")? as u64,
        appearance2: row.get::<_, i64>("appearance2")? as u64,
        details: row.get("details")?,
        shape: row.get("shape")?,
        position: row.get("slot")?,
        walk_speed: row.get("walk_speed")?,
        run_speed: row.get("run_speed")?,
        admin_level: row.get("admin_level")?,
        zone: row.get("zone")?,
        location: [
            row.get::<_, f64>("x")? as f32,
            row.get::<_, f64>("y")? as f32,
            row.get::<_, f64>("z")? as f32,
        ],
        facing: row.get("w")?,
        hp: row.get("hp")?,
        equipment: Vec::new(),
        carried: Vec::new(),
    })
}

fn read_inventory(connection: &rusqlite::Connection, id: u32) -> Vec<Carried> {
    let Ok(mut statement) = connection
        .prepare("SELECT slot, item, amount FROM inventory WHERE character = ?1 ORDER BY slot")
    else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map(params![id], |row| {
        Ok(Carried {
            slot: row.get::<_, i64>("slot")? as u64,
            item: row.get("item")?,
            amount: row.get("amount")?,
        })
    }) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

fn read_equipment(connection: &rusqlite::Connection, id: u32) -> Vec<Equipped> {
    let Ok(mut statement) =
        connection.prepare("SELECT slot, item FROM equipment WHERE character = ?1 ORDER BY slot")
    else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map(params![id], |row| {
        Ok(Equipped {
            slot: row.get::<_, i64>("slot")? as u64,
            item: row.get("item")?,
        })
    }) else {
        return Vec::new();
    };
    rows.flatten().collect()
}

pub fn user_list(characters: &[Character]) -> Object {
    Object::new()
        .with("veteran", Value::Bool(false))
        .with("bonusBufSec", Value::Int(0))
        .with("maxCharacters", Value::Int(MAX_CHARACTERS))
        .with("first", Value::Bool(true))
        .with("more", Value::Bool(false))
        .with("leftDelTimeAccountOver", Value::Int(0))
        .with("deletionSectionClassifyLevel", Value::Int(40))
        .with("deleteCharacterExpireHour1", Value::Int(0))
        .with("deleteCharacterExpireHour2", Value::Int(0))
        .with(
            "characters",
            Value::Array(characters.iter().map(Character::list_entry).collect()),
        )
}

impl World {
    pub fn visit_section(&self, character: u32, map: i64, guard: i64, section: i64) -> bool {
        let database = self.database.lock().expect("world");
        database
            .connection()
            .execute(
                "INSERT OR IGNORE INTO visited_sections (character, map, guard, section) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![character, map, guard, section],
            )
            .map(|changed| changed == 1)
            .unwrap_or(false)
    }
}
