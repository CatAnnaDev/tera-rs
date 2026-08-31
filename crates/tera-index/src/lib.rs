use memmap2::Mmap;
use rayon::prelude::*;
use std::fs::File;
use std::path::{Path, PathBuf};
use tera_package::Bundle;

pub const MAGIC: &[u8; 8] = b"TERAIDX2";

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a tera index file")]
    BadMagic,
    #[error("index is truncated")]
    Truncated,
}

pub type Result<T> = std::result::Result<T, IndexError>;

#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub offset: u32,
    pub length: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct PackageEntry {
    pub name: Span,
    pub file: u32,
    pub offset: u64,
    pub span: u64,
    pub exports: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct ObjectEntry {
    pub package: u32,
    pub name: Span,
    pub class: u32,
    pub export: u32,
}

#[derive(Default)]
pub struct Arena {
    pub bytes: Vec<u8>,
}

impl Arena {
    pub fn push(&mut self, value: &str) -> Span {
        let offset = self.bytes.len() as u32;
        self.bytes.extend_from_slice(value.as_bytes());
        Span {
            offset,
            length: value.len() as u32,
        }
    }
}

#[derive(Default)]
pub struct IndexData {
    pub files: Vec<Span>,
    pub packages: Vec<PackageEntry>,
    pub objects: Vec<ObjectEntry>,
    pub classes: Vec<Span>,
    pub arena: Arena,
}

struct FileScan {
    packages: Vec<(String, u64, u64, u32)>,
    objects: Vec<(u32, String, String, u32)>,
}

pub fn collect_package_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let extension = path
                .extension()
                .map(|value| value.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if matches!(extension.as_str(), "gpk" | "upk" | "umap") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

pub fn build(
    root: &Path,
    with_objects: bool,
    progress: impl Fn(usize, usize) + Sync,
) -> Result<IndexData> {
    let files = collect_package_files(root);
    let total = files.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let scans: Vec<FileScan> = files
        .par_iter()
        .map(|path| {
            let scan = scan_file(path, with_objects);
            let count = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if count.is_multiple_of(64) || count == total {
                progress(count, total);
            }
            scan
        })
        .collect();

    let mut data = IndexData::default();
    let mut interned: std::collections::HashMap<String, Span> = std::collections::HashMap::new();
    let mut class_ids: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for (file_index, (path, scan)) in files.iter().zip(scans).enumerate() {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        data.files.push(data.arena.push(&relative));
        let base = data.packages.len() as u32;
        for (name, offset, span, exports) in scan.packages {
            let name = data.arena.push(&name);
            data.packages.push(PackageEntry {
                name,
                file: file_index as u32,
                offset,
                span,
                exports,
            });
        }
        for (local_package, name, class, export) in scan.objects {
            let name = match interned.get(&name) {
                Some(span) => *span,
                None => {
                    let span = data.arena.push(&name);
                    interned.insert(name, span);
                    span
                }
            };
            let class = match class_ids.get(&class) {
                Some(id) => *id,
                None => {
                    let span = data.arena.push(&class);
                    let id = data.classes.len() as u32;
                    data.classes.push(span);
                    class_ids.insert(class, id);
                    id
                }
            };
            data.objects.push(ObjectEntry {
                package: base + local_package,
                name,
                class,
                export,
            });
        }
    }
    Ok(data)
}

fn scan_file(path: &Path, with_objects: bool) -> FileScan {
    let mut scan = FileScan {
        packages: Vec::new(),
        objects: Vec::new(),
    };
    let Ok(file) = File::open(path) else {
        return scan;
    };
    let Ok(map) = (unsafe { Mmap::map(&file) }) else {
        return scan;
    };
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_default();
    let packages: Vec<_> = Bundle::tables_only(&map).collect();
    let single = packages.len() == 1;
    for package in packages {
        let Ok(mut package) = package else { break };
        if single {
            package.name_hint = Some(stem.clone());
        }
        let local = scan.packages.len() as u32;
        scan.packages.push((
            package.package_name(),
            package.base as u64,
            package.span as u64,
            package.exports.len() as u32,
        ));
        if with_objects {
            let prefix = format!("{}.", package.package_name());
            for (export_index, export) in package.exports.iter().enumerate() {
                let path = package.export_path(export_index);
                let relative = path.strip_prefix(&prefix).unwrap_or(&path).to_string();
                scan.objects.push((
                    local,
                    relative,
                    package.export_class(export),
                    export_index as u32,
                ));
            }
        }
    }
    scan
}

impl IndexData {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            48 + self.files.len() * 8
                + self.packages.len() * 32
                + self.objects.len() * 24
                + self.arena.bytes.len(),
        );
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.packages.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.objects.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.classes.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.arena.bytes.len() as u64).to_le_bytes());
        for span in &self.files {
            out.extend_from_slice(&span.offset.to_le_bytes());
            out.extend_from_slice(&span.length.to_le_bytes());
        }
        for package in &self.packages {
            out.extend_from_slice(&package.name.offset.to_le_bytes());
            out.extend_from_slice(&package.name.length.to_le_bytes());
            out.extend_from_slice(&package.file.to_le_bytes());
            out.extend_from_slice(&package.exports.to_le_bytes());
            out.extend_from_slice(&package.offset.to_le_bytes());
            out.extend_from_slice(&package.span.to_le_bytes());
        }
        for span in &self.classes {
            out.extend_from_slice(&span.offset.to_le_bytes());
            out.extend_from_slice(&span.length.to_le_bytes());
        }
        for object in &self.objects {
            out.extend_from_slice(&object.package.to_le_bytes());
            out.extend_from_slice(&object.class.to_le_bytes());
            out.extend_from_slice(&object.name.offset.to_le_bytes());
            out.extend_from_slice(&object.name.length.to_le_bytes());
            out.extend_from_slice(&object.export.to_le_bytes());
        }
        out.extend_from_slice(&self.arena.bytes);
        out
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.to_bytes())?;
        Ok(())
    }
}

