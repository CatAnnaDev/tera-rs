use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};
use tera_protocol::{Object, Value};

const PLAYER_SKILL_FLAG: u64 = 0x1000_0000;
const SKILL_ID_MASK: u64 = 0x0fff_ffff;
const SHOOT_OFFSET: u32 = 10;
const SHOOT_DELAY_MS: u64 = 350;
const LOCKON_CHANNEL_MS: u64 = 1400;
const LOCKON_RELEASE_MS: u64 = 250;
const MIN_CAST_INTERVAL: Duration = Duration::from_millis(700);
const PRIEST_CLASS: i64 = 6;

const SELF_EMERGENCY: f64 = 35.0;
const SINGLE_EMERGENCY: f64 = 40.0;
const AOE_BAND: f64 = 65.0;
const AOE_MIN_HURT: usize = 2;
const SELF_TOPOFF: f64 = 55.0;
const POCKET_TOPOFF: f64 = 85.0;

const DODGE_STEP: f32 = 150.0;
const DODGE_THROTTLE: Duration = Duration::from_millis(900);

const UNITS_PER_METER: f64 = 25.0;
const ATTACK_RANGE_M: f64 = 18.0;
const ENGAGE_RANGE_M: f64 = 26.0;
const AOE_CLUSTER_M: f64 = 6.0;
const AOE_CLUSTER_MIN: usize = 2;
const STANDOFF_M: f32 = 15.0;
const MAX_ROAM_M: f64 = 70.0;
const ATTACK_SELF_FLOOR: f64 = 60.0;

const EDICT_ABNORM: [u32; 4] = [805800, 805801, 805802, 805803];
const DIVINE_CHARGE_FULL: u32 = 805713;
const CHARGE_TIMEOUT: Duration = Duration::from_millis(3200);

const COMBO_STEP_MS: u64 = 450;
const TRIPLE_NEMESIS_STAGES: u32 = 3;
const PROVISIONAL_CD: Duration = Duration::from_millis(1500);
const ZENOBIA_GLYPH_OFFSET: u32 = 20;
const SHOCKING_GLYPH_OFFSET: u32 = 20;
const HOLY_BURST_GLYPH_OFFSET: u32 = 30;

const SELF_PANIC: f64 = 22.0;
const MANA_FLOOR: f64 = 30.0;
const HOT_BAND: f64 = 95.0;

pub struct HealConfig {
    pub enabled: bool,
    pub hp_limit: f64,
    pub max_distance: f64,
    pub immersion_max_targets: usize,
    pub focus_heal: u32,
    pub healing_immersion: u32,
    pub heal_thyself: u32,
    pub grace_resurrection: u32,
    pub attack_enabled: bool,
    pub attack_single: u32,
    pub attack_aoe: u32,
    pub rotation_enabled: bool,
    pub energy_stars: u32,
    pub divine_charge: u32,
    pub triple_nemesis: u32,
    pub edict: u32,
    pub zenobia: u32,
    pub holy_burst: u32,
    pub maintain_buffs: bool,
    pub restorative_burst: u32,
    pub healing_circle: u32,
    pub regen_circle: u32,
    pub arise: u32,
    pub resurrect: u32,
    pub purifying_circle: u32,
    pub kaia_shield: u32,
    pub guardian_sanctuary: u32,
    pub divine_intervention: u32,
    pub mana_charge: u32,
    pub mana_infusion: u32,
    pub bless_shakan: u32,
    pub bless_seren: u32,
    pub bless_balder: u32,
    pub divine_prayer: u32,
    pub plague: u32,
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
            attack_enabled: false,
            attack_single: 350400,
            attack_aoe: 161120,
            rotation_enabled: false,
            energy_stars: 350400,
            divine_charge: 280200,
            triple_nemesis: 290900,
            edict: 430100,
            zenobia: 401020,
            holy_burst: 420130,
            maintain_buffs: true,
            restorative_burst: 340200,
            healing_circle: 30700,
            regen_circle: 20900,
            arise: 60200,
            resurrect: 120100,
            purifying_circle: 100100,
            kaia_shield: 220500,
            guardian_sanctuary: 310200,
            divine_intervention: 410200,
            mana_charge: 280100,
            mana_infusion: 80100,
            bless_shakan: 50300,
            bless_seren: 200100,
            bless_balder: 230100,
            divine_prayer: 320200,
            plague: 300800,
        }
    }
}

struct Npc {
    loc: [f32; 3],
    portal: bool,
    hostile: bool,
    alive: bool,
}

