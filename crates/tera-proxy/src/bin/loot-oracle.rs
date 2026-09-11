use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use tera_protocol::{value, OpcodeMap, PacketBuffer, Registry};

const HOOKS: [&str; 3] = ["S_SPAWN_NPC", "S_DESPAWN_NPC", "S_SPAWN_DROPITEM"];
const DESPAWN_DEAD: u64 = 5;

#[derive(Parser)]
#[command(about = "Lit le flux dechiffre de Noctenium (external-interface) et construit la table de loot par mob")]
struct Cli {
    #[arg(long, default_value = "127.0.0.60:5301")]
    data: String,
    #[arg(long, default_value = "http://127.0.0.61:5300")]
    control: String,
    #[arg(long, default_value = "data/opcodes/protocol.376012.map")]
    opcodes: PathBuf,
    #[arg(long, default_value = "data/definitions.classicplus")]
    definitions: PathBuf,
    #[arg(long, default_value_t = 100)]
    patch: u32,
    #[arg(long, default_value = "loot-stats.json")]
    stats: PathBuf,
    #[arg(long, default_value_t = 25)]
    autosave: u64,
    #[arg(long, default_value_t = 15000)]
    ttl_ms: u64,
    #[arg(long, help = "n'appelle pas addHooks (les opcodes sont deja pousses)")]
    no_hook: bool,
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
struct DropStat {
    count: u64,
    amount: i64,
    mw: u64,
    max_enchant: i64,
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
struct Entry {
    hz: u64,
    tpl: u64,
    kills: u64,
    drops: HashMap<u64, DropStat>,
}

#[derive(Default)]
struct State {
    stats: HashMap<String, Entry>,
    live: HashMap<u64, (u64, u64)>,
    dead: HashMap<u64, (u64, u64, Instant)>,
    kills_since_save: u64,
}

fn key(hz: u64, tpl: u64) -> String {
    format!("{hz}-{tpl}")
}

impl State {
    fn entry(&mut self, hz: u64, tpl: u64) -> &mut Entry {
        self.stats.entry(key(hz, tpl)).or_insert(Entry {
            hz,
            tpl,
            kills: 0,
            drops: HashMap::new(),
        })
    }

    fn count_kill(&mut self, id: u64, hz: u64, tpl: u64) {
        self.live.remove(&id);
        if self.dead.contains_key(&id) {
            return;
        }
        self.entry(hz, tpl).kills += 1;
        self.dead.insert(id, (hz, tpl, Instant::now()));
        self.kills_since_save += 1;
    }

    fn prune(&mut self, ttl: Duration) {
        let now = Instant::now();
        self.dead.retain(|_, (_, _, t)| now.duration_since(*t) < ttl);
    }
}

fn u(obj: &value::Object, name: &str) -> Option<u64> {
    obj.get(name).and_then(|v| v.as_uint())
}

fn i(obj: &value::Object, name: &str) -> Option<i64> {
    obj.get(name).and_then(|v| v.as_int())
}

fn load_stats(path: &PathBuf) -> HashMap<String, Entry> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_stats(path: &PathBuf, stats: &HashMap<String, Entry>) {
    if let Ok(text) = serde_json::to_vec(stats) {
        let _ = std::fs::write(path, text);
    }
}

fn request_hooks(control: &str) -> Result<()> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "addHooks",
        "params": { "hooks": HOOKS },
        "id": 1
    });
    ureq::post(control)
        .send_json(&body)
        .context("addHooks vers le control-interface")?;
    Ok(())
}

fn show_entry(e: &Entry) {
    println!("mob {} (zone {}) - {} kill(s)", e.tpl, e.hz, e.kills);
    let mut rows: Vec<_> = e.drops.iter().collect();
    rows.sort_by(|a, b| b.1.count.cmp(&a.1.count));
    if rows.is_empty() {
        println!("  aucun drop observe");
        return;
    }
    for (item, d) in rows {
        let rate = if e.kills > 0 {
            format!("{:.1}%", 100.0 * d.count as f64 / e.kills as f64)
        } else {
            "?".to_string()
        };
        let avg = d.amount as f64 / d.count.max(1) as f64;
        let mw = if d.count > 0 {
            (100 * d.mw / d.count) as u32
        } else {
            0
        };
        let extra = if d.max_enchant > 0 {
            format!(", +{} max", d.max_enchant)
        } else {
            String::new()
        };
        println!(
            "  item {item}: {rate} ({}/{}), x{avg:.1} moy, mw {mw}%{extra}",
            d.count, e.kills
        );
    }
}

