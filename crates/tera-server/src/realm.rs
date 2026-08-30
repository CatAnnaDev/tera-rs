use crate::noise::{in_disc, mixed};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tera_protocol::value::{Object, Value};

pub const DEFAULT_VISIBLE_RANGE: f32 = 3000.0;
pub const NOTICE_RANGE: f32 = 400.0;
const CREATURE_ID_SPACE: u64 = 0x2_0000_0000;
const DROP_ID_SPACE: u64 = 0x3_0000_0000;

#[derive(Clone)]
pub struct Creature {
    pub id: u64,
    pub template: i64,
    pub shape: i64,
    pub hunting_zone: i64,
    pub location: [f32; 3],
    pub facing: i64,
    pub level: i64,
    pub hp: i64,
    pub max_hp: i64,
    pub walk_speed: i64,
    pub run_speed: i64,
    pub aggressive: bool,
    pub anchor: [f32; 3],
    pub roam: f32,
    pub destination: [f32; 3],
    pub restless: Option<Instant>,
}

impl Creature {
    pub fn alive(&self) -> bool {
        self.hp > 0
    }

    pub fn spawn_packet(&self) -> Object {
        Object::new()
            .with("gameId", Value::Uint(self.id))
            .with("loc", Value::Vec3(self.location))
            .with("w", Value::Int(self.facing))
            .with("templateId", Value::Int(self.template))
            .with("huntingZoneId", Value::Int(self.hunting_zone))
            .with("level", Value::Int(self.level))
            .with("maxHp", Value::Int(self.max_hp))
            .with("relation", Value::Int(0))
            .with("walkSpeed", Value::Int(self.walk_speed))
            .with("runSpeed", Value::Int(self.run_speed))
            .with("shapeId", Value::Int(self.shape))
            .with("status", Value::Int(0))
            .with("mode", Value::Int(0))
            .with("remainingEnrageTime", Value::Int(0))
            .with("questInfo", Value::Int(0))
            .with("hpLevel", Value::Int(hp_level(self.hp, self.max_hp)))
            .with("visible", Value::Bool(true))
            .with("villager", Value::Bool(false))
            .with("aggressive", Value::Bool(self.aggressive))
            .with("repairable", Value::Bool(false))
            .with("spawnType", Value::Int(0))
            .with("seats", Value::Array(Vec::new()))
            .with("parts", Value::Array(Vec::new()))
            .with("npcName", Value::Str(String::new()))
    }

    pub fn walk_packet(&self, destination: [f32; 3], facing: i64, speed: i64) -> Object {
        Object::new()
            .with("gameId", Value::Uint(self.id))
            .with("loc", Value::Vec3(self.location))
            .with("w", Value::Int(facing))
            .with("speed", Value::Int(speed))
            .with("dest", Value::Vec3(destination))
            .with("type", Value::Int(0))
    }

    pub fn health_packet(&self) -> Object {
        Object::new()
            .with("gameId", Value::Uint(self.id))
            .with("curHp", Value::Int(self.hp))
            .with("maxHp", Value::Int(self.max_hp))
            .with("enemy", Value::Uint(1))
            .with("edgeD", Value::Int(0))
            .with("edgeF", Value::Float(0.0))
            .with("edgeDuration", Value::Int(0))
            .with("unk", Value::Int(0))
    }

    pub fn change_packet(&self, damage: i64, source: u64) -> Object {
        Object::new()
            .with("curHp", Value::Uint(self.hp.max(0) as u64))
            .with("maxHp", Value::Uint(self.max_hp.max(0) as u64))
            .with("diff", Value::Int(-damage))
            .with("type", Value::Uint(0))
            .with("target", Value::Uint(self.id))
            .with("source", Value::Uint(source))
            .with("crit", Value::Uint(0))
            .with("abnormId", Value::Uint(0))
    }