struct Ally {
    player_id: u32,
    game_id: u64,
    name: String,
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
    my_mp_pct: f64,
    run_speed: f32,
    boss_enraged: bool,
    priority_target: Option<u64>,
    party: Vec<Ally>,
    cooldowns: HashMap<u32, Instant>,
    learned_heals: HashMap<u32, i64>,
    last_cast: Option<Instant>,
    skill_counter: u32,
    moving: bool,
    pub dodge_enabled: bool,
    npcs: HashMap<u64, Npc>,
    pending_dodge: Option<[f32; 3]>,
    last_dodge: Option<Instant>,
    dialog_choice: Option<usize>,
    dialog_gate: bool,
    abnorms: HashMap<u32, Instant>,
    boss: Option<u64>,
    boss_alive: bool,
    charging: bool,
    charge_started: Option<Instant>,
    trace: Vec<String>,
    pub trace_enabled: bool,
    skill_ids: Vec<u32>,
    crests: Vec<(i32, bool)>,
    crest_points: (i32, i32),
    recent: VecDeque<String>,
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
            my_mp_pct: 100.0,
            run_speed: 130.0,
            boss_enraged: false,
            priority_target: None,
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
            abnorms: HashMap::new(),
            boss: None,
            boss_alive: false,
            charging: false,
            charge_started: None,
            trace: Vec::new(),
            trace_enabled: false,
            skill_ids: Vec::new(),
            crests: Vec::new(),
            crest_points: (0, 0),
            recent: VecDeque::new(),
        }
    }

    fn skill_name(&self, base: u32) -> &'static str {
        match base {
            b if b == self.cfg.divine_charge => "Divine Charge",
            b if b == self.cfg.energy_stars => "Energy Stars",
            b if b == self.cfg.triple_nemesis => "Triple Nemesis",
            b if b == self.cfg.edict => "Edict of Judgment",
            b if b == self.cfg.zenobia => "Zenobia's Vortex",
            b if b == self.cfg.holy_burst => "Holy Burst",
            b if b == self.cfg.attack_aoe => "Shocking Implosion",
            b if b == self.cfg.focus_heal => "Focus Heal",
            b if b == self.cfg.healing_immersion => "Healing Immersion",
            b if b == self.cfg.heal_thyself => "Heal Thyself",
            b if b == self.cfg.grace_resurrection => "Grace of Resurrection",
            _ => "?",
        }
    }

    pub fn drain_trace(&mut self) -> Vec<String> {
        std::mem::take(&mut self.trace)
    }

    fn log_trace(&mut self, line: String) {
        if self.trace.len() >= 200 {
            self.trace.remove(0);
        }
        if self.recent.len() >= 15 {
            self.recent.pop_front();
        }
        self.recent.push_back(line.clone());
        self.trace.push(line);
    }

    pub fn set_skill(&mut self, name: &str, id: u32) -> String {
        let slot: Option<(&str, &mut u32)> = match name.to_lowercase().as_str() {
            "zenobia" | "vortex" => Some(("Zenobia", &mut self.cfg.zenobia)),
            "shocking" | "aoe" => Some(("Shocking", &mut self.cfg.attack_aoe)),
            "holy" | "holyburst" => Some(("Holy Burst", &mut self.cfg.holy_burst)),
            "nemesis" | "triple" | "tn" => Some(("Triple Nemesis", &mut self.cfg.triple_nemesis)),
            "energy" | "stars" => Some(("Energy Stars", &mut self.cfg.energy_stars)),
            "edict" => Some(("Edict", &mut self.cfg.edict)),
            "charge" | "divinecharge" => Some(("Divine Charge", &mut self.cfg.divine_charge)),
            "single" => Some(("Attack single", &mut self.cfg.attack_single)),
            "plague" => Some(("Plague", &mut self.cfg.plague)),
            _ => None,
        };
        match slot {
            Some((label, field)) => {
                *field = id;
                format!("{label} = {id}")
            }
            None => format!("skill inconnu: {name} (zenobia/shocking/holy/nemesis/energy/edict/charge/single/plague)"),
        }
    }

    pub fn trace_tail(&self) -> String {
        if self.recent.is_empty() {
            "aucun cast recent (rota inactive ou pas de cible ennemie ?)".to_string()
        } else {
            self.recent.iter().cloned().collect::<Vec<_>>().join(" | ")
        }
    }

    pub fn force_opener(&mut self) -> Vec<Outgoing> {
        let target = self.boss_target().unwrap_or(self.my_game_id);
        let mut out: Vec<Outgoing> = Vec::new();

        out.push((0, "C_START_SKILL", self.start_skill(self.cfg.divine_charge, target)));
        out.push((2450, "C_PRESS_SKILL", self.press_release(self.cfg.divine_charge)));
        self.trace.push("test -> Divine Charge (charge + relache)".to_string());

        for (at, base) in [
            (3310, self.cfg.energy_stars),
            (4050, self.cfg.triple_nemesis),
            (4990, self.cfg.edict),
            (5700, self.cfg.zenobia),
            (6470, self.cfg.holy_burst),
            (7240, self.cfg.attack_aoe),
        ] {
            if self.uses_release(base) {
                out.push((at, "C_START_SKILL", self.start_skill(base, target)));
                out.push((at, "C_CAN_LOCKON_TARGET", self.lockon(base, target)));
                out.push((at + SHOOT_DELAY_MS, "C_START_SKILL", self.start_skill(base + SHOOT_OFFSET, target)));
            } else if base == self.cfg.triple_nemesis {
                for stage in 0..TRIPLE_NEMESIS_STAGES {
                    out.push((at + u64::from(stage) * COMBO_STEP_MS, "C_START_SKILL", self.start_skill(base + stage, target)));
                }
            } else {
                out.push((at, "C_START_SKILL", self.start_skill(base, target)));
            }
            self.trace.push(format!("test -> {}", self.skill_name(base)));
        }
        out
    }

    pub fn rotation_report(&self) -> String {
        let now = Instant::now();
        let boss = match (self.boss, self.boss_alive) {
            (Some(id), true) => format!("boss {id}"),
            _ => self.nearest_hostile().map_or_else(|| "aucune cible".to_string(), |(id, _, d)| format!("mob {id} ({d:.0}m)")),
        };
        let edict = if self.edict_active(now) { "EDICT actif" } else { "edict pret" };
        let order = [
            self.cfg.divine_charge,
            self.cfg.energy_stars,
            self.cfg.triple_nemesis,
            self.cfg.edict,
            self.cfg.zenobia,
            self.cfg.holy_burst,
            self.cfg.attack_aoe,
        ];
        let skills: Vec<String> = order
            .iter()
            .map(|&base| {
                let state = match self.cooldowns.get(&base) {
                    Some(deadline) if *deadline > now => format!("CD {:.1}s", (*deadline - now).as_secs_f64()),
                    _ => "pret".to_string(),
                };
                format!("{} {}", self.skill_name(base), state)
            })
            .collect();
        format!("rota [{}] {} | charge {} | {}", edict, boss, if self.charging { "en cours" } else { "non" }, skills.join(" | "))
    }

    pub fn take_dodge(&mut self) -> Option<[f32; 3]> {
        self.pending_dodge.take()
    }

    pub fn channeling(&self) -> bool {
        self.charging
    }

    pub fn run_speed(&self) -> f32 {
        self.run_speed
    }

    pub fn force_purify(&mut self) -> Vec<Outgoing> {
        self.cast_point(self.cfg.purifying_circle)
    }

    pub fn force_infuse(&mut self) -> Vec<Outgoing> {
        let target = self
            .party
            .iter()
            .filter(|a| !a.dead && a.game_id != 0 && self.in_range(a.loc))
            .map(|a| a.game_id)
            .next()
            .unwrap_or(self.my_game_id);
        self.cast_targeted(self.cfg.mana_infusion, target, &[])
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
        let hostiles = self.npcs.values().filter(|n| n.hostile && n.alive).count();
        let rota = if self.rotation_active() {
            if self.edict_active(Instant::now()) { "burst(edict)" } else { "on" }
        } else if self.cfg.rotation_enabled {
            "armee"
        } else {
            "off"
        };
        let boss = if self.boss_alive { "boss" } else { "-" };
        format!(
            "heal {} | rota {} [{}] | dps {} | esquive {} | priest {} | focus {} (~{}) immersion {} (~{}) | seuil {:.0}% | portee {:.0}m | groupe {} (blesses {}) | mobs {} | ma vie {:.0}%",
            if self.cfg.enabled { "on" } else { "off" },
            rota,
            boss,
            if self.cfg.attack_enabled { "on" } else { "off" },
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
            hostiles,
            self.my_hp_pct,
        ) + &format!(" | mp {:.0}%{}", self.my_mp_pct, if self.boss_enraged { " | ENRAGE" } else { "" })
    }

    pub fn skill_report(&self) -> String {
        let kit = [
            ("Focus Heal", self.cfg.focus_heal),
            ("Healing Immersion", self.cfg.healing_immersion),
            ("Restorative Burst", self.cfg.restorative_burst),
            ("Healing Circle", self.cfg.healing_circle),
            ("Regen Circle", self.cfg.regen_circle),
            ("Heal Thyself", self.cfg.heal_thyself),
            ("Arise", self.cfg.arise),
            ("Resurrect", self.cfg.resurrect),
            ("Grace of Res", self.cfg.grace_resurrection),
            ("Purifying Circle", self.cfg.purifying_circle),
            ("Mana Charge", self.cfg.mana_charge),
            ("Mana Infusion", self.cfg.mana_infusion),
            ("Shakan", self.cfg.bless_shakan),
            ("Seren", self.cfg.bless_seren),
            ("Balder", self.cfg.bless_balder),
            ("Divine Prayer", self.cfg.divine_prayer),
            ("Kaia Shield", self.cfg.kaia_shield),
            ("Guardian Sanctuary", self.cfg.guardian_sanctuary),
            ("Divine Intervention", self.cfg.divine_intervention),
            ("Energy Stars", self.cfg.energy_stars),
            ("Divine Charge", self.cfg.divine_charge),
            ("Triple Nemesis", self.cfg.triple_nemesis),
            ("Plague", self.cfg.plague),
            ("Edict", self.cfg.edict),
            ("Zenobia", self.cfg.zenobia),
            ("Holy Burst", self.cfg.holy_burst),
            ("Shocking Implosion", self.cfg.attack_aoe),
        ];
        let body: Vec<String> = kit
            .iter()
            .map(|(name, id)| {
                let learned = self.skill_ids.contains(id);
                format!("{name} {id}{}", if learned { "" } else { "?" })
            })
            .collect();
        format!("skills ({} appris) : {}", self.skill_ids.len(), body.join(" | "))
    }

    pub fn glyph_report(&self) -> String {
        if self.crests.is_empty() {
            return "aucun glyphe recu (S_CREST_INFO pas encore arrive)".to_string();
        }
        let active: Vec<String> = self
            .crests
            .iter()
            .filter(|(_, on)| *on)
            .map(|(id, _)| id.to_string())
            .collect();
        format!(
            "glyphes : {}/{} points | {} actifs : {}",
            self.crest_points.1,
            self.crest_points.0,
            active.len(),
            active.join(" ")
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

    pub fn party_view(&self) -> Vec<(String, f64, bool)> {
        self.party
            .iter()
            .map(|ally| {
                let label = if ally.name.is_empty() {
                    format!("#{}", ally.player_id)
                } else {
                    ally.name.clone()
                };
                (label, ally.hp_pct, ally.dead)
            })
            .collect()
    }

    pub fn set_priority(&mut self, target: Option<u64>) {
        self.priority_target = target;
    }

    pub fn cooldown_view(&self, now: Instant) -> Vec<(String, f32)> {
        let mut out: Vec<(String, f32)> = self
            .cooldowns
            .iter()
            .filter_map(|(&base, &deadline)| {
                if deadline <= now {
                    return None;
                }
                let name = self.skill_name(base);
                if name == "?" {
                    return None;
                }
                Some((name.to_string(), (deadline - now).as_secs_f32()))
            })
            .collect();
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        out
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
                let mp = int(object, "mp") as f64;
                let max_mp = int(object, "maxMp") as f64;
                if max_mp > 0.0 {
                    self.my_mp_pct = mp / max_mp * 100.0;
                }
                let speed = (int(object, "runSpeed") + int(object, "runSpeedBonus")) as f32;
                if speed > 0.0 {
                    self.run_speed = speed;
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
                self.log_trace(format!("REFUS serveur: {} ({base})", self.skill_name(base)));
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
            "S_CREST_INFO" => {
                self.crest_points = (int(object, "points") as i32, int(object, "pointsActive") as i32);
                if let Some(Value::Array(crests)) = object.get("crests") {
                    self.crests = crests
                        .iter()
                        .map(|crest| (int(crest, "id") as i32, uint(crest, "enable") != 0))
                        .collect();
                }
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
                    let villager = object.get("villager").and_then(Value::as_uint).unwrap_or(0) != 0;
                    let status = int(object, "status");
                    self.npcs.insert(game_id, Npc {
                        loc,
                        portal: is_portal,
                        hostile: !villager && !is_portal,
                        alive: status != 4,
                    });
                }
                Vec::new()
            }
            "S_NPC_LOCATION" => {
                let game_id = uint(object, "gameId");
                if let (Some(entry), Some(Value::Vec3(loc))) = (self.npcs.get_mut(&game_id), object.get("loc")) {
                    entry.loc = *loc;
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
            "S_CREATURE_LIFE" => {
                let game_id = uint(object, "gameId");
                let alive = object.get("alive").and_then(Value::as_uint).unwrap_or(1) != 0;
                if let Some(entry) = self.npcs.get_mut(&game_id) {
                    entry.alive = alive;
                    if let Some(Value::Vec3(loc)) = object.get("loc") {
                        entry.loc = *loc;
                    }
                }
                Vec::new()
            }
            "S_NPC_STATUS" => {
                let game_id = uint(object, "gameId");
                if let Some(entry) = self.npcs.get_mut(&game_id) {
                    entry.alive = int(object, "status") != 4;
                }
                if self.boss == Some(game_id) {
                    self.boss_enraged = uint(object, "enraged") != 0;
                }
                Vec::new()
            }
            "S_BOSS_GAGE_INFO" => {
                let id = uint(object, "id");
                let cur_hp = int(object, "curHp");
                if cur_hp > 0 {
                    self.boss = Some(id);
                    self.boss_alive = true;
                } else if self.boss == Some(id) {
                    self.boss_alive = false;
                    self.boss_enraged = false;
                }
                Vec::new()
            }
            "S_ABNORMALITY_BEGIN" | "S_ABNORMALITY_REFRESH" => {
                if uint(object, "target") == self.my_game_id {
                    let id = uint(object, "id") as u32;
                    let duration = int(object, "duration").clamp(0, 600_000) as u64;
                    self.abnorms.insert(id, now + Duration::from_millis(duration));
                    if id == DIVINE_CHARGE_FULL && self.charging {
                        return self.release_charge();
                    }
                }
                Vec::new()
            }
            "S_ABNORMALITY_END" => {
                if uint(object, "target") == self.my_game_id {
                    self.abnorms.remove(&(uint(object, "id") as u32));
                }
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
        if !self.cfg.enabled || !self.ready() {
            return Vec::new();
        }
        if self.charging && (self.party_needs_attention() || self.my_hp_pct <= SELF_EMERGENCY) {
            self.charging = false;
            self.charge_started = None;
        }
        if self.last_cast.is_some_and(|previous| now.duration_since(previous) < MIN_CAST_INTERVAL) {
            return Vec::new();
        }

        if self.my_hp_pct <= SELF_PANIC {
            for base in [self.cfg.divine_intervention, self.cfg.kaia_shield] {
                if self.castable(base, now) {
                    return self.cast_targeted(base, self.my_game_id, &[]);
                }
            }
            if self.castable(self.cfg.heal_thyself, now) {
                return self.cast_point(self.cfg.heal_thyself);
            }
        }

        if let Some(corpse) = self.party.iter().find(|a| a.dead && a.game_id != 0).map(|a| a.game_id) {
            for base in [self.cfg.arise, self.cfg.grace_resurrection, self.cfg.resurrect] {
                if self.castable(base, now) {
                    return self.cast_targeted(base, corpse, &[]);
                }
            }
        }

        if self.my_hp_pct <= SELF_EMERGENCY && self.castable(self.cfg.heal_thyself, now) {
            return self.cast_point(self.cfg.heal_thyself);
        }

        if let Some(priority) = self.priority_target {
            let hit = self
                .party
                .iter()
                .find(|a| a.game_id == priority && !a.dead && a.hp_pct <= POCKET_TOPOFF && self.in_range(a.loc))
                .map(|a| a.game_id);
            if let Some(gid) = hit {
                let base = self.cfg.focus_heal;
                if base != 0 && self.castable(base, now) {
                    return self.cast_targeted(base, gid, &[]);
                }
            }
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
                let clustered = hurt
                    .iter()
                    .filter(|(_, loc, hp)| *hp <= AOE_BAND && dist2(*loc, self.my_pos).sqrt() / UNITS_PER_METER <= self.cfg.max_distance)
                    .count();
                if clustered >= AOE_MIN_HURT && self.castable(self.cfg.restorative_burst, now) {
                    return self.cast_point(self.cfg.restorative_burst);
                }
                let extras: Vec<u64> = hurt
                    .iter()
                    .filter(|(id, _, _)| *id != anchor)
                    .take(self.cfg.immersion_max_targets.saturating_sub(1))
                    .map(|(id, _, _)| *id)
                    .collect();
                if self.cfg.healing_immersion != 0 && self.castable(self.cfg.healing_immersion, now) {
                    let base = self.cfg.healing_immersion;
                    return self.cast_targeted(base, anchor, &extras);
                }
                if self.castable(self.cfg.focus_heal, now) {
                    let base = self.cfg.focus_heal;
                    return self.cast_targeted(base, anchor, &[]);
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
                return self.cast_targeted(base, anchor, &[]);
            }
            if self.cfg.healing_immersion != 0 && self.castable(self.cfg.healing_immersion, now) {
                let fallback = self.cfg.healing_immersion;
                return self.cast_targeted(fallback, anchor, &[]);
            }
        }

        let group_hurt = self
            .party
            .iter()
            .any(|a| !a.dead && a.game_id != 0 && a.hp_pct <= HOT_BAND && self.in_range(a.loc));
        if group_hurt {
            for base in [self.cfg.healing_circle, self.cfg.regen_circle] {
                if self.castable(base, now) {
                    return self.cast_point(base);
                }
            }
        }

        if self.my_hp_pct <= SELF_TOPOFF && self.castable(self.cfg.heal_thyself, now) {
            return self.cast_point(self.cfg.heal_thyself);
        }

        if self.boss_enraged {
            for base in [self.cfg.guardian_sanctuary, self.cfg.divine_intervention] {
                if self.castable(base, now) {
                    return self.cast_targeted(base, self.my_game_id, &[]);
                }
            }
        }

        if self.my_mp_pct <= MANA_FLOOR && self.castable(self.cfg.mana_charge, now) {
            return self.cast_point(self.cfg.mana_charge);
        }

        if self.cfg.maintain_buffs && !self.party.is_empty() {
            for base in [self.cfg.bless_shakan, self.cfg.bless_seren, self.cfg.bless_balder, self.cfg.divine_prayer] {
                if self.castable(base, now) {
                    return self.cast_point(base);
                }
            }
        }

        if let Some(boss) = self.boss_target() {
            if self.castable(self.cfg.plague, now) {
                return self.cast_targeted(self.cfg.plague, boss, &[]);
            }
        }

        if !self.party_needs_attention() && self.my_hp_pct >= ATTACK_SELF_FLOOR {
            if self.rotation_active() {
                if let Some(cast) = self.rotate(now) {
                    return cast;
                }
            } else if self.cfg.attack_enabled {
                if let Some(cast) = self.try_attack(now) {
                    return cast;
                }
            }
        }

        Vec::new()
    }

    fn rotation_active(&self) -> bool {
        self.cfg.rotation_enabled || (self.boss_alive && self.boss.is_some())
    }

    fn edict_active(&self, now: Instant) -> bool {
        EDICT_ABNORM.iter().any(|id| self.abnorms.get(id).is_some_and(|deadline| *deadline > now))
    }

    fn boss_target(&self) -> Option<u64> {
        match (self.boss, self.boss_alive) {
            (Some(id), true) => Some(id),
            _ => self.nearest_hostile().map(|(id, _, _)| id),
        }
    }

    fn rotate(&mut self, now: Instant) -> Option<Vec<Outgoing>> {
        if self.charging {
            if self.charge_started.is_none_or(|start| now.duration_since(start) >= CHARGE_TIMEOUT) {
                return Some(self.release_charge());
            }
            return None;
        }
        let enemy = self.boss_target();
        let has_target = enemy.is_some();
        let target = enemy.unwrap_or(0);
        if self.edict_active(now) {
            for base in [self.cfg.zenobia, self.cfg.holy_burst, self.cfg.attack_aoe, self.cfg.energy_stars] {
                if self.castable(base, now) && (has_target || !self.uses_release(base)) {
                    return Some(self.cast_skill(base, target));
                }
            }
            if has_target && self.castable(self.cfg.divine_charge, now) {
                return Some(self.start_charge(now, target));
            }
            return None;
        }
        if has_target && self.castable(self.cfg.divine_charge, now) {
            return Some(self.start_charge(now, target));
        }
        if has_target && self.castable(self.cfg.energy_stars, now) {
            return Some(self.cast_skill(self.cfg.energy_stars, target));
        }
        if self.castable(self.cfg.triple_nemesis, now) {
            return Some(self.cast_combo(self.cfg.triple_nemesis, target, TRIPLE_NEMESIS_STAGES));
        }
        if self.castable(self.cfg.edict, now) {
            return Some(self.cast_skill(self.cfg.edict, target));
        }
        for base in [self.cfg.zenobia, self.cfg.holy_burst, self.cfg.attack_aoe] {
            if self.castable(base, now) && (has_target || !self.uses_release(base)) {
                return Some(self.cast_skill(base, target));
            }
        }
        None
    }

    fn uses_release(&self, base: u32) -> bool {
        base == self.cfg.energy_stars
    }

    fn cast_skill(&mut self, base: u32, target: u64) -> Vec<Outgoing> {
        self.log_trace(format!("-> {} sur {target}", self.skill_name(base)));
        if self.uses_release(base) {
            self.cast_lockon(base, target, &[])
        } else {
            self.cast_instant(base, target)
        }
    }

    fn mark_cast(&mut self, base: u32, now: Instant) {
        self.last_cast = Some(now);
        self.cooldowns
            .entry(base)
            .and_modify(|deadline| *deadline = (*deadline).max(now + PROVISIONAL_CD))
            .or_insert(now + PROVISIONAL_CD);
    }

    fn cast_instant(&mut self, base: u32, target: u64) -> Vec<Outgoing> {
        if base == 0 {
            return Vec::new();
        }
        self.mark_cast(base, Instant::now());
        vec![(0, "C_START_SKILL", self.start_skill(base, target))]
    }

    fn start_charge(&mut self, now: Instant, target: u64) -> Vec<Outgoing> {
        if self.cfg.divine_charge == 0 {
            return Vec::new();
        }
        self.charging = true;
        self.charge_started = Some(now);
        self.last_cast = Some(now);
        self.log_trace(format!("-> Divine Charge (charge...) sur {target}"));
        vec![(0, "C_START_SKILL", self.start_skill(self.cfg.divine_charge, target))]
    }

    fn release_charge(&mut self) -> Vec<Outgoing> {
        self.charging = false;
        self.charge_started = None;
        self.last_cast = Some(Instant::now());
        self.log_trace("-> Divine Charge (relache, buff groupe)".to_string());
        vec![(0, "C_PRESS_SKILL", self.press_release(self.cfg.divine_charge))]
    }

    fn party_needs_attention(&self) -> bool {
        self.party.iter().any(|ally| {
            ally.game_id != 0 && (ally.dead || (ally.hp_pct <= self.cfg.hp_limit && self.in_range(ally.loc)))
        })
    }

    fn nearest_hostile(&self) -> Option<(u64, [f32; 3], f64)> {
        self.npcs
            .iter()
            .filter(|(_, npc)| npc.hostile && npc.alive && npc.loc != [0.0; 3])
            .map(|(id, npc)| (*id, npc.loc, dist2(npc.loc, self.my_pos).sqrt() / UNITS_PER_METER))
            .filter(|(_, _, dist_m)| *dist_m <= ENGAGE_RANGE_M)
            .min_by(|a, b| a.2.total_cmp(&b.2))
    }

    fn try_attack(&mut self, now: Instant) -> Option<Vec<Outgoing>> {
        let (target, loc, dist_m) = self.nearest_hostile()?;
        if dist_m > ATTACK_RANGE_M {
            return None;
        }
        let cluster = self
            .npcs
            .values()
            .filter(|npc| npc.hostile && npc.alive && npc.loc != [0.0; 3])
            .filter(|npc| dist2(npc.loc, loc).sqrt() / UNITS_PER_METER <= AOE_CLUSTER_M)
            .count();
        if cluster >= AOE_CLUSTER_MIN && self.castable(self.cfg.attack_aoe, now) {
            let base = self.cfg.attack_aoe;
            return Some(self.cast_lockon(base, target, &[]));
        }
        if self.castable(self.cfg.attack_single, now) {
            let base = self.cfg.attack_single;
            return Some(self.cast_lockon(base, target, &[]));
        }
        None
    }

    fn nearest_mob_roam(&self) -> Option<([f32; 3], f64)> {
        self.npcs
            .values()
            .filter(|npc| npc.hostile && npc.alive && !npc.portal && npc.loc != [0.0; 3])
            .map(|npc| (npc.loc, dist2(npc.loc, self.my_pos).sqrt() / UNITS_PER_METER))
            .filter(|(_, dist_m)| *dist_m <= MAX_ROAM_M)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    pub fn farm_roam_target(&self) -> Option<[f32; 3]> {
        if !self.cfg.attack_enabled || self.party_needs_attention() || self.my_hp_pct < ATTACK_SELF_FLOOR {
            return None;
        }
        let (loc, dist_m) = self.nearest_mob_roam()?;
        if dist_m <= ATTACK_RANGE_M {
            return None;
        }
        let dx = loc[0] - self.my_pos[0];
        let dy = loc[1] - self.my_pos[1];
        let flat = (dx * dx + dy * dy).sqrt();
        if flat <= f32::EPSILON {
            return None;
        }
        let standoff = STANDOFF_M * UNITS_PER_METER as f32;
        let reach = (flat - standoff).max(0.0);
        Some([self.my_pos[0] + dx / flat * reach, self.my_pos[1] + dy / flat * reach, loc[2]])
    }

    pub fn combat_target(&self) -> Option<[f32; 3]> {
        if !(self.cfg.attack_enabled || self.rotation_active()) || self.party_needs_attention() || self.my_hp_pct < ATTACK_SELF_FLOOR {
            return None;
        }
        let (_, loc, dist_m) = self.nearest_hostile()?;
        if dist_m <= ATTACK_RANGE_M {
            return None;
        }
        let dx = loc[0] - self.my_pos[0];
        let dy = loc[1] - self.my_pos[1];
        let flat = (dx * dx + dy * dy).sqrt();
        if flat <= f32::EPSILON {
            return None;
        }
        let standoff = STANDOFF_M * UNITS_PER_METER as f32;
        let reach = (flat - standoff).max(0.0);
        Some([self.my_pos[0] + dx / flat * reach, self.my_pos[1] + dy / flat * reach, loc[2]])
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
                let name = member
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| old.map(|ally| ally.name.clone()).unwrap_or_default());
                self.party.push(Ally {
                    player_id,
                    game_id: if game_id != 0 { game_id } else { old.map_or(0, |ally| ally.game_id) },
                    name,
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
        let all: Vec<u32> = skills.iter().map(|skill| (uint(skill, "id") & SKILL_ID_MASK) as u32).collect();
        self.skill_ids = all.clone();
        let owned: Vec<u32> = all.into_iter().filter(|base| base.is_multiple_of(100)).collect();
        let best = |lo: u32, hi: u32| owned.iter().copied().filter(|base| (lo..=hi).contains(base)).max();

        for (slot, lo, hi) in [
            (&mut self.cfg.focus_heal, 190000, 199999),
            (&mut self.cfg.healing_immersion, 370000, 379999),
            (&mut self.cfg.heal_thyself, 180000, 189999),
            (&mut self.cfg.grace_resurrection, 390000, 399999),
            (&mut self.cfg.triple_nemesis, 290000, 299999),
            (&mut self.cfg.edict, 430000, 439999),
            (&mut self.cfg.restorative_burst, 340000, 349999),
            (&mut self.cfg.healing_circle, 30000, 39999),
            (&mut self.cfg.regen_circle, 20000, 29999),
            (&mut self.cfg.arise, 60000, 69999),
            (&mut self.cfg.resurrect, 120000, 129999),
            (&mut self.cfg.purifying_circle, 100000, 109999),
            (&mut self.cfg.kaia_shield, 220000, 229999),
            (&mut self.cfg.guardian_sanctuary, 310000, 319999),
            (&mut self.cfg.divine_prayer, 320000, 329999),
            (&mut self.cfg.mana_infusion, 80000, 89999),
            (&mut self.cfg.bless_shakan, 50000, 59999),
            (&mut self.cfg.bless_seren, 200000, 209999),
            (&mut self.cfg.bless_balder, 230000, 239999),
            (&mut self.cfg.plague, 300000, 309999),
            (&mut self.cfg.divine_charge, 280200, 280299),
            (&mut self.cfg.mana_charge, 280100, 280199),
            (&mut self.cfg.divine_intervention, 410200, 410299),
        ] {
            if let Some(found) = best(lo, hi) {
                *slot = found;
            }
        }
        if let Some(found) = best(350000, 359999) {
            self.cfg.energy_stars = found;
            self.cfg.attack_single = found;
        }
        if let Some(base) = best(400000, 409999) {
            self.cfg.zenobia = base + ZENOBIA_GLYPH_OFFSET;
        }
        if let Some(base) = best(160000, 169999) {
            self.cfg.attack_aoe = base + SHOCKING_GLYPH_OFFSET;
        }
        if let Some(base) = best(420000, 429999) {
            self.cfg.holy_burst = base + HOLY_BURST_GLYPH_OFFSET;
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
            .filter(|(_, npc)| npc.loc != [0.0; 3] && (!portal_only || npc.portal))
            .min_by(|a, b| dist2(a.1.loc, reference).total_cmp(&dist2(b.1.loc, reference)))
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
            .with("unk", Value::Bool(!self.moving))
            .with("moving", Value::Bool(self.moving))
            .with("continue", Value::Bool(self.moving))
            .with("target", Value::Uint(target))
            .with("unk2", Value::Bool(false))
    }

    fn press_release(&self, base: u32) -> Object {
        Object::new()
            .with("skill", self.skill_value(base))
            .with("press", Value::Bool(false))
            .with("loc", Value::Vec3(self.my_pos))
            .with("w", Value::Int(self.my_angle))
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
        let now = Instant::now();
        self.last_cast = Some(now);
        let release_at = LOCKON_CHANNEL_MS + LOCKON_RELEASE_MS;
        self.cooldowns.insert(base, now + Duration::from_millis(release_at + 600));
        let mut out: Vec<Outgoing> = Vec::with_capacity(extras.len() + 3);
        out.push((0, "C_START_SKILL", self.start_skill(base, 0)));
        out.push((LOCKON_CHANNEL_MS, "C_CAN_LOCKON_TARGET", self.lockon(base, primary)));
        for &extra in extras {
            out.push((LOCKON_CHANNEL_MS, "C_CAN_LOCKON_TARGET", self.lockon(base, extra)));
        }
        out.push((release_at, "C_START_SKILL", self.start_skill(base + SHOOT_OFFSET, primary)));
        out
    }

    fn has_release(&self, base: u32) -> bool {
        [
            self.cfg.focus_heal,
            self.cfg.healing_immersion,
            self.cfg.energy_stars,
            self.cfg.zenobia,
            self.cfg.attack_aoe,
            self.cfg.divine_intervention,
            self.cfg.mana_charge,
            self.cfg.plague,
        ]
        .contains(&base)
    }

    fn cast_targeted(&mut self, base: u32, primary: u64, extras: &[u64]) -> Vec<Outgoing> {
        if base == 0 || primary == 0 {
            return Vec::new();
        }
        self.mark_cast(base, Instant::now());
        let mut out: Vec<Outgoing> = Vec::with_capacity(extras.len() + 3);
        out.push((0, "C_START_SKILL", self.start_skill(base, primary)));
        out.push((0, "C_CAN_LOCKON_TARGET", self.lockon(base, primary)));
        for &extra in extras {
            out.push((0, "C_CAN_LOCKON_TARGET", self.lockon(base, extra)));
        }
        if self.has_release(base) {
            out.push((SHOOT_DELAY_MS, "C_START_SKILL", self.start_skill(base + SHOOT_OFFSET, primary)));
        }
        out
    }

    fn cast_point(&mut self, base: u32) -> Vec<Outgoing> {
        if base == 0 {
            return Vec::new();
        }
        self.last_cast = Some(Instant::now());
        vec![(0, "C_START_SKILL", self.start_skill(base, self.my_game_id))]
    }

    fn cast_combo(&mut self, base: u32, target: u64, stages: u32) -> Vec<Outgoing> {
        if base == 0 {
            return Vec::new();
        }
        self.log_trace(format!("-> {} combo x{stages} sur {target}", self.skill_name(base)));
        self.mark_cast(base, Instant::now());
        (0..stages)
            .map(|stage| {
                let packet = self.start_skill(base + stage, target);
                (u64::from(stage) * COMBO_STEP_MS, "C_START_SKILL", packet)
            })
            .collect()
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
