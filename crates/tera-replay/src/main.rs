use anyhow::{Context, Result};
use clap::Parser;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tera_protocol::{value, OpcodeMap, Packet, Registry};
use tera_world::World;

#[derive(Parser)]
#[command(about = "Rejoue une capture JSONL: décode, vérifie le round-trip, reconstruit le game-state")]
struct Cli {
    capture: PathBuf,
    #[arg(long, default_value = "data/opcodes/protocol.376012.map")]
    opcodes: PathBuf,
    #[arg(long, default_values = ["data/definitions"])]
    definitions: Vec<PathBuf>,
    #[arg(long, default_value_t = 100)]
    patch_version: u32,
    #[arg(long, help = "affiche chaque paquet décodé")]
    verbose: bool,
    #[arg(long, help = "code de sortie non nul si un round-trip échoue")]
    strict: bool,
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok())
        .collect()
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let opcodes = OpcodeMap::read(&cli.opcodes)
        .with_context(|| format!("lecture {}", cli.opcodes.display()))?;
    let registry = Registry::load(&cli.definitions, Some(cli.patch_version))
        .context("chargement des définitions")?;
    let capture = std::fs::read_to_string(&cli.capture)
        .with_context(|| format!("lecture {}", cli.capture.display()))?;

    let mut world = World::new();
    let mut total = 0u64;
    let mut decoded = 0u64;
    let mut roundtrip_ok = 0u64;
    let mut roundtrip_bad = 0u64;
    let mut no_definition = 0u64;
    let mut decode_error = 0u64;
    let mut unknown: BTreeMap<String, u64> = BTreeMap::new();
    let mut mismatches: Vec<String> = Vec::new();

    for line in capture.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        total += 1;
        let name = record.get("name").and_then(|value| value.as_str()).unwrap_or("");
        let hex = record.get("hex").and_then(|value| value.as_str()).unwrap_or("");
        let opcode = record.get("opcode").and_then(|value| value.as_u64()).unwrap_or(0) as u16;
        let Some(body) = hex_decode(hex) else {
            continue;
        };
        let Some(definition) = registry.get(name) else {
            no_definition += 1;
            *unknown.entry(name.to_string()).or_default() += 1;
            continue;
        };
        let frame = Packet::new(opcode, body).encode();
        let object = match value::read(definition, &frame) {
            Ok(object) => object,
            Err(_) => {
                decode_error += 1;
                continue;
            }
        };
        decoded += 1;
        match value::write(definition, opcode, &object) {
            Ok(reencoded) if reencoded == frame => roundtrip_ok += 1,
            Ok(reencoded) => {
                roundtrip_bad += 1;
                if mismatches.len() < 16 {
                    mismatches.push(format!(
                        "{name}: {} o -> ré-encodé {} o",
                        frame.len(),
                        reencoded.len()
                    ));
                }
            }
            Err(_) => {
                roundtrip_bad += 1;
                if mismatches.len() < 16 {
                    mismatches.push(format!("{name}: échec ré-encodage"));
                }
            }
        }
        if cli.verbose {
            let from = record.get("from").and_then(|value| value.as_str()).unwrap_or("?");
            println!("{from:>6}  {name}");
        }
        world.apply(name, &object);
    }

    let player = &world.player;
    println!("== paquets ({} opcodes connus, {} définitions) ==", opcodes.len(), registry.len());
    println!("  total {total}  décodés {decoded}  sans définition {no_definition}  erreur décodage {decode_error}");
    println!("  round-trip byte-exact: {roundtrip_ok} ok, {roundtrip_bad} divergents");
    if !unknown.is_empty() {
        let mut rows: Vec<(&String, &u64)> = unknown.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1));
        println!("  opcodes sans définition:");
        for (name, count) in rows.iter().take(10) {
            println!("    {count:4}  {name}");
        }
    }
    if !mismatches.is_empty() {
        println!("  divergences round-trip:");
        for entry in &mismatches {
            println!("    {entry}");
        }
    }

    println!("== game-state reconstruit ==");
    println!(
        "  joueur: {:?} (playerId {}, gameId {}, template {}, niveau {})",
        player.name, player.player_id, player.game_id, player.template_id, player.level
    );
    println!(
        "  vie {}/{}  mana {}/{}",
        player.hp, player.max_hp, player.mp, player.max_mp
    );
    println!(
        "  zone {}  position ({:.0}, {:.0}, {:.0})",
        player.zone, player.location.x, player.location.y, player.location.z
    );
    println!(
        "  entités visibles: {} npc, {} joueurs (spawns {}, despawns {})",
        world.npc_count(),
        world.user_count(),
        world.spawns,
        world.despawns
    );
    println!("  inventaire: {} slots, {} po", world.inventory_slots, world.money);
    if let Some(npc) = world.nearest_npc() {
        println!(
            "  npc le plus proche: template {} (hz {}) à {:.0} u, {} pv max",
            npc.template_id,
            npc.hunting_zone,
            npc.location.distance_to(player.location),
            npc.max_hp
        );
    }

    if cli.strict && roundtrip_bad > 0 {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod gate {
    use super::*;

    #[test]
    fn every_captured_packet_round_trips() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let capture = root.join("captures/capture.jsonl");
        if !capture.exists() {
            return;
        }
        let opcodes = OpcodeMap::read(root.join("data/opcodes/protocol.376012.map")).unwrap();
        let _ = opcodes;
        let registry = Registry::load(&[root.join("data/definitions")], Some(100)).unwrap();
        let text = std::fs::read_to_string(&capture).unwrap();

        let mut checked = 0u64;
        let mut broken = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let record: serde_json::Value = serde_json::from_str(line).unwrap();
            let name = record["name"].as_str().unwrap_or("");
            let opcode = record["opcode"].as_u64().unwrap_or(0) as u16;
            let Some(body) = record["hex"].as_str().and_then(hex_decode) else {
                continue;
            };
            let Some(definition) = registry.get(name) else {
                continue;
            };
            let frame = Packet::new(opcode, body).encode();
            let object = value::read(definition, &frame)
                .unwrap_or_else(|error| panic!("{name}: décodage échoué: {error}"));
            let reencoded = value::write(definition, opcode, &object).unwrap();
            checked += 1;
            if reencoded != frame {
                broken.push(name.to_string());
            }
        }
        assert!(checked > 100, "capture trop courte ({checked} paquets)");
        assert!(broken.is_empty(), "round-trip non byte-exact: {broken:?}");
    }
}
