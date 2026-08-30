use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::defs::{self, Definition, DefinitionFile};

#[derive(Default)]
pub struct Registry {
    definitions: HashMap<String, (u32, Definition)>,
    skipped: usize,
}

impl Registry {
    pub fn load(directories: &[PathBuf], patch: Option<u32>) -> std::io::Result<Self> {
        Self::pinned(directories, patch, &HashMap::new())
    }

    pub fn pinned(
        directories: &[PathBuf],
        patch: Option<u32>,
        pins: &HashMap<String, u32>,
    ) -> std::io::Result<Self> {
        let mut candidates: HashMap<String, Vec<DefinitionFile>> = HashMap::new();
        let mut skipped = 0;
        for directory in directories {
            skipped += collect(directory, &mut candidates)?;
        }
        let definitions = candidates
            .into_iter()
            .filter_map(|(name, files)| {
                select(files, patch, pins.get(&name).copied()).map(|file| (name, file))
            })
            .collect();
        Ok(Self {
            definitions,
            skipped,
        })
    }

    pub fn get(&self, name: &str) -> Option<&Definition> {
        self.definitions.get(name).map(|(_, definition)| definition)
    }

    pub fn version(&self, name: &str) -> Option<u32> {
        self.definitions.get(name).map(|(version, _)| *version)
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn skipped(&self) -> usize {
        self.skipped
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

fn collect(
    directory: &Path,
    candidates: &mut HashMap<String, Vec<DefinitionFile>>,
) -> std::io::Result<usize> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(0);
    };
    let mut skipped = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|value| value != "def").unwrap_or(true) {
            continue;
        }
        match defs::read_file(&path) {
            Ok(file) => {
                let versions = candidates.entry(file.name.clone()).or_default();
                if !versions.iter().any(|other| other.version == file.version) {
                    versions.push(file);
                }
            }
            Err(_) => skipped += 1,
        }
    }
    Ok(skipped)
}

fn select(
    files: Vec<DefinitionFile>,
    patch: Option<u32>,
    pin: Option<u32>,
) -> Option<(u32, Definition)> {
    if let Some(version) = pin {
        if let Some(file) = files.iter().find(|file| file.version == version) {
            return Some((file.version, file.definition.clone()));
        }
    }
    let pick = |files: &[DefinitionFile], filter: bool| {
        files
            .iter()
            .filter(|file| !filter || patch.map(|value| file.patch.admits(value)).unwrap_or(true))
            .max_by_key(|file| file.version)
            .map(|file| (file.version, file.definition.clone()))
    };
    pick(&files, true).or_else(|| pick(&files, false))
}
