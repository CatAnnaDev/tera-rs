use crate::noise::{mixed, unit};
use crate::npcs::Npcs;
use crate::realm::{Realm, Spawn};
use serde::Deserialize;
use std::collections::HashMap;
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
    #[serde(skip)]
    rank: u32,
    #[serde(skip)]
    crowd: u32,
    #[serde(skip)]
    seed: u64,
}

const SCATTER_LIMIT: f32 = 1200.0;
const GOLDEN_ANGLE: f32 = 2.399_963_2;


impl Point {
    pub fn centre(&self) -> [f32; 3] {
        [self.x as f32, self.y as f32, self.z as f32]
    }

    fn territory_key(&self) -> u64 {
        [
            self.continent.unwrap_or(i64::MIN) as u64,
            self.x.to_bits(),
            self.y.to_bits(),
            self.z.to_bits(),
        ]
        .into_iter()
        .fold(0u64, |carried, part| mixed(carried ^ part))
    }

    pub fn alone(&self) -> bool {
        self.crowd <= 1
    }

    pub fn roam(&self) -> f32 {
        let spread = (self.radius as f32).min(SCATTER_LIMIT);
        if !spread.is_finite() || spread <= 0.0 {
            return 0.0;
        }
        if self.alone() {
            return spread;
        }
        spread / (self.crowd as f32).sqrt() * 0.5
    }

    pub fn location(&self) -> [f32; 3] {
        let spread = (self.radius as f32).min(SCATTER_LIMIT);
        if self.alone() || !spread.is_finite() || spread <= 0.0 {
            return self.centre();
        }
        let rank = self.rank as f32;
        let crowd = self.crowd.max(1) as f32;
        let jitter = unit(mixed(self.seed ^ u64::from(self.rank)));
        let distance = spread * ((rank + jitter) / crowd).sqrt();
        let phase = unit(self.seed) * std::f32::consts::TAU;
        let angle = phase + rank * GOLDEN_ANGLE;
        [
            self.x as f32 + distance * angle.cos(),
            self.y as f32 + distance * angle.sin(),
            self.z as f32,
        ]
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

pub struct Around {
    pub continent: i64,
    pub origin: [f32; 3],
    pub radius: f32,
    pub limit: usize,
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
        let mut points: Vec<Point> = serde_json::from_slice(&bytes)?;
        let mut crowds: HashMap<u64, u32> = HashMap::with_capacity(points.len());
        for point in &mut points {
            point.seed = point.territory_key();
            let taken = crowds.entry(point.seed).or_default();
            point.rank = *taken;
            *taken += 1;
        }
        for point in &mut points {
            point.crowd = crowds[&point.seed];
        }
        Ok(Self { points })
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
        villagers: &crate::villagers::Villagers,
        around: &Around,
    ) -> usize {
        let Around {
            continent,
            origin,
            radius,
            limit,
        } = *around;
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
            spawn.roam = match villagers.holds_a_post(point.hunting_zone, point.template) {
                true => 0.0,
                false => point.roam(),
            };
            realm.spawn(continent, &spawn);
            placed += 1;
        }
        placed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn herd(crowd: u32, radius: f64) -> Vec<Point> {
        (0..crowd)
            .map(|rank| Point {
                hunting_zone: 13,
                continent: Some(13),
                template: 1,
                dir: 0.0,
                x: 1000.0,
                y: -2000.0,
                z: 500.0,
                radius,
                rank,
                crowd,
                seed: 0x5eed_1234_abcd_ef01,
            })
            .collect()
    }

    fn territory(rank: u32, radius: f64) -> Point {
        let crowd = rank + 1;
        herd(crowd, radius).swap_remove(rank as usize)
    }

    fn away_from_centre(point: &Point) -> f32 {
        let at = point.location();
        let centre = point.centre();
        ((at[0] - centre[0]).powi(2) + (at[1] - centre[1]).powi(2)).sqrt()
    }

    fn closest_pair(herd: &[Point]) -> f32 {
        let spots: Vec<[f32; 3]> = herd.iter().map(Point::location).collect();
        let mut closest = f32::MAX;
        for (index, one) in spots.iter().enumerate() {
            for other in &spots[index + 1..] {
                let (dx, dy) = (one[0] - other[0], one[1] - other[1]);
                closest = closest.min((dx * dx + dy * dy).sqrt());
            }
        }
        closest
    }

    #[test]
    fn creatures_sharing_a_territory_stand_clear_of_one_another() {
        for (crowd, floor) in [(2u32, 300.0f32), (24, 120.0), (128, 50.0), (542, 25.0)] {
            let apart = closest_pair(&herd(crowd, 1200.0));
            assert!(
                apart >= floor,
                "a herd of {crowd} packed two creatures {apart:.0} units apart"
            );
        }
    }

    #[test]
    fn a_creature_never_leaves_its_territory() {
        for index in 0..512 {
            let point = territory(index, 700.0);
            assert!(away_from_centre(&point) <= 700.0);
            assert_eq!(point.location()[2], point.centre()[2]);
        }
    }

    #[test]
    fn a_vast_territory_still_keeps_its_creatures_within_reach() {
        for index in 0..256 {
            assert!(away_from_centre(&territory(index, 11364.0)) <= SCATTER_LIMIT);
        }
    }

    #[test]
    fn the_same_point_always_lands_on_the_same_spot() {
        let point = territory(17, 4000.0);
        assert_eq!(point.location(), territory(17, 4000.0).location());
    }

    #[test]
    fn a_territory_with_no_room_keeps_its_centre() {
        assert_eq!(territory(3, 0.0).location(), territory(3, 0.0).centre());
    }

    #[test]
    fn an_npc_that_owns_its_spot_is_never_moved_off_it() {
        let lone = herd(1, 4000.0);
        assert_eq!(lone[0].location(), lone[0].centre());
    }

    #[test]
    fn the_scatter_fills_the_disc_rather_than_hugging_its_edge() {
        let radius = 1000.0;
        let inner = herd(2000, radius)
            .iter()
            .filter(|point| away_from_centre(point) <= radius as f32 * 0.7071)
            .count();
        assert!((950..=1050).contains(&inner), "half the disc held {inner} of 2000");
    }

    #[test]
    fn each_territory_is_turned_a_different_way() {
        let mut points = herd(8, 900.0);
        points[0].seed = 1;
        let one = points[0].location();
        points[0].seed = 2;
        assert_ne!(one, points[0].location());
    }

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
            rank: 0,
            crowd: 1,
            seed: 0,
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
