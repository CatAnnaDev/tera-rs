use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum ServerDataCommand {
    #[command(about = "Turn the official NpcSkillData sheets into a catalogue the server can use")]
    NpcSkills(ImportArgs),
    #[command(about = "Turn the official NpcData sheets into a catalogue of creature definitions")]
    Npcs(ImportArgs),
    #[command(about = "List the npcs that hold a fixed post, from the official VillagerData")]
    Villagers(ImportArgs),
}

#[derive(Args)]
pub struct ImportArgs {
    #[arg(help = "The server's Datasheet directory")]
    pub datasheet: PathBuf,
    #[arg(long, short, help = "Where to write the JSON catalogue")]
    pub out: PathBuf,
}

#[derive(Serialize)]
struct NpcSkill {
    zone: u32,
    npc: u32,
    id: u32,
    name: String,
    kind: String,
    range: f32,
    cool_time: u32,
    offensive: bool,
    projectile_ms: f32,
}

#[derive(Serialize)]
struct Npc {
    zone: u32,
    id: u32,
    name: String,
    race: String,
    size: String,
    elite: bool,
    ai: u32,
    scale: f32,
}

fn sheets(directory: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    named(directory, prefix, ".xml")
}

fn named(directory: &Path, prefix: &str, extension: &str) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut stack = vec![directory.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with(prefix) && name.ends_with(extension))
                .unwrap_or(false)
            {
                found.push(path);
            }
        }
    }
    if found.is_empty() {
        bail!("no {prefix}*{extension} under {}", directory.display());
    }
    found.sort();
    Ok(found)
}

#[allow(deprecated)]
fn attributes(event: &quick_xml::events::BytesStart<'_>) -> BTreeMap<String, String> {
    event
        .attributes()
        .flatten()
        .filter_map(|attribute| {
            let key = attribute.key.as_ref().to_string();
            let value = attribute.unescape_value().ok()?.into_owned();
            Some((key, value))
        })
        .collect()
}

fn number<T: std::str::FromStr + Default>(map: &BTreeMap<String, String>, key: &str) -> T {
    map.get(key)
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or_default()
}

fn flag(map: &BTreeMap<String, String>, key: &str) -> bool {
    map.get(key)
        .map(|text| text.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn text(map: &BTreeMap<String, String>, key: &str) -> String {
    map.get(key).cloned().unwrap_or_default()
}

fn npc_skills(args: &ImportArgs) -> Result<()> {
    let files = sheets(&args.datasheet, "NpcSkillData")?;
    let mut out: Vec<NpcSkill> = Vec::new();
    for path in &files {
        let mut reader = Reader::from_file(path)
            .with_context(|| format!("reading {}", path.display()))?;
        reader.config_mut().trim_text(true);
        let mut buffer = Vec::new();
        let mut zone = 0u32;
        let mut current: Option<NpcSkill> = None;
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Eof) => break,
                Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                    let map = attributes(&event);
                    match event.name().as_ref() {
                        "SkillData" => zone = number(&map, "huntingZoneId"),
                        "Skill" => {
                            if let Some(skill) = current.take() {
                                out.push(skill);
                            }
                            current = Some(NpcSkill {
                                zone,
                                npc: number(&map, "templateId"),
                                id: number(&map, "id"),
                                name: text(&map, "name"),
                                kind: text(&map, "type"),
                                range: number(&map, "attackRange"),
                                cool_time: 0,
                                offensive: false,
                                projectile_ms: 0.0,
                            });
                        }
                        "Precondition" => {
                            if let Some(skill) = current.as_mut() {
                                skill.cool_time = number(&map, "coolTime");
                            }
                        }
                        "Aggro" => {
                            if let Some(skill) = current.as_mut() {
                                skill.offensive = flag(&map, "offensiveSkill");
                            }
                        }
                        "Bullet" => {
                            if let Some(skill) = current.as_mut() {
                                skill.projectile_ms = number(&map, "flyingDuration");
                            }
                        }
                        _ => {}
                    }
                }
                Err(error) => bail!("{}: {error}", path.display()),
                _ => {}
            }
            buffer.clear();
        }
        if let Some(skill) = current.take() {
            out.push(skill);
        }
    }
    let zones: std::collections::BTreeSet<u32> = out.iter().map(|skill| skill.zone).collect();
    let npcs: std::collections::BTreeSet<(u32, u32)> =
        out.iter().map(|skill| (skill.zone, skill.npc)).collect();
    std::fs::write(&args.out, serde_json::to_vec(&out)?)?;
    println!(
        "{} skills for {} creatures across {} zones, from {} sheets -> {}",
        out.len(),
        npcs.len(),
        zones.len(),
        files.len(),
        args.out.display()
    );
    Ok(())
}

