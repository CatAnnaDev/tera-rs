use crate::npcs::Npcs;
use crate::realm::{Realm, Spawn};
use serde::Deserialize;
use std::path::Path;

#[derive(Clone, Deserialize)]
pub struct Point {
    #[serde(rename = "hz")]
    pub hunting_zone: i64,
    #[serde(rename = "cont")]
    pub continent: Option<i64>,
    #[serde(rename = "tid")]
    pub template: i64,
    #[serde(default)]
    pub dir: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    #[serde(default)]
    pub radius: f64,
}

impl Point {
    pub fn location(&self) -> [f32; 3] {
        [self.x as f32, self.y as f32, self.z as f32]
    }

    pub fn facing(&self) -> i64 {
        let turns = self.dir / 360.0;
        let wrapped = turns - turns.floor();
        let raw = (wrapped * 65536.0).round() as i64;
        if raw > 32767 {
            raw - 65536
        } else {
            raw
        }
    }
}

#[derive(Default)]
pub struct Spawns {
    points: Vec<Point>,
}

impl Spawns {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(Self::default());
        };
        Ok(Self {
            points: serde_json::from_slice(&bytes)?,
        })
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn on_continent(&self, continent: i64) -> impl Iterator<Item = &Point> {
        self.points
            .iter()
            .filter(move |point| point.continent == Some(continent))
    }

    pub fn placed(&self) -> usize {
        self.points
            .iter()
            .filter(|point| point.continent.is_some())
            .count()
    }

    pub fn nearest(&self, continent: i64, origin: [f32; 3], radius: f32, limit: usize) -> Vec<&Point> {
        let squared = radius * radius;
        let mut found: Vec<(f32, &Point)> = self
            .on_continent(continent)
            .filter_map(|point| {
                let at = point.location();
                let (dx, dy, dz) = (at[0] - origin[0], at[1] - origin[1], at[2] - origin[2]);
                let distance = dx * dx + dy * dy + dz * dz;
                (distance <= squared).then_some((distance, point))
            })
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.truncate(limit);
        found.into_iter().map(|(_, point)| point).collect()
    }

    pub fn populate(
        &self,
        realm: &Realm,
        npcs: &Npcs,
        continent: i64,
        origin: [f32; 3],
        radius: f32,
        limit: usize,
    ) -> usize {
        let mut placed = 0;
        for point in self.nearest(continent, origin, radius, limit) {
            let Some(known) = npcs.lookup(point.template, point.hunting_zone) else {
                continue;
            };
            if realm.occupied(continent, point.template, point.location()) {
                continue;
            }
            let mut spawn: Spawn = known.spawn(point.location(), point.facing());
            spawn.hunting_zone = point.hunting_zone;
            realm.spawn(continent, &spawn);
            placed += 1;
        }
        placed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(dir: f64) -> Point {
        Point {
            hunting_zone: 13,
            continent: Some(13),
            template: 1,
            dir,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            radius: 0.0,
        }
    }

    #[test]
    fn degrees_become_the_clients_signed_angle() {
        assert_eq!(point(0.0).facing(), 0);
        assert_eq!(point(90.0).facing(), 16384);
        assert_eq!(point(180.0).facing(), -32768);
        assert_eq!(point(270.0).facing(), -16384);
        assert_eq!(point(360.0).facing(), 0);
        assert_eq!(point(-90.0).facing(), -16384);
    }

    #[test]
    fn a_point_without_a_continent_is_never_returned() {
        let spawns = Spawns {
            points: vec![
                Point { continent: Some(13), ..point(0.0) },
                Point { continent: None, ..point(0.0) },
            ],
        };
        assert_eq!(spawns.len(), 2);
        assert_eq!(spawns.placed(), 1);
        assert_eq!(spawns.on_continent(13).count(), 1);
    }

    #[test]
    fn every_angle_fits_the_wire_type() {
        for degrees in (-720..720).step_by(7) {
            let facing = point(degrees as f64).facing();
            assert!(
                (-32768..=32767).contains(&facing),
                "{degrees} degrees produced {facing}, which does not fit an i16"
            );
        }
    }
}