pub struct Index {
    map: Mmap,
    file_count: usize,
    package_count: usize,
    object_count: usize,
    class_count: usize,
    files_offset: usize,
    packages_offset: usize,
    classes_offset: usize,
    objects_offset: usize,
    strings_offset: usize,
}

impl Index {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let map = unsafe { Mmap::map(&file)? };
        if map.len() < 32 || &map[..8] != MAGIC {
            return Err(IndexError::BadMagic);
        }
        let file_count = u32::from_le_bytes(map[8..12].try_into().unwrap()) as usize;
        let package_count = u32::from_le_bytes(map[12..16].try_into().unwrap()) as usize;
        let object_count = u32::from_le_bytes(map[16..20].try_into().unwrap()) as usize;
        let class_count = u32::from_le_bytes(map[20..24].try_into().unwrap()) as usize;
        let files_offset = 32;
        let packages_offset = files_offset + file_count * 8;
        let classes_offset = packages_offset + package_count * 32;
        let objects_offset = classes_offset + class_count * 8;
        let strings_offset = objects_offset + object_count * 20;
        if map.len() < strings_offset {
            return Err(IndexError::Truncated);
        }
        Ok(Self {
            map,
            file_count,
            package_count,
            object_count,
            class_count,
            files_offset,
            packages_offset,
            classes_offset,
            objects_offset,
            strings_offset,
        })
    }

    pub fn file_count(&self) -> usize {
        self.file_count
    }

    pub fn package_count(&self) -> usize {
        self.package_count
    }

    pub fn object_count(&self) -> usize {
        self.object_count
    }

    pub fn byte_size(&self) -> usize {
        self.map.len()
    }

    fn text(&self, span: Span) -> &str {
        let start = self.strings_offset + span.offset as usize;
        let end = start + span.length as usize;
        match self.map.get(start..end) {
            Some(bytes) => std::str::from_utf8(bytes).unwrap_or(""),
            None => "",
        }
    }

    pub fn file_name(&self, index: usize) -> &str {
        let base = self.files_offset + index * 8;
        self.text(Span {
            offset: u32::from_le_bytes(self.map[base..base + 4].try_into().unwrap()),
            length: u32::from_le_bytes(self.map[base + 4..base + 8].try_into().unwrap()),
        })
    }

    pub fn package(&self, index: usize) -> PackageEntry {
        let base = self.packages_offset + index * 32;
        let read32 = |offset: usize| {
            u32::from_le_bytes(self.map[base + offset..base + offset + 4].try_into().unwrap())
        };
        let read64 = |offset: usize| {
            u64::from_le_bytes(self.map[base + offset..base + offset + 8].try_into().unwrap())
        };
        PackageEntry {
            name: Span {
                offset: read32(0),
                length: read32(4),
            },
            file: read32(8),
            exports: read32(12),
            offset: read64(16),
            span: read64(24),
        }
    }

    pub fn package_name(&self, index: usize) -> &str {
        self.text(self.package(index).name)
    }

    pub fn class_count(&self) -> usize {
        self.class_count
    }

    pub fn class_name(&self, id: u32) -> &str {
        let base = self.classes_offset + id as usize * 8;
        if base + 8 > self.objects_offset {
            return "";
        }
        self.text(Span {
            offset: u32::from_le_bytes(self.map[base..base + 4].try_into().unwrap()),
            length: u32::from_le_bytes(self.map[base + 4..base + 8].try_into().unwrap()),
        })
    }

    pub fn classes(&self) -> Vec<&str> {
        (0..self.class_count as u32)
            .map(|id| self.class_name(id))
            .collect()
    }

    pub fn object(&self, index: usize) -> ObjectEntry {
        let base = self.objects_offset + index * 20;
        let read32 = |offset: usize| {
            u32::from_le_bytes(self.map[base + offset..base + offset + 4].try_into().unwrap())
        };
        ObjectEntry {
            package: read32(0),
            class: read32(4),
            name: Span {
                offset: read32(8),
                length: read32(12),
            },
            export: read32(16),
        }
    }

    pub fn object_name(&self, index: usize) -> &str {
        self.text(self.object(index).name)
    }

    pub fn object_class(&self, index: usize) -> &str {
        self.class_name(self.object(index).class)
    }

    pub fn object_full_path(&self, index: usize) -> String {
        let object = self.object(index);
        format!(
            "{}.{}",
            self.package_name(object.package as usize),
            self.object_name(index)
        )
    }

    pub fn search_packages(&self, needle: &str, limit: usize) -> Vec<u32> {
        self.search(self.package_count, limit, |index| {
            self.package_name(index)
        }, needle)
    }

    pub fn find_object_exact(&self, name: &str, class: Option<&str>) -> Option<u32> {
        (0..self.object_count)
            .into_par_iter()
            .filter(|index| {
                let entry = self.object(*index);
                if let Some(class) = class {
                    if !self.class_name(entry.class).eq_ignore_ascii_case(class) {
                        return false;
                    }
                }
                let stored = self.text(entry.name);
                let leaf = stored.rsplit('.').next().unwrap_or(stored);
                leaf.eq_ignore_ascii_case(name)
            })
            .map(|index| index as u32)
            .min()
    }

    pub fn search_objects(&self, needle: &str, limit: usize, class: Option<&str>) -> Vec<u32> {
        if needle.is_empty() && class.is_none() {
            return (0..self.object_count.min(limit) as u32).collect();
        }
        let lowered = needle.to_ascii_lowercase();
        let mut hits: Vec<u32> = (0..self.object_count)
            .into_par_iter()
            .filter(|index| {
                let entry = self.object(*index);
                if let Some(class) = class {
                    if !self.class_name(entry.class).eq_ignore_ascii_case(class) {
                        return false;
                    }
                }
                lowered.is_empty() || contains_ignore_case(self.text(entry.name), &lowered)
            })
            .map(|index| index as u32)
            .take_any(limit)
            .collect();
        hits.sort_unstable();
        hits
    }

    fn search<'a>(
        &'a self,
        count: usize,
        limit: usize,
        get: impl Fn(usize) -> &'a str + Sync,
        needle: &str,
    ) -> Vec<u32> {
        if needle.is_empty() {
            return (0..count.min(limit) as u32).collect();
        }
        let lowered = needle.to_ascii_lowercase();
        let mut hits: Vec<u32> = (0..count)
            .into_par_iter()
            .filter(|index| contains_ignore_case(get(*index), &lowered))
            .map(|index| index as u32)
            .take_any(limit)
            .collect();
        hits.sort_unstable();
        hits
    }

    pub fn resolve(&self, root: &Path, package: usize) -> PathBuf {
        let entry = self.package(package);
        root.join(self.file_name(entry.file as usize))
    }
}

#[inline]
pub fn contains_ignore_case(haystack: &str, lowered_needle: &str) -> bool {
    let needle = lowered_needle.as_bytes();
    if needle.is_empty() {
        return true;
    }
    let hay = haystack.as_bytes();
    if hay.len() < needle.len() {
        return false;
    }
    let first = needle[0];
    for start in 0..=hay.len() - needle.len() {
        if hay[start].to_ascii_lowercase() != first {
            continue;
        }
        if hay[start..start + needle.len()]
            .iter()
            .zip(needle)
            .all(|(left, right)| left.to_ascii_lowercase() == *right)
        {
            return true;
        }
    }
    false
}
