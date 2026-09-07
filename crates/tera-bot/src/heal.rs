use std::collections::HashMap;
use std::time::{Duration, Instant};
use tera_protocol::{Object, Value};

const PLAYER_SKILL_FLAG: u64 = 0x1000_0000;
const SKILL_ID_MASK: u64 = 0x0fff_ffff;
const SHOOT_OFFSET: u32 = 10;
const SHOOT_DELAY_MS: u64 = 350;
const MIN_CAST_INTERVAL: Duration = Duration::from_millis(700);
const PRIEST_CLASS: i64 = 6;

const SELF_EMERGENCY: f64 = 35.0;
const SINGLE_EMERGENCY: f64 = 40.0;
const AOE_BAND: f64 = 65.0;
const AOE_MIN_HURT: usize = 2;
const SELF_TOPOFF: f64 = 55.0;

const DODGE_STEP: f32 = 150.0;
const DODGE_THROTTLE: Duration = Duration::from_millis(900);

pub struct HealConfig {
    pub enabled: bool,
    pub hp_limit: f64,
    pub max_distance: f64,
    pub immersion_max_targets: usize,
    pub focus_heal: u32,
    pub healing_immersion: u32,
    pub heal_thyself: u32,
    pub grace_resurrection: u32,
}

impl Default for HealConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hp_limit: 85.0,
            max_distance: 20.0,
            immersion_max_targets: 5,
            focus_heal: 190900,
            healing_immersion: 370200,
            heal_thyself: 181200,
            grace_resurrection: 390100,
        }
    }
}

struct Ally {
    player_id: u32,
    game_id: u64,
    loc: [f32; 3],
    hp_pct: f64,
    dead: bool,
    rising: bool,
}

