use std::io::Read;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use tera_protocol::{value, OpcodeMap, PacketBuffer, Registry};

pub const HOOKS: [&str; 17] = [
    "S_EACH_SKILL_RESULT",
    "S_SPAWN_USER",
    "S_SPAWN_NPC",
    "S_DESPAWN_NPC",
    "S_LOGIN",
    "S_BOSS_GAGE_INFO",
    "S_CREATURE_CHANGE_HP",
    "S_ABNORMALITY_BEGIN",
    "S_ABNORMALITY_REFRESH",
    "S_ABNORMALITY_END",
    "S_PARTY_MEMBER_LIST",
    "S_NPC_STATUS",
    "S_ACTION_STAGE",
    "S_CREATURE_LIFE",
    "S_PARTY_MEMBER_CHANGE_HP",
    "S_PARTY_MEMBER_CHANGE_MP",
    "S_PARTY_MEMBER_STAT_UPDATE",
];

pub enum Event {
    Me { game_id: u64, name: String, template_id: u64 },
    Player { game_id: u64, name: String, template_id: u64, server_id: u64, player_id: u64 },
    Npc { game_id: u64, template_id: u64, hunting_zone: u64, max_hp: i64 },
    Despawn { game_id: u64, dead: bool },
    Hit { source: u64, target: u64, value: i64, kind: u8, crit: bool, skill: u32 },
    BossGage { id: u64, cur_hp: i64, max_hp: i64 },
    Hp { target: u64, cur_hp: i64, max_hp: i64 },
    AbnBegin { target: u64, id: u32 },
    AbnEnd { target: u64, id: u32 },
    Party { members: Vec<(u64, u64, u32)> },
    NpcStatus { game_id: u64, enraged: bool, remaining_enrage_ms: i64, target: u64 },
    Cast { source: u64, skill: u32 },
    Life { game_id: u64, alive: bool },
    PartyHp { server_id: u64, player_id: u64, cur_hp: i64, max_hp: i64 },
    PartyMp { server_id: u64, player_id: u64, cur_mp: i64, max_mp: i64 },
    PartyStat { server_id: u64, player_id: u64, hp: i64, max_hp: i64, mp: i64, max_mp: i64, alive: bool },
    Disconnected,
}

#[derive(Clone)]
pub struct Config {
    pub data: String,
    pub control: String,
    pub opcodes: PathBuf,
    pub definitions: PathBuf,
    pub patch: u32,
    pub dump: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data: "127.0.0.60:5301".into(),
            control: "http://127.0.0.61:5300".into(),
            opcodes: "data/opcodes/protocol.376012.map".into(),
            definitions: "data/definitions.classicplus".into(),
            patch: 100,
            dump: None,
        }
    }
}

fn request_hooks(control: &str, hooks: &[&str]) -> Result<()> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "addHooks",
        "params": { "hooks": hooks },
        "id": 1
    });
    ureq::post(control)
        .send_json(&body)
        .context("addHooks vers le control-interface (external-interface actif ?)")?;
    Ok(())
}

fn u(obj: &value::Object, name: &str) -> Option<u64> {
    obj.get(name).and_then(|v| v.as_uint())
}

fn i(obj: &value::Object, name: &str) -> Option<i64> {
    obj.get(name).and_then(|v| v.as_int())
}

fn s(obj: &value::Object, name: &str) -> Option<String> {
    obj.get(name).and_then(|v| v.as_str()).map(str::to_owned)
}

