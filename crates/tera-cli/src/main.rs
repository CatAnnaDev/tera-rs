mod datacenter;
mod gpk;
mod index;
mod keyfind;
mod mapper;
mod mods;
#[cfg(target_os = "macos")]
mod macos_memory;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tera", about = "TERA client toolkit", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(subcommand, about = "Read, query and export DataCenter_Final_*.dat")]
    Dc(datacenter::DcCommand),
    #[command(about = "Recover the DataCenter AES key and IV by scanning binaries or dumps")]
    Keyfind(keyfind::KeyfindArgs),
    #[command(subcommand, about = "Inspect and extract CookedPC .gpk packages")]
    Gpk(gpk::GpkCommand),
    #[command(subcommand, about = "Read PkgMapper.re, ObjectRedirectorMapper.re and DirCache.re")]
    Mapper(mapper::MapperCommand),
    #[command(subcommand, about = "Build and search the binary asset index")]
    Index(index::IndexCommand),
    #[command(subcommand, name = "mod", about = "Author, apply and revert mods")]
    Mod(mods::ModCommand),
}

fn main() -> Result<()> {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    match Cli::parse().command {
        Command::Dc(command) => datacenter::run(command),
        Command::Keyfind(args) => keyfind::run(args),
        Command::Gpk(command) => gpk::run(command),
        Command::Mapper(command) => mapper::run(command),
        Command::Index(command) => index::run(command),
        Command::Mod(command) => mods::run(command),
    }
}
