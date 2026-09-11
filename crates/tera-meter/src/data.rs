use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

const CLASS_NAMES: [&str; 13] = [
    "Warrior", "Lancer", "Slayer", "Berserker", "Sorcerer", "Archer", "Priest", "Mystic", "Reaper",
    "Gunner", "Brawler", "Ninja", "Valkyrie",
];

pub fn class_name(index: i64) -> Option<&'static str> {
    usize::try_from(index).ok().and_then(|i| CLASS_NAMES.get(i).copied())
}

pub fn player_class(template_id: u64) -> Option<&'static str> {
    class_name((template_id % 100) as i64 - 1)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Dps,
    Healer,
    Tank,
    Flex,
}

pub fn class_role(class: Option<&str>) -> Role {
    match class {
        Some("Priest") | Some("Mystic") => Role::Healer,
        Some("Lancer") => Role::Tank,
        Some("Brawler") | Some("Warrior") | Some("Berserker") => Role::Flex,
        _ => Role::Dps,
    }
}

pub fn class_role_color(class: Option<&str>) -> egui::Color32 {
    match class_role(class) {
        Role::Healer => egui::Color32::from_rgb(59, 226, 75),
        Role::Tank => egui::Color32::from_rgb(68, 178, 252),
        Role::Flex => egui::Color32::from_rgb(0xc9, 0x9a, 0x3b),
        Role::Dps => egui::Color32::from_rgb(255, 68, 102),
    }
}

#[derive(Deserialize)]
struct SkillJson {
    id: u32,
    class: String,
    name: String,
}

#[derive(Deserialize)]
struct NpcJson {
    name: String,
    id: u64,
    zone: u64,
    #[serde(default)]
    boss: bool,
}

#[derive(Deserialize)]
struct AbnJson {
    id: u32,
    name: String,
}

#[derive(Clone)]
pub struct NpcEntry {
    pub name: String,
    pub boss: bool,
}

pub struct GameData {
    skill_by_class: HashMap<(String, u32), String>,
    skill_by_id: HashMap<u32, String>,
    npc_by_key: HashMap<(u64, u64), NpcEntry>,
    npc_by_id: HashMap<u64, NpcEntry>,
    abnormalities: HashMap<u32, String>,
}

impl GameData {
    pub fn load(skills_path: &Path, npcs_path: &Path) -> Result<Self> {
        let skills: Vec<SkillJson> = serde_json::from_slice(
            &std::fs::read(skills_path).with_context(|| format!("lecture {}", skills_path.display()))?,
        )
        .context("parsing skills.json")?;
        let npcs: Vec<NpcJson> = serde_json::from_slice(
            &std::fs::read(npcs_path).with_context(|| format!("lecture {}", npcs_path.display()))?,
        )
        .context("parsing npcs.json")?;

        let mut skill_by_class = HashMap::new();
        let mut skill_by_id = HashMap::new();
        for s in skills {
            skill_by_class.insert((s.class, s.id), s.name.clone());
            skill_by_id.entry(s.id).or_insert(s.name);
        }

        let mut npc_by_key = HashMap::new();
        let mut npc_by_id = HashMap::new();
        for n in npcs {
            let entry = NpcEntry { name: n.name, boss: n.boss };
            npc_by_key.insert((n.id, n.zone), entry.clone());
            npc_by_id.entry(n.id).or_insert(entry);
        }

        let abnormalities = std::fs::read(npcs_path.with_file_name("abnormalities.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<AbnJson>>(&b).ok())
            .map(|v| v.into_iter().map(|a| (a.id, a.name)).collect())
            .unwrap_or_default();

        Ok(Self { skill_by_class, skill_by_id, npc_by_key, npc_by_id, abnormalities })
    }

    pub fn abnormality_name(&self, id: u32) -> String {
        self.abnormalities.get(&id).cloned().unwrap_or_else(|| format!("#{id}"))
    }

    pub fn skill_name(&self, class: Option<&str>, id: u32) -> String {
        if let Some(c) = class {
            if let Some(name) = self.skill_by_class.get(&(c.to_string(), id)) {
                return name.clone();
            }
        }
        if let Some(name) = self.skill_by_id.get(&id) {
            return name.clone();
        }
        id.to_string()
    }

    pub fn npc(&self, template_id: u64, zone: u64) -> Option<&NpcEntry> {
        self.npc_by_key.get(&(template_id, zone)).or_else(|| self.npc_by_id.get(&template_id))
    }
}
