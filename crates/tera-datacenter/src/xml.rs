use crate::build::{BuildValue, Builder};
use crate::error::{DataCenterError, Result};
use crate::format::{TypeCode, TEXT_NAME};
use crate::template::Template;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::path::Path;

fn exact_int(literal: &str) -> Option<i32> {
    let number: i32 = literal.parse().ok()?;
    (number.to_string() == literal).then_some(number)
}

fn exact_float(literal: &str) -> Option<f32> {
    let number: f32 = literal.parse().ok()?;
    (crate::node::format_float(number) == literal).then_some(number)
}

pub struct Importer<'a> {
    pub builder: Builder,
    template: Option<&'a Template>,
}

impl<'a> Importer<'a> {
    pub fn new(template: Option<&'a Template>) -> Self {
        let mut builder = Builder::new();
        if let Some(template) = template {
            let mut names = crate::build::Interner::default();
            for name in &template.names {
                names.intern(name);
            }
            names.intern(crate::format::ROOT_NAME);
            names.intern(TEXT_NAME);
            let root_name = names.intern(crate::format::ROOT_NAME);
            builder.names = names;
            builder.keys = template.keys.clone();
            builder.revision = template.revision;
            builder.timestamp = template.timestamp;
            builder.node_mut(builder.root).name = root_name;
        }
        Self { builder, template }
    }

    pub fn read_file(&mut self, path: &Path) -> Result<usize> {
        let text = std::fs::read_to_string(path)?;
        self.read_str(&text)
    }

