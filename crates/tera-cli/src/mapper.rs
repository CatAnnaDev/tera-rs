use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};
use tera_mapper::{
    CompositeEntry, CompositeMapper, DirCache, PairMapper, COMPOSITE_MOD_PREFIX,
    OBJECT_REDIRECTOR_MAGIC, PKG_MAPPER_MAGIC,
};
use tera_package::package::Package;

#[derive(Subcommand)]
pub enum MapperCommand {
    Pkg(MapperArgs),
    Redirects(MapperArgs),
    Dircache(MapperArgs),
    #[command(about = "List entries of the encrypted CompositePackageMapper.dat")]
    Composite(CompositeArgs),
    #[command(about = "Install a composite mod package and register it in the mapper")]
    CompositeInstall(CompositeInstallArgs),
    #[command(about = "Remove a composite entry and restore the base shard route from the backup")]
    CompositeUninstall(CompositeUninstallArgs),
    #[command(about = "Restore the whole mapper from its .clean backup (removes every mod)")]
    CompositeRestore(CompositeRestoreArgs),
}

#[derive(Args)]
pub struct MapperArgs {
    pub file: PathBuf,
    #[arg(long, help = "Only print entries containing this text")]
    pub grep: Option<String>,
    #[arg(long, default_value_t = 40, help = "Maximum entries to print")]
    pub limit: usize,
}

#[derive(Args)]
pub struct CompositeArgs {
    #[arg(help = "Path to CompositePackageMapper.dat")]
    pub file: PathBuf,
    #[arg(long, help = "Only print entries whose object path or composite contains this text")]
    pub grep: Option<String>,
    #[arg(long, default_value_t = 40, help = "Maximum entries to print")]
    pub limit: usize,
}

#[derive(Args)]
pub struct CompositeInstallArgs {
    #[arg(long, help = "CookedPC directory that holds CompositePackageMapper.dat")]
    pub game: PathBuf,
    #[arg(help = "Mod package file to install (its sub-packages must carry OmiMapper folders)")]
    pub mod_file: PathBuf,
    #[arg(long, help = "Container filename to store it under in CookedPC (default: derived)")]
    pub name: Option<String>,
}

#[derive(Args)]
pub struct CompositeUninstallArgs {
    #[arg(long, help = "CookedPC directory that holds CompositePackageMapper.dat")]
    pub game: PathBuf,
    #[arg(help = "Composite name, or the container filename, to remove")]
    pub target: String,
    #[arg(long, help = "Also delete the container .gpk file from CookedPC")]
    pub delete_file: bool,
}

#[derive(Args)]
pub struct CompositeRestoreArgs {
    #[arg(long, help = "CookedPC directory that holds CompositePackageMapper.dat")]
    pub game: PathBuf,
}

