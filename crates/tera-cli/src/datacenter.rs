use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use tera_crypto::KeyIv;
use tera_datacenter::export::{write_json, write_xml};
use tera_datacenter::{query, DataCenter, Node};

#[derive(Subcommand)]
pub enum DcCommand {
    Info(CommonArgs),
    Keys(CommonArgs),
    Sheets(CommonArgs),
    Names(CommonArgs),
    Export(ExportArgs),
    Query(QueryArgs),
    Unwrap(UnwrapArgs),
    Repack(RepackArgs),
    Verify(CommonArgs),
    Set(SetArgs),
    Pack(PackArgs),
}

#[derive(Args)]
pub struct PackArgs {
    #[arg(help = "Directory of XML files produced by `tera dc export`")]
    pub input: PathBuf,
    #[arg(long, short)]
    pub out: PathBuf,
    #[arg(long, help = "Original .dat used for attribute types, keys and name order")]
    pub template: Option<PathBuf>,
    #[arg(long, help = "AES key as 32 hex characters")]
    pub key: Option<String>,
    #[arg(long, help = "AES IV as 32 hex characters")]
    pub iv: Option<String>,
    #[arg(long, help = "Write the inflated image instead of an encrypted .dat")]
    pub plain_out: bool,
    #[arg(long, default_value_t = 6)]
    pub level: u32,
}

#[derive(Args)]
pub struct SetArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long, help = "Query selecting the nodes to edit, e.g. /ItemData/Item[@id=\"1\"]")]
    pub select: String,
    #[arg(long, value_name = "NAME=VALUE", help = "Attribute to set, repeatable")]
    pub set: Vec<String>,
    #[arg(long, value_name = "NAME", help = "Attribute to remove, repeatable")]
    pub remove: Vec<String>,
    #[arg(long, short)]
    pub out: PathBuf,
    #[arg(long, help = "Write the inflated image instead of an encrypted .dat")]
    pub plain_out: bool,
    #[arg(long, default_value_t = 6)]
    pub level: u32,
    #[arg(long, help = "Show what would change without writing anything")]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct RepackArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long, short)]
    pub out: PathBuf,
    #[arg(long, help = "Write the inflated image instead of an encrypted .dat")]
    pub plain_out: bool,
    #[arg(long, default_value_t = 6, help = "Deflate level 0-9")]
    pub level: u32,
}

#[derive(Args)]
pub struct CommonArgs {
    pub file: PathBuf,
    #[arg(long, help = "AES key as 32 hex characters")]
    pub key: Option<String>,
    #[arg(long, help = "AES IV as 32 hex characters")]
    pub iv: Option<String>,
    #[arg(long, help = "File is already decrypted and inflated")]
    pub plain: bool,
}

#[derive(Args)]
pub struct ExportArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long, short)]
    pub out: PathBuf,
    #[arg(long, value_enum, default_value_t = Format::Xml)]
    pub format: Format,
    #[arg(long, help = "Only export root children with this name")]
    pub sheet: Option<String>,
    #[arg(long, help = "Write every sheet group into one file instead of one file per node")]
    pub merge: bool,
}

#[derive(Args)]
pub struct QueryArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    pub path: String,
    #[arg(long, value_enum, default_value_t = Format::Xml)]
    pub format: Format,
    #[arg(long, default_value_t = 20, help = "Maximum number of results to print")]
    pub limit: usize,
    #[arg(long, help = "Print only the value of this attribute")]
    pub attribute: Option<String>,
}

#[derive(Args)]
pub struct UnwrapArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[arg(long, short)]
    pub out: PathBuf,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Xml,
    Json,
    Text,
}

fn load(common: &CommonArgs) -> Result<DataCenter> {
    if common.plain {
        return Ok(DataCenter::open_plain(&common.file)?);
    }
    match (&common.key, &common.iv) {
        (Some(key), Some(iv)) => {
            let keyiv = KeyIv::from_hex(key, iv)?;
            Ok(DataCenter::open_with_key(&common.file, &keyiv)?)
        }
        (None, None) => Ok(DataCenter::open(&common.file)?),
        _ => bail!("--key and --iv must be given together"),
    }
}

pub fn run(command: DcCommand) -> Result<()> {
    match command {
        DcCommand::Info(args) => info(&args),
        DcCommand::Keys(args) => keys(&args),
        DcCommand::Sheets(args) => sheets(&args),
        DcCommand::Names(args) => names(&args),
        DcCommand::Export(args) => export(&args),
        DcCommand::Query(args) => run_query(&args),
        DcCommand::Unwrap(args) => unwrap(&args),
        DcCommand::Repack(args) => repack(&args),
        DcCommand::Verify(args) => verify(&args),
        DcCommand::Set(args) => set(&args),
        DcCommand::Pack(args) => pack(&args),
    }
}

