use anyhow::{Context, Result};
use clap::Parser;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use tera_protocol::{value, Packet, Registry};

#[derive(Parser)]
#[command(about = "Recover the opcode map from a raw capture by matching packets to definitions")]
struct Cli {
    capture: PathBuf,
    #[arg(long, default_values = ["data/definitions"])]
    definitions: Vec<PathBuf>,
    #[arg(long, default_value_t = 100)]
    patch: u32,
    #[arg(long, default_value = "protocol.guessed.map")]
    out: PathBuf,
    #[arg(long, default_value_t = 12, help = "sample packets kept per opcode")]
    samples: usize,
    #[arg(long, default_value_t = 0, help = "stop after this many capture lines (0 = all)")]
    limit: u64,
    #[arg(long, help = "also write bijection-resolved guesses, not only unique matches")]
    include_guesses: bool,
    #[arg(long, help = "score the result against the name field carried in the capture")]
    score: bool,
}

#[derive(PartialEq, Eq, Hash, Clone, Copy)]
struct Key {
    client: bool,
    opcode: u16,
}

#[derive(Default)]
struct Observed {
    frames: Vec<Vec<u8>>,
    truth: HashMap<String, u64>,
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok())
        .collect()
}

fn roundtrips(definition: &tera_protocol::Definition, opcode: u16, frame: &[u8]) -> bool {
    match value::read(definition, frame) {
        Ok(object) => {
            matches!(value::write(definition, opcode, &object), Ok(bytes) if bytes == frame)
        }
        Err(_) => false,
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let registry = Registry::load(&cli.definitions, Some(cli.patch))
        .context("loading definitions")?;

    let mut observed: HashMap<Key, Observed> = HashMap::new();
    let file = File::open(&cli.capture)
        .with_context(|| format!("opening {}", cli.capture.display()))?;
    let reader = BufReader::new(file);
    let mut lines = 0u64;
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        lines += 1;
        if cli.limit != 0 && lines > cli.limit {
            break;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let from = record.get("from").and_then(|v| v.as_str()).unwrap_or("");
        let client = match from {
            "client" => true,
            "server" => false,
            _ => continue,
        };
        let opcode = record.get("opcode").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        let hex = record.get("hex").and_then(|v| v.as_str()).unwrap_or("");
        let Some(body) = hex_decode(hex) else {
            continue;
        };
        let entry = observed.entry(Key { client, opcode }).or_default();
        if let Some(name) = record.get("name").and_then(|v| v.as_str()) {
            if !name.is_empty() {
                *entry.truth.entry(name.to_string()).or_default() += 1;
            }
        }
        if entry.frames.len() < cli.samples {
            entry.frames.push(Packet::new(opcode, body).encode());
        }
    }

    let client_defs = collect_defs(&registry, "C_");
    let server_defs = collect_defs(&registry, "S_");

    let mut candidates: HashMap<Key, HashSet<String>> = HashMap::new();
    for (key, data) in &observed {
        if data.frames.is_empty() {
            continue;
        }
        let pool = if key.client { &client_defs } else { &server_defs };
        let mut names = HashSet::new();
        for (name, definition) in pool {
            if data
                .frames
                .iter()
                .all(|frame| roundtrips(definition, key.opcode, frame))
            {
                names.insert((*name).to_string());
            }
        }
        if !names.is_empty() {
            candidates.insert(*key, names);
        }
    }

    let assignment = solve(candidates);

    let mut rows: Vec<(String, u16)> = assignment
        .iter()
        .filter(|(_, (_, confident))| cli.include_guesses || *confident)
        .map(|(key, (name, _))| (name.clone(), key.opcode))
        .collect();
    rows.sort();
    let mut file = File::create(&cli.out).with_context(|| format!("writing {}", cli.out.display()))?;
    for (name, opcode) in &rows {
        writeln!(file, "{name} {opcode}")?;
    }

    let confident = assignment.values().filter(|(_, c)| *c).count();
    println!(
        "observed {} opcodes, definitions {}, assigned {} ({} confident, {} guessed)",
        observed.len(),
        registry.len(),
        assignment.len(),
        confident,
        assignment.len() - confident
    );
    println!("wrote {} entries to {}", rows.len(), cli.out.display());

    if cli.score {
        score(&observed, &assignment);
    }
    Ok(())
}

fn collect_defs<'a>(
    registry: &'a Registry,
    prefix: &str,
) -> Vec<(&'a str, &'a tera_protocol::Definition)> {
    let mut out = Vec::new();
    let mut names: Vec<&str> = registry.names().filter(|n| n.starts_with(prefix)).collect();
    names.sort();
    for name in names {
        if let Some(definition) = registry.get(name) {
            out.push((name, definition));
        }
    }
    out
}

