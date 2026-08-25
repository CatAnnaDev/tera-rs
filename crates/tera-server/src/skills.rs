use crate::catalogue::{self, Named};
use serde::Deserialize;
use std::path::Path;

const CLASS_NAMES: [&str; 13] = [
    "Warrior",
    "Lancer",
    "Slayer",
    "Berserker",
    "Sorcerer",
    "Archer",
    "Priest",
    "Mystic",
    "Reaper",
    "Gunner",
    "Brawler",
    "Ninja",
    "Valkyrie",
];

pub fn class_name(class: i64) -> Option<&'static str> {
    usize::try_from(class).ok().and_then(|index| CLASS_NAMES.get(index).copied())
}

#[derive(Clone, Deserialize)]
pub struct Skill {
    pub id: i64,
    pub class: String,
    pub name: String,
}

impl Named for Skill {
    fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Default)]
pub struct Skills {
    entries: Vec<Skill>,
}

impl Skills {
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

    pub fn search(&self, fragment: &str, limit: usize) -> Vec<&Skill> {
        catalogue::search(&self.entries, fragment, limit)
    }

    pub fn for_class(&self, class: i64, include_common: bool) -> Vec<&Skill> {
        let Some(wanted) = class_name(class) else {
            return Vec::new();
        };
        self.entries
            .iter()
            .filter(|skill| {
                skill.class == wanted || (include_common && skill.class == "Common")
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_indexes_match_the_creation_packet() {
        assert_eq!(class_name(0), Some("Warrior"));
        assert_eq!(class_name(6), Some("Priest"));
        assert_eq!(class_name(12), Some("Valkyrie"));
        assert_eq!(class_name(13), None);
        assert_eq!(class_name(-1), None);
    }
}