    pub fn status_packet(&self, target: u64) -> Object {
        Object::new()
            .with("gameId", Value::Uint(self.id))
            .with("enraged", Value::Bool(false))
            .with("remainingEnrageTime", Value::Int(0))
            .with("hpLevel", Value::Int(hp_level(self.hp, self.max_hp)))
            .with("target", Value::Uint(target))
            .with("status", Value::Int(if target == 0 { 0 } else { 2 }))
    }

    pub fn despawn_packet(&self, dead: bool) -> Object {
        Object::new()
            .with("gameId", Value::Uint(self.id))
            .with("loc", Value::Vec3(self.location))
            .with("type", Value::Uint(if dead { 5 } else { 1 }))
            .with("unk", Value::Int(0))
    }
}

fn hp_level(hp: i64, max_hp: i64) -> i64 {
    if max_hp <= 0 {
        return 5;
    }
    let ratio = hp.clamp(0, max_hp) * 5 / max_hp;
    ratio.clamp(0, 5)
}

pub struct Spawn {
    pub template: i64,
    pub shape: i64,
    pub hunting_zone: i64,
    pub location: [f32; 3],
    pub facing: i64,
    pub level: i64,
    pub max_hp: i64,
    pub walk_speed: i64,
    pub run_speed: i64,
    pub aggressive: bool,
    pub anchor: [f32; 3],
    pub roam: f32,
}

impl Default for Spawn {
    fn default() -> Self {
        Self {
            template: 0,
            shape: 0,
            hunting_zone: 0,
            location: [0.0; 3],
            facing: 0,
            level: 1,
            max_hp: 1000,
            walk_speed: 25,
            run_speed: 100,
            aggressive: false,
            anchor: [0.0; 3],
            roam: 0.0,
        }
    }
}

pub const STROLL_PAUSE: Duration = Duration::from_secs(4);
pub const STROLL_VARIETY: Duration = Duration::from_secs(6);
const HOME_TOLERANCE: f32 = 30.0;

impl Realm {
    pub fn stroll(
        &self,
        zone: i64,
        now: Instant,
        stride: f32,
        budget: usize,
        busy: impl Fn(&Creature) -> bool,
    ) -> Vec<(Creature, [f32; 3], i64)> {
        let mut state = self.state.lock().expect("realm");
        state.strolls += 1;
        let turn = state.strolls;
        let Some(creatures) = state.zones.get_mut(&zone) else {
            return Vec::new();
        };
        let mut moved = Vec::new();
        for creature in creatures.iter_mut() {
            if moved.len() >= budget {
                break;
            }
            if !creature.alive() || busy(creature) {
                continue;
            }
            let leash = creature.roam.max(HOME_TOLERANCE);
            let strayed = distance_squared(creature.location, creature.anchor) > leash * leash;
            let due = creature.restless.map(|until| now >= until).unwrap_or(true);
            if strayed {
                creature.destination = creature.anchor;
                creature.restless = Some(now + STROLL_PAUSE);
            } else if due {
                if creature.roam <= 0.0 {
                    creature.restless = Some(now + STROLL_PAUSE);
                    continue;
                }
                let seed = creature.id ^ mixed(turn);
                creature.destination = in_disc(seed, creature.anchor, creature.roam);
                let pause = STROLL_PAUSE
                    + STROLL_VARIETY.mul_f32(crate::noise::unit(mixed(seed)));
                creature.restless = Some(now + pause);
            }
            let gap = distance_squared(creature.location, creature.destination).sqrt();
            if gap <= 1.0 {
                continue;
            }
            let facing = bearing(creature.location, creature.destination);
            let step = step_towards(creature.location, creature.destination, stride.min(gap));
            let before = creature.clone();
            creature.location = step;
            creature.facing = facing;
            moved.push((before, step, facing));
        }
        moved
    }
}

#[derive(Clone)]
pub struct Drop {
    pub id: u64,
    pub zone: i64,
    pub item: i64,
    pub amount: i64,
    pub location: [f32; 3],
    pub source: u64,
}