fn npcs(args: &ImportArgs) -> Result<()> {
    let files = sheets(&args.datasheet, "NpcData")?;
    let mut out: Vec<Npc> = Vec::new();
    for path in &files {
        let mut reader = Reader::from_file(path)
            .with_context(|| format!("reading {}", path.display()))?;
        reader.config_mut().trim_text(true);
        let mut buffer = Vec::new();
        let mut zone = 0u32;
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Eof) => break,
                Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                    let map = attributes(&event);
                    match event.name().as_ref() {
                        "NpcData" => zone = number(&map, "huntingZoneId"),
                        "Template" => out.push(Npc {
                            zone,
                            id: number(&map, "id"),
                            name: text(&map, "name"),
                            race: text(&map, "race"),
                            size: text(&map, "size"),
                            elite: flag(&map, "elite"),
                            ai: number(&map, "aiid"),
                            scale: number(&map, "scale"),
                        }),
                        _ => {}
                    }
                }
                Err(error) => bail!("{}: {error}", path.display()),
                _ => {}
            }
            buffer.clear();
        }
    }
    let zones: std::collections::BTreeSet<u32> = out.iter().map(|npc| npc.zone).collect();
    std::fs::write(&args.out, serde_json::to_vec(&out)?)?;
    println!(
        "{} creatures across {} zones, from {} sheets -> {}",
        out.len(),
        zones.len(),
        files.len(),
        args.out.display()
    );
    Ok(())
}

fn villagers(args: &ImportArgs) -> Result<()> {
    let files = named(&args.datasheet, "", ".condition")?;
    let mut posts: std::collections::BTreeSet<(i64, i64)> = std::collections::BTreeSet::new();
    let mut unplaced = 0;
    for path in &files {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut reader = Reader::from_str(&text);
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Eof) => break,
                Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                    if event.name().as_ref() != "Villager" {
                        continue;
                    }
                    let map = attributes(&event);
                    let Some(zone) = map.get("huntingZoneId").and_then(|v| v.trim().parse().ok())
                    else {
                        unplaced += 1;
                        continue;
                    };
                    posts.insert((zone, number::<i64>(&map, "id")));
                }
                Ok(_) => {}
                Err(error) => bail!("{}: {error}", path.display()),
            }
            buffer.clear();
        }
    }
    let rows: Vec<[i64; 2]> = posts.iter().map(|(zone, id)| [*zone, *id]).collect();
    std::fs::write(&args.out, serde_json::to_vec(&rows)?)
        .with_context(|| format!("writing {}", args.out.display()))?;
    let zones = posts.iter().map(|(zone, _)| *zone).collect::<std::collections::BTreeSet<_>>();
    println!(
        "{} fixed posts across {} hunting zones, from {} files -> {}{}",
        rows.len(),
        zones.len(),
        files.len(),
        args.out.display(),
        match unplaced {
            0 => String::new(),
            n => format!(" ({n} entries carried no hunting zone and were left out)"),
        }
    );
    Ok(())
}

pub fn run(command: ServerDataCommand) -> Result<()> {
    match command {
        ServerDataCommand::NpcSkills(args) => npc_skills(&args),
        ServerDataCommand::Npcs(args) => npcs(&args),
        ServerDataCommand::Villagers(args) => villagers(&args),
    }
}
