use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::data::GameData;
use crate::net::Event;

pub const KIND_DAMAGE: u8 = 1;
pub const KIND_HEAL: u8 = 2;
pub const KIND_MANA: u8 = 3;

#[derive(Default, Clone)]
pub struct SkillStats {
    pub amount: i64,
    pub hits: u64,
    pub crits: u64,
    pub white: u64,
    pub amount_crit: i64,
    pub amount_white: i64,
    pub biggest_crit: i64,
    pub biggest_hit: i64,
}

impl SkillStats {
    fn add(&mut self, value: i64, crit: bool) {
        self.amount += value;
        self.hits += 1;
        if crit {
            self.crits += 1;
            self.amount_crit += value;
            self.biggest_crit = self.biggest_crit.max(value);
        } else {
            self.white += 1;
            self.amount_white += value;
            self.biggest_hit = self.biggest_hit.max(value);
        }
    }
    pub fn crit_rate(&self) -> f64 {
        if self.hits == 0 { 0.0 } else { self.crits as f64 * 100.0 / self.hits as f64 }
    }
    pub fn avg(&self) -> f64 {
        if self.hits == 0 { 0.0 } else { self.amount as f64 / self.hits as f64 }
    }
    pub fn avg_crit(&self) -> f64 {
        if self.crits == 0 { 0.0 } else { self.amount_crit as f64 / self.crits as f64 }
    }
    pub fn avg_white(&self) -> f64 {
        if self.white == 0 { 0.0 } else { self.amount_white as f64 / self.white as f64 }
    }
}

pub struct EntityStats {
    pub source: u64,
    pub damage: i64,
    pub damage_taken: i64,
    pub heal: i64,
    pub hits: u64,
    pub crits: u64,
    pub crit_amount: i64,
    pub heal_crits: u64,
    pub heal_hits: u64,
    pub begin: Instant,
    pub end: Instant,
    pub damage_skills: HashMap<u32, SkillStats>,
    pub heal_skills: HashMap<u32, SkillStats>,
    pub mana_skills: HashMap<u32, SkillStats>,
    pub cast_counts: HashMap<u32, u64>,
}

impl EntityStats {
    fn new(source: u64, now: Instant) -> Self {
        Self {
            source,
            damage: 0,
            damage_taken: 0,
            heal: 0,
            hits: 0,
            crits: 0,
            crit_amount: 0,
            heal_crits: 0,
            heal_hits: 0,
            begin: now,
            end: now,
            damage_skills: HashMap::new(),
            heal_skills: HashMap::new(),
            mana_skills: HashMap::new(),
            cast_counts: HashMap::new(),
        }
    }
    pub fn interval_secs(&self) -> f64 {
        (self.end - self.begin).as_secs_f64().max(0.001)
    }
    pub fn crit_rate(&self) -> f64 {
        if self.hits == 0 { 0.0 } else { self.crits as f64 * 100.0 / self.hits as f64 }
    }
    pub fn heal_crit_rate(&self) -> f64 {
        if self.heal_hits == 0 { 0.0 } else { self.heal_crits as f64 * 100.0 / self.heal_hits as f64 }
    }
}

pub struct Encounter {
    pub boss: Option<u64>,
    pub name: String,
    pub entities: HashMap<u64, EntityStats>,
    pub begin: Instant,
    pub end: Instant,
    pub total_damage: i64,
    pub total_heal: i64,
    pub total_taken: i64,
    pub ended: bool,
}

impl Encounter {
    fn new(boss: Option<u64>, name: String, now: Instant) -> Self {
        Self {
            boss,
            name,
            entities: HashMap::new(),
            begin: now,
            end: now,
            total_damage: 0,
            total_heal: 0,
            total_taken: 0,
            ended: false,
        }
    }

    fn record_taken(&mut self, target: u64, value: i64, now: Instant) {
        if self.ended {
            return;
        }
        let e = self.entities.entry(target).or_insert_with(|| EntityStats::new(target, now));
        e.end = now;
        e.damage_taken += value;
        self.total_taken += value;
    }