impl Drop {
    pub fn spawn_packet(&self, owner: &str) -> Object {
        Object::new()
            .with("gameId", Value::Uint(self.id))
            .with("loc", Value::Vec3(self.location))
            .with("item", Value::Int(self.item))
            .with("amount", Value::Int(self.amount))
            .with("expiry", Value::Int(300))
            .with("explode", Value::Bool(true))
            .with("masterwork", Value::Bool(false))
            .with("enchant", Value::Int(0))
            .with("source", Value::Uint(self.source))
            .with("debug", Value::Bool(false))
            .with("autoLoot", Value::Bool(false))
            .with("owners", Value::Array(Vec::new()))
            .with("ownerName", Value::Str(owner.to_string()))
    }

    pub fn despawn_packet(&self) -> Object {
        Object::new().with("gameId", Value::Uint(self.id))
    }
}

#[derive(Default)]
struct State {
    zones: HashMap<i64, Vec<Creature>>,
    drops: Vec<Drop>,
    next: u64,
    strolls: u64,
}

#[derive(Default)]
pub struct Realm {
    state: Mutex<State>,
}

impl Realm {
    pub fn spawn(&self, zone: i64, wanted: &Spawn) -> Creature {
        let mut state = self.state.lock().expect("realm");
        state.next += 1;
        let creature = Creature {
            id: CREATURE_ID_SPACE | state.next,
            template: wanted.template,
            shape: wanted.shape,
            hunting_zone: wanted.hunting_zone,
            location: wanted.location,
            facing: wanted.facing,
            level: wanted.level,
            hp: wanted.max_hp,
            max_hp: wanted.max_hp,
            walk_speed: wanted.walk_speed,
            run_speed: wanted.run_speed,
            aggressive: wanted.aggressive,
            anchor: wanted.anchor,
            roam: wanted.roam,
            destination: wanted.location,
            restless: None,
        };
        state.zones.entry(zone).or_default().push(creature.clone());
        creature
    }

    pub fn occupied(&self, zone: i64, template: i64, at: [f32; 3]) -> bool {
        let state = self.state.lock().expect("realm");
        state
            .zones
            .get(&zone)
            .map(|creatures| {
                creatures.iter().any(|creature| {
                    creature.template == template && distance_squared(creature.anchor, at) < 1.0
                })
            })
            .unwrap_or(false)
    }

    pub fn near(&self, zone: i64, origin: [f32; 3], radius: f32) -> Vec<Creature> {
        let state = self.state.lock().expect("realm");
        let Some(creatures) = state.zones.get(&zone) else {
            return Vec::new();
        };
        let squared = radius * radius;
        creatures
            .iter()
            .filter(|creature| distance_squared(creature.location, origin) <= squared)
            .cloned()
            .collect()
    }

    pub fn find(&self, id: u64) -> Option<(i64, Creature)> {
        let state = self.state.lock().expect("realm");
        state.zones.iter().find_map(|(zone, creatures)| {
            creatures
                .iter()
                .find(|creature| creature.id == id)
                .map(|creature| (*zone, creature.clone()))
        })
    }

    pub fn move_to(&self, id: u64, location: [f32; 3], facing: i64) -> Option<Creature> {
        let mut state = self.state.lock().expect("realm");
        for creatures in state.zones.values_mut() {
            if let Some(creature) = creatures.iter_mut().find(|creature| creature.id == id) {
                creature.location = location;
                creature.facing = facing;
                return Some(creature.clone());
            }
        }
        None
    }

    pub fn damage(&self, id: u64, amount: i64) -> Option<Creature> {
        let mut state = self.state.lock().expect("realm");
        for creatures in state.zones.values_mut() {
            if let Some(creature) = creatures.iter_mut().find(|creature| creature.id == id) {
                creature.hp = (creature.hp - amount).max(0);
                return Some(creature.clone());
            }
        }
        None
    }

