use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default = "unversioned")]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "change")]
    pub changes: Vec<Change>,
}

fn unversioned() -> String {
    "0.1.0".to_string()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    DataCenter {
        select: String,
        #[serde(default)]
        set: BTreeMap<String, String>,
        #[serde(default)]
        remove: Vec<String>,
    },
    File {
        source: PathBuf,
        target: PathBuf,
    },
    AddNode {
        parent: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        copy_of: Option<String>,
        #[serde(default)]
        set: BTreeMap<String, String>,
    },
    RemoveNode {
        select: String,
    },
    Texture {
        package: PathBuf,
        object: String,
        source: PathBuf,
    },
    RemoveFile {
        target: PathBuf,
    },
    Config {
        target: PathBuf,
        section: String,
        #[serde(default)]
        set: BTreeMap<String, String>,
        #[serde(default)]
        remove: Vec<String>,
        #[serde(default)]
        append: Vec<String>,
        #[serde(default)]
        detach: Vec<String>,
    },
    Property {
        package: PathBuf,
        object: String,
        #[serde(default)]
        set: BTreeMap<String, String>,
    },
    Sound {
        package: PathBuf,
        object: String,
        source: PathBuf,
    },
    Object {
        package: PathBuf,
        object: String,
        source: PathBuf,
    },
    Mesh {
        package: PathBuf,
        object: String,
        source: PathBuf,
    },
    Gfx {
        package: PathBuf,
        object: String,
        source: PathBuf,
    },
    Material {
        package: PathBuf,
        object: String,
        #[serde(default)]
        set: BTreeMap<String, String>,
    },
    NewTexture {
        source: PathBuf,
        target: PathBuf,
        package: String,
        object: String,
        #[serde(default = "dxt5")]
        format: String,
        #[serde(default = "ui_group")]
        lod_group: String,
        #[serde(default)]
        mip_chain: bool,
    },
}

fn dxt5() -> String {
    "PF_DXT5".to_string()
}

fn ui_group() -> String {
    "TEXTUREGROUP_UI".to_string()
}

impl Change {
    pub fn is_data_center(&self) -> bool {
        matches!(
            self,
            Self::DataCenter { .. } | Self::AddNode { .. } | Self::RemoveNode { .. }
        )
    }

    pub fn operation(&self) -> Option<tera_datacenter::Operation> {
        let pairs = |set: &BTreeMap<String, String>| -> Vec<(String, String)> {
            set.iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        };
        match self {
            Self::DataCenter {
                select,
                set,
                remove,
            } => Some(tera_datacenter::Operation::Set {
                select: select.clone(),
                set: pairs(set),
                remove: remove.clone(),
            }),
            Self::AddNode {
                parent,
                name,
                copy_of,
                set,
            } => Some(tera_datacenter::Operation::Add {
                parent: parent.clone(),
                name: name.clone(),
                copy_of: copy_of.clone(),
                set: pairs(set),
            }),
            Self::RemoveNode { select } => Some(tera_datacenter::Operation::Remove {
                select: select.clone(),
            }),
            _ => None,
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Self::DataCenter { select, set, remove } => {
                let mut parts: Vec<String> =
                    set.iter().map(|(name, value)| format!("{name}={value}")).collect();
                parts.extend(remove.iter().map(|name| format!("-{name}")));
                format!("data center {select} [{}]", parts.join(" "))
            }
            Self::File { source, target } => {
                format!("file {} -> {}", source.display(), target.display())
            }
            Self::AddNode {
                parent,
                name,
                copy_of,
                set,
            } => {
                let assignments: Vec<String> = set
                    .iter()
                    .map(|(attribute, value)| format!("{attribute}={value}"))
                    .collect();
                match copy_of {
                    Some(original) => {
                        format!("add {parent}/{name} copied from {original} [{}]", assignments.join(" "))
                    }
                    None => format!("add {parent}/{name} [{}]", assignments.join(" ")),
                }
            }
            Self::RemoveNode { select } => format!("remove {select}"),
            Self::RemoveFile { target } => format!("remove file {}", target.display()),
            Self::Config {
                target,
                section,
                set,
                remove,
                append,
                detach,
            } => {
                let mut parts: Vec<String> = set
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect();
                parts.extend(remove.iter().map(|key| format!("-{key}")));
                parts.extend(append.iter().map(|entry| format!("+{entry}")));
                parts.extend(detach.iter().map(|entry| format!("-{entry}")));
                format!("config [{section}] in {} [{}]", target.display(), parts.join(" "))
            }
            Self::Gfx {
                package,
                object,
                source,
            } => format!(
                "interface {object} in {} from {}",
                package.display(),
                source.display()
            ),
            Self::Mesh {
                package,
                object,
                source,
            } => format!(
                "mesh {object} in {} from {}",
                package.display(),
                source.display()
            ),
            Self::Object {
                package,
                object,
                source,
            } => format!(
                "object {object} in {} from {}",
                package.display(),
                source.display()
            ),
            Self::Sound {
                package,
                object,
                source,
            } => format!(
                "sound {object} in {} from {}",
                package.display(),
                source.display()
            ),
            Self::Material {
                package,
                object,
                set,
            } => {
                let assignments: Vec<String> = set
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect();
                format!(
                    "material {object} in {} [{}]",
                    package.display(),
                    assignments.join(" ")
                )
            }
            Self::NewTexture {
                source,
                target,
                package,
                object,
                format,
                ..
            } => format!(
                "new texture {package}.{object} ({format}) from {} into {}",
                source.display(),
                target.display()
            ),
            Self::Property {
                package,
                object,
                set,
            } => {
                let assignments: Vec<String> = set
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect();
                format!(
                    "property {object} in {} [{}]",
                    package.display(),
                    assignments.join(" ")
                )
            }
            Self::Texture {
                package,
                object,
                source,
            } => format!(
                "texture {object} in {} from {}",
                package.display(),
                source.display()
            ),
        }
    }
}

