use tera_server::{items, log, npcs, realm, registry, responses, session, skills, spawns, world, worlds};

use anyhow::{Context, Result};
use clap::Parser;
use std::net::TcpListener;
use std::path::PathBuf;
use tera_protocol::OpcodeMap;

#[derive(Parser)]
#[command(name = "tera-serverd", about = "Local TERA test server", version)]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:10001")]
    bind: String,
    #[arg(long, help = "protocol.<revision>.map for the client build")]
    opcodes: PathBuf,
    #[arg(long, help = "Dump packet bodies as hex")]
    hex: bool,
    #[arg(long, help = "Use the pre-45 key shift constants")]
    legacy: bool,
    #[arg(
        long,
        default_values = ["data/definitions"],
        help = "Folders of .def files"
    )]
    definitions: Vec<PathBuf>,
    #[arg(
        long,
        default_value_t = 100,
        help = "Client major patch version, used to pick the right .def version"
    )]
    patch_version: u32,
    #[arg(
        long,
        default_value = "data/world.db",
        help = "SQLite file holding accounts, characters, equipment and skills"
    )]
    database: PathBuf,
    #[arg(long, default_value = "35171", help = "Account the characters belong to")]
    account: String,
    #[arg(
        long,
        default_value = "data/worlds.json",
        help = "Table of world positions exported from the data center"
    )]
    worlds: PathBuf,
    #[arg(
        long,
        default_value = "data/npcs.json",
        help = "Table of npc template ids and their hunting zones"
    )]
    npcs: PathBuf,
    #[arg(
        long,
        default_value = "data/items.json",
        help = "Table of item ids and names"
    )]
    items: PathBuf,
    #[arg(
        long,
        default_value = "data/skills.json",
        help = "Table of skill ids by class"
    )]
    skills: PathBuf,
    #[arg(
        long,
        default_value = "data/spawns.json",
        help = "Creature placements extracted from the data center's TerritoryData"
    )]
    spawns: PathBuf,
    #[arg(
        long,
        default_value = "data/responses.json",
        help = "Table of extra replies, keyed by request packet name"
    )]
    responses: PathBuf,
    #[arg(
        long,
        help = "Answer any unhandled C_X with a default-filled S_X when both are known"
    )]
    auto_reply: bool,
    #[arg(
        long,
        help = "Also answer through request/reply naming rules, which some packets dislike"
    )]
    auto_reply_aliases: bool,
    #[arg(
        long,
        value_parser = parse_pin,
        help = "Force one packet to a definition version, as NAME=VERSION, repeatable"
    )]
    pin: Vec<(String, u32)>,
}

fn parse_pin(value: &str) -> Result<(String, u32), String> {
    let (name, version) = value
        .split_once('=')
        .ok_or_else(|| format!("expected NAME=VERSION, got `{value}`"))?;
    let version = version
        .parse()
        .map_err(|_| format!("`{version}` is not a version number"))?;
    Ok((name.to_string(), version))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let opcodes = OpcodeMap::read(&cli.opcodes)
        .with_context(|| format!("reading {}", cli.opcodes.display()))?;
    let logger = log::Log::new(cli.hex);
    logger.line(format!(
        "{} opcodes loaded{}",
        opcodes.len(),
        opcodes
            .revision
            .map(|value| format!(" (revision {value})"))
            .unwrap_or_default()
    ));

    let pins: std::collections::HashMap<String, u32> = cli.pin.iter().cloned().collect();
    let registry =
        registry::Registry::pinned(&cli.definitions, Some(cli.patch_version), &pins)?;
    for (name, version) in &pins {
        logger.line(format!("pinned {name} to version {version}"));
    }
    logger.line(format!(
        "{} packet definitions loaded for major patch {}{}",
        registry.len(),
        cli.patch_version,
        match registry.skipped() {
            0 => String::new(),
            count => format!(", {count} unreadable"),
        }
    ));

    let table = responses::Responses::load(&cli.responses)?;
    logger.line(format!(
        "{} scripted responses loaded from {}",
        table.len(),
        cli.responses.display()
    ));
    let places = worlds::Worlds::load(&cli.worlds)?;
    logger.line(format!(
        "{} world positions loaded from {}",
        places.len(),
        cli.worlds.display()
    ));

    let creatures = npcs::Npcs::load(&cli.npcs)?;
    logger.line(format!(
        "{} npcs loaded from {}",
        creatures.len(),
        cli.npcs.display()
    ));

    let catalogue = items::Items::load(&cli.items)?;
    logger.line(format!(
        "{} items loaded from {}",
        catalogue.len(),
        cli.items.display()
    ));

    let abilities = skills::Skills::load(&cli.skills)?;
    logger.line(format!(
        "{} skills loaded from {}",
        abilities.len(),
        cli.skills.display()
    ));

    let world = world::World::open(&cli.database, &cli.account)?;
    logger.line(format!(
        "{} characters for account {} in {}",
        world.characters().len(),
        cli.account,
        cli.database.display()
    ));
    let placements = spawns::Spawns::load(&cli.spawns)?;
    logger.line(format!(
        "{} creature placements loaded from {} ({} without a continent, skipped)",
        placements.placed(),
        cli.spawns.display(),
        placements.len() - placements.placed()
    ));

    let realm = realm::Realm::default();
    let server = session::Server {
        opcodes: &opcodes,
        registry: &registry,
        logger: &logger,
        world: &world,
        worlds: &places,
        npcs: &creatures,
        items: &catalogue,
        skills: &abilities,
        realm: &realm,
        spawns: &placements,
        responses: &table,
        auto_reply: cli.auto_reply,
        auto_reply_aliases: cli.auto_reply_aliases,
    };

    let listener = TcpListener::bind(&cli.bind).with_context(|| format!("binding {}", cli.bind))?;
    logger.line(format!("listening on {}", cli.bind));

    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                logger.line(format!("accept failed: {error}"));
                continue;
            }
        };
        let peer = stream
            .peer_addr()
            .map(|address| address.to_string())
            .unwrap_or_else(|_| "?".into());
        logger.line(format!("client connected from {peer}"));
        if let Err(error) = session::serve(stream, &server, cli.legacy) {
            logger.line(format!("session ended: {error}"));
        } else {
            logger.line("session closed");
        }
    }
    Ok(())
}