    pub fn nearest(&self, zone: i64, origin: [f32; 3], radius: f32) -> Option<Creature> {
        let state = self.state.lock().expect("realm");
        let squared = radius * radius;
        state
            .zones
            .get(&zone)?
            .iter()
            .filter(|creature| creature.alive())
            .map(|creature| (distance_squared(creature.location, origin), creature))
            .filter(|(distance, _)| *distance <= squared)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, creature)| creature.clone())
    }

    pub fn life_packet(&self, creature: &Creature) -> Object {
        Object::new()
            .with("gameId", Value::Uint(creature.id))
            .with("loc", Value::Vec3(creature.location))
            .with("alive", Value::Bool(creature.alive()))
            .with("inShuttle", Value::Bool(false))
            .with("resItem", Value::Bool(false))
            .with("resPassive", Value::Bool(false))
    }

    pub fn remove(&self, id: u64) -> Option<Creature> {
        let mut state = self.state.lock().expect("realm");
        for creatures in state.zones.values_mut() {
            if let Some(index) = creatures.iter().position(|creature| creature.id == id) {
                return Some(creatures.remove(index));
            }
        }
        None
    }

    pub fn drop_item(
        &self,
        zone: i64,
        item: i64,
        amount: i64,
        location: [f32; 3],
        source: u64,
    ) -> Drop {
        let mut state = self.state.lock().expect("realm");
        state.next += 1;
        let dropped = Drop {
            id: DROP_ID_SPACE | state.next,
            zone,
            item,
            amount,
            location,
            source,
        };
        state.drops.push(dropped.clone());
        dropped
    }

    pub fn take_drop(&self, id: u64) -> Option<Drop> {
        let mut state = self.state.lock().expect("realm");
        let index = state.drops.iter().position(|dropped| dropped.id == id)?;
        Some(state.drops.remove(index))
    }

    pub fn drops_near(&self, zone: i64, origin: [f32; 3], radius: f32) -> Vec<Drop> {
        let state = self.state.lock().expect("realm");
        let squared = radius * radius;
        state
            .drops
            .iter()
            .filter(|dropped| {
                dropped.zone == zone && distance_squared(dropped.location, origin) <= squared
            })
            .cloned()
            .collect()
    }

    pub fn set_aggressive(&self, zone: i64, hostile: bool) -> usize {
        let mut state = self.state.lock().expect("realm");
        let Some(creatures) = state.zones.get_mut(&zone) else {
            return 0;
        };
        for creature in creatures.iter_mut() {
            creature.aggressive = hostile;
        }
        creatures.len()
    }

    pub fn clear(&self, zone: i64) -> usize {
        let mut state = self.state.lock().expect("realm");
        state
            .zones
            .remove(&zone)
            .map(|creatures| creatures.len())
            .unwrap_or(0)
    }

    pub fn count(&self, zone: i64) -> usize {
        let state = self.state.lock().expect("realm");
        state.zones.get(&zone).map(Vec::len).unwrap_or(0)
    }

    pub fn total(&self) -> usize {
        let state = self.state.lock().expect("realm");
        state.zones.values().map(Vec::len).sum()
    }
}

pub fn bearing(from: [f32; 3], to: [f32; 3]) -> i64 {
    let angle = (to[1] - from[1]).atan2(to[0] - from[0]);
    let raw = (angle * 32768.0 / std::f32::consts::PI).round() as i64;
    ((raw + 32768).rem_euclid(65536)) - 32768
}

pub fn step_towards(from: [f32; 3], to: [f32; 3], distance: f32) -> [f32; 3] {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let planar = (dx * dx + dy * dy).sqrt();
    if planar <= distance || planar == 0.0 {
        return [to[0], to[1], from[2]];
    }
    let ratio = distance / planar;
    [from[0] + dx * ratio, from[1] + dy * ratio, from[2]]
}

pub fn in_front_of(origin: [f32; 3], facing: i64, distance: f32) -> [f32; 3] {
    let radians = facing as f32 * std::f32::consts::TAU / 65536.0;
    [
        origin[0] + radians.cos() * distance,
        origin[1] + radians.sin() * distance,
        origin[2],
    ]
}