pub fn safe_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("a mod name must be between 1 and 64 characters, got `{name}`");
    }
    if name == "." || name == ".." {
        anyhow::bail!("`{name}` is not a usable mod name");
    }
    if let Some(bad) = name
        .chars()
        .find(|character| !character.is_ascii_alphanumeric() && !"._- ".contains(*character))
    {
        anyhow::bail!("a mod name may not contain `{bad}`, got `{name}`");
    }
    Ok(())
}

fn inside_the_game(label: &str, path: &Path) -> anyhow::Result<()> {
    for part in path.components() {
        match part {
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            _ => anyhow::bail!(
                "{label} `{}` must stay inside the game folder",
                path.display()
            ),
        }
    }
    Ok(())
}

impl Change {
    pub fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::File { source, target } => {
                inside_the_game("source", source)?;
                inside_the_game("target", target)
            }
            Self::RemoveFile { target } | Self::Config { target, .. } => {
                inside_the_game("target", target)
            }
            Self::Texture {
                package,
                source,
                ..
            }
            | Self::Sound {
                package,
                source,
                ..
            }
            | Self::Object {
                package,
                source,
                ..
            }
            | Self::Mesh {
                package,
                source,
                ..
            }
            | Self::Gfx {
                package,
                source,
                ..
            } => {
                inside_the_game("package", package)?;
                inside_the_game("source", source)
            }
            Self::Property { package, .. } | Self::Material { package, .. } => {
                inside_the_game("package", package)
            }
            Self::NewTexture { source, target, .. } => {
                inside_the_game("source", source)?;
                inside_the_game("target", target)
            }
            Self::DataCenter { .. } | Self::AddNode { .. } | Self::RemoveNode { .. } => Ok(()),
        }
    }
}

