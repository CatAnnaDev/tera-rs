use crate::manifest::{Change, Manifest};
use std::collections::BTreeMap;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Touched {
    pub target: PathBuf,
    pub backup: Option<PathBuf>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncate: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grown_to: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Receipt {
    pub name: String,
    pub version: String,
    pub applied: Vec<Touched>,
}

pub struct Install {
    root: PathBuf,
    store: PathBuf,
}

impl Install {
    pub fn new(root: impl Into<PathBuf>, store: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            store: store.into(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn backups(&self, name: &str) -> PathBuf {
        self.store.join("backups").join(name)
    }

    fn receipt_path(&self, name: &str) -> PathBuf {
        self.store.join("applied").join(format!("{name}.json"))
    }

    pub fn is_applied(&self, name: &str) -> bool {
        self.receipt_path(name).exists()
    }

    pub fn applied(&self) -> Vec<Receipt> {
        let Ok(entries) = std::fs::read_dir(self.store.join("applied")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|entry| entry.path().extension().map(|kind| kind == "json").unwrap_or(false))
            .filter_map(|entry| std::fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
            .collect()
    }

    fn backup_path(&self, name: &str, target: &Path) -> PathBuf {
        let relative = target.strip_prefix(&self.root).unwrap_or(target);
        let mut path = self.backups(name);
        for part in relative.components() {
            match part {
                std::path::Component::Normal(text) => path.push(text),
                std::path::Component::ParentDir => path.push("up"),
                _ => {}
            }
        }
        path
    }

    fn preserve(&self, name: &str, target: &Path) -> Result<Option<PathBuf>> {
        if !target.exists() {
            return Ok(None);
        }
        let backup = self.backup_path(name, target);
        if backup == target {
            bail!(
                "refusing to back {} up onto itself",
                target.display()
            );
        }
        if backup.exists() {
            return Ok(Some(backup));
        }
        if let Some(parent) = backup.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(target, &backup)
            .with_context(|| format!("backing up {}", target.display()))?;
        Ok(Some(backup))
    }

    fn claim(
        &self,
        name: &str,
        target: &Path,
        summary: String,
        applied: &mut Vec<Touched>,
    ) -> Result<Option<PathBuf>> {
        let backup = self.preserve(name, target)?;
        applied.push(Touched {
            target: target.to_path_buf(),
            backup: backup.clone(),
            summary,
            truncate: None,
            grown_to: None,
        });
        Ok(backup)
    }

    pub fn plan(&self, manifest: &Manifest, data_center: &Path) -> Vec<(PathBuf, String)> {
        manifest
            .changes
            .iter()
            .map(|change| (self.target_of(change, data_center), change.summary()))
            .collect()
    }

    fn target_of(&self, change: &Change, data_center: &Path) -> PathBuf {
        match change {
            change if change.is_data_center() => data_center.to_path_buf(),
            Change::File { target, .. }
            | Change::RemoveFile { target }
            | Change::Config { target, .. }
            | Change::NewTexture { target, .. } => self.root.join(target),
            Change::Texture { package, .. }
            | Change::Property { package, .. }
            | Change::Sound { package, .. }
            | Change::Object { package, .. }
            | Change::Mesh { package, .. }
            | Change::Gfx { package, .. }
            | Change::Material { package, .. } => self.root.join(package),
            _ => PathBuf::new(),
        }
    }

    fn edit_data_center(
        &self,
        manifest: &Manifest,
        data_center: &Path,
        applied: &mut Vec<Touched>,
    ) -> Result<()> {
        let changes: Vec<&Change> = manifest
            .changes
            .iter()
            .filter(|change| change.is_data_center())
            .collect();
        if changes.is_empty() {
            return Ok(());
        }
        let operations: Vec<tera_datacenter::Operation> =
            changes.iter().filter_map(|change| change.operation()).collect();
        for change in &changes {
            self.claim(&manifest.name, data_center, change.summary(), applied)?;
        }
        let matched = tera_datacenter::apply_all(data_center, &operations)
            .with_context(|| format!("editing {}", data_center.display()))?;
        for (change, count) in changes.iter().zip(&matched) {
            if *count == 0 {
                bail!("{} matched nothing", change.summary());
            }
        }
        Ok(())
    }

    fn edit_textures(
        &self,
        manifest: &Manifest,
        from: &Path,
        applied: &mut Vec<Touched>,
    ) -> Result<()> {
        let mut by_package: BTreeMap<PathBuf, Vec<(&String, &PathBuf)>> = BTreeMap::new();
        for change in &manifest.changes {
            if let Change::Texture {
                package,
                object,
                source,
            } = change
            {
                by_package
                    .entry(self.root.join(package))
                    .or_default()
                    .push((object, source));
            }
        }
        for (target, wanted) in by_package {
            for (object, source) in &wanted {
                let summary = format!(
                    "texture {object} in {} from {}",
                    target.display(),
                    source.display()
                );
                self.claim(&manifest.name, &target, summary, applied)?;
            }
            let image = std::fs::read(&target)
                .with_context(|| format!("reading {}", target.display()))?;
            let sources: Vec<Vec<u8>> = wanted
                .iter()
                .map(|(_, source)| {
                    std::fs::read(from.join(source))
                        .with_context(|| format!("reading {}", source.display()))
                })
                .collect::<Result<_>>()?;
            let request: Vec<(&str, &[u8])> = wanted
                .iter()
                .zip(&sources)
                .map(|((object, _), bytes)| (object.as_str(), bytes.as_slice()))
                .collect();
            let mut appender = tera_package::texture::CacheAppender::at(
                target.parent().unwrap_or(Path::new(".")),
            );
            let replaced =
                tera_package::replace_textures_into(&image, &request, Some(&mut appender))
                    .with_context(|| format!("replacing textures in {}", target.display()))?;
            let extended: Vec<(PathBuf, u64)> = appender
                .touched()
                .map(|(name, length)| (appender.path(name), length))
                .collect();
            appender
                .flush()
                .with_context(|| format!("extending caches next to {}", target.display()))?;
            for (cache, length) in extended {
                let grown = std::fs::metadata(&cache).map(|meta| meta.len()).ok();
                applied.push(Touched {
                    summary: format!("extend {}", cache.display()),
                    target: cache,
                    backup: None,
                    truncate: Some(length),
                    grown_to: grown,
                });
            }
            std::fs::write(&target, &replaced.bytes)?;
        }
        Ok(())
    }



    fn edit_payloads(
        &self,
        manifest: &Manifest,
        from: &Path,
        applied: &mut Vec<Touched>,
        label: &str,
        pick: fn(&Change) -> Option<(&PathBuf, &String, &PathBuf)>,
        replace: fn(&[u8], &[(&str, &[u8])]) -> tera_package::Result<tera_package::Replaced>,
    ) -> Result<()> {
        let mut by_package: BTreeMap<PathBuf, Vec<(&String, &PathBuf)>> = BTreeMap::new();
        for change in &manifest.changes {
            if let Some((package, object, source)) = pick(change) {
                by_package
                    .entry(self.root.join(package))
                    .or_default()
                    .push((object, source));
            }
        }
        for (target, wanted) in by_package {
            for (object, source) in &wanted {
                let summary = format!(
                    "{label} {object} in {} from {}",
                    target.display(),
                    source.display()
                );
                self.claim(&manifest.name, &target, summary, applied)?;
            }
            let image = std::fs::read(&target)
                .with_context(|| format!("reading {}", target.display()))?;
            let sources: Vec<Vec<u8>> = wanted
                .iter()
                .map(|(_, source)| {
                    std::fs::read(from.join(source))
                        .with_context(|| format!("reading {}", source.display()))
                })
                .collect::<Result<_>>()?;
            let request: Vec<(&str, &[u8])> = wanted
                .iter()
                .zip(&sources)
                .map(|((object, _), bytes)| (object.as_str(), bytes.as_slice()))
                .collect();
            let replaced = replace(&image, &request)
                .with_context(|| format!("replacing {label} in {}", target.display()))?;
            std::fs::write(&target, &replaced.bytes)?;
        }
        Ok(())
    }

    fn edit_materials(&self, manifest: &Manifest, applied: &mut Vec<Touched>) -> Result<()> {
        for change in &manifest.changes {
            let Change::Material {
                package,
                object,
                set,
            } = change
            else {
                continue;
            };
            let target = self.root.join(package);
            self.claim(&manifest.name, &target, change.summary(), applied)?;
            let image = std::fs::read(&target)
                .with_context(|| format!("reading {}", target.display()))?;
            let assignments: Vec<(String, String)> = set
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            let edited = tera_package::set_parameters(&image, object, &assignments)
                .with_context(|| format!("setting material parameters on {object}"))?;
            std::fs::write(&target, &edited.bytes)?;
        }
        Ok(())
    }

    fn edit_properties(&self, manifest: &Manifest, applied: &mut Vec<Touched>) -> Result<()> {
        for change in &manifest.changes {
            let Change::Property {
                package,
                object,
                set,
            } = change
            else {
                continue;
            };
            let target = self.root.join(package);
            self.claim(&manifest.name, &target, change.summary(), applied)?;
            let image = std::fs::read(&target)
                .with_context(|| format!("reading {}", target.display()))?;
            let assignments: Vec<(String, String)> = set
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            let edited = tera_package::set_properties(&image, object, &assignments)
                .with_context(|| format!("setting properties on {object}"))?;
            std::fs::write(&target, &edited.bytes)?;
        }
        Ok(())
    }

    fn edit_files(
        &self,
        manifest: &Manifest,
        from: &Path,
        applied: &mut Vec<Touched>,
    ) -> Result<()> {
        for change in &manifest.changes {
            match change {
                Change::File { source, target } => {
                    let target = self.root.join(target);
                    self.claim(&manifest.name, &target, change.summary(), applied)?;
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::copy(from.join(source), &target)
                        .with_context(|| format!("copying into {}", target.display()))?;
                }
                Change::NewTexture {
                    source,
                    target,
                    package,
                    object,
                    format,
                    lod_group,
                    mip_chain,
                } => {
                    let target = self.root.join(target);
                    self.claim(&manifest.name, &target, change.summary(), applied)?;
                    let image = std::fs::read(from.join(source))
                        .with_context(|| format!("reading {}", source.display()))?;
                    let built = tera_package::texture_package(
                        &image,
                        &tera_package::NewTexture {
                            package,
                            object,
                            format,
                            lod_group,
                            source_path: &source.to_string_lossy(),
                            mip_chain: *mip_chain,
                        },
                    )
                    .with_context(|| format!("building {package}.{object}"))?;
                    if let Some(parent) = target.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&target, &built)?;
                }
                Change::Config {
                    target,
                    section,
                    set,
                    remove,
                    append,
                    detach,
                } => {
                    let target = self.root.join(target);
                    if self
                        .claim(&manifest.name, &target, change.summary(), applied)?
                        .is_none()
                    {
                        bail!("{} is not there to configure", target.display());
                    }
                    let text = std::fs::read_to_string(&target)
                        .with_context(|| format!("reading {}", target.display()))?;
                    let mut config = crate::config::Config::parse(&text);
                    for (key, value) in set {
                        config.set(section, key, value);
                    }
                    for key in remove {
                        config.remove(section, key);
                    }
                    for entry in append {
                        let (key, value) = entry
                            .split_once('=')
                            .with_context(|| format!("`{entry}` is not KEY=VALUE"))?;
                        config.push(section, key, value);
                    }
                    for entry in detach {
                        let (key, value) = entry
                            .split_once('=')
                            .with_context(|| format!("`{entry}` is not KEY=VALUE"))?;
                        config.pull(section, key, value);
                    }
                    std::fs::write(&target, config.render())?;
                }
                Change::RemoveFile { target } => {
                    let target = self.root.join(target);
                    if self
                        .claim(&manifest.name, &target, change.summary(), applied)?
                        .is_none()
                    {
                        bail!("{} is not there to remove", target.display());
                    }
                    std::fs::remove_file(&target)
                        .with_context(|| format!("removing {}", target.display()))?;
                }
                _ => continue,
            }
        }
        Ok(())
    }

    pub fn conflicts(&self, manifest: &Manifest, data_center: &Path) -> Vec<(String, PathBuf)> {
        self.conflicts_with(manifest, data_center, &self.applied())
    }

    pub fn conflicts_with(
        &self,
        manifest: &Manifest,
        data_center: &Path,
        applied: &[Receipt],
    ) -> Vec<(String, PathBuf)> {
        let wanted: std::collections::BTreeSet<PathBuf> = self
            .plan(manifest, data_center)
            .into_iter()
            .map(|(target, _)| target)
            .collect();
        let mut clashes = Vec::new();
        for receipt in applied {
            if receipt.name == manifest.name {
                continue;
            }
            for touched in &receipt.applied {
                if wanted.contains(&touched.target) {
                    clashes.push((receipt.name.clone(), touched.target.clone()));
                }
            }
        }
        clashes.sort();
        clashes.dedup();
        clashes
    }

    fn write_receipt(&self, receipt: &Receipt) -> Result<()> {
        let path = self.receipt_path(&receipt.name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, serde_json::to_vec_pretty(receipt)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn apply(&self, manifest: &Manifest, data_center: &Path, from: &Path) -> Result<Receipt> {
        manifest.validate()?;
        if self.is_applied(&manifest.name) {
            bail!("{} is already applied, revert it first", manifest.name);
        }
        let clashes = self.conflicts(manifest, data_center);
        if let Some((other, target)) = clashes.first() {
            bail!(
                "{} already changed {}; revert it first or the backups will tangle",
                other,
                target.display()
            );
        }
        let mut applied = Vec::with_capacity(manifest.changes.len());
        let outcome = self
            .edit_data_center(manifest, data_center, &mut applied)
            .and_then(|()| self.edit_textures(manifest, from, &mut applied))
            .and_then(|()| {
                self.edit_payloads(
                    manifest,
                    from,
                    &mut applied,
                    "sound",
                    pick_sound,
                    tera_package::replace_sounds,
                )
            })
            .and_then(|()| {
                self.edit_payloads(
                    manifest,
                    from,
                    &mut applied,
                    "mesh",
                    pick_mesh,
                    tera_package::replace_meshes,
                )
            })
            .and_then(|()| {
                self.edit_payloads(
                    manifest,
                    from,
                    &mut applied,
                    "object",
                    pick_object,
                    tera_package::replace_blobs,
                )
            })
            .and_then(|()| {
                self.edit_payloads(
                    manifest,
                    from,
                    &mut applied,
                    "interface",
                    pick_gfx,
                    replace_gfx,
                )
            })
            .and_then(|()| self.edit_materials(manifest, &mut applied))
            .and_then(|()| self.edit_properties(manifest, &mut applied))
            .and_then(|()| self.edit_files(manifest, from, &mut applied));
        let receipt = Receipt {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            applied,
        };
        let outcome = outcome.and_then(|()| self.write_receipt(&receipt));
        if let Err(error) = outcome {
            if let Err(problem) = self.restore(&receipt) {
                return Err(error.context(format!(
                    "rollback was incomplete so the backups were kept in {}: {problem:#}",
                    self.backups(&manifest.name).display()
                )));
            }
            let _ = std::fs::remove_file(self.receipt_path(&manifest.name));
            let _ = std::fs::remove_dir_all(self.backups(&manifest.name));
            return Err(error);
        }
        Ok(receipt)
    }

    fn restore_one(&self, touched: &Touched) -> Result<()> {
        if let Some(length) = touched.truncate {
            let current = std::fs::metadata(&touched.target)
                .map(|meta| meta.len())
                .with_context(|| format!("reading {}", touched.target.display()))?;
            if touched.grown_to.map(|grown| grown != current).unwrap_or(false) {
                bail!(
                    "{} grew past what this mod appended, so another mod is using it; revert that one first",
                    touched.target.display()
                );
            }
            if current > length {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&touched.target)
                    .and_then(|file| file.set_len(length))
                    .with_context(|| format!("trimming {}", touched.target.display()))?;
            }
            return Ok(());
        }
        match &touched.backup {
            Some(backup) => {
                if backup == &touched.target {
                    bail!(
                        "receipt claims {} is its own backup",
                        touched.target.display()
                    );
                }
                std::fs::copy(backup, &touched.target)
                    .with_context(|| format!("restoring {}", touched.target.display()))?;
            }
            None => match std::fs::remove_file(&touched.target) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("removing {}", touched.target.display())
                    })
                }
            },
        }
        Ok(())
    }

    fn restore(&self, receipt: &Receipt) -> Result<()> {
        let mut problems = Vec::new();
        for touched in receipt.applied.iter().rev() {
            if let Err(error) = self.restore_one(touched) {
                problems.push(format!("{error:#}"));
            }
        }
        if problems.is_empty() {
            return Ok(());
        }
        bail!("{}", problems.join("; "))
    }

    pub fn revert(&self, name: &str) -> Result<Receipt> {
        crate::manifest::safe_name(name)?;
        let path = self.receipt_path(name);
        let bytes =
            std::fs::read(&path).with_context(|| format!("{name} is not applied"))?;
        let receipt: Receipt = serde_json::from_slice(&bytes)?;
        self.restore(&receipt).with_context(|| {
            format!(
                "{name} was not fully reverted, so its receipt and backups were kept in {}",
                self.backups(name).display()
            )
        })?;
        std::fs::remove_file(&path)?;
        let _ = std::fs::remove_dir_all(self.backups(name));
        Ok(receipt)
    }
}

