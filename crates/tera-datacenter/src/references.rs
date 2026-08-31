use crate::file::DataCenter;
use crate::format::Address;
use crate::node::Node;
use std::collections::HashMap;

#[derive(Clone)]
pub struct Target {
    pub sheet: String,
    pub address: Address,
    pub node_name: String,
}

#[derive(Clone)]
pub struct Backlink {
    pub sheet: String,
    pub attribute: String,
    pub address: Address,
    pub node_name: String,
}

pub struct Reference {
    pub attribute: String,
    pub value: i32,
    pub targets: Vec<Target>,
}

pub struct RefIndex {
    by_id: HashMap<i32, Vec<Target>>,
    backlinks: HashMap<i32, Vec<Backlink>>,
}

impl RefIndex {
    pub fn build(dc: &DataCenter) -> Self {
        let mut by_id: HashMap<i32, Vec<Target>> = HashMap::new();
        let mut backlinks: HashMap<i32, Vec<Backlink>> = HashMap::new();
        if let Ok(root) = dc.root() {
            for sheet in root.children() {
                let Ok(sheet_name) = sheet.name() else {
                    continue;
                };
                for record in sheet.children() {
                    let address = record.address();
                    let node_name = record.name().unwrap_or("?").to_string();
                    if let Some(id) = record.get("id").and_then(|value| value.as_i32()) {
                        by_id.entry(id).or_default().push(Target {
                            sheet: sheet_name.to_string(),
                            address,
                            node_name: node_name.clone(),
                        });
                    }
                    for attribute in record.attributes() {
                        let (Ok(name), Ok(value)) = (attribute.name(), attribute.value()) else {
                            continue;
                        };
                        if !is_reference_attr(name) {
                            continue;
                        }
                        let Some(target) = value.as_i32() else {
                            continue;
                        };
                        if target == 0 {
                            continue;
                        }
                        backlinks.entry(target).or_default().push(Backlink {
                            sheet: sheet_name.to_string(),
                            attribute: name.to_string(),
                            address,
                            node_name: node_name.clone(),
                        });
                    }
                }
            }
        }
        Self { by_id, backlinks }
    }

    pub fn outbound(&self, node: &Node) -> Vec<Reference> {
        let mut refs = Vec::new();
        for attribute in node.attributes() {
            let (Ok(name), Ok(value)) = (attribute.name(), attribute.value()) else {
                continue;
            };
            if !is_reference_attr(name) {
                continue;
            }
            let Some(target) = value.as_i32() else {
                continue;
            };
            if target == 0 {
                continue;
            }
            let mut targets = self.by_id.get(&target).cloned().unwrap_or_default();
            rank_targets(name, &mut targets);
            refs.push(Reference {
                attribute: name.to_string(),
                value: target,
                targets,
            });
        }
        refs
    }

    pub fn incoming(&self, id: i32) -> &[Backlink] {
        self.backlinks.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn indexed_ids(&self) -> usize {
        self.by_id.len()
    }
}

fn is_reference_attr(name: &str) -> bool {
    if name.eq_ignore_ascii_case("id") {
        return false;
    }
    name.to_ascii_lowercase().ends_with("id")
}

fn rank_targets(attribute: &str, targets: &mut [Target]) {
    let lowered = attribute.to_ascii_lowercase();
    let stem = lowered.strip_suffix("id").unwrap_or(&lowered);
    if stem.len() < 3 {
        return;
    }
    targets.sort_by_key(|target| !target.sheet.to_ascii_lowercase().contains(stem));
}