fn info(args: &CommonArgs) -> Result<()> {
    let dc = load(args)?;
    println!("file          {}", args.file.display());
    if let Some(keyiv) = &dc.keyiv {
        println!("key           {}", keyiv.key_hex());
        println!("iv            {}", keyiv.iv_hex());
    }
    println!("version       {}", dc.header.version);
    println!("revision      {}", dc.header.revision);
    println!("timestamp     {}", dc.header.timestamp);
    println!("inflated size {} bytes", dc.raw_len());
    println!("keys          {}", dc.keys.len());
    println!(
        "nodes         {} in {} segments",
        dc.node_count(),
        dc.node_segments.len()
    );
    println!(
        "attributes    {} in {} segments",
        dc.attribute_count(),
        dc.attribute_segments.len()
    );
    println!(
        "names         {} strings, {} data segments (first full {}), {} table entries",
        dc.names.addresses.len(),
        dc.names.data_segments.len(),
        dc.names.data_segments.first().map(|s| s.full).unwrap_or(0),
        dc.names.entries.len()
    );
    println!(
        "values        {} strings, {} data segments (first full {}), {} table entries",
        dc.values.addresses.len(),
        dc.values.data_segments.len(),
        dc.values.data_segments.first().map(|s| s.full).unwrap_or(0),
        dc.values.entries.len()
    );
    println!(
        "segment sizes attributes full {} used {} / nodes full {} used {}",
        dc.attribute_segments.first().map(|s| s.full).unwrap_or(0),
        dc.attribute_segments.first().map(|s| s.used).unwrap_or(0),
        dc.node_segments.first().map(|s| s.full).unwrap_or(0),
        dc.node_segments.first().map(|s| s.used).unwrap_or(0)
    );
    let root = dc.root()?;
    println!("root          {} ({} children)", root.name()?, root.child_count());
    Ok(())
}

fn keys(args: &CommonArgs) -> Result<()> {
    let dc = load(args)?;
    for (index, key) in dc.keys.iter().enumerate() {
        let names: Vec<String> = key
            .name_indexes
            .iter()
            .map(|value| match value {
                0 => "-".to_string(),
                index => dc.name(*index).unwrap_or("?").to_string(),
            })
            .collect();
        println!("{index:>3} {}", names.join(", "));
    }
    let mut per_name: BTreeMap<&str, BTreeMap<u16, u64>> = BTreeMap::new();
    let mut stack = vec![dc.root()?];
    while let Some(node) = stack.pop() {
        for child in node.children() {
            let entry = per_name.entry(child.name()?).or_default();
            *entry.entry(child.raw().key_index).or_insert(0) += 1;
            stack.push(child);
        }
    }
    let ambiguous: Vec<&&str> = per_name
        .iter()
        .filter(|(_, uses)| uses.len() > 1)
        .map(|(name, _)| name)
        .collect();
    println!(
        "\n{} distinct node names, {} of them use more than one key index",
        per_name.len(),
        ambiguous.len()
    );
    for name in ambiguous.iter().take(20) {
        println!("  {name}: {:?}", per_name[**name]);
    }
    Ok(())
}

fn sheets(args: &CommonArgs) -> Result<()> {
    let dc = load(args)?;
    let root = dc.root()?;
    let mut counts: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for child in root.children() {
        let entry = counts.entry(child.name()?).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += u64::from(child.child_count());
    }
    println!("{:<48} {:>8} {:>12}", "sheet", "nodes", "records");
    for (name, (nodes, records)) in &counts {
        println!("{name:<48} {nodes:>8} {records:>12}");
    }
    println!("\n{} distinct sheet names", counts.len());
    Ok(())
}

fn names(args: &CommonArgs) -> Result<()> {
    let dc = load(args)?;
    for (index, name) in dc.names_iter().enumerate() {
        println!("{:>6} {}", index + 1, name);
    }
    Ok(())
}

fn unwrap(args: &UnwrapArgs) -> Result<()> {
    let mut bytes = std::fs::read(&args.common.file)?;
    let keyiv = match (&args.common.key, &args.common.iv) {
        (Some(key), Some(iv)) => KeyIv::from_hex(key, iv)?,
        (None, None) => {
            tera_datacenter::detect_key(&bytes).context("no known key decrypts this file")?
        }
        _ => bail!("--key and --iv must be given together"),
    };
    tera_crypto::decrypt_in_place(&keyiv, &mut bytes);
    let inflated = tera_datacenter::inflate(&bytes)?;
    std::fs::write(&args.out, &inflated)?;
    println!(
        "wrote {} bytes to {} (key {} iv {})",
        inflated.len(),
        args.out.display(),
        keyiv.key_hex(),
        keyiv.iv_hex()
    );
    Ok(())
}

