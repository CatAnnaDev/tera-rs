use crate::catalogue::{self, Named};
use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Deserialize)]
pub struct Item {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub level: i64,
    #[serde(default)]
    pub grade: i64,
}

impl Item {
    pub fn describe(&self) -> String {
        let mut text = format!("{} [{}]", self.name, self.id);
        if !self.kind.is_empty() {
            text.push_str(&format!(" {}", self.kind));
        }
        if !self.class.is_empty() {
            text.push_str(&format!(" {}", self.class));
        }
        if self.level > 0 {
            text.push_str(&format!(" lvl{}", self.level));
        }
        text
    }

    pub fn is_equipment(&self) -> bool {
        self.kind.starts_with("EQUIP")
    }
}

impl Named for Item {
    fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Default)]
pub struct Items {
    entries: Vec<Item>,
}

impl Items {
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

    pub fn find(&self, fragment: &str) -> Option<&Item> {
        catalogue::find(&self.entries, fragment)
    }

    pub fn search(&self, fragment: &str, limit: usize) -> Vec<&Item> {
        catalogue::search(&self.entries, fragment, limit)
    }

    pub fn by_id(&self, id: i64) -> Option<&Item> {
        self.entries.iter().find(|item| item.id == id)
    }
}
