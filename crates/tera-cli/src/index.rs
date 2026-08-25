use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Subcommand)]
pub enum IndexCommand {
    Build(BuildArgs),
    Info(InfoArgs),
    Search(SearchArgs),
}

#[derive(Args)]
pub struct BuildArgs {
    #[arg(help = "CookedPC directory")]
    pub root: PathBuf,
    #[arg(long, short)]
    pub out: PathBuf,
    #[arg(long, help = "Also index every object inside every package")]
    pub objects: bool,
}

#[derive(Args)]
pub struct InfoArgs {
    pub index: PathBuf,
}

#[derive(Args)]
pub struct SearchArgs {
    pub index: PathBuf,
    pub needle: String,
    #[arg(long, help = "Search objects instead of packages")]
    pub objects: bool,
    #[arg(long, help = "Only objects of this class")]
    pub class: Option<String>,
    #[arg(long, default_value_t = 30)]
    pub limit: usize,
}

pub fn run(command: IndexCommand) -> Result<()> {
    match command {
        IndexCommand::Build(args) => build(&args),
        IndexCommand::Info(args) => info(&args),
        IndexCommand::Search(args) => search(&args),
    }
}

fn build(args: &BuildArgs) -> Result<()> {
    let started = Instant::now();
    let data = tera_index::build(&args.root, args.objects, |done, total| {
        if done == total {
            println!("scanned {done}/{total} files");
        }
    })?;
    println!(
        "{} files, {} packages, {} objects in {:.1}s",
        data.files.len(),
        data.packages.len(),
        data.objects.len(),
        started.elapsed().as_secs_f32()
    );
    data.write(&args.out)?;
    let size = std::fs::metadata(&args.out)?.len();
    println!(
        "wrote {} ({:.1} MiB) in {:.1}s total",
        args.out.display(),
        size as f64 / (1024.0 * 1024.0),
        started.elapsed().as_secs_f32()
    );
    Ok(())
}

fn info(args: &InfoArgs) -> Result<()> {
    let started = Instant::now();
    let index = tera_index::Index::open(&args.index)?;
    println!(
        "{} files, {} packages, {} objects, {:.1} MiB mapped in {:.2}ms",
        index.file_count(),
        index.package_count(),
        index.object_count(),
        index.byte_size() as f64 / (1024.0 * 1024.0),
        started.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}

fn search(args: &SearchArgs) -> Result<()> {
    let index = tera_index::Index::open(&args.index)?;
    let started = Instant::now();
    if args.objects {
        if index.object_count() == 0 {
            bail!("this index has no object table; rebuild it with --objects");
        }
        let hits = index.search_objects(&args.needle, args.limit, args.class.as_deref());
        let elapsed = started.elapsed();
        for hit in &hits {
            let object = index.object(*hit as usize);
            println!(
                "{:<28} {:<64} {}",
                index.object_class(*hit as usize),
                index.object_name(*hit as usize),
                index.file_name(index.package(object.package as usize).file as usize)
            );
        }
        println!(
            "-- {} hits over {} objects in {:.1}ms",
            hits.len(),
            index.object_count(),
            elapsed.as_secs_f64() * 1000.0
        );
    } else {
        let hits = index.search_packages(&args.needle, args.limit);
        let elapsed = started.elapsed();
        for hit in &hits {
            let entry = index.package(*hit as usize);
            println!(
                "{:<40} {:<44} @{} ({} exports)",
                index.package_name(*hit as usize),
                index.file_name(entry.file as usize),
                entry.offset,
                entry.exports
            );
        }
        println!(
            "-- {} hits over {} packages in {:.1}ms",
            hits.len(),
            index.package_count(),
            elapsed.as_secs_f64() * 1000.0
        );
    }
    Ok(())
}
