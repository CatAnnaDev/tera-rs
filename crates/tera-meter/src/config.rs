use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub opacity: u8,
    pub party_only: bool,
    pub view: u8,
    pub notify_enrage: bool,
    pub notify_enrage_end: bool,
    pub notify_missing_debuff: bool,
    pub sound: bool,
    pub auto_height: bool,
    pub required_debuffs: Vec<u32>,
    pub window: Option<[f32; 4]>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            opacity: 0xdc,
            party_only: false,
            view: 0,
            notify_enrage: true,
            notify_enrage_end: true,
            notify_missing_debuff: false,
            sound: true,
            auto_height: false,
            required_debuffs: Vec::new(),
            window: None,
        }
    }
}

pub fn path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config").join("tera-meter").join("config.json")
}

impl Config {
    pub fn load() -> Self {
        std::fs::read(path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(p, text);
        }
    }
}
