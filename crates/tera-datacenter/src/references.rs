use crate::file::DataCenter;
use crate::format::Address;
use crate::node::Node;
use std::collections::HashMap;

#[derive(Clone, Copy)]
struct SheetRef {
    sheet: u32,
    address: Address,
}

#[derive(Clone, Copy)]
pub struct Backlink {
    pub sheet: u32,
    pub attribute: u32,
    pub address: Address,
}

pub struct Target {
    pub sheet: String,
    pub address: Address,
}

pub struct Reference {
    pub attribute: String,
    pub value: i32,
    pub targets: Vec<Target>,
    pub total: usize,
    pub ambiguous: bool,
}

#[derive(Default)]
struct Interner {
    names: Vec<String>,
    lookup: HashMap<String, u32>,
}

impl Interner {
    fn intern(&mut self, name: &str) -> u32 {
        if let Some(id) = self.lookup.get(name) {
            return *id;
        }
        let id = self.names.len() as u32;
        self.names.push(name.to_string());
        self.lookup.insert(name.to_string(), id);
        id
    }
}

pub struct RefIndex {
    sheets: Vec<String>,
    sheets_lower: Vec<String>,
    attrs: Vec<String>,
    by_id: HashMap<i32, Vec<SheetRef>>,
    backlinks: HashMap<i32, Vec<Backlink>>,
}

impl RefIndex {
    pub fn build(dc: &DataCenter) -> Self {
        let mut sheets = Interner::default();
        let mut attrs = Interner::default();
        let mut by_id: HashMap<i32, Vec<SheetRef>> = HashMap::new();
        let mut backlinks: HashMap<i32, Vec<Backlink>> = HashMap::new();
        if let Ok(root) = dc.root() {
            for sheet in root.children() {
                let Ok(sheet_name) = sheet.name() else {
                    continue;
                };
                let sheet_id = sheets.intern(sheet_name);
                for record in sheet.children() {
                    let address = record.address();
                    if let Some(id) = record.get("id").and_then(|value| value.as_i32()) {
                        by_id.entry(id).or_default().push(SheetRef {
                            sheet: sheet_id,
                            address,
                        });
                    }
                    for attribute in record.attributes() {
                        let (Ok(name), Ok(value)) = (attribute.name(), attribute.value()) else {
                            continue;
                        };
                        if !is_reference_attr(name) {
                            continue;
                        }
                        let Some(target) = value.as_reference() else {
                            continue;
                        };
                        if target == 0 {
                            continue;
                        }
                        let attribute_id = attrs.intern(name);
                        backlinks.entry(target).or_default().push(Backlink {
                            sheet: sheet_id,
                            attribute: attribute_id,
                            address,
                        });
                    }
                }
            }
        }
        let sheets_lower = sheets.names.iter().map(|name| name.to_ascii_lowercase()).collect();
        Self {
            sheets: sheets.names,
            sheets_lower,
            attrs: attrs.names,
            by_id,
            backlinks,
        }
    }

    pub fn sheet_name(&self, id: u32) -> &str {
        self.sheets.get(id as usize).map(String::as_str).unwrap_or("?")
    }

    pub fn attr_name(&self, id: u32) -> &str {
        self.attrs.get(id as usize).map(String::as_str).unwrap_or("?")
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
            let Some(target) = value.as_reference() else {
                continue;
            };
            if target == 0 {
                continue;
            }
            let mut hits: Vec<SheetRef> = self.by_id.get(&target).cloned().unwrap_or_default();
            let total = hits.len();
            let confident = self.rank(name, &mut hits);
            let ambiguous = confident == 0;
            let keep = if confident > 0 {
                confident.min(8)
            } else {
                total.min(2)
            };
            hits.truncate(keep);
            let targets = hits
                .into_iter()
                .map(|hit| Target {
                    sheet: self.sheets[hit.sheet as usize].clone(),
                    address: hit.address,
                })
                .collect();
            refs.push(Reference {
                attribute: name.to_string(),
                value: target,
                targets,
                total,
                ambiguous,
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

    fn rank(&self, attribute: &str, hits: &mut [SheetRef]) -> usize {
        let lowered = attribute.to_ascii_lowercase();
        let stem = lowered.strip_suffix("id").unwrap_or(&lowered);
        if stem.len() < 3 {
            return 0;
        }
        hits.sort_by_key(|hit| !self.sheets_lower[hit.sheet as usize].contains(stem));
        hits.iter()
            .filter(|hit| self.sheets_lower[hit.sheet as usize].contains(stem))
            .count()
    }
}

pub fn asset_references(node: &Node) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for attribute in node.attributes() {
        let (Ok(name), Ok(value)) = (attribute.name(), attribute.value()) else {
            continue;
        };
        if !is_asset_ref(name) {
            continue;
        }
        if let Some(text) = value.as_str() {
            if !text.is_empty() {
                out.push((name.to_string(), text.to_string()));
            }
        }
    }
    out
}

fn is_reference_attr(name: &str) -> bool {
    if name.eq_ignore_ascii_case("id") {
        return false;
    }
    name.to_ascii_lowercase().ends_with("id")
}

fn is_asset_ref(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "icon", "model", "mesh", "texture", "sound", "gfx", "effect", "movie", "material",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
}