pub fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanderer(anchor: [f32; 3], roam: f32) -> Spawn {
        Spawn {
            template: 7001,
            hunting_zone: 13,
            location: anchor,
            anchor,
            roam,
            walk_speed: 60,
            ..Default::default()
        }
    }

    fn walk_for(realm: &Realm, creature: u64, ticks: u32) -> Vec<[f32; 3]> {
        let mut now = Instant::now();
        let mut path = Vec::new();
        for _ in 0..ticks {
            now += Duration::from_millis(700);
            for (moved, to, _) in realm.stroll(13, now, 60.0, 8, |_| false) {
                if moved.id == creature {
                    path.push(to);
                }
            }
        }
        path
    }

    #[test]
    fn a_wandering_creature_never_leaves_its_territory() {
        let realm = Realm::default();
        let anchor = [1000.0, 2000.0, 50.0];
        let creature = realm.spawn(13, &wanderer(anchor, 400.0));
        for step in walk_for(&realm, creature.id, 400) {
            let leash = distance_squared(step, anchor).sqrt();
            assert!(leash <= 400.0 + 1.0, "it strayed {leash:.0} units from home");
            assert_eq!(step[2], anchor[2]);
        }
    }

    #[test]
    fn a_wandering_creature_actually_covers_ground() {
        let realm = Realm::default();
        let anchor = [0.0, 0.0, 0.0];
        let creature = realm.spawn(13, &wanderer(anchor, 400.0));
        let path = walk_for(&realm, creature.id, 400);
        assert!(path.len() > 100, "it only moved {} times in 400 ticks", path.len());
        let reached = path
            .iter()
            .map(|step| distance_squared(*step, anchor).sqrt())
            .fold(0.0f32, f32::max);
        assert!(reached > 200.0, "it never got further than {reached:.0} units out");
    }

    #[test]
    fn a_creature_with_a_fixed_post_stays_on_it() {
        let realm = Realm::default();
        let anchor = [500.0, -500.0, 12.0];
        let creature = realm.spawn(13, &wanderer(anchor, 0.0));
        assert!(walk_for(&realm, creature.id, 200).is_empty());
    }

    #[test]
    fn a_creature_that_chased_the_player_walks_back_to_its_post() {
        let realm = Realm::default();
        let anchor = [0.0, 0.0, 0.0];
        let creature = realm.spawn(13, &wanderer(anchor, 0.0));
        realm.move_to(creature.id, [3000.0, 0.0, 0.0], 0);
        let path = walk_for(&realm, creature.id, 200);
        let home = path.last().expect("it never headed home");
        assert!(
            distance_squared(*home, anchor).sqrt() <= 1.0,
            "it stopped {:.0} units short of home",
            distance_squared(*home, anchor).sqrt()
        );
    }

    #[test]
    fn a_creature_the_player_is_fighting_is_left_where_it_stands() {
        let realm = Realm::default();
        let creature = realm.spawn(13, &wanderer([0.0, 0.0, 0.0], 400.0));
        let mut now = Instant::now();
        for _ in 0..50 {
            now += Duration::from_millis(700);
            assert!(realm.stroll(13, now, 60.0, 8, |_| true).is_empty());
        }
    }

    fn realm() -> Realm {
        let realm = Realm::default();
        let at = |template: i64, hunting_zone: i64, x: f32| Spawn {
            template,
            hunting_zone,
            location: [x, 0.0, 0.0],
            level: 10,
            ..Spawn::default()
        };
        realm.spawn(13, &at(7001, 13, 0.0));
        realm.spawn(13, &at(7002, 13, 2000.0));
        realm.spawn(13, &at(7003, 13, 9000.0));
        realm.spawn(2000, &at(7004, 2000, 0.0));
        realm
    }

    #[test]
    fn only_creatures_in_range_and_in_the_same_zone_are_visible() {
        let realm = realm();
        let near = realm.near(13, [0.0, 0.0, 0.0], DEFAULT_VISIBLE_RANGE);
        assert_eq!(near.len(), 2, "the one at 9000 is out of a 3000 range");
        assert_eq!(realm.near(2000, [0.0, 0.0, 0.0], DEFAULT_VISIBLE_RANGE).len(), 1);
        assert_eq!(realm.total(), 4);
    }

    #[test]
    fn identifiers_are_unique_and_outside_the_player_space() {
        let realm = realm();
        let all = realm.near(13, [0.0, 0.0, 0.0], f32::MAX);
        let mut ids: Vec<u64> = all.iter().map(|creature| creature.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|id| *id > 0x1_ffff_ffff));
    }

    #[test]
    fn a_creature_can_be_found_and_removed() {
        let realm = realm();
        let first = realm.near(13, [0.0, 0.0, 0.0], 1.0)[0].clone();
        assert!(realm.find(first.id).is_some());
        assert!(realm.remove(first.id).is_some());
        assert!(realm.find(first.id).is_none());
        assert_eq!(realm.count(13), 2);
    }

    #[test]
    fn spawning_in_front_never_changes_the_height() {
        let ground = [100.0, 200.0, -4435.0];
        for facing in [0, 8192, 16384, -32768, 32767] {
            let placed = in_front_of(ground, facing, 150.0);
            assert_eq!(
                placed[2], ground[2],
                "a creature must keep the height of the ground the player stands on"
            );
            let (dx, dy) = (placed[0] - ground[0], placed[1] - ground[1]);
            let planar = (dx * dx + dy * dy).sqrt();
            assert!((planar - 150.0).abs() < 0.01, "distance {planar} should be 150");
        }
    }

    #[test]
    fn facing_zero_points_along_x() {
        let placed = in_front_of([0.0; 3], 0, 100.0);
        assert!((placed[0] - 100.0).abs() < 0.01);
        assert!(placed[1].abs() < 0.01);
    }

    #[test]
    fn health_maps_onto_the_clients_five_steps() {
        assert_eq!(hp_level(1000, 1000), 5);
        assert_eq!(hp_level(500, 1000), 2);
        assert_eq!(hp_level(0, 1000), 0);
        assert_eq!(hp_level(10, 0), 5);
    }
}

