use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn ago(seconds: i64) -> String {
    if seconds <= 0 {
        return "a l'instant".to_string();
    }
    let s = seconds as u64;
    if s < 60 {
        format!("il y a {s}s")
    } else if s < 3600 {
        format!("il y a {}min", s / 60)
    } else if s < 86400 {
        format!("il y a {}h{:02}", s / 3600, (s % 3600) / 60)
    } else {
        format!("il y a {}j", s / 86400)
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Mail {
    pub from: String,
    pub text: String,
    pub at: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Timer {
    pub name: String,
    pub next: i64,
    pub repeat: i64,
    pub lead: i64,
    pub lead_done: bool,
}

pub enum TimerFire {
    Lead { name: String, remaining: i64 },
    Fire { name: String },
}

#[derive(Serialize, Deserialize, Default)]
pub struct BotState {
    #[serde(default)]
    pub mailbox: HashMap<String, Vec<Mail>>,
    #[serde(default)]
    pub seen: HashMap<String, i64>,
    #[serde(default)]
    pub watch: Vec<String>,
    #[serde(default)]
    pub timers: Vec<Timer>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl BotState {
    pub fn load(path: PathBuf) -> Self {
        let mut state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<BotState>(&raw).ok())
            .unwrap_or_default();
        state.path = Some(path);
        state
    }

    pub fn save(&self) {
        if let Some(path) = &self.path {
            if let Ok(raw) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(path, raw);
            }
        }
    }

    pub fn mark_seen(&mut self, name: &str) {
        self.seen.insert(name.to_lowercase(), now_unix());
    }

    pub fn seen_ago(&self, name: &str) -> Option<i64> {
        self.seen.get(&name.to_lowercase()).map(|at| now_unix() - at)
    }

    pub fn add_mail(&mut self, to: &str, from: &str, text: &str) -> usize {
        let queue = self.mailbox.entry(to.to_lowercase()).or_default();
        queue.push(Mail { from: from.to_string(), text: text.to_string(), at: now_unix() });
        let count = queue.len();
        self.save();
        count
    }

    pub fn take_mail(&mut self, name: &str) -> Vec<Mail> {
        let mail = self.mailbox.remove(&name.to_lowercase()).unwrap_or_default();
        if !mail.is_empty() {
            self.save();
        }
        mail
    }

    pub fn is_watched(&self, name: &str) -> bool {
        let lc = name.to_lowercase();
        self.watch.contains(&lc)
    }

    pub fn watch_add(&mut self, name: &str) -> bool {
        let lc = name.to_lowercase();
        if self.watch.contains(&lc) {
            return false;
        }
        self.watch.push(lc);
        self.save();
        true
    }

    pub fn watch_del(&mut self, name: &str) -> bool {
        let lc = name.to_lowercase();
        let before = self.watch.len();
        self.watch.retain(|w| *w != lc);
        let changed = self.watch.len() != before;
        if changed {
            self.save();
        }
        changed
    }

    pub fn timer_add(&mut self, name: &str, in_secs: i64, repeat: i64, lead: i64) {
        self.timers.retain(|t| !t.name.eq_ignore_ascii_case(name));
        self.timers.push(Timer {
            name: name.to_string(),
            next: now_unix() + in_secs,
            repeat,
            lead,
            lead_done: false,
        });
        self.save();
    }

    pub fn timer_del(&mut self, name: &str) -> bool {
        let before = self.timers.len();
        self.timers.retain(|t| !t.name.eq_ignore_ascii_case(name));
        let changed = self.timers.len() != before;
        if changed {
            self.save();
        }
        changed
    }

    pub fn timers_summary(&self) -> String {
        if self.timers.is_empty() {
            return "aucun minuteur".to_string();
        }
        let now = now_unix();
        let mut list: Vec<(i64, String)> = self
            .timers
            .iter()
            .map(|t| {
                let remaining = t.next - now;
                let repeat = if t.repeat > 0 { format!(" (toutes les {}min)", t.repeat / 60) } else { String::new() };
                (remaining, format!("{} dans {}{repeat}", t.name, fmt_remaining(remaining)))
            })
            .collect();
        list.sort_by_key(|(r, _)| *r);
        list.into_iter().map(|(_, s)| s).collect::<Vec<_>>().join(", ")
    }

    pub fn tick_timers(&mut self) -> Vec<TimerFire> {
        let now = now_unix();
        let mut fires = Vec::new();
        let mut dirty = false;
        let mut drop_indices = Vec::new();
        for (index, timer) in self.timers.iter_mut().enumerate() {
            if timer.lead > 0 && !timer.lead_done && now >= timer.next - timer.lead && now < timer.next {
                timer.lead_done = true;
                dirty = true;
                fires.push(TimerFire::Lead { name: timer.name.clone(), remaining: timer.next - now });
            }
            if now >= timer.next {
                fires.push(TimerFire::Fire { name: timer.name.clone() });
                if timer.repeat > 0 {
                    while timer.next <= now {
                        timer.next += timer.repeat;
                    }
                    timer.lead_done = false;
                } else {
                    drop_indices.push(index);
                }
                dirty = true;
            }
        }
        for index in drop_indices.into_iter().rev() {
            self.timers.remove(index);
        }
        if dirty {
            self.save();
        }
        fires
    }
}

fn fmt_remaining(seconds: i64) -> String {
    if seconds <= 0 {
        return "maintenant".to_string();
    }
    let s = seconds as u64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}min", s / 60)
    } else {
        format!("{}h{:02}", s / 3600, (s % 3600) / 60)
    }
}

