use crate::decompress::decompress_chunk;
use crate::error::{PackageError, Result};
use crate::reader::Reader;
use crate::summary::Summary;
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Name {
    pub index: i32,
    pub number: i32,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub class_package: Name,
    pub class_name: Name,
    pub outer_index: i32,
    pub object_name: Name,
}

#[derive(Clone, Debug)]
pub struct Export {
    pub class_index: i32,
    pub super_index: i32,
    pub outer_index: i32,
    pub object_name: Name,
    pub archetype_index: i32,
    pub object_flags: u64,
    pub serial_size: i32,
    pub serial_offset: i32,
    pub export_flags: u32,
    pub package_flags: u32,
}

pub struct Package<'a> {
    pub base: usize,
    pub span: usize,
    pub summary: Summary,
    pub names: Vec<String>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
    pub export_entry_offsets: Vec<usize>,
    pub name_hint: Option<String>,
    image: Cow<'a, [u8]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ParseMode {
    Full,
    TablesOnly,
}

impl<'a> Package<'a> {
    pub fn parse(file: &'a [u8], base: usize) -> Result<Self> {
        Self::parse_with(file, base, ParseMode::Full)
    }

    pub fn parse_tables(file: &'a [u8], base: usize) -> Result<Self> {
        Self::parse_with(file, base, ParseMode::TablesOnly)
    }

    pub fn parse_with(file: &'a [u8], base: usize, mode: ParseMode) -> Result<Self> {
        let tail = file.get(base..).ok_or(PackageError::Truncated {
            offset: base,
            needed: 1,
            available: 0,
        })?;
        let summary = Summary::parse(tail)?;
        let image: Cow<'a, [u8]> = if summary.is_compressed() {
            Cow::Owned(build_image(tail, &summary, mode)?)
        } else {
            Cow::Borrowed(tail)
        };

        let sane = |count: i32| (count.max(0) as usize).min(image.len() / 8 + 16);
        let mut reader = Reader::at(&image, summary.name_offset as usize);
        let mut names = Vec::with_capacity(sane(summary.name_count));
        for _ in 0..summary.name_count {
            let name = reader.string()?;
            let _flags = reader.u64()?;
            names.push(name);
        }
        let mut tables_end = reader.offset();

        let mut reader = Reader::at(&image, summary.import_offset as usize);
        let mut imports = Vec::with_capacity(sane(summary.import_count));
        for _ in 0..summary.import_count {
            imports.push(Import {
                class_package: read_name(&mut reader)?,
                class_name: read_name(&mut reader)?,
                outer_index: reader.i32()?,
                object_name: read_name(&mut reader)?,
            });
        }
        tables_end = tables_end.max(reader.offset());

        let mut reader = Reader::at(&image, summary.export_offset as usize);
        let mut exports = Vec::with_capacity(sane(summary.export_count));
        let mut export_entry_offsets = Vec::with_capacity(sane(summary.export_count));
        for _ in 0..summary.export_count {
            export_entry_offsets.push(reader.offset());
            let class_index = reader.i32()?;
            let super_index = reader.i32()?;
            let outer_index = reader.i32()?;
            let object_name = read_name(&mut reader)?;
            let archetype_index = reader.i32()?;
            let object_flags = reader.u64()?;
            let serial_size = reader.i32()?;
            let serial_offset = reader.i32()?;
            let export_flags = reader.u32()?;
            let net_object_count = reader.i32()?;
            for _ in 0..net_object_count.max(0) {
                reader.i32()?;
            }
            let _package_guid = reader.guid()?;
            let package_flags = reader.u32()?;
            exports.push(Export {
                class_index,
                super_index,
                outer_index,
                object_name,
                archetype_index,
                object_flags,
                serial_size,
                serial_offset,
                export_flags,
                package_flags,
            });
        }
        tables_end = tables_end
            .max(reader.offset())
            .max(summary.depends_offset.max(0) as usize);