    pub fn ranked_taken(&self) -> Vec<&EntityStats> {
        let mut rows: Vec<&EntityStats> = self.entities.values().filter(|e| e.damage_taken > 0).collect();
        rows.sort_by(|a, b| b.damage_taken.cmp(&a.damage_taken));
        rows
    }
    pub fn interval_secs(&self) -> f64 {
        (self.end - self.begin).as_secs_f64().max(0.001)
    }
    pub fn dps(&self) -> f64 {
        self.total_damage as f64 / self.interval_secs()
    }
    fn record(&mut self, source: u64, value: i64, kind: u8, crit: bool, skill: u32, now: Instant) {
        if self.ended {
            return;
        }
        self.end = now;
        let e = self.entities.entry(source).or_insert_with(|| EntityStats::new(source, now));
        e.end = now;
        match kind {
            KIND_DAMAGE => {
                e.damage += value;
                e.hits += 1;
                if crit {
                    e.crits += 1;
                    e.crit_amount += value;
                }
                e.damage_skills.entry(skill).or_default().add(value, crit);
                self.total_damage += value;
            }
            KIND_HEAL => {
                e.heal += value;
                e.heal_hits += 1;
                if crit {
                    e.heal_crits += 1;
                }
                e.heal_skills.entry(skill).or_default().add(value, crit);
                self.total_heal += value;
            }
            KIND_MANA => {
                e.mana_skills.entry(skill).or_default().add(value, crit);
            }
            _ => {}
        }
    }
    pub fn ranked(&self) -> Vec<&EntityStats> {
        let mut rows: Vec<&EntityStats> = self.entities.values().filter(|e| e.damage > 0).collect();
        rows.sort_by(|a, b| b.damage.cmp(&a.damage));
        rows
    }

    pub fn ranked_heal(&self) -> Vec<&EntityStats> {
        let mut rows: Vec<&EntityStats> = self.entities.values().filter(|e| e.heal > 0).collect();
        rows.sort_by(|a, b| b.heal.cmp(&a.heal));
        rows
    }
}

pub struct PlayerInfo {
    pub name: String,
    pub class: Option<&'static str>,
    pub server_id: u64,
    pub player_id: u64,
}

#[derive(Default, Clone)]
pub struct MemberState {
    pub name: Option<String>,
    pub class: Option<&'static str>,
    pub cur_hp: i64,
    pub max_hp: i64,
    pub cur_mp: i64,
    pub max_mp: i64,
    pub alive: bool,
}

#[derive(Default)]
struct AbnState {
    begin: Option<Instant>,
    total: Duration,
}

#[derive(Default)]
pub struct AbnormalityTracker {
    per_target: HashMap<u64, HashMap<u32, AbnState>>,
}

impl AbnormalityTracker {
    fn begin(&mut self, target: u64, id: u32, now: Instant) {
        let st = self.per_target.entry(target).or_default().entry(id).or_default();
        if st.begin.is_none() {
            st.begin = Some(now);
        }
    }
    fn end(&mut self, target: u64, id: u32, now: Instant) {
        if let Some(map) = self.per_target.get_mut(&target) {
            if let Some(st) = map.get_mut(&id) {
                if let Some(b) = st.begin.take() {
                    st.total += now.saturating_duration_since(b);
                }
            }
        }
    }
    pub fn active(&self, target: u64) -> HashSet<u32> {
        match self.per_target.get(&target) {
            Some(map) => map.iter().filter(|(_, st)| st.begin.is_some()).map(|(id, _)| *id).collect(),
            None => HashSet::new(),
        }
    }

    pub fn uptime(&self, target: u64, now: Instant, window: f64) -> Vec<(u32, f64)> {
        let Some(map) = self.per_target.get(&target) else { return Vec::new() };
        let mut rows: Vec<(u32, f64)> = map
            .iter()
            .map(|(id, st)| {
                let mut total = st.total;
                if let Some(b) = st.begin {
                    total += now.saturating_duration_since(b);
                }
                (*id, (total.as_secs_f64() * 100.0 / window).min(100.0))
            })
            .filter(|(_, pct)| *pct > 0.0)
            .collect();
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        rows
    }
}

pub struct NpcInfo {
    pub template_id: u64,
    pub hunting_zone: u64,
    pub name: String,
    pub boss: bool,
}

pub struct Meter {
    pub total: Encounter,
    pub bosses: Vec<Encounter>,
    pub selected: usize,
    pub players: HashMap<u64, PlayerInfo>,
    pub npcs: HashMap<u64, NpcInfo>,
    pub boss_hp: HashMap<u64, (i64, i64)>,
    pub boss_enraged: HashMap<u64, (bool, i64)>,
    pub boss_target: HashMap<u64, u64>,
    pub deaths: HashMap<u64, u32>,
    pub abnormals: AbnormalityTracker,
    pub party: HashSet<(u64, u64)>,
    pub party_members: HashMap<(u64, u64), MemberState>,
    pub party_only: bool,
    pub auto_reset: bool,
    pub reset_idle: Duration,
    last_combat: Option<Instant>,
    pub me: u64,
    pub data: GameData,
}