pub type Outgoing = (u64, &'static str, Object);

pub struct HealBot {
    pub cfg: HealConfig,
    my_player_id: u32,
    my_game_id: u64,
    is_priest: bool,
    my_pos: [f32; 3],
    my_angle: i64,
    my_hp_pct: f64,
    party: Vec<Ally>,
    cooldowns: HashMap<u32, Instant>,
    learned_heals: HashMap<u32, i64>,
    last_cast: Option<Instant>,
    skill_counter: u32,
    moving: bool,
    pub dodge_enabled: bool,
    npcs: HashMap<u64, ([f32; 3], bool)>,
    pending_dodge: Option<[f32; 3]>,
    last_dodge: Option<Instant>,
    dialog_choice: Option<usize>,
    dialog_gate: bool,
}

impl HealBot {
    pub fn new(cfg: HealConfig) -> Self {
        Self {
            cfg,
            my_player_id: 0,
            my_game_id: 0,
            is_priest: false,
            my_pos: [0.0; 3],
            my_angle: 0,
            my_hp_pct: 100.0,
            party: Vec::new(),
            cooldowns: HashMap::new(),
            learned_heals: HashMap::new(),
            last_cast: None,
            skill_counter: 1,
            moving: false,
            dodge_enabled: false,
            npcs: HashMap::new(),
            pending_dodge: None,
            last_dodge: None,
            dialog_choice: None,
            dialog_gate: false,
        }
    }

    pub fn take_dodge(&mut self) -> Option<[f32; 3]> {
        self.pending_dodge.take()
    }

    pub fn set_moving(&mut self, moving: bool) {
        self.moving = moving;
    }

    pub fn ready(&self) -> bool {
        self.is_priest && self.cfg.focus_heal != 0
    }

    pub fn status(&self) -> String {
        let hurt = self.party.iter().filter(|a| a.hp_pct <= self.cfg.hp_limit && !a.dead).count();
        let learned_focus = self.learned_heals.get(&self.cfg.focus_heal).copied().unwrap_or(0);
        let learned_immersion = self.learned_heals.get(&self.cfg.healing_immersion).copied().unwrap_or(0);
        format!(
            "heal {} | esquive {} | priest {} | focus {} (~{}) immersion {} (~{}) | seuil {:.0}% | portee {:.0}m | groupe {} (blesses {}) | ma vie {:.0}%",
            if self.cfg.enabled { "on" } else { "off" },
            if self.dodge_enabled { "on" } else { "off" },
            if self.is_priest { "oui" } else { "non" },
            self.cfg.focus_heal,
            learned_focus,
            self.cfg.healing_immersion,
            learned_immersion,
            self.cfg.hp_limit,
            self.cfg.max_distance,
            self.party.len(),
            hurt,
            self.my_hp_pct,
        )
    }

    pub fn set_self(&mut self, pos: Option<[f32; 3]>, angle: i64) {
        if let Some(pos) = pos {
            self.my_pos = pos;
        }
        self.my_angle = angle;
    }

    pub fn first_target(&self) -> Option<u64> {
        self.party.iter().map(|ally| ally.game_id).find(|&game_id| game_id != 0)
    }

    pub fn ally_loc(&self, game_id: u64) -> Option<[f32; 3]> {
        self.party
            .iter()
            .find(|ally| ally.game_id == game_id && ally.loc != [0.0; 3])
            .map(|ally| ally.loc)
    }

    pub fn force_heal(&mut self, target: u64) -> Vec<Outgoing> {
        let base = self.cfg.focus_heal;
        self.cast_lockon(base, target, &[])
    }

    pub fn observe(&mut self, name: &str, object: &Object, now: Instant) -> Vec<Outgoing> {
        match name {
            "S_LOGIN" => {
                self.my_player_id = uint(object, "playerId") as u32;
                self.my_game_id = uint(object, "gameId");
                let template = int(object, "templateId");
                self.is_priest = (template - 10101).rem_euclid(100) == PRIEST_CLASS;
                Vec::new()
            }
            "S_SPAWN_ME" => {
                if let Some(Value::Vec3(loc)) = object.get("loc") {
                    self.my_pos = *loc;
                }
                self.my_angle = int(object, "w");
                let game_id = uint(object, "gameId");
                if game_id != 0 {
                    self.my_game_id = game_id;
                }
                Vec::new()
            }
            "S_PLAYER_STAT_UPDATE" => {
                let hp = int(object, "hp") as f64;
                let max_hp = int(object, "maxHp") as f64;
                if max_hp > 0.0 {
                    self.my_hp_pct = hp / max_hp * 100.0;
                }
                Vec::new()
            }
            "S_PARTY_MEMBER_LIST" => {
                self.rebuild_party(object);
                Vec::new()
            }
            "S_LEAVE_PARTY" => {
                self.party.clear();
                Vec::new()
            }
            "S_LEAVE_PARTY_MEMBER" | "S_BAN_PARTY_MEMBER" | "S_LOGOUT_PARTY_MEMBER" => {
                let gone = uint(object, "playerId") as u32;
                self.party.retain(|ally| ally.player_id != gone);
                Vec::new()
            }
            "S_SPAWN_USER" => {
                let game_id = uint(object, "gameId");
                let player_id = uint(object, "playerId") as u32;
                let alive = object.get("alive").and_then(Value::as_uint).unwrap_or(1) != 0;
                if let Some(ally) = self.party.iter_mut().find(|ally| ally.player_id == player_id) {
                    ally.game_id = game_id;
                    if let Some(Value::Vec3(loc)) = object.get("loc") {
                        ally.loc = *loc;
                    }
                    ally.dead = !alive;
                    if alive && ally.hp_pct == 0.0 {
                        ally.hp_pct = 100.0;
                    }
                }
                Vec::new()
            }
            "S_USER_LOCATION" => {
                let game_id = uint(object, "gameId");
                if let Some(ally) = self.party.iter_mut().find(|ally| ally.game_id == game_id) {
                    if let Some(Value::Vec3(loc)) = object.get("loc") {
                        ally.loc = *loc;
                    }
                }
                Vec::new()
            }
            "S_PARTY_MEMBER_INTERVAL_POS_UPDATE" => {
                let player_id = uint(object, "playerId") as u32;
                if let Some(ally) = self.party.iter_mut().find(|ally| ally.player_id == player_id) {
                    if let Some(Value::Vec3(loc)) = object.get("loc") {
                        ally.loc = *loc;
                    }
                }
                Vec::new()
            }
            "S_START_COOLTIME_SKILL" => {
                let base = (uint(object, "skill") & SKILL_ID_MASK) as u32;
                let cooldown = Duration::from_millis(int(object, "cooldown").max(0) as u64);
                self.cooldowns.insert(base, now + cooldown);
                Vec::new()
            }
            "S_CANNOT_START_SKILL" => {
                let base = (uint(object, "skill") & SKILL_ID_MASK) as u32;
                self.cooldowns.insert(base, now + Duration::from_millis(2500));
                Vec::new()
            }
            "S_EACH_SKILL_RESULT" => {
                if uint(object, "source") == self.my_game_id && int(object, "type") == 2 {
                    let base = (uint(object, "skill") & SKILL_ID_MASK) as u32;
                    let value = int(object, "value");
                    let entry = self.learned_heals.entry(base).or_insert(0);
                    *entry = (*entry).max(value);
                }
                Vec::new()
            }
            "S_SKILL_LIST" => {
                self.learn_skills(object);
                Vec::new()
            }
            "S_SPAWN_NPC" => {
                let game_id = uint(object, "gameId");
                if game_id != 0 {
                    let loc = match object.get("loc") {
                        Some(Value::Vec3(loc)) => *loc,
                        _ => [0.0; 3],
                    };
                    let name = object.get("npcName").and_then(Value::as_str).unwrap_or("").to_lowercase();
                    let is_portal = name.contains("portal") || name.contains("teleport") || uint(object, "templateId") == 902;
                    self.npcs.insert(game_id, (loc, is_portal));
                }
                Vec::new()
            }
            "S_NPC_LOCATION" => {
                let game_id = uint(object, "gameId");
                if let (Some(entry), Some(Value::Vec3(loc))) = (self.npcs.get_mut(&game_id), object.get("loc")) {
                    entry.0 = *loc;
                }
                Vec::new()
            }
            "S_DIALOG" => {
                return self.drive_dialog(object);
            }
            "S_DESPAWN_NPC" => {
                self.npcs.remove(&uint(object, "gameId"));
                Vec::new()
            }
            "S_ACTION_STAGE" => {
                self.consider_dodge(object, now);
                Vec::new()
            }
            "S_PARTY_MEMBER_CHANGE_HP" => {
                let player_id = uint(object, "playerId") as u32;
                let current = int(object, "currentHp") as f64;
                let maximum = int(object, "maxHp") as f64;
                let pct = if maximum > 0.0 { current / maximum * 100.0 } else { 100.0 };
                if player_id == self.my_player_id {
                    self.my_hp_pct = pct;
                } else if let Some(ally) = self.party.iter_mut().find(|ally| ally.player_id == player_id) {
                    ally.rising = pct > ally.hp_pct;
                    ally.hp_pct = pct;
                    ally.dead = current <= 0.0;
                }
                self.evaluate(now)
            }
            _ => Vec::new(),
        }
    }

    pub fn tick(&mut self, now: Instant) -> Vec<Outgoing> {
        self.evaluate(now)
    }

    pub fn needs_heal(&self, now: Instant) -> bool {
        if !self.cfg.enabled || !self.ready() {
            return false;
        }
        if self.last_cast.is_some_and(|previous| now.duration_since(previous) < MIN_CAST_INTERVAL) {
            return false;
        }
        if self.party.iter().any(|ally| ally.dead && ally.game_id != 0) {
            return true;
        }
        self.party.iter().any(|ally| {
            !ally.dead
                && ally.game_id != 0
                && ally.hp_pct <= self.cfg.hp_limit
                && self.in_range(ally.loc)
                && !(ally.rising && ally.hp_pct > SINGLE_EMERGENCY)
        })
    }

    fn evaluate(&mut self, now: Instant) -> Vec<Outgoing> {
        if !self.cfg.enabled || !self.ready() || self.moving {
            return Vec::new();
        }
        if self.last_cast.is_some_and(|previous| now.duration_since(previous) < MIN_CAST_INTERVAL) {
            return Vec::new();
        }

        if let Some(corpse) = self.party.iter().find(|a| a.dead && a.game_id != 0).map(|a| a.game_id) {
            if self.castable(self.cfg.grace_resurrection, now) {
                let base = self.cfg.grace_resurrection;
                return self.cast_lockon(base, corpse, &[]);
            }
        }

        if self.my_hp_pct <= SELF_EMERGENCY && self.cfg.heal_thyself != 0 && self.castable(self.cfg.heal_thyself, now) {
            let base = self.cfg.heal_thyself;
            return self.cast_self(base);
        }

        let mut hurt: Vec<(u64, [f32; 3], f64)> = self
            .party
            .iter()
            .filter(|a| {
                !a.dead
                    && a.game_id != 0
                    && a.hp_pct <= self.cfg.hp_limit
                    && self.in_range(a.loc)
                    && !(a.rising && a.hp_pct > SINGLE_EMERGENCY)
            })
            .map(|a| (a.game_id, a.loc, a.hp_pct))
            .collect();
        hurt.sort_by(|a, b| a.2.total_cmp(&b.2));

        let lowest = hurt.first().copied();
        let critical = hurt.iter().filter(|(_, _, hp)| *hp <= AOE_BAND).count();

        if critical >= AOE_MIN_HURT {
            if let Some((anchor, _, _)) = lowest {
                let extras: Vec<u64> = hurt
                    .iter()
                    .filter(|(id, _, _)| *id != anchor)
                    .take(self.cfg.immersion_max_targets.saturating_sub(1))
                    .map(|(id, _, _)| *id)
                    .collect();
                if self.cfg.healing_immersion != 0 && self.castable(self.cfg.healing_immersion, now) {
                    let base = self.cfg.healing_immersion;
                    return self.cast_lockon(base, anchor, &extras);
                }
                if self.castable(self.cfg.focus_heal, now) {
                    let base = self.cfg.focus_heal;
                    return self.cast_lockon(base, anchor, &[]);
                }
            }
        }

        if let Some((anchor, _, hp)) = lowest {
            let base = if hp <= SINGLE_EMERGENCY && self.cfg.healing_immersion != 0 && self.castable(self.cfg.healing_immersion, now) {
                self.cfg.healing_immersion
            } else {
                self.cfg.focus_heal
            };
            if self.castable(base, now) {
                return self.cast_lockon(base, anchor, &[]);
            }
            if self.cfg.healing_immersion != 0 && self.castable(self.cfg.healing_immersion, now) {
                let fallback = self.cfg.healing_immersion;
                return self.cast_lockon(fallback, anchor, &[]);
            }
        }

        if self.my_hp_pct <= SELF_TOPOFF && self.cfg.heal_thyself != 0 && self.castable(self.cfg.heal_thyself, now) {
            let base = self.cfg.heal_thyself;
            return self.cast_self(base);
        }

        Vec::new()
    }

    fn rebuild_party(&mut self, object: &Object) {
        let previous = std::mem::take(&mut self.party);
        if let Some(Value::Array(members)) = object.get("members") {
            for member in members {
                let player_id = uint(member, "playerId") as u32;
                if player_id == self.my_player_id {
                    continue;
                }
                let game_id = uint(member, "gameId");
                let old = previous.iter().find(|ally| ally.player_id == player_id);
                self.party.push(Ally {
                    player_id,
                    game_id: if game_id != 0 { game_id } else { old.map_or(0, |ally| ally.game_id) },
                    loc: old.map_or([0.0; 3], |ally| ally.loc),
                    hp_pct: old.map_or(100.0, |ally| ally.hp_pct),
                    dead: old.is_some_and(|ally| ally.dead),
                    rising: false,
                });
            }
        }
    }

    fn learn_skills(&mut self, object: &Object) {
        let Some(Value::Array(skills)) = object.get("skills") else { return };
        let (mut focus, mut immersion, mut thyself, mut grace) = (0u32, 0u32, 0u32, 0u32);
        for skill in skills {
            let base = (uint(skill, "id") & SKILL_ID_MASK) as u32;
            match base {
                190100..=190900 => focus = focus.max(base),
                370100..=370200 => immersion = immersion.max(base),
                180100..=181200 => thyself = thyself.max(base),
                390100 => grace = base,
                _ => {}
            }
        }
        if focus != 0 {
            self.cfg.focus_heal = focus;
        }
        if immersion != 0 {
            self.cfg.healing_immersion = immersion;
        }
        if thyself != 0 {
            self.cfg.heal_thyself = thyself;
        }
        if grace != 0 {
            self.cfg.grace_resurrection = grace;
        }
    }

    fn castable(&self, base: u32, now: Instant) -> bool {
        base != 0 && self.cooldowns.get(&base).map_or(true, |deadline| now >= *deadline)
    }

    fn consider_dodge(&mut self, object: &Object, now: Instant) {
        if !self.dodge_enabled || self.my_game_id == 0 {
            return;
        }
        let source = uint(object, "gameId");
        if !self.npcs.contains_key(&source) || uint(object, "target") != self.my_game_id {
            return;
        }
        if self.last_dodge.is_some_and(|previous| now.duration_since(previous) < DODGE_THROTTLE) {
            return;
        }
        let w_rad = (int(object, "w") as f64) / 65536.0 * std::f64::consts::TAU;
        let facing = [w_rad.cos() as f32, w_rad.sin() as f32];
        let mut perp = [-facing[1], facing[0]];
        if let Some(Value::Vec3(boss_loc)) = object.get("loc") {
            let to_me = [self.my_pos[0] - boss_loc[0], self.my_pos[1] - boss_loc[1]];
            if perp[0] * to_me[0] + perp[1] * to_me[1] < 0.0 {
                perp = [-perp[0], -perp[1]];
            }
        }
        self.pending_dodge = Some([
            self.my_pos[0] + perp[0] * DODGE_STEP,
            self.my_pos[1] + perp[1] * DODGE_STEP,
            self.my_pos[2],
        ]);
        self.last_dodge = Some(now);
    }

    pub fn enter_dungeon(&mut self, choice: Option<usize>) -> Option<Outgoing> {
        let reference = self.first_target().and_then(|id| self.ally_loc(id)).unwrap_or(self.my_pos);
        let target = self.nearest_npc(reference, true)?;
        self.dialog_gate = true;
        self.dialog_choice = choice;
        Some((0, "C_NPC_CONTACT", Object::new().with("gameId", Value::Uint(target))))
    }

    fn nearest_npc(&self, reference: [f32; 3], portal_only: bool) -> Option<u64> {
        self.npcs
            .iter()
            .filter(|(_, (loc, is_portal))| *loc != [0.0; 3] && (!portal_only || *is_portal))
            .min_by(|a, b| dist2(a.1 .0, reference).total_cmp(&dist2(b.1 .0, reference)))
            .map(|(id, _)| *id)
    }

    fn drive_dialog(&mut self, object: &Object) -> Vec<Outgoing> {
        if !self.dialog_gate {
            return Vec::new();
        }
        let session = uint(object, "id");
        let Some(Value::Array(buttons)) = object.get("buttons") else {
            return Vec::new();
        };
        let index = if buttons.len() == 1 {
            Some(0)
        } else {
            self.dialog_gate = false;
            self.dialog_choice.and_then(|choice| choice.checked_sub(1)).filter(|&i| i < buttons.len())
        };
        let Some(index) = index else {
            return Vec::new();
        };
        let button = &buttons[index];
        let reply = Object::new()
            .with("id", Value::Uint(uint(button, "id")))
            .with("index", Value::Uint(uint(button, "index")))
            .with("questReward", Value::Int(session as i64))
            .with("unk", Value::Int(uint(button, "id") as i64));
        vec![(0, "C_DIALOG", reply)]
    }

    fn in_range(&self, loc: [f32; 3]) -> bool {
        let dx = f64::from(loc[0] - self.my_pos[0]);
        let dy = f64::from(loc[1] - self.my_pos[1]);
        let dz = f64::from(loc[2] - self.my_pos[2]);
        (dx * dx + dy * dy + dz * dz).sqrt() / 25.0 <= self.cfg.max_distance
    }

    fn skill_value(&self, base: u32) -> Value {
        Value::Uint(u64::from(base) | PLAYER_SKILL_FLAG)
    }

    fn start_skill(&mut self, base: u32, target: u64) -> Object {
        let counter = self.skill_counter;
        self.skill_counter = self.skill_counter.wrapping_add(1);
        Object::new()
            .with("counter", Value::Uint(u64::from(counter)))
            .with("unk0", Value::Uint(0))
            .with("skill", self.skill_value(base))
            .with("w", Value::Int(self.my_angle))
            .with("loc", Value::Vec3(self.my_pos))
            .with("dest", Value::Vec3([0.0, 0.0, 0.0]))
            .with("unk", Value::Bool(true))
            .with("moving", Value::Bool(false))
            .with("continue", Value::Bool(false))
            .with("target", Value::Uint(target))
            .with("unk2", Value::Bool(false))
    }

    fn lockon(&self, base: u32, target: u64) -> Object {
        Object::new()
            .with("target", Value::Uint(target))
            .with("unk", Value::Int(0))
            .with("skill", self.skill_value(base))
    }

    fn cast_lockon(&mut self, base: u32, primary: u64, extras: &[u64]) -> Vec<Outgoing> {
        if base == 0 || primary == 0 {
            return Vec::new();
        }
        self.last_cast = Some(Instant::now());
        let mut out: Vec<Outgoing> = Vec::with_capacity(extras.len() + 3);
        out.push((0, "C_START_SKILL", self.start_skill(base, primary)));
        out.push((0, "C_CAN_LOCKON_TARGET", self.lockon(base, primary)));
        for &extra in extras {
            out.push((0, "C_CAN_LOCKON_TARGET", self.lockon(base, extra)));
        }
        out.push((SHOOT_DELAY_MS, "C_START_SKILL", self.start_skill(base + SHOOT_OFFSET, primary)));
        out
    }

    fn cast_self(&mut self, base: u32) -> Vec<Outgoing> {
        let target = self.my_game_id;
        if base == 0 {
            return Vec::new();
        }
        self.last_cast = Some(Instant::now());
        vec![
            (0, "C_START_SKILL", self.start_skill(base, target)),
            (SHOOT_DELAY_MS, "C_START_SKILL", self.start_skill(base + SHOOT_OFFSET, target)),
        ]
    }
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f64 {
    let dx = f64::from(a[0] - b[0]);
    let dy = f64::from(a[1] - b[1]);
    dx * dx + dy * dy
}

fn uint(object: &Object, field: &str) -> u64 {
    object.get(field).and_then(Value::as_uint).unwrap_or(0)
}

fn int(object: &Object, field: &str) -> i64 {
    object.get(field).and_then(Value::as_int).unwrap_or(0)
}
