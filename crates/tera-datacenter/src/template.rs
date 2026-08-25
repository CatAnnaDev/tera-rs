use crate::error::Result;
use crate::file::DataCenter;
use crate::format::{KeyDefinition, TypeCode};
use crate::node::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct PathInfo {
    pub key_flags: u8,
    pub key_index: u16,
    pub attribute_types: HashMap<String, TypeCode>,
    pub boolean_attributes: Vec<String>,
}

pub struct Template {
    pub keys: Vec<KeyDefinition>,
    pub names: Vec<String>,
    pub revision: u32,
    pub timestamp: f64,
    pub paths: HashMap<String, PathInfo>,
}

impl Template {
    pub fn from_datacenter(dc: &DataCenter) -> Result<Self> {
        let mut template = Self {
            keys: dc.keys.clone(),
            names: dc.names_iter().map(|name| name.to_string()).collect(),
            revision: dc.header.revision,
            timestamp: dc.header.timestamp,
            paths: HashMap::new(),
        };
        let root = dc.root()?;
        let mut stack = vec![(String::new(), root)];
        while let Some((prefix, node)) = stack.pop() {
            for child in node.children() {
                let name = child.name()?;
                let mut path = String::with_capacity(prefix.len() + name.len() + 1);
                path.push_str(&prefix);
                path.push('/');
                path.push_str(name);
                if !template.paths.contains_key(&path) {
                    let raw = child.raw();
                    let mut info = PathInfo {
                        key_flags: raw.key_flags,
                        key_index: raw.key_index,
                        ..PathInfo::default()
                    };
                    for attribute in child.attributes() {
                        let attribute_name = attribute.name()?.to_string();
                        let code = match attribute.value()? {
                            Value::Int(_) => TypeCode::Int,
                            Value::Bool(_) => {
                                info.boolean_attributes.push(attribute_name.clone());
                                TypeCode::Int
                            }
                            Value::Float(_) => TypeCode::Float,
                            Value::Str(_) => TypeCode::String,
                        };
                        info.attribute_types.insert(attribute_name, code);
                    }
                    template.paths.insert(path.clone(), info);
                }
                stack.push((path, child));
            }
        }
        Ok(template)
    }

    pub fn info(&self, path: &str) -> Option<&PathInfo> {
        self.paths.get(path)
    }
}