fn pick_sound(change: &Change) -> Option<(&PathBuf, &String, &PathBuf)> {
    match change {
        Change::Sound {
            package,
            object,
            source,
        } => Some((package, object, source)),
        _ => None,
    }
}

fn replace_gfx(
    data: &[u8],
    wanted: &[(&str, &[u8])],
) -> tera_package::Result<tera_package::Replaced> {
    let mut current = data.to_vec();
    let mut touched = Vec::new();
    for (object, movie) in wanted {
        let replaced = tera_package::replace_movie(&current, object, movie)?;
        current = replaced.bytes;
        touched.extend(replaced.textures);
    }
    Ok(tera_package::Replaced {
        bytes: current,
        textures: touched,
    })
}

fn pick_gfx(change: &Change) -> Option<(&PathBuf, &String, &PathBuf)> {
    match change {
        Change::Gfx {
            package,
            object,
            source,
        } => Some((package, object, source)),
        _ => None,
    }
}

fn pick_mesh(change: &Change) -> Option<(&PathBuf, &String, &PathBuf)> {
    match change {
        Change::Mesh {
            package,
            object,
            source,
        } => Some((package, object, source)),
        _ => None,
    }
}

fn pick_object(change: &Change) -> Option<(&PathBuf, &String, &PathBuf)> {
    match change {
        Change::Object {
            package,
            object,
            source,
        } => Some((package, object, source)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Change;

    struct Tree {
        base: PathBuf,
    }

    impl Tree {
        fn new(label: &str) -> Self {
            let base = std::env::temp_dir().join(format!("tera-mod-{label}"));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(base.join("game")).unwrap();
            std::fs::create_dir_all(base.join("mod")).unwrap();
            Self { base }
        }

        fn install(&self) -> Install {
            Install::new(self.base.join("game"), self.base.join("store"))
        }

        fn write(&self, relative: &str, text: &str) -> PathBuf {
            let path = self.base.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        }

        fn read(&self, relative: &str) -> String {
            std::fs::read_to_string(self.base.join(relative)).unwrap()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn replacing(target: &str) -> Manifest {
        Manifest {
            name: "reskin".into(),
            version: "1.0".into(),
            author: String::new(),
            description: String::new(),
            changes: vec![Change::File {
                source: "new.txt".into(),
                target: target.into(),
            }],
        }
    }

    #[test]
    fn an_overwritten_file_comes_back_byte_for_byte() {
        let tree = Tree::new("overwrite");
        tree.write("game/Config/S1Game.ini", "original");
        tree.write("mod/new.txt", "modified");
        let install = tree.install();
        let manifest = replacing("Config/S1Game.ini");

        install
            .apply(&manifest, Path::new("unused"), &tree.base.join("mod"))
            .unwrap();
        assert_eq!(tree.read("game/Config/S1Game.ini"), "modified");

        install.revert("reskin").unwrap();
        assert_eq!(tree.read("game/Config/S1Game.ini"), "original");
    }

    #[test]
    fn a_file_the_mod_invented_is_removed_on_revert() {
        let tree = Tree::new("invented");
        tree.write("mod/new.txt", "brand new");
        let install = tree.install();
        let manifest = replacing("Config/Added.ini");

        install
            .apply(&manifest, Path::new("unused"), &tree.base.join("mod"))
            .unwrap();
        assert!(tree.base.join("game/Config/Added.ini").exists());

        install.revert("reskin").unwrap();
        assert!(!tree.base.join("game/Config/Added.ini").exists());
    }

    #[test]
    fn applying_twice_is_refused_so_the_backup_stays_the_original() {
        let tree = Tree::new("twice");
        tree.write("game/a.txt", "original");
        tree.write("mod/new.txt", "modified");
        let install = tree.install();
        let manifest = replacing("a.txt");
        let from = tree.base.join("mod");

        install.apply(&manifest, Path::new("unused"), &from).unwrap();
        assert!(install.apply(&manifest, Path::new("unused"), &from).is_err());

        install.revert("reskin").unwrap();
        assert_eq!(tree.read("game/a.txt"), "original");
    }

    #[test]
    fn what_is_applied_is_readable_back() {
        let tree = Tree::new("listing");
        tree.write("game/a.txt", "original");
        tree.write("mod/new.txt", "modified");
        let install = tree.install();
        let manifest = replacing("a.txt");

        assert!(!install.is_applied("reskin"));
        install
            .apply(&manifest, Path::new("unused"), &tree.base.join("mod"))
            .unwrap();

        assert!(install.is_applied("reskin"));
        let applied = install.applied();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].name, "reskin");
        assert_eq!(applied[0].applied.len(), 1);

        install.revert("reskin").unwrap();
        assert!(!install.is_applied("reskin"));
        assert!(install.applied().is_empty());
    }

    #[test]
    fn two_mods_touching_the_same_file_are_refused() {
        let tree = Tree::new("clash");
        tree.write("game/shared.txt", "original");
        tree.write("mod/new.txt", "first");
        let install = tree.install();
        let from = tree.base.join("mod");

        let mut first = replacing("shared.txt");
        first.name = "first".into();
        let mut second = replacing("shared.txt");
        second.name = "second".into();

        install.apply(&first, Path::new("unused"), &from).unwrap();
        let refusal = install
            .apply(&second, Path::new("unused"), &from)
            .unwrap_err()
            .to_string();
        assert!(refusal.contains("first"), "{refusal}");

        install.revert("first").unwrap();
        assert_eq!(tree.read("game/shared.txt"), "original");
        install.apply(&second, Path::new("unused"), &from).unwrap();
        install.revert("second").unwrap();
        assert_eq!(tree.read("game/shared.txt"), "original");
    }

    #[test]
    fn reverting_something_that_was_never_applied_says_so() {
        let tree = Tree::new("absent");
        assert!(tree.install().revert("ghost").is_err());
    }

    #[test]
    fn a_plan_names_every_file_before_anything_is_touched() {
        let tree = Tree::new("plan");
        let install = tree.install();
        let manifest = Manifest {
            name: "mixed".into(),
            version: "1.0".into(),
            author: String::new(),
            description: String::new(),
            changes: vec![
                Change::File {
                    source: "a".into(),
                    target: "Config/a.ini".into(),
                },
                Change::DataCenter {
                    select: "/StrSheet_Item".into(),
                    set: Default::default(),
                    remove: Vec::new(),
                },
            ],
        };
        let plan = install.plan(&manifest, Path::new("/dc/DataCenter.dat"));
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].0, tree.base.join("game/Config/a.ini"));
        assert_eq!(plan[1].0, Path::new("/dc/DataCenter.dat"));
        assert!(!tree.base.join("game/Config/a.ini").exists());
    }

    #[test]
    fn a_removed_file_comes_back_on_revert() {
        let tree = Tree::new("removal");
        tree.write("game/Config/Unwanted.ini", "delete me");
        let install = tree.install();
        let manifest = Manifest {
            name: "cleanup".into(),
            version: "1.0".into(),
            author: String::new(),
            description: String::new(),
            changes: vec![Change::RemoveFile {
                target: "Config/Unwanted.ini".into(),
            }],
        };

        install
            .apply(&manifest, Path::new("unused"), &tree.base.join("mod"))
            .unwrap();
        assert!(!tree.base.join("game/Config/Unwanted.ini").exists());

        install.revert("cleanup").unwrap();
        assert_eq!(tree.read("game/Config/Unwanted.ini"), "delete me");
    }

    #[test]
    fn removing_a_file_that_is_not_there_is_refused() {
        let tree = Tree::new("absent-file");
        let install = tree.install();
        let manifest = Manifest {
            name: "cleanup".into(),
            version: "1.0".into(),
            author: String::new(),
            description: String::new(),
            changes: vec![Change::RemoveFile {
                target: "Config/Ghost.ini".into(),
            }],
        };
        assert!(install
            .apply(&manifest, Path::new("unused"), &tree.base.join("mod"))
            .is_err());
    }

    #[test]
    fn a_change_that_fails_undoes_the_ones_before_it() {
        let tree = Tree::new("rollback");
        tree.write("game/first.txt", "original");
        tree.write("mod/new.txt", "modified");
        let install = tree.install();
        let manifest = Manifest {
            name: "halfway".into(),
            version: "1.0".into(),
            author: String::new(),
            description: String::new(),
            changes: vec![
                Change::File {
                    source: "new.txt".into(),
                    target: "first.txt".into(),
                },
                Change::RemoveFile {
                    target: "second.txt".into(),
                },
            ],
        };

        assert!(install
            .apply(&manifest, Path::new("unused"), &tree.base.join("mod"))
            .is_err());
        assert_eq!(tree.read("game/first.txt"), "original");
        assert!(!install.is_applied("halfway"));
        assert!(install.applied().is_empty());
    }
}