fn handle_command(line: &str, state: &Mutex<State>, stats_path: &PathBuf) {
    let mut parts = line.trim().split_whitespace();
    let cmd = parts.next().unwrap_or("");
    let mut s = state.lock().unwrap();
    match cmd {
        "" | "help" => {
            println!("commandes : near | list | reset | save | <templateId>");
        }
        "near" => {
            let mut seen = std::collections::HashSet::new();
            let targets: Vec<(u64, u64)> = s
                .live
                .values()
                .copied()
                .filter(|(hz, tpl)| seen.insert(key(*hz, *tpl)))
                .collect();
            if targets.is_empty() {
                println!("aucun mob visible autour.");
            }
            for (hz, tpl) in targets {
                match s.stats.get(&key(hz, tpl)) {
                    Some(e) => show_entry(e),
                    None => println!("mob {tpl} (zone {hz}) - jamais observe encore"),
                }
            }
        }
        "list" => {
            let mut rows: Vec<&Entry> = s.stats.values().collect();
            rows.sort_by(|a, b| b.kills.cmp(&a.kills));
            if rows.is_empty() {
                println!("aucune donnee encore, tue des mobs.");
            }
            for e in rows.into_iter().take(20) {
                println!(
                    "  mob {} (zone {}) : {} kills, {} type(s) de drop",
                    e.tpl,
                    e.hz,
                    e.kills,
                    e.drops.len()
                );
            }
        }
        "reset" => {
            s.stats.clear();
            s.dead.clear();
            save_stats(stats_path, &s.stats);
            println!("stats remises a zero.");
        }
        "save" => {
            save_stats(stats_path, &s.stats);
            println!("stats sauvegardees.");
        }
        other => {
            if let Ok(tpl) = other.parse::<u64>() {
                let hits: Vec<Entry> =
                    s.stats.values().filter(|e| e.tpl == tpl).cloned().collect();
                if hits.is_empty() {
                    println!("mob {tpl} jamais observe.");
                }
                for e in &hits {
                    show_entry(e);
                }
            } else {
                println!("inconnu : {other}");
            }
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let opcodes = OpcodeMap::read(&cli.opcodes)
        .with_context(|| format!("lecture des opcodes {}", cli.opcodes.display()))?;
    let registry = Registry::load(&[cli.definitions.clone()], Some(cli.patch))
        .context("chargement des definitions")?;

    let state = Arc::new(Mutex::new(State {
        stats: load_stats(&cli.stats),
        ..Default::default()
    }));

    if !cli.no_hook {
        request_hooks(&cli.control).context("le control-interface ne repond pas (external-interface actif ?)")?;
    }

    let stats_path = cli.stats.clone();
    let cmd_state = Arc::clone(&state);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines().map_while(Result::ok) {
            handle_command(&line, &cmd_state, &stats_path);
        }
    });

    let ttl = Duration::from_millis(cli.ttl_ms);
    let mut stream = TcpStream::connect(&cli.data)
        .with_context(|| format!("connexion au data-interface {}", cli.data))?;
    println!("connecte a {} - flux en cours, tape 'near'/'list' + Entree", cli.data);

    let mut buffer = PacketBuffer::new();
    let mut chunk = [0u8; 16384];
    let mut last_prune = Instant::now();

    loop {
        let n = stream.read(&mut chunk).context("lecture du flux data")?;
        if n == 0 {
            println!("flux ferme par Noctenium.");
            break;
        }
        buffer.push(&chunk[..n]);
        while let Some(packet) = buffer.take_packet() {
            let Some(name) = opcodes.name(packet.opcode) else {
                continue;
            };
            if !HOOKS.contains(&name) {
                continue;
            }
            let Some(def) = registry.get(name) else {
                continue;
            };
            let Ok(obj) = value::read(def, &packet.encode()) else {
                continue;
            };

            let mut s = state.lock().unwrap();
            match name {
                "S_SPAWN_NPC" => {
                    if let (Some(id), Some(tpl), Some(hz)) =
                        (u(&obj, "gameId"), u(&obj, "templateId"), u(&obj, "huntingZoneId"))
                    {
                        s.live.insert(id, (hz, tpl));
                    }
                }
                "S_DESPAWN_NPC" => {
                    if let Some(id) = u(&obj, "gameId") {
                        if let Some(&(hz, tpl)) = s.live.get(&id) {
                            if u(&obj, "type") == Some(DESPAWN_DEAD) {
                                s.count_kill(id, hz, tpl);
                                if s.kills_since_save >= cli.autosave {
                                    s.kills_since_save = 0;
                                    save_stats(&cli.stats, &s.stats);
                                }
                            } else {
                                s.live.remove(&id);
                            }
                        }
                    }
                }
                "S_SPAWN_DROPITEM" => {
                    if u(&obj, "explode") != Some(1) {
                        continue;
                    }
                    let Some(id) = u(&obj, "source") else { continue };
                    let src = s
                        .live
                        .get(&id)
                        .copied()
                        .or_else(|| s.dead.get(&id).map(|(hz, tpl, _)| (*hz, *tpl)));
                    let Some((hz, tpl)) = src else { continue };
                    s.count_kill(id, hz, tpl);
                    let item = u(&obj, "item").unwrap_or(0);
                    let amount = i(&obj, "amount").unwrap_or(0);
                    let mw = u(&obj, "masterwork") == Some(1);
                    let enchant = i(&obj, "enchant").unwrap_or(0);
                    let d = s.entry(hz, tpl).drops.entry(item).or_default();
                    d.count += 1;
                    d.amount += amount;
                    if mw {
                        d.mw += 1;
                    }
                    if enchant > d.max_enchant {
                        d.max_enchant = enchant;
                    }
                }
                _ => {}
            }

            if last_prune.elapsed() > Duration::from_secs(10) {
                s.prune(ttl);
                last_prune = Instant::now();
            }
        }
    }

    let s = state.lock().unwrap();
    save_stats(&cli.stats, &s.stats);
    Ok(())
}