pub fn run(command: MapperCommand) -> Result<()> {
    match command {
        MapperCommand::Pkg(args) => pairs(&args, PKG_MAPPER_MAGIC),
        MapperCommand::Redirects(args) => pairs(&args, OBJECT_REDIRECTOR_MAGIC),
        MapperCommand::Dircache(args) => dircache(&args),
        MapperCommand::Composite(args) => composite_list(&args),
        MapperCommand::CompositeInstall(args) => composite_install(&args),
        MapperCommand::CompositeUninstall(args) => composite_uninstall(&args),
        MapperCommand::CompositeRestore(args) => composite_restore(&args),
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

fn composite_list(args: &CompositeArgs) -> Result<()> {
    let mapper = CompositeMapper::read(&args.file)
        .with_context(|| format!("reading {}", args.file.display()))?;
    let needle = args.grep.as_ref().map(|value| value.to_ascii_lowercase());
    let mut shown = 0usize;
    for (filename, entry) in mapper.entries() {
        if let Some(needle) = &needle {
            if !entry.object_path.to_ascii_lowercase().contains(needle)
                && !entry.composite_name.to_ascii_lowercase().contains(needle)
            {
                continue;
            }
        }
        println!(
            "{}\t{}\t{}\toff {} size {}",
            filename, entry.composite_name, entry.object_path, entry.offset, entry.size
        );
        shown += 1;
        if shown >= args.limit {
            break;
        }
    }
    println!("-- {} entries total", mapper.len());
    Ok(())
}

fn mapper_path(game: &Path) -> PathBuf {
    game.join("CompositePackageMapper.dat")
}

fn backup_mapper(dat: &Path) -> Result<()> {
    let clean = dat.with_extension("clean");
    if !clean.exists() {
        std::fs::copy(dat, &clean).with_context(|| format!("backing up to {}", clean.display()))?;
        println!("backup -> {}", clean.display());
    }
    let bak = dat.with_extension("dat.bak");
    std::fs::copy(dat, &bak).ok();
    Ok(())
}

fn mod_entries(mod_file: &Path) -> Result<Vec<CompositeEntry>> {
    let bytes = std::fs::read(mod_file)
        .with_context(|| format!("reading {}", mod_file.display()))?;
    let mut entries = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let package = Package::parse(&bytes, offset)
            .with_context(|| format!("parsing sub-package at offset {offset}"))?;
        let span = package.span;
        if span == 0 {
            break;
        }
        let folder = package.summary.folder_name.value.clone();
        let object_path = folder
            .strip_prefix(COMPOSITE_MOD_PREFIX)
            .map(|rest| rest.to_string());
        if let Some(object_path) = object_path {
            let composite_name = object_path
                .split('.')
                .next()
                .unwrap_or(&object_path)
                .to_string();
            entries.push(CompositeEntry {
                object_path,
                composite_name,
                offset: offset as u64,
                size: span as u64,
            });
        }
        offset += span;
    }
    if entries.is_empty() {
        bail!(
            "no OmiMapper sub-packages found in {} (folder_name must start with \"{}\")",
            mod_file.display(),
            COMPOSITE_MOD_PREFIX
        );
    }
    Ok(entries)
}

fn composite_install(args: &CompositeInstallArgs) -> Result<()> {
    let dat = mapper_path(&args.game);
    if !dat.exists() {
        bail!("{} not found", dat.display());
    }
    let container = match &args.name {
        Some(name) => name.clone(),
        None => args
            .mod_file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("mod file has no usable name")?
            .to_string(),
    };
    let entries = mod_entries(&args.mod_file)?;
    backup_mapper(&dat)?;
    let destination = args.game.join(format!("{container}.gpk"));
    std::fs::copy(&args.mod_file, &destination)
        .with_context(|| format!("copying mod to {}", destination.display()))?;
    let mut mapper = CompositeMapper::read(&dat)?;
    for entry in entries {
        println!(
            "register {} -> {} off {} size {}",
            entry.composite_name, container, entry.offset, entry.size
        );
        mapper.upsert(&container, entry);
    }
    mapper.write(&dat)?;
    println!("installed {} -> {}", args.mod_file.display(), destination.display());
    Ok(())
}

fn composite_uninstall(args: &CompositeUninstallArgs) -> Result<()> {
    let dat = mapper_path(&args.game);
    let mut mapper = CompositeMapper::read(&dat)?;
    let composites: Vec<String> = if mapper.groups.iter().any(|group| group.filename == args.target)
    {
        mapper
            .groups
            .iter()
            .filter(|group| group.filename == args.target)
            .flat_map(|group| group.entries.iter().map(|entry| entry.composite_name.clone()))
            .collect()
    } else if mapper.find_composite(&args.target).is_some() {
        vec![args.target.clone()]
    } else {
        bail!("no entry matched {}", args.target);
    };
    backup_mapper(&dat)?;
    let clean = dat.with_extension("clean");
    let pristine = if clean.exists() {
        Some(CompositeMapper::read(&clean)?)
    } else {
        None
    };
    for composite in &composites {
        mapper.remove_composite(composite);
        if let Some(pristine) = &pristine {
            if let Some((filename, entry)) = pristine.find_composite(composite) {
                mapper.upsert(filename, entry.clone());
                println!("restored {} -> base shard {}", composite, filename);
            }
        }
    }
    mapper.write(&dat)?;
    if args.delete_file {
        let container = args.game.join(format!("{}.gpk", args.target));
        if container.exists() {
            std::fs::remove_file(&container).ok();
            println!("deleted {}", container.display());
        }
    }
    println!("removed {} ({} composite route(s))", args.target, composites.len());
    Ok(())
}

fn composite_restore(args: &CompositeRestoreArgs) -> Result<()> {
    let dat = mapper_path(&args.game);
    let clean = dat.with_extension("clean");
    if !clean.exists() {
        bail!("{} not found", clean.display());
    }
    std::fs::copy(&clean, &dat).with_context(|| format!("restoring {}", dat.display()))?;
    println!("restored {} from {}", dat.display(), clean.display());
    Ok(())
}
