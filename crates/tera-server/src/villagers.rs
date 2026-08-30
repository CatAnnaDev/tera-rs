use std::collections::HashSet;
use std::path::Path;

#[derive(Default)]
pub struct Villagers {
    posts: HashSet<(i64, i64)>,
}

impl Villagers {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(Self::default());
        };
        let rows: Vec<(i64, i64)> = serde_json::from_slice(&bytes)?;
        Ok(Self {
            posts: rows.into_iter().collect(),
        })
    }

    pub fn len(&self) -> usize {
        self.posts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.posts.is_empty()
    }

    pub fn zones(&self) -> usize {
        self.posts
            .iter()
            .map(|(zone, _)| *zone)
            .collect::<HashSet<i64>>()
            .len()
    }

    pub fn holds_a_post(&self, hunting_zone: i64, template: i64) -> bool {
        self.posts.contains(&(hunting_zone, template))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(body: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("villagers-{}.json", body.len()));
        std::fs::write(&path, body).expect("write");
        path
    }

    #[test]
    fn a_villager_is_recognised_by_its_zone_and_template() {
        let villagers = Villagers::load(&written("[[183,200023],[780,1502]]")).expect("load");
        assert!(villagers.holds_a_post(183, 200023));
        assert!(villagers.holds_a_post(780, 1502));
        assert!(!villagers.holds_a_post(183, 1502), "the pair must match, not either half");
        assert_eq!(villagers.zones(), 2);
    }

    #[test]
    fn a_missing_file_leaves_every_creature_free_to_roam() {
        let villagers = Villagers::load(Path::new("/nowhere/villagers.json")).expect("load");
        assert!(villagers.is_empty());
        assert!(!villagers.holds_a_post(183, 200023));
    }
}
