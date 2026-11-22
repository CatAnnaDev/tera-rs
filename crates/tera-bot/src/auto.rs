use std::collections::HashSet;
use std::time::{Duration, Instant};

use tera_protocol::{Object, Value};

pub type Send = (u64, &'static str, Object);

pub struct AutoConfig {
    pub vanguard: bool,
    pub guild_quest: bool,
    pub guardian: bool,
    pub daily: bool,
    pub announce_guild_quest: bool,
    pub battlegrounds: Vec<u32>,
}

impl Default for AutoConfig {
    fn default() -> Self {
        Self {
            vanguard: true,
            guild_quest: true,
            guardian: true,
            daily: true,
            announce_guild_quest: false,
            battlegrounds: vec![102, 103, 110, 111, 112, 116, 117, 118, 119],
        }
    }
}

#[derive(Default)]
pub struct AutoOut {
    pub sends: Vec<Send>,
    pub logs: Vec<String>,
}

#[derive(Clone, Copy)]
struct Guardian {
    points: u32,
    max_points: u32,
    claimed: i32,
    cleared: i32,
}

impl Guardian {
    fn done_boxes(&self) -> i32 {
        (self.cleared - 1).max(0)
    }

    fn late_boxes(&self) -> i32 {
        (self.done_boxes() - self.claimed.max(0)).max(0)
    }
}

struct ClaimState {
    active: bool,
    attempts: u32,
    slow: u32,
    baseline: Option<i32>,
    next_at: Instant,
}

impl Default for ClaimState {
    fn default() -> Self {
        Self { active: false, attempts: 0, slow: 0, baseline: None, next_at: Instant::now() }
    }
}

const DELAY_MS: u64 = 2000;
const GUILD_QUEST_RESTART_MS: u64 = DELAY_MS * 2;
const CLAIM_TIMEOUT: Duration = Duration::from_millis(1000);
const CLAIM_STEP_MS: u64 = 250;
const CLAIM_MAX_ATTEMPTS: u32 = 6;

pub struct AutoBot {
    pub cfg: AutoConfig,
    guild_quests_in_progress: HashSet<i64>,
    pending_vanguard: Vec<i64>,
    guardian: Option<Guardian>,
    claim: ClaimState,
    in_world: bool,
    on_hold: bool,
    daily_done: bool,
    daily_count: i64,
}

impl AutoBot {
    pub fn new(cfg: AutoConfig) -> Self {
        Self {
            cfg,
            guild_quests_in_progress: HashSet::new(),
            pending_vanguard: Vec::new(),
            guardian: None,
            claim: ClaimState::default(),
            in_world: false,
            on_hold: false,
            daily_done: false,
            daily_count: 0,
        }
    }

    fn is_battleground(&self, zone: u32) -> bool {
        self.cfg.battlegrounds.contains(&zone)
    }

    pub fn status(&self) -> String {
        format!(
            "auto: VG {} | GQ {} | GL {} | DC {} | daily {}",
            on_off(self.cfg.vanguard),
            on_off(self.cfg.guild_quest),
            on_off(self.cfg.guardian),
            on_off(self.cfg.daily),
            self.daily_count,
        )
    }

    pub fn toggle(&mut self, key: &str) -> Option<String> {
        let (slot, label): (&mut bool, &str) = match key {
            "vg" | "vanguard" => (&mut self.cfg.vanguard, "Auto-Vanguard"),
            "gq" | "guildquest" => (&mut self.cfg.guild_quest, "Auto-Guildquest"),
            "gl" | "guardian" => (&mut self.cfg.guardian, "Auto-Guardian-Legion"),
            "dc" | "daily" => (&mut self.cfg.daily, "Auto-Daily-Credit"),
            _ => return None,
        };
        *slot = !*slot;
        Some(format!("{label}: {}", on_off(*slot)))
    }

    pub fn guardian_summary(&self) -> String {
        match &self.guardian {
            Some(guardian) => {
                let late = guardian.late_boxes();
                let state = if late > 0 {
                    format!("{late} box(es) a reclamer")
                } else {
                    "tout reclame".to_string()
                };
                format!(
                    "Guardian: {}/{} fait - {} reclame ({state}) - points {}/{}",
                    guardian.done_boxes(),
                    guardian.max_points,
                    guardian.claimed.max(0),
                    guardian.points,
                    guardian.max_points,
                )
            }
            None => "Guardian: aucune donnee recue".to_string(),
        }
    }

    pub fn set_zone(&mut self, zone: u32) -> AutoOut {
        let mut out = AutoOut::default();
        if self.is_battleground(zone) {
            self.on_hold = true;
            return out;
        }
        let was_hold = self.on_hold;
        self.on_hold = false;
        if was_hold && !self.pending_vanguard.is_empty() {
            let ids = std::mem::take(&mut self.pending_vanguard);
            for (index, id) in ids.into_iter().enumerate() {
                self.push_vanguard_completion(id, index as u64 * DELAY_MS, &mut out);
            }
            self.push_daily_credit(&mut out);
        }
        out
    }

    pub fn on_enter_world(&mut self) -> AutoOut {
        let mut out = AutoOut::default();
        self.in_world = true;
        self.push_daily_credit(&mut out);
        out
    }

    pub fn on_leave_game(&mut self) {
        self.guild_quests_in_progress.clear();
        self.pending_vanguard.clear();
        self.guardian = None;
        self.claim = ClaimState::default();
        self.in_world = false;
        self.on_hold = false;
        self.daily_done = false;
        self.daily_count = 0;
    }

    pub fn observe(&mut self, name: &str, object: &Object) -> AutoOut {
        let mut out = AutoOut::default();
        match name {
            "S_AVAILABLE_EVENT_MATCHING_LIST" => {
                if let Some(credits) = object.get("vanguardCredits").and_then(Value::as_int) {
                    self.daily_count = credits;
                }
            }
            "S_COMPLETE_EVENT_MATCHING_QUEST" => {
                self.daily_count += 1;
                if self.cfg.vanguard {
                    let id = object.get("id").and_then(Value::as_int).unwrap_or(0);
                    if self.on_hold {
                        self.pending_vanguard.push(id);
                    } else {
                        self.push_vanguard_completion(id, DELAY_MS, &mut out);
                    }
                }
            }
            "S_UPDATE_GUILD_QUEST_STATUS" => {
                self.observe_guild_quest(object, &mut out);
            }
            "S_FIELD_EVENT_PROGRESS_INFO" => {
                if self.cfg.guardian && !self.claim.active {
                    self.start_claim(true, &mut out);
                }
            }
            "S_FIELD_POINT_INFO" => {
                self.observe_field_point(object, &mut out);
            }
            _ => {}
        }
        out
    }

    pub fn tick(&mut self, now: Instant) -> AutoOut {
        let mut out = AutoOut::default();
        if !self.claim.active || now < self.claim.next_at {
            return out;
        }
        let late = self.guardian.map(|g| g.late_boxes()).unwrap_or(0);
        if late <= 0 {
            self.finish_claim(false, &mut out);
            return out;
        }
        if self.claim.attempts >= CLAIM_MAX_ATTEMPTS {
            self.finish_claim(true, &mut out);
            return out;
        }
        self.claim.attempts += 1;
        self.claim.slow += 1;
        out.sends.push((0, "C_REQUEST_ONGOING_FIELD_EVENT_LIST", Object::new()));
        self.send_claim(now, &mut out);
        out
    }

    fn observe_guild_quest(&mut self, object: &Object, out: &mut AutoOut) {
        if !self.cfg.guild_quest {
            return;
        }
        let quest = object.get("quest").and_then(Value::as_int).unwrap_or(0);
        let targets = match object.get("targets") {
            Some(Value::Array(list)) => list,
            _ => return,
        };
        if targets.is_empty() {
            return;
        }
        let all_done = targets.iter().all(|target| {
            let completed = target.get("completed").and_then(Value::as_int).unwrap_or(0);
            let total = target.get("total").and_then(Value::as_int).unwrap_or(-1);
            completed == total
        });
        if !all_done || self.guild_quests_in_progress.contains(&quest) {
            return;
        }
        self.guild_quests_in_progress.insert(quest);
        out.logs.push(format!("guildquest {quest} termine, relance"));
        if self.cfg.announce_guild_quest {
            out.sends.push((
                DELAY_MS / 2,
                "C_CHAT",
                Object::new()
                    .with("channel", Value::Uint(2))
                    .with("message", Value::Str(format!("Guild quest {quest} terminee, relancee."))),
            ));
        }
        out.sends.push((
            DELAY_MS,
            "C_REQUEST_FINISH_GUILD_QUEST",
            Object::new().with("quest", Value::Int(quest)),
        ));
        out.sends.push((
            GUILD_QUEST_RESTART_MS,
            "C_REQUEST_START_GUILD_QUEST",
            Object::new().with("questId", Value::Int(quest)),
        ));
    }

    fn observe_field_point(&mut self, object: &Object, out: &mut AutoOut) {
        let previous = self.guardian;
        let guardian = Guardian {
            points: object.get("points").and_then(Value::as_uint).unwrap_or(0) as u32,
            max_points: object.get("maxPoints").and_then(Value::as_uint).unwrap_or(0) as u32,
            claimed: object.get("claimed").and_then(Value::as_int).unwrap_or(0) as i32,
            cleared: object.get("cleared").and_then(Value::as_int).unwrap_or(0) as i32,
        };
        self.guardian = Some(guardian);

        if self.claim.active {
            let advanced = self.claim.baseline.map(|b| guardian.claimed > b).unwrap_or(true);
            if advanced {
                self.claim.attempts = 0;
                out.logs.push(format!(
                    "guardian box reclame: {}/{}",
                    guardian.claimed.max(0),
                    guardian.done_boxes()
                ));
                if guardian.late_boxes() > 0 {
                    self.claim.next_at = Instant::now() + Duration::from_millis(self.step_delay());
                } else {
                    self.finish_claim(false, out);
                }
            }
            return;
        }

        let changed = previous.map(|p| p.claimed != guardian.claimed || p.cleared != guardian.cleared).unwrap_or(true);
        if changed && guardian.late_boxes() > 0 {
            out.logs.push(self.guardian_summary());
        }
        if guardian.late_boxes() > 0 {
            self.start_claim(false, out);
        }
    }

    pub fn claim_now(&mut self) -> AutoOut {
        let mut out = AutoOut::default();
        self.start_claim(true, &mut out);
        out
    }

    fn start_claim(&mut self, blind: bool, out: &mut AutoOut) {
        if !self.cfg.guardian || self.claim.active || !self.in_world {
            return;
        }
        let late = self.guardian.map(|g| g.late_boxes()).unwrap_or(0);
        if !blind && late <= 0 {
            return;
        }
        self.claim.active = true;
        self.claim.attempts = 0;
        self.claim.slow = 0;
        let now = Instant::now();
        self.send_claim(now, out);
    }

    fn send_claim(&mut self, now: Instant, out: &mut AutoOut) {
        if !self.claim.active {
            return;
        }
        self.claim.baseline = self.guardian.map(|g| g.claimed);
        out.sends.push((0, "C_REQUEST_FIELD_POINT_REWARD", Object::new()));
        self.claim.next_at = now + CLAIM_TIMEOUT;
    }

    fn step_delay(&self) -> u64 {
        CLAIM_STEP_MS + u64::from(self.claim.slow) * 250
    }

    fn finish_claim(&mut self, stalled: bool, out: &mut AutoOut) {
        let late = self.guardian.map(|g| g.late_boxes()).unwrap_or(0);
        self.claim = ClaimState::default();
        if stalled {
            out.logs.push(format!("guardian: {late} box(es) non reclamees, nouvel essai au prochain update"));
        }
    }

    fn push_vanguard_completion(&mut self, id: i64, base_delay: u64, out: &mut AutoOut) {
        out.sends.push((base_delay, "C_COMPLETE_DAILY_EVENT", Object::new().with("id", Value::Int(id))));
        out.sends.push((
            base_delay + DELAY_MS,
            "C_COMPLETE_EXTRA_EVENT",
            Object::new().with("type", Value::Int(0)),
        ));
        out.sends.push((
            base_delay + DELAY_MS,
            "C_COMPLETE_EXTRA_EVENT",
            Object::new().with("type", Value::Int(1)),
        ));
        out.logs.push(format!("vanguard {id} valide (daily + extra)"));
    }

    fn push_daily_credit(&mut self, out: &mut AutoOut) {
        if !self.cfg.daily || self.daily_done {
            return;
        }
        self.daily_done = true;
        out.sends.push((DELAY_MS, "C_REQUEST_RECV_DAILY_TOKEN", Object::new()));
        out.logs.push("daily credit reclame".to_string());
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "On"
    } else {
        "Off"
    }
}
