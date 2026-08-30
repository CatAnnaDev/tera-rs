use crate::error::{DataCenterError, Result};
use crate::file::{wrap, DataCenter};
use crate::{query_builder, Builder};
use std::path::Path;

pub const DEFAULT_LEVEL: u32 = 6;

pub struct Edit<'a> {
    pub select: &'a str,
    pub set: &'a [(String, String)],
    pub remove: &'a [String],
}

pub struct Outcome {
    pub matched: usize,
    pub edits: usize,
}

pub fn edit(builder: &mut Builder, edit: &Edit<'_>) -> Result<Outcome> {
    let targets = query_builder(builder, edit.select)?;
    let mut edits = 0;
    for id in &targets {
        for (name, literal) in edit.set {
            builder.set_attribute(*id, name, literal)?;
            edits += 1;
        }
        for name in edit.remove {
            if builder.remove_attribute(*id, name) {
                edits += 1;
            }
        }
    }
    Ok(Outcome {
        matched: targets.len(),
        edits,
    })
}

pub fn edit_in_place(
    path: impl AsRef<Path>,
    select: &str,
    set: &[(String, String)],
    remove: &[String],
) -> Result<usize> {
    let path = path.as_ref();
    let source = DataCenter::open(path)?;
    let keyiv = source.keyiv;
    let mut builder = Builder::from_datacenter(&source)?;
    let outcome = edit(
        &mut builder,
        &Edit {
            select,
            set,
            remove,
        },
    )?;
    if outcome.matched == 0 {
        return Ok(0);
    }
    let image = builder.pack()?;
    drop(source);
    match keyiv {
        Some(keyiv) => std::fs::write(path, wrap(&image, &keyiv, DEFAULT_LEVEL)?)?,
        None => std::fs::write(path, &image)?,
    }
    Ok(outcome.matched)
}

#[derive(Clone, Debug)]
pub enum Operation {
    Set {
        select: String,
        set: Vec<(String, String)>,
        remove: Vec<String>,
    },
    Add {
        parent: String,
        name: String,
        copy_of: Option<String>,
        set: Vec<(String, String)>,
    },
    Remove {
        select: String,
    },
}

impl Operation {
    pub fn target(&self) -> &str {
        match self {
            Self::Set { select, .. } | Self::Remove { select } => select,
            Self::Add { parent, .. } => parent,
        }
    }
}

pub fn apply(builder: &mut Builder, operation: &Operation) -> Result<usize> {
    match operation {
        Operation::Set {
            select,
            set,
            remove,
        } => Ok(edit(
            builder,
            &Edit {
                select,
                set,
                remove,
            },
        )?
        .matched),
        Operation::Remove { select } => {
            let targets = query_builder(builder, select)?;
            if targets.is_empty() {
                return Ok(0);
            }
            let parents = builder.parent_map();
            let mut removed = 0;
            for id in targets {
                if let Some(parent) = parents.get(&id) {
                    if builder.detach(*parent, id) {
                        removed += 1;
                    }
                }
            }
            Ok(removed)
        }
        Operation::Add {
            parent,
            name,
            copy_of,
            set,
        } => {
            let parents = query_builder(builder, parent)?;
            let source = match copy_of {
                Some(select) => Some(
                    *query_builder(builder, select)?
                        .first()
                        .ok_or_else(|| DataCenterError::Query(format!("`{select}` matched nothing")))?,
                ),
                None => None,
            };
            let template = source.map(|original| builder.duplicate(original));
            let mut added = 0;
            for id in parents {
                let child = match template {
                    Some(original) => {
                        let copy = builder.duplicate(original);
                        builder.attach(id, copy);
                        copy
                    }
                    None => builder.insert(id, name),
                };
                if !name.is_empty() {
                    let name_index = builder.name_index(name);
                    builder.node_mut(child).name = name_index;
                    builder.adopt_key(id, child);
                }
                for (attribute, literal) in set {
                    builder.set_attribute(child, attribute, literal)?;
                }
                added += 1;
            }
            Ok(added)
        }
    }
}

pub fn apply_all(path: impl AsRef<Path>, operations: &[Operation]) -> Result<Vec<usize>> {
    let path = path.as_ref();
    let source = DataCenter::open(path)?;
    let keyiv = source.keyiv;
    let mut builder = Builder::from_datacenter(&source)?;
    let mut matched = Vec::with_capacity(operations.len());
    for operation in operations {
        matched.push(apply(&mut builder, operation)?);
    }
    if matched.iter().all(|count| *count == 0) {
        return Ok(matched);
    }
    let image = builder.pack()?;
    drop(source);
    match keyiv {
        Some(keyiv) => std::fs::write(path, wrap(&image, &keyiv, DEFAULT_LEVEL)?)?,
        None => std::fs::write(path, &image)?,
    }
    Ok(matched)
}