fn solve(mut candidates: HashMap<Key, HashSet<String>>) -> HashMap<Key, (String, bool)> {
    let mut assignment: HashMap<Key, (String, bool)> = HashMap::new();
    loop {
        let mut pick: Option<(Key, String, bool)> = None;

        for (key, names) in &candidates {
            if names.len() == 1 {
                pick = Some((*key, names.iter().next().unwrap().clone(), true));
                break;
            }
        }
        if pick.is_none() {
            let mut owners: HashMap<&str, Vec<Key>> = HashMap::new();
            for (key, names) in &candidates {
                for name in names {
                    owners.entry(name.as_str()).or_default().push(*key);
                }
            }
            for (name, keys) in &owners {
                if keys.len() == 1 {
                    pick = Some((keys[0], name.to_string(), false));
                    break;
                }
            }
        }

        let Some((key, name, confident)) = pick else {
            break;
        };
        assignment.insert(key, (name.clone(), confident));
        candidates.remove(&key);
        for names in candidates.values_mut() {
            names.remove(&name);
        }
        candidates.retain(|_, names| !names.is_empty());
    }
    assignment
}

fn score(observed: &HashMap<Key, Observed>, assignment: &HashMap<Key, (String, bool)>) {
    let mut truth: HashMap<Key, String> = HashMap::new();
    for (key, data) in observed {
        if let Some((name, _)) = data.truth.iter().max_by_key(|(_, count)| **count) {
            truth.insert(*key, name.clone());
        }
    }
    let labelled: HashSet<Key> = truth.keys().copied().collect();
    let mut correct = 0usize;
    let mut wrong = 0usize;
    let mut confident_correct = 0usize;
    let mut confident_wrong = 0usize;
    let mut wrong_examples: Vec<String> = Vec::new();
    for (key, (name, confident)) in assignment {
        if let Some(expected) = truth.get(key) {
            if expected == name {
                correct += 1;
                if *confident {
                    confident_correct += 1;
                }
            } else {
                wrong += 1;
                if *confident {
                    confident_wrong += 1;
                }
                if wrong_examples.len() < 16 {
                    wrong_examples.push(format!("opcode {} guessed {name}, truth {expected}", key.opcode));
                }
            }
        }
    }
    let missed: Vec<&Key> = labelled
        .iter()
        .filter(|key| !assignment.contains_key(key))
        .collect();

    let mut missed_names: BTreeMap<String, u16> = BTreeMap::new();
    for key in &missed {
        if let Some(name) = truth.get(key) {
            missed_names.insert(name.clone(), key.opcode);
        }
    }

    println!("== score against capture ground truth ==");
    println!("  labelled opcodes in capture: {}", labelled.len());
    println!("  correct {correct}  wrong {wrong}  unmapped {}", missed.len());
    if confident_correct + confident_wrong > 0 {
        println!(
            "  confident only: {confident_correct} correct, {confident_wrong} wrong, precision {:.1}%",
            confident_correct as f64 * 100.0 / (confident_correct + confident_wrong) as f64
        );
    }
    if correct + wrong > 0 {
        println!(
            "  all assignments: precision {:.1}%  coverage {:.1}%",
            correct as f64 * 100.0 / (correct + wrong) as f64,
            correct as f64 * 100.0 / labelled.len() as f64
        );
    }
    if !wrong_examples.is_empty() {
        println!("  wrong assignments:");
        for entry in &wrong_examples {
            println!("    {entry}");
        }
    }
    if !missed_names.is_empty() {
        println!("  unmapped (first 16):");
        for (name, opcode) in missed_names.iter().take(16) {
            println!("    {opcode} {name}");
        }
    }
}