#[cfg(test)]
mod occupancy_tests {
    use super::*;

    #[test]
    fn the_same_point_is_never_populated_twice() {
        let realm = Realm::default();
        let spot = Spawn {
            template: 7001,
            location: [1000.0, 2000.0, -30.0],
            anchor: [1000.0, 2000.0, -30.0],
            ..Spawn::default()
        };
        assert!(!realm.occupied(13, 7001, spot.location));
        realm.spawn(13, &spot);
        assert!(realm.occupied(13, 7001, spot.location));
        assert!(!realm.occupied(13, 7002, spot.location), "a different creature may share the spot");
        assert!(!realm.occupied(2000, 7001, spot.location), "another zone is unrelated");
        assert!(!realm.occupied(13, 7001, [1100.0, 2000.0, -30.0]), "a metre away is a different point");
    }

    #[test]
    fn a_creature_that_wandered_off_still_holds_its_spawn_point() {
        let realm = Realm::default();
        let spot = Spawn {
            template: 7001,
            location: [1000.0, 2000.0, -30.0],
            anchor: [1000.0, 2000.0, -30.0],
            roam: 500.0,
            ..Spawn::default()
        };
        let creature = realm.spawn(13, &spot);
        realm.move_to(creature.id, [1400.0, 2300.0, -30.0], 0);
        assert!(
            realm.occupied(13, 7001, spot.location),
            "a second pass would spawn a twin while the first one is out walking"
        );
    }
}

#[cfg(test)]
mod combat_tests {
    use super::*;

