use anyhow::{bail, Context, Result};
use clap::Args;
use memmap2::Mmap;
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use tera_crypto::{scan_bytes, Candidate, KeyIv, ScanMode, ZlibOracle, ORACLE_PREFIX_LEN};

#[derive(Args)]
pub struct KeyfindArgs {
    #[arg(long, help = "Encrypted DataCenter_Final_*.dat used as the oracle")]
    pub data: PathBuf,
    #[arg(help = "Files, directories or raw memory dumps to scan")]
    pub targets: Vec<PathBuf>,
    #[arg(long, help = "Scan the memory of a running process (macOS, needs sudo)")]
    pub pid: Option<i32>,
    #[arg(
        long,
        default_value = "adjacent",
        help = "adjacent (key and iv next to each other), window (iv within --radius of the key) or exhaustive"
    )]
    pub mode: String,
    #[arg(long, default_value_t = 256, help = "Search radius in bytes for window mode")]
    pub radius: usize,
    #[arg(long, default_value_t = 1, help = "Candidate offset alignment")]
    pub align: usize,
    #[arg(long, help = "Report unverified quick-test hits as well")]
    pub loose: bool,
    #[arg(long, help = "Also try every key in the built-in table first")]
    pub known: bool,
}

pub fn run(args: KeyfindArgs) -> Result<()> {
    let encrypted = File::open(&args.data)
        .with_context(|| format!("opening {}", args.data.display()))?;
    let length = encrypted.metadata()?.len();
    let map = unsafe { Mmap::map(&encrypted)? };
    let prefix_len = map.len().min(ORACLE_PREFIX_LEN);
    let oracle = ZlibOracle::new(&map[..prefix_len], length);

    if args.known {
        for known in tera_crypto::known_keys() {
            let keyiv = known.keyiv();
            if oracle.verify(&keyiv) {
                println!("built-in key matches: {}", known.label);
                report(&keyiv, "built-in table", 0, 0, true);
                return Ok(());
            }
        }
        println!("no built-in key matched, scanning");
    }

    let mode = match args.mode.as_str() {
        "adjacent" => ScanMode::Adjacent,
        "window" => ScanMode::Window(args.radius),
        "exhaustive" => ScanMode::Exhaustive,
        other => bail!("unknown scan mode `{other}`"),
    };

    let mut seen = HashSet::new();
    let mut found = 0usize;

    if let Some(pid) = args.pid {
        for (base, chunk) in read_process_memory(pid)? {
            found += scan_chunk(
                &format!("pid {pid} @ {base:#x}"),
                &chunk,
                &oracle,
                mode,
                args.align,
                args.loose,
                &mut seen,
            );
        }
    }

    for target in &args.targets {
        for path in collect_files(target)? {
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(_) => continue,
            };
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            if metadata.len() < 32 {
                continue;
            }
            let Ok(map) = (unsafe { Mmap::map(&file) }) else {
                continue;
            };
            found += scan_chunk(
                &path.display().to_string(),
                &map,
                &oracle,
                mode,
                args.align,
                args.loose,
                &mut seen,
            );
        }
    }

    if found == 0 {
        println!("no key/iv pair found");
        println!("hint: the retail client is packed, so scan a memory dump of a running TERA.exe");
    }
    Ok(())
}

fn scan_chunk(
    source: &str,
    haystack: &[u8],
    oracle: &ZlibOracle,
    mode: ScanMode,
    align: usize,
    loose: bool,
    seen: &mut HashSet<(String, String)>,
) -> usize {
    let candidates: Vec<Candidate> = scan_bytes(haystack, oracle, mode, align);
    let mut count = 0;
    for candidate in candidates {
        if !candidate.verified && !loose {
            continue;
        }
        let identity = (candidate.keyiv.key_hex(), candidate.keyiv.iv_hex());
        if !seen.insert(identity) {
            continue;
        }
        report(
            &candidate.keyiv,
            source,
            candidate.key_offset,
            candidate.iv_offset,
            candidate.verified,
        );
        count += 1;
    }
    count
}

fn report(keyiv: &KeyIv, source: &str, key_offset: usize, iv_offset: usize, verified: bool) {
    println!(
        "{} key {} iv {}  [{} key@{:#x} iv@{:#x}]",
        if verified { "VERIFIED" } else { "candidate" },
        keyiv.key_hex(),
        keyiv.iv_hex(),
        source,
        key_offset,
        iv_offset
    );
}

fn collect_files(target: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if target.is_file() {
        out.push(target.to_path_buf());
        return Ok(out);
    }
    let mut stack = vec![target.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(kind) if kind.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    Ok(out)
}

#[cfg(target_os = "macos")]
fn read_process_memory(pid: i32) -> Result<Vec<(u64, Vec<u8>)>> {
    crate::macos_memory::read_all(pid)
}

#[cfg(not(target_os = "macos"))]
fn read_process_memory(_pid: i32) -> Result<Vec<(u64, Vec<u8>)>> {
    bail!("live process scanning is only implemented on macOS")
}