pub fn run(cfg: Config, tx: Sender<Event>) -> Result<()> {
    let opcodes = OpcodeMap::read(&cfg.opcodes)
        .with_context(|| format!("lecture des opcodes {}", cfg.opcodes.display()))?;
    let registry = Registry::load(&[cfg.definitions.clone()], Some(cfg.patch))
        .context("chargement des definitions")?;

    let mut dumper = cfg.dump.as_deref().map(crate::dump::Dumper::new);
    let hook_names: Vec<&str> = if dumper.is_some() {
        opcodes.names().map(|(n, _)| n).collect()
    } else {
        HOOKS.to_vec()
    };
    request_hooks(&cfg.control, &hook_names)?;

    let mut stream = TcpStream::connect(&cfg.data)
        .with_context(|| format!("connexion au data-interface {}", cfg.data))?;
    let mut buffer = PacketBuffer::new();
    let mut chunk = [0u8; 32768];

    loop {
        let n = stream.read(&mut chunk).context("lecture du flux data")?;
        if n == 0 {
            if let Some(d) = dumper.as_mut() {
                d.summary(&opcodes);
            }
            let _ = tx.send(Event::Disconnected);
            break;
        }
        buffer.push(&chunk[..n]);
        while let Some(packet) = buffer.take_packet() {
            let name = opcodes.name(packet.opcode);
            if let Some(d) = dumper.as_mut() {
                d.record(packet.opcode, name, &packet.body, &registry);
            }
            let Some(name) = name else { continue };
            if !HOOKS.contains(&name) {
                continue;
            }
            let Some(def) = registry.get(name) else { continue };
            let Ok(obj) = value::read(def, &packet.encode()) else { continue };
            if let Some(event) = object_to_event(name, &obj) {
                if tx.send(event).is_err() {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

pub fn object_to_event(name: &str, obj: &value::Object) -> Option<Event> {
    match name {
        "S_LOGIN" => u(obj, "gameId").map(|game_id| Event::Me {
            game_id,
            name: s(obj, "name").unwrap_or_default(),
            template_id: u(obj, "templateId").unwrap_or(0),
        }),
        "S_SPAWN_USER" => match (u(obj, "gameId"), s(obj, "name"), u(obj, "templateId")) {
            (Some(game_id), Some(name), Some(template_id)) => Some(Event::Player {
                game_id,
                name,
                template_id,
                server_id: u(obj, "serverId").unwrap_or(0),
                player_id: u(obj, "playerId").unwrap_or(0),
            }),
            _ => None,
        },
        "S_SPAWN_NPC" => match (u(obj, "gameId"), u(obj, "templateId")) {
            (Some(game_id), Some(template_id)) => Some(Event::Npc {
                game_id,
                template_id,
                hunting_zone: u(obj, "huntingZoneId").unwrap_or(0),
                max_hp: i(obj, "maxHp").unwrap_or(0),
            }),
            _ => None,
        },
        "S_DESPAWN_NPC" => u(obj, "gameId").map(|game_id| Event::Despawn {
            game_id,
            dead: u(obj, "type") == Some(5),
        }),
        "S_BOSS_GAGE_INFO" => u(obj, "id").map(|id| Event::BossGage {
            id,
            cur_hp: i(obj, "curHp").unwrap_or(0),
            max_hp: i(obj, "maxHp").unwrap_or(0),
        }),
        "S_CREATURE_CHANGE_HP" => u(obj, "target").map(|target| Event::Hp {
            target,
            cur_hp: i(obj, "curHp").unwrap_or(0),
            max_hp: i(obj, "maxHp").unwrap_or(0),
        }),
        "S_ABNORMALITY_BEGIN" | "S_ABNORMALITY_REFRESH" => match (u(obj, "target"), u(obj, "id")) {
            (Some(target), Some(id)) => Some(Event::AbnBegin { target, id: id as u32 }),
            _ => None,
        },
        "S_ABNORMALITY_END" => match (u(obj, "target"), u(obj, "id")) {
            (Some(target), Some(id)) => Some(Event::AbnEnd { target, id: id as u32 }),
            _ => None,
        },
        "S_PARTY_MEMBER_LIST" => {
            let members = match obj.get("members") {
                Some(value::Value::Array(rows)) => rows
                    .iter()
                    .filter_map(|m| Some((u(m, "serverId")?, u(m, "playerId")?, u(m, "class")? as u32)))
                    .collect(),
                _ => Vec::new(),
            };
            Some(Event::Party { members })
        }
        "S_CREATURE_LIFE" => u(obj, "gameId").map(|game_id| Event::Life {
            game_id,
            alive: u(obj, "alive") == Some(1),
        }),
        "S_PARTY_MEMBER_CHANGE_HP" => match (u(obj, "serverId"), u(obj, "playerId")) {
            (Some(server_id), Some(player_id)) => Some(Event::PartyHp {
                server_id,
                player_id,
                cur_hp: i(obj, "currentHp").unwrap_or(0),
                max_hp: i(obj, "maxHp").unwrap_or(0),
            }),
            _ => None,
        },
        "S_PARTY_MEMBER_CHANGE_MP" => match (u(obj, "serverId"), u(obj, "playerId")) {
            (Some(server_id), Some(player_id)) => Some(Event::PartyMp {
                server_id,
                player_id,
                cur_mp: i(obj, "currentMp").unwrap_or(0),
                max_mp: i(obj, "maxMp").unwrap_or(0),
            }),
            _ => None,
        },
        "S_PARTY_MEMBER_STAT_UPDATE" => match (u(obj, "serverId"), u(obj, "playerId")) {
            (Some(server_id), Some(player_id)) => Some(Event::PartyStat {
                server_id,
                player_id,
                hp: i(obj, "hp").unwrap_or(0),
                max_hp: i(obj, "maxHp").unwrap_or(0),
                mp: i(obj, "mp").unwrap_or(0),
                max_mp: i(obj, "maxMp").unwrap_or(0),
                alive: u(obj, "alive") == Some(1),
            }),
            _ => None,
        },
        "S_NPC_STATUS" => u(obj, "gameId").map(|game_id| Event::NpcStatus {
            game_id,
            enraged: u(obj, "enraged") == Some(1),
            remaining_enrage_ms: i(obj, "remainingEnrageTime").unwrap_or(0),
            target: u(obj, "target").unwrap_or(0),
        }),
        "S_ACTION_STAGE" => {
            if i(obj, "stage") == Some(0) {
                u(obj, "gameId").map(|source| Event::Cast {
                    source,
                    skill: value::SkillId::from_raw(u(obj, "skill").unwrap_or(0)).id,
                })
            } else {
                None
            }
        }
        "S_EACH_SKILL_RESULT" => {
            let source = u(obj, "source").unwrap_or(0);
            let owner = u(obj, "owner").unwrap_or(0);
            let attacker = if owner != 0 { owner } else { source };
            Some(Event::Hit {
                source: attacker,
                target: u(obj, "target").unwrap_or(0),
                value: i(obj, "value").unwrap_or(0),
                kind: u(obj, "type").unwrap_or(0) as u8,
                crit: u(obj, "crit") == Some(1),
                skill: value::SkillId::from_raw(u(obj, "skill").unwrap_or(0)).id,
            })
        }
        _ => None,
    }
}