    #[test]
    fn damage_lowers_health_and_never_goes_below_zero() {
        let realm = Realm::default();
        let creature = realm.spawn(
            13,
            &Spawn {
                template: 1,
                max_hp: 500,
                ..Spawn::default()
            },
        );
        let hurt = realm.damage(creature.id, 200).expect("hurt");
        assert_eq!(hurt.hp, 300);
        assert!(hurt.alive());
        let dead = realm.damage(creature.id, 9999).expect("dead");
        assert_eq!(dead.hp, 0);
        assert!(!dead.alive());
    }

    #[test]
    fn the_nearest_living_creature_is_the_target() {
        let realm = Realm::default();
        let far = realm.spawn(13, &Spawn { template: 1, location: [400.0, 0.0, 0.0], max_hp: 100, ..Spawn::default() });
        let near = realm.spawn(13, &Spawn { template: 2, location: [100.0, 0.0, 0.0], max_hp: 100, ..Spawn::default() });
        assert_eq!(realm.nearest(13, [0.0; 3], 1000.0).map(|c| c.id), Some(near.id));
        realm.damage(near.id, 100);
        assert_eq!(
            realm.nearest(13, [0.0; 3], 1000.0).map(|c| c.id),
            Some(far.id),
            "a corpse is not a target"
        );
        assert!(realm.nearest(13, [0.0; 3], 50.0).is_none());
    }

    #[test]
    fn health_steps_follow_the_damage() {
        let realm = Realm::default();
        let creature = realm.spawn(13, &Spawn { template: 1, max_hp: 1000, ..Spawn::default() });
        assert_eq!(creature.spawn_packet().get("hpLevel").and_then(Value::as_int), Some(5));
        let hurt = realm.damage(creature.id, 600).expect("hurt");
        assert_eq!(hurt.spawn_packet().get("hpLevel").and_then(Value::as_int), Some(2));
    }
}

#[cfg(test)]
mod movement_tests {
    use super::*;

    #[test]
    fn a_bearing_points_where_the_target_is() {
        assert_eq!(bearing([0.0; 3], [100.0, 0.0, 0.0]), 0);
        assert_eq!(bearing([0.0; 3], [0.0, 100.0, 0.0]), 16384);
        assert_eq!(bearing([0.0; 3], [0.0, -100.0, 0.0]), -16384);
        for target in [[1.0, 1.0, 0.0], [-5.0, 3.0, 0.0], [-2.0, -7.0, 0.0]] {
            let facing = bearing([0.0; 3], target);
            assert!((-32768..=32767).contains(&facing), "{facing} does not fit an i16");
        }
    }

    #[test]
    fn a_step_never_overshoots_and_never_changes_height() {
        let from = [0.0, 0.0, -4435.0];
        let to = [300.0, 400.0, 900.0];
        let stepped = step_towards(from, to, 100.0);
        assert_eq!(stepped[2], from[2], "walking must not lift a creature off the ground");
        let travelled = ((stepped[0] - from[0]).powi(2) + (stepped[1] - from[1]).powi(2)).sqrt();
        assert!((travelled - 100.0).abs() < 0.01);

        let arrived = step_towards(from, to, 10_000.0);
        assert_eq!([arrived[0], arrived[1]], [to[0], to[1]], "a long step lands exactly on target");
        assert_eq!(arrived[2], from[2]);
        assert_eq!(step_towards(from, from, 50.0), from);
    }

    #[test]
    fn a_creature_can_be_walked_and_keeps_its_identity() {
        let realm = Realm::default();
        let creature = realm.spawn(13, &Spawn { template: 1, max_hp: 100, ..Spawn::default() });
        let moved = realm.move_to(creature.id, [50.0, 60.0, -10.0], 4096).expect("moved");
        assert_eq!(moved.id, creature.id);
        assert_eq!(moved.location, [50.0, 60.0, -10.0]);
        assert_eq!(moved.facing, 4096);
        assert_eq!(realm.near(13, [50.0, 60.0, -10.0], 1.0).len(), 1);
    }
}