    pub fn read_str(&mut self, text: &str) -> Result<usize> {
        let mut reader = Reader::from_str(text);
        reader.config_mut().trim_text(false);
        let mut stack: Vec<u32> = vec![self.builder.root];
        let mut paths: Vec<String> = vec![String::new()];
        let mut texts: Vec<String> = vec![String::new()];
        let mut child_counts: Vec<usize> = vec![0];
        let mut roots = 0usize;
        loop {
            match reader
                .read_event()
                .map_err(|error| DataCenterError::Query(error.to_string()))?
            {
                Event::Start(element) if element.name().as_ref() == "Collection" => {
                    stack.push(*stack.last().expect("root on the stack"));
                    paths.push(paths.last().cloned().unwrap_or_default());
                    texts.push(String::new());
                    child_counts.push(0);
                }
                Event::Start(element) => {
                    let name = element.name().as_ref().to_string();
                    let parent = *stack.last().expect("root on the stack");
                    let path = format!("{}/{name}", paths.last().expect("root path"));
                    let name_index = self.builder.name_index(&name);
                    let id = self.builder.add_node(name_index);
                    self.builder.attach(parent, id);
                    if let Some(slot) = child_counts.last_mut() {
                        *slot += 1;
                    }
                    if parent == self.builder.root {
                        roots += 1;
                    }
                    let info = self.template.and_then(|template| template.info(&path));
                    if let Some(info) = info {
                        let node = self.builder.node_mut(id);
                        node.key_flags = info.key_flags;
                        node.key_index = info.key_index;
                    }
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| DataCenterError::Query(error.to_string()))?;
                        let key = attribute.key.as_ref().to_string();
                        let raw = attribute
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(|error| DataCenterError::Query(error.to_string()))?;
                        let value = self.convert(&path, &key, &raw);
                        let key_index = self.builder.name_index(&key);
                        self.builder.node_mut(id).attributes.push((key_index, value));
                    }
                    stack.push(id);
                    paths.push(path);
                    texts.push(String::new());
                    child_counts.push(0);
                }
                Event::Empty(element) => {
                    let name = element.name().as_ref().to_string();
                    let parent = *stack.last().expect("root on the stack");
                    let path = format!("{}/{name}", paths.last().expect("root path"));
                    let name_index = self.builder.name_index(&name);
                    let id = self.builder.add_node(name_index);
                    self.builder.attach(parent, id);
                    if let Some(slot) = child_counts.last_mut() {
                        *slot += 1;
                    }
                    if parent == self.builder.root {
                        roots += 1;
                    }
                    if let Some(info) = self.template.and_then(|template| template.info(&path)) {
                        let node = self.builder.node_mut(id);
                        node.key_flags = info.key_flags;
                        node.key_index = info.key_index;
                    }
                    for attribute in element.attributes() {
                        let attribute =
                            attribute.map_err(|error| DataCenterError::Query(error.to_string()))?;
                        let key = attribute.key.as_ref().to_string();
                        let raw = attribute
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .map_err(|error| DataCenterError::Query(error.to_string()))?;
                        let value = self.convert(&path, &key, &raw);
                        let key_index = self.builder.name_index(&key);
                        self.builder.node_mut(id).attributes.push((key_index, value));
                    }
                }
                Event::End(_) => {
                    let id = stack.pop().expect("node on the stack");
                    paths.pop();
                    let text = texts.pop().unwrap_or_default();
                    let children = child_counts.pop().unwrap_or(0);
                    if id != self.builder.root && !text.is_empty() {
                        let value = if children == 0 {
                            text
                        } else if text.trim().is_empty() {
                            String::new()
                        } else {
                            text.trim().to_string()
                        };
                        if !value.is_empty() {
                            let index = self.builder.value_index(&value);
                            let name_index = self.builder.name_index(TEXT_NAME);
                            self.builder
                                .node_mut(id)
                                .attributes
                                .push((name_index, BuildValue::Str(index)));
                        }
                    }
                }
                Event::Text(text) => {
                    if let Some(slot) = texts.last_mut() {
                        slot.push_str(&text);
                    }
                }
                Event::GeneralRef(reference) => {
                    let resolved = match reference
                        .resolve_char_ref()
                        .map_err(|error| DataCenterError::Query(error.to_string()))?
                    {
                        Some(character) => character.to_string(),
                        None => match &*reference {
                            "lt" => "<".to_string(),
                            "gt" => ">".to_string(),
                            "amp" => "&".to_string(),
                            "quot" => "\"".to_string(),
                            "apos" => "'".to_string(),
                            other => {
                                return Err(DataCenterError::Query(format!(
                                    "unknown entity `&{other};`"
                                )))
                            }
                        },
                    };
                    if let Some(slot) = texts.last_mut() {
                        slot.push_str(&resolved);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(roots)
    }

    fn convert(&mut self, path: &str, attribute: &str, literal: &str) -> BuildValue {
        let hint = self
            .template
            .and_then(|template| template.info(path))
            .and_then(|info| {
                info.attribute_types
                    .get(attribute)
                    .map(|code| (*code, info.boolean_attributes.iter().any(|name| name == attribute)))
            });
        match hint {
            Some((TypeCode::Int, true)) if literal == "true" || literal == "false" => {
                BuildValue::Bool(literal == "true")
            }
            Some((TypeCode::Int, false)) => match exact_int(literal) {
                Some(number) => BuildValue::Int(number),
                None => BuildValue::Str(self.builder.value_index(literal)),
            },
            Some((TypeCode::Float, _)) => match exact_float(literal) {
                Some(number) => BuildValue::Float(number),
                None => BuildValue::Str(self.builder.value_index(literal)),
            },
            Some((TypeCode::String, _)) => BuildValue::Str(self.builder.value_index(literal)),
            _ => self.infer(literal),
        }
    }

    fn infer(&mut self, literal: &str) -> BuildValue {
        if literal == "true" || literal == "false" {
            return BuildValue::Bool(literal == "true");
        }
        if let Some(number) = exact_int(literal) {
            return BuildValue::Int(number);
        }
        if literal.contains(['.', 'e', 'E']) {
            if let Some(number) = exact_float(literal) {
                return BuildValue::Float(number);
            }
        }
        BuildValue::Str(self.builder.value_index(literal))
    }
}
