use crate::catalogue::{self, Named};
use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Deserialize)]
pub struct Place {
    pub name: String,
    pub continent: i64,
    pub pos: [f32; 3],
}

impl Named for Place {
    fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Default)]
pub struct Worlds {
    places: Vec<Place>,
}

impl Worlds {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(Self::default());
        };
        Ok(Self {
            places: serde_json::from_slice(&bytes)?,
        })
    }

    pub fn len(&self) -> usize {
        self.places.len()
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    pub fn find(&self, fragment: &str) -> Option<&Place> {
        catalogue::find(&self.places, fragment)
    }

    pub fn search(&self, fragment: &str, limit: usize) -> Vec<&Place> {
        catalogue::search(&self.places, fragment, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Worlds {
        Worlds {
            places: vec![
                Place {
                    name: "StartZone_ATW_P_x46y52_Center".into(),
                    continent: 13,
                    pos: [53760.0, -84480.0, -4435.0],
                },
                Place {
                    name: "A_ANC_A_P_x48y56_Center".into(),
                    continent: 7001,
                    pos: [84480.0, -23040.0, 1180.0],
                },
                Place {
                    name: "startzone_atw_p_x47y52_center".into(),
                    continent: 13,
                    pos: [69120.0, -84480.0, -3549.0],
                },
            ],
        }
    }

    #[test]
    fn a_fragment_matches_regardless_of_case() {
        let worlds = sample();
        assert_eq!(worlds.search("STARTZONE", 10).len(), 2);
        assert_eq!(worlds.search("x48y56", 10).len(), 1);
        assert!(worlds.search("nowhere", 10).is_empty());
    }

    #[test]
    fn an_exact_name_wins_over_a_partial_one() {
        let worlds = sample();
        let found = worlds
            .find("startzone_atw_p_x47y52_center")
            .expect("a place");
        assert_eq!(found.pos[0], 69120.0);
    }

    #[test]
    fn the_limit_is_respected() {
        assert_eq!(sample().search("center", 2).len(), 2);
    }
}
