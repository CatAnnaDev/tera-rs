use std::collections::HashMap;

fn spawns() -> tera_server::spawns::Spawns {
    tera_server::spawns::Spawns::load(&root().join("data/spawns.json")).expect("spawns")
}

fn villagers() -> tera_server::villagers::Villagers {
    tera_server::villagers::Villagers::load(&root().join("data/official-villagers.json"))
        .expect("villagers")
}

fn herds(spawns: &tera_server::spawns::Spawns, continent: i64) -> HashMap<[u32; 3], Vec<[f32; 3]>> {
    let mut grouped: HashMap<[u32; 3], Vec<[f32; 3]>> = HashMap::new();
    for point in spawns.on_continent(continent) {
        let centre = point.centre();
        grouped
            .entry([
                centre[0].to_bits(),
                centre[1].to_bits(),
                centre[2].to_bits(),
            ])
            .or_default()
            .push(point.location());
    }
    grouped
}

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn closest_pair(spots: &[[f32; 3]]) -> f32 {
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
fn an_npc_that_stands_alone_keeps_the_exact_spot_the_game_recorded() {
    let spawns = spawns();
    assert!(!spawns.is_empty(), "the spawn table is missing");
    let mut kept = 0;
    for continent in [13i64, 7004, 9034, 3051] {
        for point in spawns.on_continent(continent) {
            if !point.alone() {
                continue;
            }
            assert_eq!(
                point.location(),
                point.centre(),
                "a lone npc was pushed off the coordinate the game gave it"
            );
            kept += 1;
        }
    }
    assert!(kept > 900, "expected the world's fixed npcs, found {kept}");
}

#[test]
fn a_shared_territory_never_stacks_its_creatures_on_one_spot() {
    let spawns = spawns();
    for continent in [13i64, 7004, 9034, 3051] {
        for (_, spots) in herds(&spawns, continent).iter().filter(|(_, s)| s.len() > 1) {
            let apart = closest_pair(spots);
            assert!(
                apart >= 20.0,
                "a herd of {} on continent {continent} packs two creatures {apart:.0} units apart",
                spots.len()
            );
        }
    }
}

#[test]
fn the_most_crowded_territory_still_gives_each_creature_room() {
    let spawns = spawns();
    let biggest = herds(&spawns, 3051)
        .into_values()
        .max_by_key(Vec::len)
        .expect("herds");
    assert!(biggest.len() > 500, "expected the 542-creature territory");
    let apart = closest_pair(&biggest);
    assert!(
        apart >= 25.0,
        "the largest herd in the game packs two creatures {apart:.0} units apart"
    );
}

#[test]
fn an_npc_that_holds_a_post_is_never_given_room_to_wander() {
    let spawns = spawns();
    let villagers = villagers();
    assert!(villagers.len() > 8000, "the official villager list is missing");
    let mut posted = 0;
    let mut roamers = 0;
    for continent in [13i64, 7004, 9034, 3051] {
        for point in spawns.on_continent(continent) {
            let roam = match villagers.holds_a_post(point.hunting_zone, point.template) {
                true => 0.0,
                false => point.roam(),
            };
            if villagers.holds_a_post(point.hunting_zone, point.template) {
                assert_eq!(roam, 0.0, "a villager was given a wandering radius");
                posted += 1;
            } else if roam > 0.0 {
                roamers += 1;
            }
        }
    }
    assert!(posted > 300, "expected the world's fixed posts, found {posted}");
    assert!(roamers > 300, "expected wandering creatures, found {roamers}");
}

#[test]
fn a_villager_standing_alone_is_placed_exactly_where_the_game_put_it() {
    let spawns = spawns();
    let villagers = villagers();
    let realm = tera_server::realm::Realm::default();
    let npcs = tera_server::npcs::Npcs::load(&root().join("data/npcs.json")).expect("npcs");
    let placed = spawns.populate(
        &realm,
        &npcs,
        &villagers,
        &tera_server::spawns::Around {
            continent: 13,
            origin: [0.0; 3],
            radius: 500_000.0,
            limit: 4096,
        },
    );
    assert!(placed > 50, "only {placed} creatures were placed in zone 13");
    let mut checked = 0;
    for creature in realm.near(13, [0.0; 3], 1_000_000.0) {
        if villagers.holds_a_post(creature.hunting_zone, creature.template) {
            assert_eq!(creature.roam, 0.0);
            assert_eq!(creature.location, creature.anchor);
            checked += 1;
        }
    }
    assert!(checked > 10, "zone 13 held only {checked} posted npcs");
}
