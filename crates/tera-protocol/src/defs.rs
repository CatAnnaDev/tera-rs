use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum DefError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("line {line}: malformed `{text}`")]
    Malformed { line: usize, text: String },
    #[error("line {line}: unknown type `{name}`")]
    UnknownType { line: usize, name: String },
    #[error("array nesting is too deep at line {0}")]
    Nesting(usize),
    #[error("bad definition file name `{0}`")]
    BadName(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primitive {
    Bool,
    Byte,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Int64,
    Uint64,
    Float,
    Double,
    Vec3,
    Vec3Fa,
    Angle,
    SkillId32,
    SkillId,
    Customize,
}

impl Primitive {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "bool" => Self::Bool,
            "byte" => Self::Byte,
            "int16" => Self::Int16,
            "uint16" => Self::Uint16,
            "int32" => Self::Int32,
            "uint32" => Self::Uint32,
            "int64" => Self::Int64,
            "uint64" => Self::Uint64,
            "float" => Self::Float,
            "double" => Self::Double,
            "vec3" => Self::Vec3,
            "vec3fa" => Self::Vec3Fa,
            "angle" => Self::Angle,
            "skillid32" => Self::SkillId32,
            "skillid" => Self::SkillId,
            "customize" => Self::Customize,
            _ => return None,
        })
    }

    pub fn size(self) -> usize {
        match self {
            Self::Bool | Self::Byte => 1,
            Self::Int16 | Self::Uint16 | Self::Angle => 2,
            Self::Int32 | Self::Uint32 | Self::Float | Self::SkillId32 => 4,
            Self::Int64 | Self::Uint64 | Self::Double | Self::SkillId | Self::Customize => 8,
            Self::Vec3 | Self::Vec3Fa => 12,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Field {
    RefArray(String),
    RefBytes(String),
    RefString(String),
    Value(Primitive),
    Str,
    Bytes,
    Object(Definition),
    Array {
        subtype: Option<Primitive>,
        string_subtype: bool,
        definition: Definition,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Definition {
    pub fields: Vec<(String, Field)>,
}

impl Definition {
    pub fn find(&self, name: &str) -> Option<&Field> {
        self.fields
            .iter()
            .find(|(field, kind)| {
                field == name
                    && !matches!(
                        kind,
                        Field::RefArray(_) | Field::RefBytes(_) | Field::RefString(_)
                    )
            })
            .map(|(_, field)| field)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatchRange {
    pub min: Option<u32>,
    pub max: Option<u32>,
}

impl PatchRange {
    pub fn admits(&self, version: u32) -> bool {
        self.min.map(|value| version >= value).unwrap_or(true)
            && self.max.map(|value| version < value).unwrap_or(true)
    }

    pub fn parse(text: &str) -> Self {
        let text = text.trim_start_matches('\u{feff}');
        let mut range = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some(comment) = line.strip_prefix('#') else {
                break;
            };
            let lowered = comment.to_ascii_lowercase();
            let mut rest = lowered.as_str();
            while let Some(position) = rest.find("majorpatchversion") {
                rest = &rest[position + "majorpatchversion".len()..];
                let tail = rest.trim_start();
                let (operator, number) = if let Some(tail) = tail.strip_prefix(">=") {
                    (">=", tail)
                } else if let Some(tail) = tail.strip_prefix("<=") {
                    ("<=", tail)
                } else if let Some(tail) = tail.strip_prefix('>') {
                    (">", tail)
                } else if let Some(tail) = tail.strip_prefix('<') {
                    ("<", tail)
                } else {
                    continue;
                };
                let digits: String = number
                    .trim_start()
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                let Ok(value) = digits.parse::<u32>() else {
                    continue;
                };
                match operator {
                    ">=" => range.min = Some(value),
                    ">" => range.min = Some(value + 1),
                    "<" => range.max = Some(value),
                    _ => range.max = Some(value + 1),
                }
            }
        }
        range
    }
}

#[derive(Clone, Debug)]
pub struct DefinitionFile {
    pub name: String,
    pub version: u32,
    pub patch: PatchRange,
    pub definition: Definition,
}

enum Slot {
    Declared(Field),
    Reference(String),
}

struct Level {
    meta: Vec<(String, Field)>,
    fields: Vec<(String, Slot)>,
}

impl Level {
    fn new() -> Self {
        Self {
            meta: Vec::new(),
            fields: Vec::new(),
        }
    }

    fn push(&mut self, name: String, field: Field) {
        self.fields.push((name, Slot::Declared(field)));
    }

    fn reference(&mut self, name: String) {
        self.fields.push((name.clone(), Slot::Reference(name)));
    }

    fn into_definition(self) -> Definition {
        let Self { meta, fields } = self;
        let kind_of = |target: &str| {
            fields
                .iter()
                .find_map(|(name, slot)| match slot {
                    Slot::Declared(field) if name == target => Some(field),
                    _ => None,
                })
                .map(|field| match field {
                    Field::Array { .. } => Field::RefArray(target.to_string()),
                    Field::Bytes => Field::RefBytes(target.to_string()),
                    _ => Field::RefString(target.to_string()),
                })
                .unwrap_or_else(|| Field::RefString(target.to_string()))
        };
        let mut out = Vec::with_capacity(meta.len() + fields.len());
        out.extend(meta);
        for (name, slot) in &fields {
            match slot {
                Slot::Declared(field) => out.push((name.clone(), field.clone())),
                Slot::Reference(target) => out.push((name.clone(), kind_of(target))),
            }
        }
        Definition { fields: out }
    }
}

fn meta_target(kinds: &[bool], names: &[String], field: &str) -> (usize, String) {
    let mut level = kinds.len() - 1;
    let mut path = Vec::new();
    while level > 0 && kinds[level] {
        path.push(names[level - 1].clone());
        level -= 1;
    }
    path.reverse();
    path.push(field.to_string());
    (level, path.join("."))
}

fn close_level(
    parent: &mut Level,
    name: String,
    definition: Definition,
    is_object: bool,
    subtype: Option<Primitive>,
    string_subtype: bool,
) {
    if is_object {
        parent.push(name, Field::Object(definition));
        return;
    }
    parent.push(
        name,
        Field::Array {
            subtype,
            string_subtype,
            definition,
        },
    );
}

fn split_type(token: &str) -> (&str, Option<String>) {
    let base = token
        .find(['<', '['])
        .map(|position| &token[..position])
        .unwrap_or(token);
    let subtype = token.find('<').and_then(|open| {
        let tail = &token[open + 1..];
        tail.find('>')
            .map(|close| tail[..close].trim().to_string())
    });
    (base, subtype)
}

pub fn parse(text: &str) -> Result<Definition, DefError> {
    let text = text.trim_start_matches('\u{feff}');
    let mut stack: Vec<Level> = vec![Level::new()];
    let mut kinds: Vec<bool> = vec![false];
    let mut names: Vec<String> = Vec::new();
    let mut subtypes: Vec<(Option<Primitive>, bool)> = Vec::new();
    let mut implicit_meta = true;

    for (number, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut depth = 0usize;
        let mut rest = line;
        loop {
            let trimmed = rest.trim_start();
            if let Some(stripped) = trimmed.strip_prefix('-') {
                depth += 1;
                rest = stripped;
            } else {
                rest = trimmed;
                break;
            }
        }
        let mut parts = rest.split_whitespace();
        let (Some(type_token), Some(name)) = (parts.next(), parts.next()) else {
            return Err(DefError::Malformed {
                line: number + 1,
                text: line.to_string(),
            });
        };
        let (type_name, subtype) = split_type(type_token);

        if type_name == "count" {
            continue;
        }
        let type_name = if type_name == "offset" {
            implicit_meta = false;
            "ref"
        } else {
            type_name
        };
        if type_name == "ref" {
            implicit_meta = false;
        }

        while depth + 1 < stack.len() {
            let level = stack.pop().expect("stack depth");
            let is_object = kinds.pop().unwrap_or(false);
            let definition = level.into_definition();
            let child_name = names.pop().unwrap_or_default();
            let (subtype, string_subtype) = subtypes.pop().unwrap_or((None, false));
            let parent = stack.last_mut().expect("parent level");
            close_level(parent, child_name, definition, is_object, subtype, string_subtype);
        }
        if depth + 1 > stack.len() {
            return Err(DefError::Nesting(number + 1));
        }

        if type_name == "ref" {
            stack.last_mut().expect("level").reference(name.to_string());
            continue;
        }

        let field = match type_name {
            "array" => {
                let string_subtype = subtype.as_deref() == Some("string");
                let primitive = subtype.as_deref().and_then(Primitive::parse);
                if implicit_meta {
                    let (target, key) = meta_target(&kinds, &names, name);
                    stack[target]
                        .meta
                        .push((key.clone(), Field::RefArray(key)));
                }
                stack.push(Level::new());
                kinds.push(false);
                names.push(name.to_string());
                subtypes.push((primitive, string_subtype));
                continue;
            }
            "object" => {
                stack.push(Level::new());
                kinds.push(true);
                names.push(name.to_string());
                subtypes.push((None, false));
                continue;
            }
            "string" => {
                if implicit_meta {
                    let (target, key) = meta_target(&kinds, &names, name);
                    stack[target]
                        .meta
                        .push((key.clone(), Field::RefString(key)));
                }
                Field::Str
            }
            "bytes" => {
                if implicit_meta {
                    let (target, key) = meta_target(&kinds, &names, name);
                    stack[target]
                        .meta
                        .push((key.clone(), Field::RefBytes(key)));
                }
                Field::Bytes
            }
            other => match Primitive::parse(other) {
                Some(primitive) => Field::Value(primitive),
                None => {
                    return Err(DefError::UnknownType {
                        line: number + 1,
                        name: other.to_string(),
                    })
                }
            },
        };
        stack.last_mut().expect("level").push(name.to_string(), field);
    }

    while stack.len() > 1 {
        let level = stack.pop().expect("stack depth");
        let is_object = kinds.pop().unwrap_or(false);
        let definition = level.into_definition();
        let child_name = names.pop().unwrap_or_default();
        let (subtype, string_subtype) = subtypes.pop().unwrap_or((None, false));
        let parent = stack.last_mut().expect("parent level");
        close_level(parent, child_name, definition, is_object, subtype, string_subtype);
    }

    Ok(stack.pop().expect("root level").into_definition())
}


pub fn read_file(path: impl AsRef<Path>) -> Result<DefinitionFile, DefError> {
    let path = path.as_ref();
    let stem = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .ok_or_else(|| DefError::BadName(path.display().to_string()))?;
    let mut parts = stem.trim_end_matches(".def").split('.');
    let name = parts
        .next()
        .ok_or_else(|| DefError::BadName(stem.clone()))?
        .to_string();
    let version = parts
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| DefError::BadName(stem.clone()))?;
    let text = std::fs::read_to_string(path)?;
    let definition = parse(&text)?;
    Ok(DefinitionFile {
        name,
        version,
        patch: PatchRange::parse(&text),
        definition,
    })
}

#[cfg(test)]
mod patch_range_tests {
    use super::PatchRange;

    #[test]
    fn parses_a_lower_bound() {
        let range = PatchRange::parse("# majorPatchVersion >= 86\n\nint32 x\n");
        assert_eq!(range.min, Some(86));
        assert_eq!(range.max, None);
        assert!(range.admits(100));
        assert!(!range.admits(85));
    }

    #[test]
    fn parses_a_bounded_range() {
        let range = PatchRange::parse("# majorPatchVersion >= 93 && majorPatchVersion < 105\n");
        assert_eq!((range.min, range.max), (Some(93), Some(105)));
        assert!(range.admits(100));
        assert!(!range.admits(105));
        assert!(!range.admits(92));
    }

    #[test]
    fn accepts_the_capitalised_spelling() {
        assert_eq!(PatchRange::parse("# MajorPatchVersion >= 67\n").min, Some(67));
    }

    #[test]
    fn an_unconstrained_file_admits_everything() {
        let range = PatchRange::parse("uint32 unk1\n");
        assert_eq!(range, PatchRange::default());
        assert!(range.admits(1));
        assert!(range.admits(500));
    }

    #[test]
    fn stops_at_the_first_real_line() {
        let range = PatchRange::parse("uint32 x # majorPatchVersion >= 200\n");
        assert_eq!(range.min, None);
    }
}

#[cfg(test)]
mod reference_position_tests {
    use super::{parse, Field};

    #[test]
    fn an_explicit_reference_stays_where_it_is_declared() {
        let definition = parse(
            "int32 a\nref name\nref data\nint32 b\nstring name\nbytes data\n",
        )
        .expect("parse");
        let kinds: Vec<&str> = definition
            .fields
            .iter()
            .map(|(_, field)| match field {
                Field::Value(_) => "value",
                Field::RefString(_) => "refstring",
                Field::RefBytes(_) => "refbytes",
                Field::RefArray(_) => "refarray",
                Field::Str => "str",
                Field::Bytes => "bytes",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            ["value", "refstring", "refbytes", "value", "str", "bytes"]
        );
    }

    #[test]
    fn a_reference_knows_the_kind_of_its_target() {
        let definition = parse("ref rows\narray rows\n- int32 x\n").expect("parse");
        assert!(matches!(definition.fields[0].1, Field::RefArray(_)));
    }
}

#[cfg(test)]
mod type_token_tests {
    use super::{split_type, parse, Field, Primitive};

    #[test]
    fn a_subtype_and_a_flag_are_independent() {
        assert_eq!(split_type("array"), ("array", None));
        assert_eq!(split_type("array<int32>"), ("array", Some("int32".into())));
        assert_eq!(split_type("array<string>"), ("array", Some("string".into())));
        assert_eq!(
            split_type("array<vec3>[interleaved]"),
            ("array", Some("vec3".into())),
            "the flag must not leak into the subtype"
        );
        assert_eq!(split_type("array[interleaved]"), ("array", None));
        assert_eq!(split_type("int32"), ("int32", None));
    }

    #[test]
    fn a_flagged_array_keeps_its_element_type() {
        let definition = parse("array<vec3>[interleaved] points\n").expect("parse");
        let field = definition.find("points").expect("points");
        match field {
            Field::Array { subtype, .. } => assert_eq!(*subtype, Some(Primitive::Vec3)),
            other => panic!("expected an array, got {other:?}"),
        }
    }
}
