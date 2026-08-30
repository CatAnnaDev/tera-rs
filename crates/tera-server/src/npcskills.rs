use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Deserialize)]
pub struct Attack {
    pub zone: i64,
    pub npc: i64,
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub range: f32,
    pub cool_time: i64,
    pub offensive: bool,
    pub projectile_ms: f32,
}

impl Attack {
    pub fn reaches(&self, gap: f32) -> bool {
        gap <= self.range.max(1.0)
    }
}

#[derive(Default)]
pub struct Attacks {
    by_creature: HashMap<(i64, i64), Vec<Attack>>,
}

impl Attacks {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        let all: Vec<Attack> = serde_json::from_str(&text)?;
        let mut by_creature: HashMap<(i64, i64), Vec<Attack>> = HashMap::new();
        for attack in all {
            if !attack.offensive || attack.range <= 0.0 {
                continue;
            }
            by_creature
                .entry((attack.zone, attack.npc))
                .or_default()
                .push(attack);
        }
        for list in by_creature.values_mut() {
            list.sort_by(|left, right| {
                right
                    .range
                    .partial_cmp(&left.range)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        Ok(Self { by_creature })
    }

    pub fn len(&self) -> usize {
        self.by_creature.values().map(Vec::len).sum()
    }

    pub fn creatures(&self) -> usize {
        self.by_creature.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_creature.is_empty()
    }

    pub fn of(&self, zone: i64, npc: i64) -> &[Attack] {
        self.by_creature
            .get(&(zone, npc))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn longest_reach(&self, zone: i64, npc: i64) -> Option<f32> {
        self.of(zone, npc).first().map(|attack| attack.range)
    }

    pub fn choose(&self, zone: i64, npc: i64, gap: f32, ready: impl Fn(i64) -> bool) -> Option<&Attack> {
        self.of(zone, npc)
            .iter()
            .filter(|attack| attack.reaches(gap) && ready(attack.id))
            .min_by(|left, right| {
                left.range
                    .partial_cmp(&right.range)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attack(id: i64, range: f32, cool: i64) -> Attack {
        Attack {
            zone: 2,
            npc: 1,
            id,
            name: format!("attack {id}"),
            kind: "normal".into(),
            range,
            cool_time: cool,
            offensive: true,
            projectile_ms: 0.0,
        }
    }

    fn catalogue(attacks: Vec<Attack>) -> Attacks {
        let mut by_creature: HashMap<(i64, i64), Vec<Attack>> = HashMap::new();
        for attack in attacks {
            by_creature.entry((attack.zone, attack.npc)).or_default().push(attack);
        }
        for list in by_creature.values_mut() {
            list.sort_by(|a, b| b.range.partial_cmp(&a.range).unwrap());
        }
        Attacks { by_creature }
    }

    #[test]
    fn the_shortest_attack_that_still_reaches_is_chosen() {
        let attacks = catalogue(vec![attack(1, 300.0, 0), attack(2, 150.0, 0), attack(3, 100.0, 0)]);
        assert_eq!(attacks.choose(2, 1, 120.0, |_| true).map(|a| a.id), Some(2));
        assert_eq!(attacks.choose(2, 1, 90.0, |_| true).map(|a| a.id), Some(3));
        assert_eq!(attacks.choose(2, 1, 280.0, |_| true).map(|a| a.id), Some(1));
    }

    #[test]
    fn nothing_is_chosen_when_the_target_is_out_of_every_reach() {
        let attacks = catalogue(vec![attack(1, 100.0, 0)]);
        assert!(attacks.choose(2, 1, 400.0, |_| true).is_none());
    }

    #[test]
    fn a_cooling_attack_is_skipped_for_the_next_one_that_reaches() {
        let attacks = catalogue(vec![attack(1, 300.0, 0), attack(2, 150.0, 0)]);
        assert_eq!(attacks.choose(2, 1, 120.0, |id| id != 2).map(|a| a.id), Some(1));
    }

    #[test]
    fn a_creature_we_have_no_data_for_has_no_attacks() {
        let attacks = catalogue(vec![attack(1, 100.0, 0)]);
        assert!(attacks.of(99, 99).is_empty());
        assert!(attacks.longest_reach(99, 99).is_none());
    }

    #[test]
    fn the_longest_reach_is_reported_for_the_ai_to_close_the_gap() {
        let attacks = catalogue(vec![attack(1, 141.0, 0), attack(2, 100.0, 0)]);
        assert_eq!(attacks.longest_reach(2, 1), Some(141.0));
    }
}