pub struct ActivityLog {
    path: Option<PathBuf>,
}

impl ActivityLog {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }

    pub fn log(&self, kind: &str, detail: &str) {
        let Some(path) = &self.path else { return };
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{}\t{}\t{}", now_unix(), kind, detail);
        }
    }
}

pub struct PingCsv {
    path: Option<PathBuf>,
}

impl PingCsv {
    pub fn new(path: Option<PathBuf>) -> Self {
        if let Some(p) = &path {
            if !p.exists() {
                let _ = std::fs::write(p, "timestamp,rtt_ms,jitter_ms,loss\n");
            }
        }
        Self { path }
    }

    pub fn record(&self, rtt: u32, jitter: u32, loss: bool) {
        let Some(path) = &self.path else { return };
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{},{},{},{}", now_unix(), rtt, jitter, if loss { 1 } else { 0 });
        }
    }
}

#[derive(Default)]
pub struct ZoneTime {
    totals: HashMap<i64, u64>,
    current: Option<i64>,
    since: i64,
}

impl ZoneTime {
    pub fn enter(&mut self, zone: i64) {
        let now = now_unix();
        if let Some(prev) = self.current {
            if prev != zone {
                *self.totals.entry(prev).or_default() += (now - self.since).max(0) as u64;
                self.since = now;
            }
        } else {
            self.since = now;
        }
        self.current = Some(zone);
    }

    pub fn snapshot(&self) -> Vec<(i64, u64)> {
        let now = now_unix();
        let mut out: Vec<(i64, u64)> = self.totals.iter().map(|(z, s)| (*z, *s)).collect();
        if let Some(cur) = self.current {
            let live = (now - self.since).max(0) as u64;
            if let Some(entry) = out.iter_mut().find(|(z, _)| *z == cur) {
                entry.1 += live;
            } else {
                out.push((cur, live));
            }
        }
        out.sort_by_key(|(_, secs)| std::cmp::Reverse(*secs));
        out
    }
}

#[derive(Deserialize, Clone)]
pub struct BossNpc {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub region: String,
}

#[derive(Default)]
pub struct BossDb(HashMap<(u32, u32), BossNpc>);