impl Manifest {
    pub fn validate(&self) -> anyhow::Result<()> {
        safe_name(&self.name)?;
        for change in &self.changes {
            change.validate()?;
        }
        Ok(())
    }

    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let manifest: Self = toml::from_str(&text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn touches_data_center(&self) -> bool {
        self.changes.iter().any(Change::is_data_center)
    }

    pub fn example(name: &str) -> Self {
        let pairs = |entries: &[(&str, &str)]| -> BTreeMap<String, String> {
            entries
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        };
        Self {
            name: name.to_string(),
            version: unversioned(),
            author: String::new(),
            description: String::new(),
            changes: vec![
                Change::DataCenter {
                    select: "/StrSheet_Item/String[@id=\"1\"]".to_string(),
                    set: pairs(&[("string", "Renamed by a mod")]),
                    remove: Vec::new(),
                },
                Change::AddNode {
                    parent: "/ItemData[0]".to_string(),
                    name: "Item".to_string(),
                    copy_of: Some("/ItemData/Item[@id=\"1\"]".to_string()),
                    set: pairs(&[("id", "999001"), ("name", "my_item")]),
                },
                Change::RemoveNode {
                    select: "/ItemData/Item[@id=\"999002\"]".to_string(),
                },
                Change::Texture {
                    package: "S1Game/CookedPC/some.gpk".into(),
                    object: "SomeTexture".to_string(),
                    source: "art/replacement.png".into(),
                },
                Change::Property {
                    package: "S1Game/CookedPC/some.gpk".into(),
                    object: "SomeObject".to_string(),
                    set: pairs(&[("SRGB", "true")]),
                },
                Change::Sound {
                    package: "S1Game/CookedPC/Sound_Data/Packages/some.gpk".into(),
                    object: "SomeSound".to_string(),
                    source: "snd/replacement.ogg".into(),
                },
                Change::Mesh {
                    package: "S1Game/CookedPC/some.gpk".into(),
                    object: "SomeMesh".to_string(),
                    source: "art/reshaped.obj".into(),
                },
                Change::Material {
                    package: "S1Game/CookedPC/some.gpk".into(),
                    object: "SomeMaterial_MI".to_string(),
                    set: pairs(&[("DiffuseMap", "Pack.Tex.other_diff")]),
                },
                Change::Gfx {
                    package: "S1Game/CookedPC/c7a706fb_6.gpk".into(),
                    object: "Crosshair_dup".to_string(),
                    source: "ui/Crosshair.gfx".into(),
                },
                Change::Object {
                    package: "S1Game/CookedPC/some.gpk".into(),
                    object: "SomeObject".to_string(),
                    source: "raw/patched.bin".into(),
                },
                Change::File {
                    source: "files/S1Engine.ini".into(),
                    target: "S1Game/Config/S1Engine.ini".into(),
                },
                Change::RemoveFile {
                    target: "S1Game/CookedPC/unwanted.gpk".into(),
                },
                Change::Config {
                    target: "S1Game/Config/S1SystemSettings.ini".into(),
                    section: "SystemSettings".to_string(),
                    set: pairs(&[("MotionBlur", "False")]),
                    remove: Vec::new(),
                    append: Vec::new(),
                    detach: Vec::new(),
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
name = "pink-names"
version = "1.2.0"
author = "CatAnnaDev"
description = "renames a thing and drops a file in"

[[change]]
kind = "data_center"
select = '/StrSheet_Item/String[@id="1"]'
set = { string = "Huile de Minuit Rose" }

[[change]]
kind = "file"
source = "extra/thing.gpk"
target = "S1Game/CookedPC/thing.gpk"

[[change]]
kind = "texture"
package = "CookedPC/art.gpk"
object = "Chat_BG"
source = "art/chat.png"
"#;

    #[test]
    fn a_manifest_round_trips() {
        let manifest: Manifest = toml::from_str(SAMPLE).expect("parse");
        assert_eq!(manifest.name, "pink-names");
        assert_eq!(manifest.version, "1.2.0");
        assert_eq!(manifest.changes.len(), 3);

        let text = toml::to_string_pretty(&manifest).expect("serialise");
        let again: Manifest = toml::from_str(&text).expect("reparse");
        assert_eq!(again.changes.len(), 3);
        assert_eq!(again.name, manifest.name);
    }

    #[test]
    fn every_change_kind_parses_into_its_own_shape() {
        let manifest: Manifest = toml::from_str(SAMPLE).expect("parse");
        match &manifest.changes[0] {
            Change::DataCenter { select, set, remove } => {
                assert!(select.contains("StrSheet_Item"));
                assert_eq!(set.get("string").map(String::as_str), Some("Huile de Minuit Rose"));
                assert!(remove.is_empty());
            }
            other => panic!("expected a data center change, got {other:?}"),
        }
        assert!(matches!(&manifest.changes[1], Change::File { .. }));
        assert!(matches!(&manifest.changes[2], Change::Texture { .. }));
    }

    #[test]
    fn a_version_is_optional() {
        let manifest: Manifest = toml::from_str("name = \"bare\"\n").expect("parse");
        assert_eq!(manifest.version, "0.1.0");
        assert!(manifest.changes.is_empty());
    }

    #[test]
    fn a_summary_says_what_will_happen() {
        let manifest: Manifest = toml::from_str(SAMPLE).expect("parse");
        assert!(manifest.changes[0].summary().contains("string=Huile de Minuit Rose"));
        assert!(manifest.changes[1].summary().contains("thing.gpk"));
        assert!(manifest.changes[2].summary().contains("Chat_BG"));
    }
}
