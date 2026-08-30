use crate::catalogue::{self, Named};
use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Deserialize)]
pub struct Npc {
    pub name: String,
    pub id: i64,
    pub zone: i64,
    #[serde(default)]
    pub shape: i64,
    #[serde(default = "one")]
    pub level: i64,
    pub boss: bool,
    pub hp: i64,
    #[serde(default = "default_walk")]
    pub walk: i64,
    #[serde(default = "default_run")]
    pub run: i64,
}

fn one() -> i64 {
    1
}

fn default_walk() -> i64 {
    25
}

fn default_run() -> i64 {
    100
}

impl Npc {
    pub fn spawn(&self, location: [f32; 3], facing: i64) -> crate::realm::Spawn {
        crate::realm::Spawn {
            template: self.id,
            shape: self.shape,
            hunting_zone: self.zone,
            location,
            facing,
            level: self.level,
            max_hp: self.hp.max(1),
            walk_speed: self.walk,
            run_speed: self.run,
            aggressive: false,
            anchor: location,
            roam: 0.0,
        }
    }
}

impl Named for Npc {
    fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Default)]
pub struct Npcs {
    entries: Vec<Npc>,
}

impl Npcs {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(Self::default());
        };
        Ok(Self {
            entries: serde_json::from_slice(&bytes)?,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn find(&self, fragment: &str) -> Option<&Npc> {
        catalogue::find(&self.entries, fragment)
    }

    pub fn search(&self, fragment: &str, limit: usize) -> Vec<&Npc> {
        catalogue::search(&self.entries, fragment, limit)
    }

    pub fn by_id(&self, id: i64) -> Option<&Npc> {
        self.entries.iter().find(|npc| npc.id == id)
    }

    pub fn lookup(&self, id: i64, zone: i64) -> Option<&Npc> {
        self.entries
            .iter()
            .find(|npc| npc.id == id && npc.zone == zone)
            .or_else(|| self.by_id(id))
    }
}
