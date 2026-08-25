use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};
use tera_mod::apply::Install;
use tera_mod::manifest::Manifest;

const MANIFEST: &str = "mod.toml";

#[derive(Subcommand)]
pub enum ModCommand {
    #[command(about = "Write a commented example manifest into a new folder")]
    New(NewArgs),
    #[command(about = "Read a manifest and say what it declares")]
    Show(ShowArgs),
    #[command(about = "List every file a mod would touch, changing nothing")]
    Plan(ApplyArgs),
    #[command(about = "Apply a mod, backing up every file first")]
    Apply(ApplyArgs),
    #[command(about = "Restore everything a mod replaced")]
    Revert(RevertArgs),
    #[command(about = "List the mods currently applied")]
    Status(StoreArgs),
}

#[derive(Args)]
pub struct StoreArgs {
    #[arg(long, help = "Game install root")]
    pub game: PathBuf,
    #[arg(
        long,
        help = "Where backups and receipts live, defaults to <game>/.tera-mods"
    )]
    pub store: Option<PathBuf>,
}

impl StoreArgs {
    fn install(&self) -> Install {
        let store = self
            .store
            .clone()
            .unwrap_or_else(|| self.game.join(".tera-mods"));
        Install::new(&self.game, store)
    }
}

#[derive(Args)]
pub struct NewArgs {
    #[arg(help = "Folder to create")]
    pub directory: PathBuf,
    #[arg(long, help = "Mod name, defaults to the folder name")]
    pub name: Option<String>,
}

#[derive(Args)]
pub struct ShowArgs {
    #[arg(help = "Mod folder or mod.toml")]
    pub path: PathBuf,
}

#[derive(Args)]
pub struct ApplyArgs {
    #[command(flatten)]
    pub store: StoreArgs,
    #[arg(help = "Mod folder or mod.toml")]
    pub path: PathBuf,
    #[arg(long, help = "DataCenter_Final_*.dat, needed for data_center changes")]
    pub datacenter: Option<PathBuf>,
}

#[derive(Args)]
pub struct RevertArgs {
    #[command(flatten)]
    pub store: StoreArgs,
    #[arg(help = "Name of the applied mod")]
    pub name: String,
}

fn locate(path: &Path) -> Result<(Manifest, PathBuf)> {
    let (file, directory) = if path.is_dir() {
        (path.join(MANIFEST), path.to_path_buf())
    } else {
        (
            path.to_path_buf(),
            path.parent().unwrap_or(Path::new(".")).to_path_buf(),
        )
    };
    let manifest =
        Manifest::read(&file).with_context(|| format!("reading {}", file.display()))?;
    Ok((manifest, directory))
}

fn datacenter_for(manifest: &Manifest, given: Option<&PathBuf>) -> Result<PathBuf> {
    match given {
        Some(path) => Ok(path.clone()),
        None if manifest.touches_data_center() => {
            bail!("{} edits the data center, pass --datacenter", manifest.name)
        }
        None => Ok(PathBuf::new()),
    }
}

pub fn run(command: ModCommand) -> Result<()> {
    match command {
        ModCommand::New(args) => {
            let name = args
                .name
                .clone()
                .or_else(|| {
                    args.directory
                        .file_name()
                        .map(|value| value.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| "untitled".into());
            std::fs::create_dir_all(&args.directory)?;
            let path = args.directory.join(MANIFEST);
            if path.exists() {
                bail!("{} already exists", path.display());
            }
            Manifest::example(&name).write(&path)?;
            println!("wrote {}", path.display());
            Ok(())
        }
        ModCommand::Show(args) => {
            let (manifest, directory) = locate(&args.path)?;
            println!("{} {}", manifest.name, manifest.version);
            if !manifest.author.is_empty() {
                println!("by {}", manifest.author);
            }
            if !manifest.description.is_empty() {
                println!("{}", manifest.description);
            }
            println!("{}", directory.display());
            for change in &manifest.changes {
                println!("  {}", change.summary());
            }
            Ok(())
        }
        ModCommand::Plan(args) => {
            let (manifest, _) = locate(&args.path)?;
            let datacenter = datacenter_for(&manifest, args.datacenter.as_ref())?;
            let install = args.store.install();
            for (target, summary) in install.plan(&manifest, &datacenter) {
                println!("{}\n  {summary}", target.display());
            }
            Ok(())
        }
        ModCommand::Apply(args) => {
            let (manifest, directory) = locate(&args.path)?;
            let datacenter = datacenter_for(&manifest, args.datacenter.as_ref())?;
            let install = args.store.install();
            let receipt = install.apply(&manifest, &datacenter, &directory)?;
            for touched in &receipt.applied {
                println!("{}\n  {}", touched.target.display(), touched.summary);
            }
            println!(
                "applied {} ({} file(s) touched)",
                receipt.name,
                receipt.applied.len()
            );
            Ok(())
        }
        ModCommand::Revert(args) => {
            let receipt = args.store.install().revert(&args.name)?;
            for touched in &receipt.applied {
                println!("restored {}", touched.target.display());
            }
            println!("reverted {}", receipt.name);
            Ok(())
        }
        ModCommand::Status(args) => {
            let applied = args.install().applied();
            if applied.is_empty() {
                println!("no mods applied");
                return Ok(());
            }
            for receipt in &applied {
                println!(
                    "{} {} ({} file(s))",
                    receipt.name,
                    receipt.version,
                    receipt.applied.len()
                );
            }
            Ok(())
        }
    }
}