fn pack(args: &PackArgs) -> Result<()> {
    let template_dc = match &args.template {
        Some(path) => Some(DataCenter::open(path)?),
        None => None,
    };
    let template = match &template_dc {
        Some(dc) => Some(tera_datacenter::Template::from_datacenter(dc)?),
        None => None,
    };
    let mut importer = tera_datacenter::Importer::new(template.as_ref());
    let mut files = Vec::new();
    collect_xml(&args.input, &mut files)?;
    files.sort();
    if files.is_empty() {
        bail!("no .xml files under {}", args.input.display());
    }
    let mut roots = 0usize;
    for file in &files {
        roots += importer
            .read_file(file)
            .with_context(|| format!("parsing {}", file.display()))?;
    }
    println!("parsed {} file(s), {roots} sheet node(s)", files.len());
    let image = importer.builder.pack()?;
    println!("packed image: {} bytes", image.len());
    let keyiv = match (&args.key, &args.iv) {
        (Some(key), Some(iv)) => Some(KeyIv::from_hex(key, iv)?),
        _ => template_dc.as_ref().and_then(|dc| dc.keyiv),
    };
    match keyiv {
        Some(keyiv) if !args.plain_out => {
            let wrapped = tera_datacenter::wrap(&image, &keyiv, args.level)?;
            std::fs::write(&args.out, &wrapped)?;
            println!("encrypted with key {}", keyiv.key_hex());
        }
        _ => std::fs::write(&args.out, &image)?,
    }
    println!("wrote {}", args.out.display());
    Ok(())
}

fn collect_xml(directory: &PathBuf, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_xml(&path, out)?;
        } else if path.extension().map(|ext| ext == "xml").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(())
}

fn set(args: &SetArgs) -> Result<()> {
    let dc = load(&args.common)?;
    let keyiv = match (&args.common.key, &args.common.iv) {
        (Some(key), Some(iv)) => Some(KeyIv::from_hex(key, iv)?),
        _ => dc.keyiv,
    };
    let mut builder = tera_datacenter::Builder::from_datacenter(&dc)?;
    let assignments = args
        .set
        .iter()
        .map(|assignment| {
            assignment
                .split_once('=')
                .map(|(name, literal)| (name.to_string(), literal.to_string()))
                .with_context(|| format!("`{assignment}` is not NAME=VALUE"))
        })
        .collect::<Result<Vec<_>>>()?;
    let outcome = tera_datacenter::edit(
        &mut builder,
        &tera_datacenter::Edit {
            select: &args.select,
            set: &assignments,
            remove: &args.remove,
        },
    )?;
    if outcome.matched == 0 {
        bail!("`{}` matched no nodes", args.select);
    }
    println!(
        "{} node(s) matched, {} edit(s)",
        outcome.matched, outcome.edits
    );
    if args.dry_run {
        return Ok(());
    }
    let image = builder.pack()?;
    match keyiv {
        Some(keyiv) if !args.plain_out => {
            let wrapped = tera_datacenter::wrap(&image, &keyiv, args.level)?;
            std::fs::write(&args.out, &wrapped)?;
        }
        _ => std::fs::write(&args.out, &image)?,
    }
    println!("wrote {}", args.out.display());
    Ok(())
}

fn repack(args: &RepackArgs) -> Result<()> {
    let dc = load(&args.common)?;
    let builder = tera_datacenter::Builder::from_datacenter(&dc)?;
    let image = builder.pack()?;
    println!(
        "rebuilt image: {} bytes (original {} bytes)",
        image.len(),
        dc.raw_len()
    );
    if args.plain_out {
        std::fs::write(&args.out, &image)?;
    } else {
        let keyiv = match (&args.common.key, &args.common.iv) {
            (Some(key), Some(iv)) => KeyIv::from_hex(key, iv)?,
            _ => dc
                .keyiv
                .context("no key known for this file; pass --key and --iv")?,
        };
        let wrapped = tera_datacenter::wrap(&image, &keyiv, args.level)?;
        std::fs::write(&args.out, &wrapped)?;
        println!("encrypted with key {} iv {}", keyiv.key_hex(), keyiv.iv_hex());
    }
    println!("wrote {}", args.out.display());
    Ok(())
}