impl Meter {
    pub fn new(data: GameData, now: Instant) -> Self {
        Self {
            total: Encounter::new(None, "TOTAL".into(), now),
            bosses: Vec::new(),
            selected: 0,
            players: HashMap::new(),
            npcs: HashMap::new(),
            boss_hp: HashMap::new(),
            boss_enraged: HashMap::new(),
            boss_target: HashMap::new(),
            deaths: HashMap::new(),
            abnormals: AbnormalityTracker::default(),
            party: HashSet::new(),
            party_members: HashMap::new(),
            party_only: false,
            auto_reset: false,
            reset_idle: Duration::from_secs(8),
            last_combat: None,
            me: 0,
            data,
        }
    }

    pub fn reset(&mut self, now: Instant) {
        self.total = Encounter::new(None, "TOTAL".into(), now);
        self.bosses.clear();
        self.selected = 0;
        self.last_combat = None;
    }

    pub fn active_bosses(&self) -> Vec<(u64, String, i64, i64, bool, i64)> {
        let mut out: Vec<(u64, String, i64, i64, bool, i64)> = self
            .boss_hp
            .iter()
            .filter(|(_, (cur, _))| *cur > 0)
            .map(|(gid, (cur, max))| {
                let name = self.npcs.get(gid).map(|n| n.name.clone()).unwrap_or_else(|| "Boss".into());
                let (enraged, ms) = self.boss_enraged.get(gid).copied().unwrap_or((false, 0));
                (*gid, name, *cur, *max, enraged, ms)
            })
            .collect();
        out.sort_by(|a, b| b.3.cmp(&a.3));
        out
    }

    pub fn current(&self) -> &Encounter {
        if self.selected == 0 || self.selected > self.bosses.len() {
            &self.total
        } else {
            &self.bosses[self.selected - 1]
        }
    }

    pub fn player_name(&self, gid: u64) -> String {
        if let Some(p) = self.players.get(&gid) {
            p.name.clone()
        } else if gid == self.me {
            "You".into()
        } else {
            format!("{gid:x}")
        }
    }