impl BossDb {
    pub fn load(path: Option<&PathBuf>) -> Self {
        let Some(path) = path else { return Self::default() };
        let map = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<HashMap<String, BossNpc>>(&raw).ok())
            .map(|m| {
                m.into_iter()
                    .filter_map(|(k, v)| {
                        let (hz, tid) = k.split_once(':')?;
                        Some(((hz.parse().ok()?, tid.parse().ok()?), v))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self(map)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn lookup(&self, hunting_zone: u32, template_id: u32) -> Option<&BossNpc> {
        self.0.get(&(hunting_zone, template_id))
    }
}

#[derive(Default)]
pub struct ZoneNames(HashMap<i64, String>);

impl ZoneNames {
    pub fn load(path: Option<&PathBuf>) -> Self {
        let Some(path) = path else { return Self::default() };
        let map = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str::<HashMap<String, String>>(&raw).ok())
            .map(|m| m.into_iter().filter_map(|(k, v)| k.parse::<i64>().ok().map(|id| (id, v))).collect())
            .unwrap_or_default();
        Self(map)
    }

    pub fn label(&self, id: i64) -> String {
        match self.0.get(&id) {
            Some(name) => format!("{name} ({id})"),
            None => format!("zone {id}"),
        }
    }
}

pub struct SessionStats {
    started: std::time::Instant,
    xp: u64,
    kills: u64,
    loot: u64,
    deaths: u64,
    level_xp: u64,
    total_level_xp: u64,
}

impl Default for SessionStats {
    fn default() -> Self {
        Self {
            started: std::time::Instant::now(),
            xp: 0,
            kills: 0,
            loot: 0,
            deaths: 0,
            level_xp: 0,
            total_level_xp: 0,
        }
    }
}

impl SessionStats {
    pub fn on_exp(&mut self, gained: u64, level_xp: u64, total_level_xp: u64, monster: u64) {
        self.xp = self.xp.saturating_add(gained);
        if monster != 0 {
            self.kills += 1;
        }
        self.level_xp = level_xp;
        if total_level_xp != 0 {
            self.total_level_xp = total_level_xp;
        }
    }

    pub fn on_loot(&mut self, amount: u64) {
        self.loot = self.loot.saturating_add(amount.max(1));
    }

    pub fn on_death(&mut self) {
        self.deaths += 1;
    }

    fn hours(&self) -> f64 {
        (self.started.elapsed().as_secs_f64() / 3600.0).max(1e-6)
    }

    pub fn xp_per_hour(&self) -> u64 {
        (self.xp as f64 / self.hours()).round() as u64
    }

    pub fn kills_per_hour(&self) -> u64 {
        (self.kills as f64 / self.hours()).round() as u64
    }

    pub fn kills(&self) -> u64 {
        self.kills
    }

    pub fn loot(&self) -> u64 {
        self.loot
    }

    pub fn deaths(&self) -> u64 {
        self.deaths
    }

    pub fn level_pct(&self) -> f64 {
        if self.total_level_xp == 0 {
            0.0
        } else {
            (self.level_xp as f64 / self.total_level_xp as f64 * 100.0).clamp(0.0, 100.0)
        }
    }

    pub fn eta_level(&self) -> String {
        let per_hour = self.xp_per_hour();
        if per_hour == 0 || self.total_level_xp == 0 || self.level_xp >= self.total_level_xp {
            return "-".to_string();
        }
        let remaining = self.total_level_xp - self.level_xp;
        let hours = remaining as f64 / per_hour as f64;
        let minutes = (hours * 60.0).round() as u64;
        if minutes >= 60 {
            format!("{}h{:02}", minutes / 60, minutes % 60)
        } else {
            format!("{minutes}min")
        }
    }
}

fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

#[derive(Deserialize)]
struct FaqRule {
    #[serde(default)]
    keywords: Vec<String>,
    answer: String,
    #[serde(default)]
    all: bool,
}

#[derive(Deserialize, Default)]
struct FaqFile {
    #[serde(default)]
    rules: Vec<FaqRule>,
    #[serde(default)]
    fallback: Option<String>,
}

pub struct Faq {
    rules: Vec<(Vec<String>, String, bool)>,
    fallback: Option<String>,
}

impl Faq {
    pub fn load(path: Option<&PathBuf>) -> Self {
        let Some(path) = path else {
            return Self { rules: Vec::new(), fallback: None };
        };
        let file: FaqFile = std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let rules = file
            .rules
            .into_iter()
            .map(|r| (r.keywords.iter().map(|k| normalize(k)).collect(), r.answer, r.all))
            .collect();
        Self { rules, fallback: file.fallback }
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.fallback.is_none()
    }

    pub fn answer(&self, message: &str) -> Option<&str> {
        let haystack = normalize(message);
        for (keywords, answer, all) in &self.rules {
            if keywords.is_empty() {
                continue;
            }
            let hit = if *all {
                keywords.iter().all(|k| haystack.contains(k.as_str()))
            } else {
                keywords.iter().any(|k| haystack.contains(k.as_str()))
            };
            if hit {
                return Some(answer.as_str());
            }
        }
        self.fallback.as_deref()
    }
}