fn verify(args: &CommonArgs) -> Result<()> {
    let dc = load(args)?;
    let builder = tera_datacenter::Builder::from_datacenter(&dc)?;
    let image = builder.pack()?;
    let rebuilt = tera_datacenter::DataCenter::from_inflated(image)?;
    let original_sheets = sheet_summary(&dc)?;
    let rebuilt_sheets = sheet_summary(&rebuilt)?;
    println!(
        "nodes      {} -> {}",
        dc.node_count(),
        rebuilt.node_count()
    );
    println!(
        "attributes {} -> {}",
        dc.attribute_count(),
        rebuilt.attribute_count()
    );
    println!(
        "names      {} -> {}",
        dc.names.addresses.len(),
        rebuilt.names.addresses.len()
    );
    println!(
        "values     {} -> {}",
        dc.values.addresses.len(),
        rebuilt.values.addresses.len()
    );
    let mut mismatches = 0usize;
    for (name, count) in &original_sheets {
        match rebuilt_sheets.get(name) {
            Some(other) if other == count => {}
            Some(other) => {
                println!("sheet {name}: {count} -> {other}");
                mismatches += 1;
            }
            None => {
                println!("sheet {name}: missing after repack");
                mismatches += 1;
            }
        }
    }
    if mismatches == 0 && original_sheets.len() == rebuilt_sheets.len() {
        println!("all {} sheets match", original_sheets.len());
    }
    Ok(())
}

fn sheet_summary(dc: &DataCenter) -> Result<BTreeMap<String, u64>> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for child in dc.root()?.children() {
        *counts.entry(child.name()?.to_string()).or_insert(0) += u64::from(child.child_count());
    }
    Ok(counts)
}

fn export(args: &ExportArgs) -> Result<()> {
    let dc = load(&args.common)?;
    let root = dc.root()?;
    std::fs::create_dir_all(&args.out)?;
    let mut groups: BTreeMap<String, Vec<Node<'_>>> = BTreeMap::new();
    for child in root.children() {
        let name = child.name()?;
        if let Some(filter) = &args.sheet {
            if name != filter {
                continue;
            }
        }
        groups.entry(name.to_string()).or_default().push(child);
    }
    if groups.is_empty() {
        bail!("nothing to export");
    }
    let extension = match args.format {
        Format::Json => "json",
        _ => "xml",
    };
    let written: usize = groups
        .par_iter()
        .map(|(name, nodes)| -> Result<usize> {
            if args.merge {
                let path = args.out.join(format!("{name}.{extension}"));
                let file = File::create(&path)
                    .with_context(|| format!("creating {}", path.display()))?;
                let mut out = BufWriter::with_capacity(1 << 20, file);
                write_group(&mut out, name, nodes, args.format)?;
                out.flush()?;
                return Ok(1);
            }
            let directory = args.out.join(name);
            std::fs::create_dir_all(&directory)?;
            for (index, node) in nodes.iter().enumerate() {
                let path = directory.join(format!("{name}-{:05}.{extension}", index + 1));
                let file = File::create(&path)
                    .with_context(|| format!("creating {}", path.display()))?;
                let mut out = BufWriter::with_capacity(1 << 20, file);
                match args.format {
                    Format::Json => write_json(&mut out, node, true)?,
                    _ => write_xml(&mut out, node, true)?,
                }
                out.flush()?;
            }
            Ok(nodes.len())
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .sum();
    println!(
        "exported {} sheet groups ({} files) to {}",
        groups.len(),
        written,
        args.out.display()
    );
    Ok(())
}

fn write_group<W: Write>(
    out: &mut W,
    name: &str,
    nodes: &[Node<'_>],
    format: Format,
) -> Result<()> {
    match format {
        Format::Json => {
            out.write_all(b"[")?;
            for (index, node) in nodes.iter().enumerate() {
                if index > 0 {
                    out.write_all(b",")?;
                }
                write_json(out, node, true)?;
            }
            out.write_all(b"]\n")?;
        }
        _ => {
            out.write_all(b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n")?;
            writeln!(out, "<Collection name=\"{name}\">")?;
            for node in nodes {
                write_xml(out, node, false)?;
            }
            writeln!(out, "</Collection>")?;
        }
    }
    Ok(())
}

fn run_query(args: &QueryArgs) -> Result<()> {
    let dc = load(&args.common)?;
    let root = dc.root()?;
    let results = query(root, &args.path)?;
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    for node in results.iter().take(args.limit) {
        if let Some(attribute) = &args.attribute {
            match node.get(attribute) {
                Some(value) => writeln!(out, "{}", value.to_text())?,
                None => writeln!(out)?,
            }
            continue;
        }
        match args.format {
            Format::Json => write_json(&mut out, node, true)?,
            Format::Text => print_text(&mut out, node)?,
            Format::Xml => write_xml(&mut out, node, false)?,
        }
    }
    writeln!(out, "-- {} result(s)", results.len())?;
    Ok(())
}

fn print_text<W: Write>(out: &mut W, node: &Node<'_>) -> Result<()> {
    write!(out, "{}", node.name()?)?;
    for attribute in node.attributes() {
        write!(out, " {}={}", attribute.name()?, attribute.value()?.to_text())?;
    }
    writeln!(out)?;
    Ok(())
}