        let span = if summary.is_compressed() {
            summary.compressed_end()
        } else {
            exports
                .iter()
                .map(|export| (export.serial_offset + export.serial_size).max(0) as usize)
                .chain(std::iter::once(summary.total_header_size.max(0) as usize))
                .chain(std::iter::once(tables_end))
                .max()
                .unwrap_or_default()
        };
        let image = match image {
            Cow::Borrowed(slice) if span <= slice.len() => Cow::Borrowed(&slice[..span]),
            other => other,
        };
        Ok(Self {
            base,
            span,
            summary,
            names,
            imports,
            exports,
            export_entry_offsets,
            name_hint: None,
            image,
        })
    }

    pub fn image(&self) -> &[u8] {
        &self.image
    }

    pub fn name(&self, name: Name) -> &str {
        match self.names.get(name.index.max(0) as usize) {
            Some(text) => text,
            None => "<invalid>",
        }
    }

    pub fn object_index(&self, path: &str) -> Option<i32> {
        let wanted = path.to_ascii_lowercase();
        let suffix = format!(".{wanted}");
        let matches = |candidate: &str| {
            let candidate = candidate.to_ascii_lowercase();
            candidate == wanted || candidate.ends_with(&suffix)
        };
        if let Some(index) =
            (0..self.exports.len()).find(|index| matches(&self.export_path(*index)))
        {
            return Some(index as i32 + 1);
        }
        if let Some(index) = (0..self.exports.len())
            .find(|index| matches(&self.full_object_path(*index as i32 + 1)))
        {
            return Some(index as i32 + 1);
        }
        self.imports
            .iter()
            .position(|import| matches(&self.name_text(import.object_name)))
            .map(|index| -(index as i32) - 1)
    }

    pub fn find_name(&self, text: &str) -> Option<Name> {
        if let Some(index) = self.names.iter().position(|name| name == text) {
            return Some(Name {
                index: index as i32,
                number: 0,
            });
        }
        let (base, suffix) = text.rsplit_once('_')?;
        let number = suffix.parse::<i32>().ok()?;
        let index = self.names.iter().position(|name| name == base)?;
        Some(Name {
            index: index as i32,
            number: number + 1,
        })
    }

    pub fn name_text(&self, name: Name) -> String {
        let base = self.name(name);
        if name.number == 0 {
            base.to_string()
        } else {
            format!("{base}_{}", name.number - 1)
        }
    }

    pub fn with_name_hint(mut self, hint: impl Into<String>) -> Self {
        self.name_hint = Some(hint.into());
        self
    }

    pub fn package_name(&self) -> String {
        if let Some(hint) = &self.name_hint {
            return hint.clone();
        }
        self.detected_package_name()
    }

    pub fn detected_package_name(&self) -> String {
        let mut fallback: Option<String> = None;
        for (index, export) in self.exports.iter().enumerate() {
            if export.outer_index != 0 || self.export_class(export) != "Package" {
                continue;
            }
            let name = self.name_text(export.object_name);
            let owner = index as i32 + 1;
            if !self.exports.iter().any(|other| other.outer_index == owner) {
                return name;
            }
            fallback.get_or_insert(name);
        }
        fallback
            .or_else(|| self.names.first().cloned())
            .unwrap_or_default()
    }

    pub fn export_class(&self, export: &Export) -> String {
        match export.class_index {
            0 => "Class".to_string(),
            index if index < 0 => self
                .imports
                .get((-index - 1) as usize)
                .map(|import| self.name_text(import.object_name))
                .unwrap_or_else(|| "<invalid>".to_string()),
            index => self
                .exports
                .get((index - 1) as usize)
                .map(|other| self.name_text(other.object_name))
                .unwrap_or_else(|| "<invalid>".to_string()),
        }
    }

    pub fn object_name(&self, index: i32) -> Option<String> {
        match index {
            0 => None,
            index if index < 0 => self
                .imports
                .get((-index - 1) as usize)
                .map(|import| self.name_text(import.object_name)),
            index => self
                .exports
                .get((index - 1) as usize)
                .map(|export| self.name_text(export.object_name)),
        }
    }

    fn outer_of(&self, index: i32) -> i32 {
        match index {
            0 => 0,
            index if index < 0 => self
                .imports
                .get((-index - 1) as usize)
                .map(|import| import.outer_index)
                .unwrap_or(0),
            index => self
                .exports
                .get((index - 1) as usize)
                .map(|export| export.outer_index)
                .unwrap_or(0),
        }
    }

    pub fn full_object_path(&self, index: i32) -> String {
        let mut parts = Vec::new();
        let mut current = index;
        let mut guard = 0;
        while current != 0 && guard < 32 {
            if let Some(name) = self.object_name(current) {
                parts.push(name);
            }
            current = self.outer_of(current);
            guard += 1;
        }
        parts.reverse();
        parts.join(".")
    }

    pub fn export_path(&self, export_index: usize) -> String {
        let local = self.full_object_path(export_index as i32 + 1);
        let package = self.package_name();
        if package.is_empty() || local == package || local.starts_with(&format!("{package}.")) {
            local
        } else {
            format!("{package}.{local}")
        }
    }

    pub fn export_data(&self, export: &Export) -> Result<&[u8]> {
        let start = export.serial_offset.max(0) as usize;
        let end = start + export.serial_size.max(0) as usize;
        self.image
            .get(start..end)
            .ok_or(PackageError::Truncated {
                offset: start,
                needed: end - start,
                available: self.image.len().saturating_sub(start),
            })
    }

    pub fn find_export(&self, path: &str) -> Option<usize> {
        let needle = path.rsplit('.').next()?;
        (0..self.exports.len()).find(|index| {
            let export = &self.exports[*index];
            self.name_text(export.object_name).eq_ignore_ascii_case(needle)
        })
    }
}

