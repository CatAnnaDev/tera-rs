use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::path::PathBuf;
use tera_mapper::{
    install_mod, restore_mapper, uninstall_mod, CompositeMapper, DirCache, PairMapper,
    OBJECT_REDIRECTOR_MAGIC, PKG_MAPPER_MAGIC,
};

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

fn composite_install(args: &CompositeInstallArgs) -> Result<()> {
    let report = install_mod(&args.game, &args.mod_file, args.name.as_deref())?;
    if let Some(backup) = &report.backup {
        println!("backup -> {}", backup.display());
    }
    for entry in &report.registered {
        println!(
            "register {} -> {} off {} size {}",
            entry.composite_name, report.container, entry.offset, entry.size
        );
    }
    println!(
        "installed {} -> {}",
        args.mod_file.display(),
        report.destination.display()
    );
    Ok(())
}

fn composite_uninstall(args: &CompositeUninstallArgs) -> Result<()> {
    let report = uninstall_mod(&args.game, &args.target, args.delete_file)?;
    for (composite, filename) in &report.restored {
        println!("restored {composite} -> base shard {filename}");
    }
    if let Some(deleted) = &report.deleted_file {
        println!("deleted {}", deleted.display());
    }
    println!(
        "removed {} ({} composite route(s))",
        report.target,
        report.removed.len()
    );
    Ok(())
}

fn composite_restore(args: &CompositeRestoreArgs) -> Result<()> {
    let dat = restore_mapper(&args.game)?;
    println!("restored {} from its .clean backup", dat.display());
    Ok(())
}
