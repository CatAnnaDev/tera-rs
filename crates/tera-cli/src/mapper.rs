use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;
use tera_mapper::{DirCache, PairMapper, OBJECT_REDIRECTOR_MAGIC, PKG_MAPPER_MAGIC};

#[derive(Subcommand)]
pub enum MapperCommand {
    Pkg(MapperArgs),
    Redirects(MapperArgs),
    Dircache(MapperArgs),
}

#[derive(Args)]
pub struct MapperArgs {
    pub file: PathBuf,
    #[arg(long, help = "Only print entries containing this text")]
    pub grep: Option<String>,
    #[arg(long, default_value_t = 40, help = "Maximum entries to print")]
    pub limit: usize,
}

pub fn run(command: MapperCommand) -> Result<()> {
    match command {
        MapperCommand::Pkg(args) => pairs(&args, PKG_MAPPER_MAGIC),
        MapperCommand::Redirects(args) => pairs(&args, OBJECT_REDIRECTOR_MAGIC),
        MapperCommand::Dircache(args) => dircache(&args),
    }
}

fn pairs(args: &MapperArgs, magic: u32) -> Result<()> {
    let mapper = PairMapper::read(&args.file, magic)?;
    let mut shown = 0usize;
    for (key, value) in &mapper.entries {
        if let Some(needle) = &args.grep {
            let needle = needle.to_ascii_lowercase();
            if !key.to_ascii_lowercase().contains(&needle)
                && !value.to_ascii_lowercase().contains(&needle)
            {
                continue;
            }
        }
        println!("{key}\t{value}");
        shown += 1;
        if shown >= args.limit {
            break;
        }
    }
    println!("-- {} entries total", mapper.entries.len());
    Ok(())
}

fn dircache(args: &MapperArgs) -> Result<()> {
    let cache = DirCache::read(&args.file)?;
    let mut shown = 0usize;
    for entry in &cache.entries {
        if let Some(needle) = &args.grep {
            if !entry.to_ascii_lowercase().contains(&needle.to_ascii_lowercase()) {
                continue;
            }
        }
        println!("{entry}");
        shown += 1;
        if shown >= args.limit {
            break;
        }
    }
    println!("-- {} entries total", cache.entries.len());
    Ok(())
}