    pub fn player_class(&self, gid: u64) -> Option<&'static str> {
        self.players.get(&gid).and_then(|p| p.class)
    }

    pub fn is_player(&self, gid: u64) -> bool {
        self.players.contains_key(&gid) || gid == self.me
    }

    pub fn is_party(&self, gid: u64) -> bool {
        match self.players.get(&gid) {
            Some(p) => self.party.contains(&(p.server_id, p.player_id)),
            None => gid == self.me,
        }
    }

    pub fn current_boss(&self) -> Option<u64> {
        self.current().boss
    }

    pub fn aggro_target(&self, boss: u64) -> Option<u64> {
        self.boss_target.get(&boss).copied().filter(|t| *t != 0)
    }

    pub fn abnormality_name(&self, id: u32) -> String {
        self.data.abnormality_name(id)
    }

    pub fn deaths(&self, gid: u64) -> u32 {
        self.deaths.get(&gid).copied().unwrap_or(0)
    }

    pub fn skill_name(&self, gid: u64, skill: u32) -> String {
        let class = self.player_class(gid);
        self.data.skill_name(class, skill)
    }

    pub fn apply(&mut self, event: Event, now: Instant) {
        match event {
            Event::Me { game_id, name, template_id } => {
                self.me = game_id;
                let class = crate::data::player_class(template_id);
                self.players.insert(game_id, PlayerInfo { name, class, server_id: 0, player_id: 0 });
            }
            Event::Player { game_id, name, template_id, server_id, player_id } => {
                let class = crate::data::player_class(template_id);
                if let Some(m) = self.party_members.get_mut(&(server_id, player_id)) {
                    m.name = Some(name.clone());
                    if m.class.is_none() {
                        m.class = class;
                    }
                }
                self.players.insert(game_id, PlayerInfo { name, class, server_id, player_id });
            }
            Event::Npc { game_id, template_id, hunting_zone, max_hp } => {
                let looked = self.data.npc(template_id, hunting_zone);
                let boss = looked.map(|n| n.boss).unwrap_or(false) || max_hp >= 1_000_000;
                let name = looked
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| format!("NPC {template_id}"));
                self.npcs.insert(game_id, NpcInfo { template_id, hunting_zone, name, boss });
            }
            Event::Despawn { game_id, dead } => {
                let is_boss = self.npcs.get(&game_id).map(|n| n.boss).unwrap_or(false)
                    || self.boss_hp.contains_key(&game_id);
                if is_boss {
                    if dead {
                        if let Some(enc) = self.bosses.iter_mut().find(|e| e.boss == Some(game_id)) {
                            enc.ended = true;
                        }
                    }
                    self.boss_hp.remove(&game_id);
                    self.boss_enraged.remove(&game_id);
                    self.boss_target.remove(&game_id);
                } else {
                    self.npcs.remove(&game_id);
                }
            }
            Event::Hit { source, target, value, kind, crit, skill } => {
                if value <= 0 && kind == KIND_DAMAGE {
                    return;
                }
                let activity = (kind == KIND_DAMAGE || kind == KIND_HEAL)
                    && (self.is_player(source) || self.is_player(target));
                if activity {
                    if self.auto_reset {
                        if let Some(last) = self.last_combat {
                            if now.saturating_duration_since(last) > self.reset_idle {
                                self.reset(now);
                            }
                        }
                    }
                    self.last_combat = Some(now);
                }
                if self.is_player(source) {
                    self.total.record(source, value, kind, crit, skill, now);
                    if kind == KIND_DAMAGE
                        && self.npcs.get(&target).map(|n| n.boss).unwrap_or(false)
                    {
                        let idx = match self.bosses.iter().position(|e| e.boss == Some(target)) {
                            Some(i) => i,
                            None => {
                                let name =
                                    self.npcs.get(&target).map(|n| n.name.clone()).unwrap_or_default();
                                self.bosses.push(Encounter::new(Some(target), name, now));
                                let i = self.bosses.len() - 1;
                                self.selected = i + 1;
                                i
                            }
                        };
                        self.bosses[idx].record(source, value, kind, crit, skill, now);
                    }
                }
                if kind == KIND_DAMAGE && self.is_player(target) {
                    self.total.record_taken(target, value, now);
                    if let Some(idx) = self.bosses.iter().position(|e| e.boss == Some(source)) {
                        self.bosses[idx].record_taken(target, value, now);
                    }
                }
            }
            Event::BossGage { id, cur_hp, max_hp } => {
                self.boss_hp.insert(id, (cur_hp, max_hp));
            }
            Event::Hp { target, cur_hp, max_hp } => {
                let is_boss = self.npcs.get(&target).map(|n| n.boss).unwrap_or(false)
                    || self.boss_hp.contains_key(&target);
                if is_boss {
                    self.boss_hp.insert(target, (cur_hp, max_hp));
                }
            }
            Event::NpcStatus { game_id, enraged, remaining_enrage_ms, target } => {
                let is_boss = self.npcs.get(&game_id).map(|n| n.boss).unwrap_or(false)
                    || self.boss_hp.contains_key(&game_id);
                if is_boss {
                    self.boss_enraged.insert(game_id, (enraged, remaining_enrage_ms));
                    if target != 0 {
                        self.boss_target.insert(game_id, target);
                    } else {
                        self.boss_target.remove(&game_id);
                    }
                }
            }
            Event::Cast { source, skill } => {
                if self.players.contains_key(&source) || source == self.me {
                    *self.total.entities.entry(source).or_insert_with(|| EntityStats::new(source, now)).cast_counts.entry(skill).or_default() += 1;
                    for enc in self.bosses.iter_mut() {
                        if let Some(e) = enc.entities.get_mut(&source) {
                            *e.cast_counts.entry(skill).or_default() += 1;
                        }
                    }
                }
            }
            Event::Life { game_id, alive } => {
                if !alive && (self.players.contains_key(&game_id) || game_id == self.me) {
                    *self.deaths.entry(game_id).or_default() += 1;
                }
            }
            Event::AbnBegin { target, id } => self.abnormals.begin(target, id, now),
            Event::AbnEnd { target, id } => self.abnormals.end(target, id, now),
            Event::Party { members } => {
                self.party = members.iter().map(|(s, p, _)| (*s, *p)).collect();
                let mut fresh = HashMap::new();
                for (s, p, class) in members {
                    let mut m = self.party_members.remove(&(s, p)).unwrap_or_default();
                    m.class = crate::data::class_name(class as i64 - 1);
                    if m.name.is_none() {
                        m.name = self
                            .players
                            .values()
                            .find(|pi| pi.server_id == s && pi.player_id == p)
                            .map(|pi| pi.name.clone());
                    }
                    fresh.insert((s, p), m);
                }
                self.party_members = fresh;
            }
            Event::PartyHp { server_id, player_id, cur_hp, max_hp } => {
                let m = self.party_members.entry((server_id, player_id)).or_default();
                m.cur_hp = cur_hp;
                m.max_hp = max_hp;
            }
            Event::PartyMp { server_id, player_id, cur_mp, max_mp } => {
                let m = self.party_members.entry((server_id, player_id)).or_default();
                m.cur_mp = cur_mp;
                m.max_mp = max_mp;
            }
            Event::PartyStat { server_id, player_id, hp, max_hp, mp, max_mp, alive } => {
                let m = self.party_members.entry((server_id, player_id)).or_default();
                m.cur_hp = hp;
                m.max_hp = max_hp;
                m.cur_mp = mp;
                m.max_mp = max_mp;
                m.alive = alive;
            }
            Event::Disconnected => {}
        }
    }
}
