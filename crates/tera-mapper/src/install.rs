use crate::{CompositeEntry, CompositeMapper, MapperError, Result, COMPOSITE_MOD_PREFIX};
use std::path::{Path, PathBuf};
use tera_package::package::Package;

pub struct InstallReport {
    pub container: String,
    pub destination: PathBuf,
    pub backup: Option<PathBuf>,
    pub registered: Vec<CompositeEntry>,
}

pub struct UninstallReport {
    pub target: String,
    pub removed: Vec<String>,
    pub restored: Vec<(String, String)>,
    pub deleted_file: Option<PathBuf>,
}

pub fn mapper_path(game: &Path) -> PathBuf {
    game.join("CompositePackageMapper.dat")
}

pub fn mod_entries(mod_file: &Path) -> Result<Vec<CompositeEntry>> {
    let bytes = std::fs::read(mod_file)?;
    let mut entries = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let package = Package::parse(&bytes, offset)
            .map_err(|error| MapperError::Package(error.to_string()))?;
        let span = package.span;
        if span == 0 {
            break;
        }
        if let Some(object_path) = package
            .summary
            .folder_name
            .value
            .strip_prefix(COMPOSITE_MOD_PREFIX)
        {
            let object_path = object_path.to_string();
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
        return Err(MapperError::NoModEntries(mod_file.to_path_buf()));
    }
    Ok(entries)
}

fn backup_mapper(dat: &Path) -> Result<Option<PathBuf>> {
    let clean = dat.with_extension("clean");
    let created = if clean.exists() {
        None
    } else {
        std::fs::copy(dat, &clean)?;
        Some(clean)
    };
    std::fs::copy(dat, dat.with_extension("dat.bak")).ok();
    Ok(created)
}

pub fn install_mod(game: &Path, mod_file: &Path, name: Option<&str>) -> Result<InstallReport> {
    let dat = mapper_path(game);
    if !dat.exists() {
        return Err(MapperError::Missing(dat));
    }
    let container = match name {
        Some(name) => name.to_string(),
        None => mod_file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| MapperError::BadName(mod_file.to_path_buf()))?
            .to_string(),
    };
    let registered = mod_entries(mod_file)?;
    let backup = backup_mapper(&dat)?;
    let destination = game.join(format!("{container}.gpk"));
    std::fs::copy(mod_file, &destination)?;
    let mut mapper = CompositeMapper::read(&dat)?;
    for entry in &registered {
        mapper.upsert(&container, entry.clone());
    }
    mapper.write(&dat)?;
    Ok(InstallReport {
        container,
        destination,
        backup,
        registered,
    })
}

pub fn uninstall_mod(game: &Path, target: &str, delete_file: bool) -> Result<UninstallReport> {
    let dat = mapper_path(game);
    if !dat.exists() {
        return Err(MapperError::Missing(dat));
    }
    let mut mapper = CompositeMapper::read(&dat)?;
    let composites: Vec<String> = if mapper.groups.iter().any(|group| group.filename == target) {
        mapper
            .groups
            .iter()
            .filter(|group| group.filename == target)
            .flat_map(|group| group.entries.iter().map(|entry| entry.composite_name.clone()))
            .collect()
    } else if mapper.find_composite(target).is_some() {
        vec![target.to_string()]
    } else {
        return Err(MapperError::NoMatch(target.to_string()));
    };
    let clean = dat.with_extension("clean");
    let pristine = if clean.exists() {
        Some(CompositeMapper::read(&clean)?)
    } else {
        None
    };
    std::fs::copy(&dat, dat.with_extension("dat.bak")).ok();
    let mut restored = Vec::new();
    for composite in &composites {
        mapper.remove_composite(composite);
        if let Some(pristine) = &pristine {
            if let Some((filename, entry)) = pristine.find_composite(composite) {
                mapper.upsert(filename, entry.clone());
                restored.push((composite.clone(), filename.to_string()));
            }
        }
    }
    mapper.write(&dat)?;
    let deleted_file = if delete_file {
        let container = game.join(format!("{target}.gpk"));
        if container.exists() && std::fs::remove_file(&container).is_ok() {
            Some(container)
        } else {
            None
        }
    } else {
        None
    };
    Ok(UninstallReport {
        target: target.to_string(),
        removed: composites,
        restored,
        deleted_file,
    })
}

pub fn set_folder_name(data: &[u8], folder: &str) -> Result<Vec<u8>> {
    let mut package = Package::parse(data, 0).map_err(|error| MapperError::Package(error.to_string()))?;
    package.summary.folder_name = tera_package::summary::Text::from(folder);
    tera_package::writer::rebuild(&package, &std::collections::BTreeMap::new())
        .map_err(|error| MapperError::Package(error.to_string()))
}

pub fn author_composite(game: &Path, patched: &[u8], container: &str) -> Result<InstallReport> {
    let dat = mapper_path(game);
    if !dat.exists() {
        return Err(MapperError::Missing(dat));
    }
    let package = Package::parse(patched, 0).map_err(|error| MapperError::Package(error.to_string()))?;
    let existing = package.summary.folder_name.value.clone();
    let folder = if existing.starts_with(COMPOSITE_MOD_PREFIX) {
        existing
    } else {
        let name = package.package_name();
        let mapper = CompositeMapper::read(&dat)?;
        let (_, entry) = mapper
            .find_composite(&name)
            .ok_or(MapperError::NoMatch(name))?;
        format!("{COMPOSITE_MOD_PREFIX}{}", entry.object_path)
    };
    let tagged = if package.summary.folder_name.value == folder {
        patched.to_vec()
    } else {
        set_folder_name(patched, &folder)?
    };
    let destination = game.join(format!("{container}.gpk"));
    std::fs::write(&destination, &tagged)?;
    let registered = mod_entries(&destination)?;
    let backup = backup_mapper(&dat)?;
    let mut mapper = CompositeMapper::read(&dat)?;
    for entry in &registered {
        mapper.upsert(container, entry.clone());
    }
    mapper.write(&dat)?;
    Ok(InstallReport {
        container: container.to_string(),
        destination,
        backup,
        registered,
    })
}

pub fn restore_mapper(game: &Path) -> Result<PathBuf> {
    let dat = mapper_path(game);
    let clean = dat.with_extension("clean");
    if !clean.exists() {
        return Err(MapperError::Missing(clean));
    }
    std::fs::copy(&clean, &dat)?;
    Ok(dat)
}