fn read_name(reader: &mut Reader<'_>) -> Result<Name> {
    Ok(Name {
        index: reader.i32()?,
        number: reader.i32()?,
    })
}

fn table_span(summary: &Summary) -> (usize, usize) {
    let offsets = [
        summary.name_offset.max(0) as usize,
        summary.import_offset.max(0) as usize,
        summary.export_offset.max(0) as usize,
    ];
    let start = offsets.iter().copied().min().unwrap_or(0);
    let end = [
        summary.total_header_size.max(0) as usize,
        summary.depends_offset.max(0) as usize + 4096,
        summary.export_offset.max(0) as usize + summary.export_count.max(0) as usize * 80 + 64,
        summary.import_offset.max(0) as usize + summary.import_count.max(0) as usize * 28 + 64,
        summary.name_offset.max(0) as usize + summary.name_count.max(0) as usize * 72 + 64,
    ]
    .into_iter()
    .max()
    .unwrap_or(0);
    (start, end)
}

fn build_image(tail: &[u8], summary: &Summary, mode: ParseMode) -> Result<Vec<u8>> {
    if !summary.is_compressed() {
        return Ok(tail.to_vec());
    }
    let size = summary.uncompressed_size();
    if size > tail.len().saturating_mul(64) + (1 << 20) {
        return Err(PackageError::Truncated {
            offset: 0,
            needed: size,
            available: tail.len(),
        });
    }
    let mut image = vec![0u8; size];
    let first = summary
        .compressed_chunks
        .iter()
        .map(|chunk| chunk.uncompressed_offset as usize)
        .min()
        .unwrap_or(0);
    let prefix = first.min(tail.len()).min(size);
    image[..prefix].copy_from_slice(&tail[..prefix]);
    let (wanted_start, wanted_end) = table_span(summary);
    for chunk in &summary.compressed_chunks {
        let start = chunk.uncompressed_offset as usize;
        let end = start + chunk.uncompressed_size as usize;
        if mode == ParseMode::TablesOnly && (end <= wanted_start || start >= wanted_end) {
            continue;
        }
        if end > image.len() {
            return Err(PackageError::Truncated {
                offset: start,
                needed: end,
                available: image.len(),
            });
        }
        decompress_chunk(tail, chunk, summary.compression_flags, &mut image[start..end])?;
    }
    Ok(image)
}

pub struct Bundle<'a> {
    file: &'a [u8],
    offset: usize,
    mode: ParseMode,
}

impl<'a> Bundle<'a> {
    pub fn new(file: &'a [u8]) -> Self {
        Self {
            file,
            offset: 0,
            mode: ParseMode::Full,
        }
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn tables_only(file: &'a [u8]) -> Self {
        Self {
            file,
            offset: 0,
            mode: ParseMode::TablesOnly,
        }
    }
}

impl<'a> Iterator for Bundle<'a> {
    type Item = Result<Package<'a>>;

    fn next(&mut self) -> Option<Result<Package<'a>>> {
        if self.offset + 16 >= self.file.len() {
            return None;
        }
        let package = Package::parse_with(self.file, self.offset, self.mode);
        match package {
            Ok(package) => {
                let span = package.span.max(1);
                self.offset += span;
                Some(Ok(package))
            }
            Err(error) => {
                self.offset = self.file.len();
                Some(Err(error))
            }
        }
    }
}
