use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::File;
use std::io::Write;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW,
    MODULEENTRY32W, PROCESSENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Memory::{
    VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, MEM_IMAGE, MEM_MAPPED, PAGE_EXECUTE,
    PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOACCESS,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

const TARGET: &str = "tera.exe";
const ADDR_CEILING: u64 = 0x0000_7FFF_FFFF_0000;
const EXEC_MASK: u32 =
    PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;

#[derive(Parser)]
#[command(about = "Live memory scanner for the running 64-bit TERA client (run inside its wine bottle)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "List committed memory regions (executable ones and large ones)")]
    Regions,
    #[command(about = "Search executable regions for a hex byte pattern")]
    Find { hex: String },
    #[command(about = "Search all readable regions for a hex byte pattern")]
    FindAll { hex: String },
    #[command(about = "Search all readable regions for a UTF-16 string")]
    Findw { text: String },
    #[command(about = "Dump a byte range to a file, tolerating unreadable pages")]
    Dump {
        #[arg(value_parser = parse_hex_u64)]
        addr: u64,
        #[arg(value_parser = parse_hex_u64)]
        len: u64,
        out: String,
    },
    #[command(about = "Extract the system-message id->name table and write it as a toolbox map")]
    Sysmsg {
        #[arg(default_value = "sysmsg.map")]
        out: String,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let pid = find_process(TARGET).context("TERA.exe not found in the running processes")?;
    let process = Process::open(pid)?;
    match cli.command {
        Command::Regions => regions(&process),
        Command::Find { hex } => find(&process, &parse_hex_bytes(&hex)?, true),
        Command::FindAll { hex } => find(&process, &parse_hex_bytes(&hex)?, false),
        Command::Findw { text } => find(&process, &utf16_bytes(&text), false),
        Command::Dump { addr, len, out } => dump(&process, addr, len, &out),
        Command::Sysmsg { out } => sysmsg(&process, &out),
    }
}

struct Process {
    handle: HANDLE,
}

impl Process {
    fn open(pid: u32) -> Result<Self> {
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            bail!("OpenProcess failed for pid {pid}");
        }
        Ok(Self { handle })
    }

    fn read(&self, addr: u64, buf: &mut [u8]) -> bool {
        let mut got = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                self.handle,
                addr as *const c_void,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                &mut got,
            )
        };
        ok != 0 && got == buf.len()
    }

    fn read_region(&self, base: u64, size: u64) -> Vec<u8> {
        let mut out = vec![0u8; size as usize];
        let mut page = [0u8; 0x1000];
        let mut off = 0u64;
        while off < size {
            let chunk = 0x1000.min((size - off) as usize);
            if self.read(base + off, &mut page[..chunk]) {
                out[off as usize..off as usize + chunk].copy_from_slice(&page[..chunk]);
            }
            off += 0x1000;
        }
        out
    }

    fn regions(&self) -> Vec<Region> {
        let mut out = Vec::new();
        let mut addr = 0u64;
        loop {
            if addr >= ADDR_CEILING {
                break;
            }
            let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
            let got = unsafe {
                VirtualQueryEx(
                    self.handle,
                    addr as *const c_void,
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if got == 0 {
                break;
            }
            let base = mbi.BaseAddress as u64;
            let size = mbi.RegionSize as u64;
            if size == 0 {
                break;
            }
            if mbi.State == MEM_COMMIT && (mbi.Protect & (PAGE_NOACCESS | PAGE_GUARD)) == 0 {
                out.push(Region {
                    base,
                    size,
                    protect: mbi.Protect,
                    kind: mbi.Type,
                });
            }
            addr = base + size;
        }
        out
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

struct Region {
    base: u64,
    size: u64,
    protect: u32,
    kind: u32,
}

impl Region {
    fn exec(&self) -> bool {
        self.protect & EXEC_MASK != 0
    }
}

fn find_process(name: &str) -> Option<u32> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut found = None;
    if unsafe { Process32FirstW(snapshot, &mut entry) } != 0 {
        loop {
            if wide_to_string(&entry.szExeFile).eq_ignore_ascii_case(name) {
                found = Some(entry.th32ProcessID);
                break;
            }
            if unsafe { Process32NextW(snapshot, &mut entry) } == 0 {
                break;
            }
        }
    }
    unsafe { CloseHandle(snapshot) };
    found
}

fn module_bounds(pid: u32, name: &str) -> Option<(u64, u64)> {
    let snapshot =
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry: MODULEENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
    let mut bounds = None;
    if unsafe { Module32FirstW(snapshot, &mut entry) } != 0 {
        loop {
            if wide_to_string(&entry.szModule).eq_ignore_ascii_case(name) {
                bounds = Some((entry.modBaseAddr as u64, entry.modBaseSize as u64));
                break;
            }
            if unsafe { Module32NextW(snapshot, &mut entry) } == 0 {
                break;
            }
        }
    }
    unsafe { CloseHandle(snapshot) };
    bounds
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn regions(process: &Process) -> Result<()> {
    let mut total = 0u64;
    let regions = process.regions();
    for region in &regions {
        total += region.size;
        if region.exec() || region.size >= 0x100000 {
            let kind = match region.kind {
                MEM_IMAGE => "IMG",
                MEM_MAPPED => "MAP",
                _ => "PRV",
            };
            println!(
                "{:012x} {:>11} {} prot={:x}{}",
                region.base,
                region.size,
                kind,
                region.protect,
                if region.exec() { " EXEC" } else { "" }
            );
        }
    }
    println!("-- {} regions, {} bytes", regions.len(), total);
    Ok(())
}

fn find(process: &Process, pattern: &[u8], exec_only: bool) -> Result<()> {
    if pattern.is_empty() {
        bail!("empty pattern");
    }
    let mut hits = 0usize;
    for region in process.regions() {
        if exec_only && !region.exec() {
            continue;
        }
        let buf = process.read_region(region.base, region.size);
        let mut i = 0usize;
        while i + pattern.len() <= buf.len() {
            if &buf[i..i + pattern.len()] == pattern {
                println!(
                    "hit @{:012x} (region {:012x} prot={:x})",
                    region.base + i as u64,
                    region.base,
                    region.protect
                );
                hits += 1;
                if hits >= 60 {
                    println!("-- stopping at 60 hits");
                    return Ok(());
                }
            }
            i += 1;
        }
    }
    println!("-- {hits} hits");
    Ok(())
}

fn dump(process: &Process, addr: u64, len: u64, out: &str) -> Result<()> {
    let buf = process.read_region(addr, len);
    let mut file = File::create(out).with_context(|| format!("creating {out}"))?;
    file.write_all(&buf)?;
    println!("dumped {len:#x} bytes from {addr:012x} to {out}");
    Ok(())
}

fn parse_hex_u64(value: &str) -> Result<u64, String> {
    let trimmed = value.trim_start_matches("0x");
    u64::from_str_radix(trimmed, 16).map_err(|e| e.to_string())
}

fn parse_hex_bytes(value: &str) -> Result<Vec<u8>> {
    let cleaned: String = value.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if cleaned.len() % 2 != 0 {
        bail!("hex pattern must have an even number of digits");
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).context("invalid hex"))
        .collect()
}

fn utf16_bytes(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

struct ModuleImage {
    blocks: Vec<(u64, Vec<u8>)>,
}

impl ModuleImage {
    fn read(process: &Process, base: u64, size: u64) -> Self {
        let mut blocks = Vec::new();
        for region in process.regions() {
            if region.base < base || region.base >= base + size {
                continue;
            }
            if region.kind != MEM_IMAGE {
                continue;
            }
            blocks.push((region.base, process.read_region(region.base, region.size)));
        }
        Self { blocks }
    }

    fn qword(&self, vaddr: u64) -> Option<u64> {
        for (block_base, data) in &self.blocks {
            if vaddr >= *block_base && vaddr + 8 <= block_base + data.len() as u64 {
                let off = (vaddr - block_base) as usize;
                return Some(u64::from_le_bytes(data[off..off + 8].try_into().unwrap()));
            }
        }
        None
    }

    fn strings(&self) -> HashMap<u64, String> {
        let mut map = HashMap::new();
        for (block_base, data) in &self.blocks {
            let n = data.len();
            let mut i = 0usize;
            while i + 2 <= n {
                let mut j = i;
                let mut text = String::new();
                while j + 2 <= n && data[j + 1] == 0 {
                    let c = data[j];
                    if c.is_ascii_graphic() && (c.is_ascii_alphanumeric() || c == b'_') {
                        text.push(c as char);
                        j += 2;
                    } else {
                        break;
                    }
                }
                if j + 2 <= n && data[j] == 0 && data[j + 1] == 0 && text.len() >= 3 {
                    map.insert(block_base + i as u64, text);
                    i = j + 2;
                } else {
                    i += 2;
                }
            }
        }
        map
    }
}

fn sysmsg(process: &Process, out: &str) -> Result<()> {
    let pid = find_process(TARGET).context("TERA.exe not found")?;
    let (base, size) = module_bounds(pid, TARGET).context("TERA.exe module bounds not found")?;
    let image = ModuleImage::read(process, base, size);
    let strings = image.strings();

    let anchor = strings
        .iter()
        .find(|(_, name)| name.as_str() == "SMT_UNDEFINED")
        .map(|(addr, _)| *addr)
        .context("SMT_UNDEFINED string not found in client memory")?;

    let mut array_base = None;
    'outer: for (block_base, data) in &image.blocks {
        let n = data.len();
        let mut i = 0usize;
        while i + 8 <= n {
            let value = u64::from_le_bytes(data[i..i + 8].try_into().unwrap());
            if value == anchor {
                let next_at = block_base + i as u64 + 8;
                if image
                    .qword(next_at)
                    .map(|q| strings.contains_key(&q))
                    .unwrap_or(false)
                {
                    array_base = Some(block_base + i as u64);
                    break 'outer;
                }
            }
            i += 8;
        }
    }
    let array_base = array_base.context("sysmsg pointer array not located")?;

    let mut file = File::create(out).with_context(|| format!("creating {out}"))?;
    let mut id = 0u64;
    let mut written = 0usize;
    loop {
        let slot = array_base + id * 8;
        let pointer = match image.qword(slot) {
            Some(value) => value,
            None => break,
        };
        if let Some(name) = strings.get(&pointer) {
            writeln!(file, "{name} {id}")?;
            written += 1;
            if name == "SMT_MAX" {
                break;
            }
        }
        id += 1;
        if id > 0x20000 {
            break;
        }
    }
    println!(
        "wrote {written} system-message entries to {out} (table base {array_base:012x})"
    );
    Ok(())
}
