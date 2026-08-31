use crate::hooks::{ModRegistration, Plugin, TERA_MOD_ABI};
use anyhow::{bail, Context, Result};
use libloading::{Library, Symbol};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

type AbiFn = unsafe extern "C" fn() -> u32;
type RegisterFn = unsafe extern "C" fn() -> *mut ModRegistration;

static LOAD_SEQ: AtomicU64 = AtomicU64::new(0);

pub struct LoadedMods {
    _libraries: Vec<Library>,
    temp_paths: Vec<PathBuf>,
    registers: Vec<(String, RegisterFn)>,
}

impl Drop for LoadedMods {
    fn drop(&mut self) {
        for path in &self.temp_paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl LoadedMods {
    pub fn empty() -> Self {
        Self {
            _libraries: Vec::new(),
            temp_paths: Vec::new(),
            registers: Vec::new(),
        }
    }

    pub fn load(dir: &Path, disabled: &[String]) -> Self {
        let mut mods = Self::empty();
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => {
                println!("[mods] no {} directory, running without dynamic mods", dir.display());
                return mods;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some(std::env::consts::DLL_EXTENSION)
            {
                continue;
            }
            let file = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
            let name = file.strip_prefix("lib").unwrap_or(file).to_string();
            if disabled.iter().any(|item| item == &name || item == file) {
                println!("[mods] {name}: disabled, skipped");
                continue;
            }
            match load_one(&path, &name) {
                Ok((library, temp, register)) => {
                    println!("[mods] loaded {name}");
                    mods._libraries.push(library);
                    mods.temp_paths.push(temp);
                    mods.registers.push((name, register));
                }
                Err(error) => eprintln!("[mods] {name}: {error:#}"),
            }
        }
        println!("[mods] {} dynamic mod(s) active", mods.registers.len());
        mods
    }

    pub fn instantiate(&self) -> Vec<Box<dyn Plugin>> {
        let mut plugins = Vec::new();
        for (name, register) in &self.registers {
            let register = *register;
            match std::panic::catch_unwind(|| unsafe { Box::from_raw(register()).plugin }) {
                Ok(plugin) => plugins.push(plugin),
                Err(_) => eprintln!("[mods] {name}: register panicked, skipped"),
            }
        }
        plugins
    }
}

pub fn signature(dir: &Path, disabled: &[String]) -> Vec<(String, u64, u64)> {
    let mut sig = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return sig;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some(std::env::consts::DLL_EXTENSION) {
            continue;
        }
        let file = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
        let name = file.strip_prefix("lib").unwrap_or(file).to_string();
        if disabled.iter().any(|item| item == &name || item == file) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|delta| delta.as_secs())
            .unwrap_or(0);
        sig.push((name, mtime, meta.len()));
    }
    sig.sort();
    sig
}

fn load_one(path: &Path, name: &str) -> Result<(Library, PathBuf, RegisterFn)> {
    let seq = LOAD_SEQ.fetch_add(1, Ordering::Relaxed);
    let temp = std::env::temp_dir().join(format!(
        "tera-mod-{}-{}-{}.{}",
        std::process::id(),
        seq,
        name,
        std::env::consts::DLL_EXTENSION
    ));
    std::fs::copy(path, &temp).with_context(|| format!("copying {} to a temp image", path.display()))?;
    match unsafe { open(&temp) } {
        Ok((library, register)) => Ok((library, temp, register)),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

unsafe fn open(path: &Path) -> Result<(Library, RegisterFn)> {
    let library = Library::new(path).with_context(|| format!("opening {}", path.display()))?;
    let abi: Symbol<AbiFn> = library
        .get(b"tera_mod_abi")
        .context("missing tera_mod_abi, not built with export_mod!")?;
    let reported = abi();
    if reported != TERA_MOD_ABI {
        bail!("ABI {reported} != proxy {TERA_MOD_ABI}, rebuild the mod against this tera-hook");
    }
    let register: Symbol<RegisterFn> = library
        .get(b"tera_mod_register")
        .context("missing tera_mod_register")?;
    let register = *register;
    Ok((library, register))
}
