use crate::error::{DataCenterError, Result};
use crate::file::DataCenter;
use crate::format::*;
use crate::hash::{string_hash_units, table_segment_index, value_hash_units};
use crate::node::Value;
use std::collections::HashMap;

pub const SEGMENT_CAPACITY: u32 = 65535;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BuildValue {
    Int(i32),
    Bool(bool),
    Float(f32),
    Str(u32),
}

#[derive(Clone, Debug, Default)]
pub struct BuildNode {
    pub name: u32,
    pub key_flags: u8,
    pub key_index: u16,
    pub attributes: Vec<(u32, BuildValue)>,
    pub children: Vec<u32>,
}

#[derive(Default)]
pub struct Interner {
    values: Vec<String>,
    lookup: HashMap<String, u32>,
}

impl Interner {
    pub fn intern(&mut self, value: &str) -> u32 {
        if let Some(index) = self.lookup.get(value) {
            return *index;
        }
        let index = self.values.len() as u32;
        self.values.push(value.to_string());
        self.lookup.insert(value.to_string(), index);
        index
    }

    pub fn get(&self, index: u32) -> &str {
        self.values.get(index as usize).map(|v| &**v).unwrap_or("")
    }

    pub fn index_of(&self, value: &str) -> Option<u32> {
        self.lookup.get(value).copied()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.values.iter().map(|value| &**value)
    }
}

pub struct Builder {
    pub names: Interner,
    pub values: Interner,
    pub keys: Vec<KeyDefinition>,
    pub revision: u32,
    pub timestamp: f64,
    pub nodes: Vec<BuildNode>,
    pub root: u32,
}

impl Builder {
    pub fn new() -> Self {
        let mut names = Interner::default();
        let root_name = names.intern(ROOT_NAME);
        names.intern(TEXT_NAME);
        let nodes = vec![BuildNode {
            name: root_name,
            ..BuildNode::default()
        }];
        Self {
            names,
            values: Interner::default(),
            keys: vec![KeyDefinition::default()],
            revision: 0,
            timestamp: -1.0,
            nodes,
            root: 0,
        }
    }

    pub fn from_datacenter(dc: &DataCenter) -> Result<Self> {
        let mut names = Interner::default();
        for name in dc.names_iter() {
            names.intern(name);
        }
        names.intern(ROOT_NAME);
        names.intern(TEXT_NAME);
        let mut builder = Self {
            names,
            values: Interner::default(),
            keys: dc.keys.clone(),
            revision: dc.header.revision,
            timestamp: dc.header.timestamp,
            nodes: Vec::with_capacity(dc.node_count() as usize),
            root: 0,
        };
        builder.nodes.push(BuildNode::default());
        let root = dc.root()?;
        builder.convert(&root, 0)?;
        Ok(builder)
    }

    fn convert(&mut self, node: &crate::node::Node<'_>, target: u32) -> Result<()> {
        let raw = node.raw();
        let name = self.names.intern(node.name()?);
        let mut attributes = Vec::with_capacity(raw.attribute_count as usize);
        for attribute in node.attributes() {
            let attribute_name = self.names.intern(attribute.name()?);
            let value = match attribute.value()? {
                Value::Int(value) => BuildValue::Int(value),
                Value::Bool(value) => BuildValue::Bool(value),
                Value::Float(value) => BuildValue::Float(value),
                Value::Str(text) => BuildValue::Str(self.values.intern(&text)),
            };
            attributes.push((attribute_name, value));
        }
        let mut children = Vec::with_capacity(raw.child_count as usize);
        for child in node.children() {
            let id = self.nodes.len() as u32;
            self.nodes.push(BuildNode::default());
            children.push((id, child));
        }
        let child_ids = children.iter().map(|(id, _)| *id).collect();
        self.nodes[target as usize] = BuildNode {
            name,
            key_flags: raw.key_flags,
            key_index: raw.key_index,
            attributes,
            children: child_ids,
        };
        for (id, child) in children {
            self.convert(&child, id)?;
        }
        Ok(())
    }

    pub fn add_node(&mut self, name: u32) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(BuildNode {
            name,
            ..BuildNode::default()
        });
        id
    }

    pub fn attach(&mut self, parent: u32, child: u32) {
        self.nodes[parent as usize].children.push(child);
    }

    pub fn parent_of(&self, child: u32) -> Option<u32> {
        self.nodes
            .iter()
            .position(|node| node.children.contains(&child))
            .map(|index| index as u32)
    }

    pub fn parent_map(&self) -> std::collections::HashMap<u32, u32> {
        let mut parents = std::collections::HashMap::with_capacity(self.nodes.len());
        for (index, node) in self.nodes.iter().enumerate() {
            for child in &node.children {
                parents.insert(*child, index as u32);
            }
        }
        parents
    }

    pub fn detach(&mut self, parent: u32, child: u32) -> bool {
        let children = &mut self.nodes[parent as usize].children;
        let before = children.len();
        children.retain(|existing| *existing != child);
        before != children.len()
    }

    pub fn remove(&mut self, id: u32) -> bool {
        match self.parent_of(id) {
            Some(parent) => self.detach(parent, id),
            None => false,
        }
    }

    pub fn adopt_key(&mut self, parent: u32, child: u32) {
        let wanted = self.node(child).name;
        let sibling = self.nodes[parent as usize]
            .children
            .iter()
            .find(|other| **other != child && self.node(**other).name == wanted)
            .map(|other| (self.node(*other).key_flags, self.node(*other).key_index));
        let node = self.node_mut(child);
        match sibling {
            Some((flags, index)) => {
                node.key_flags = flags;
                node.key_index = index;
            }
            None => {
                node.key_flags = 0;
                node.key_index = 0;
            }
        }
    }

    pub fn insert(&mut self, parent: u32, name: &str) -> u32 {
        let name_index = self.names.intern(name);
        let child = self.add_node(name_index);
        self.attach(parent, child);
        self.adopt_key(parent, child);
        child
    }

    pub fn duplicate(&mut self, id: u32) -> u32 {
        let node = self.nodes[id as usize].clone();
        let copy = self.nodes.len() as u32;
        self.nodes.push(BuildNode {
            name: node.name,
            key_flags: node.key_flags,
            key_index: node.key_index,
            attributes: node.attributes,
            children: Vec::with_capacity(node.children.len()),
        });
        for child in node.children {
            let child_copy = self.duplicate(child);
            self.nodes[copy as usize].children.push(child_copy);
        }
        copy
    }

    pub fn node(&self, id: u32) -> &BuildNode {
        &self.nodes[id as usize]
    }

    pub fn node_mut(&mut self, id: u32) -> &mut BuildNode {
        &mut self.nodes[id as usize]
    }

    pub fn name_index(&mut self, name: &str) -> u32 {
        self.names.intern(name)
    }

    pub fn value_index(&mut self, value: &str) -> u32 {
        self.values.intern(value)
    }

    pub fn value_text(&self, value: BuildValue) -> String {
        match value {
            BuildValue::Int(number) => number.to_string(),
            BuildValue::Bool(flag) => flag.to_string(),
            BuildValue::Float(number) => crate::node::format_float(number),
            BuildValue::Str(index) => self.values.get(index).to_string(),
        }
    }

    pub fn set_attribute(&mut self, id: u32, name: &str, literal: &str) -> Result<BuildValue> {
        let name_index = self.names.intern(name);
        let existing = self.nodes[id as usize]
            .attributes
            .iter()
            .find(|(attribute, _)| *attribute == name_index)
            .map(|(_, value)| *value);
        let value = match existing {
            Some(BuildValue::Int(_)) => BuildValue::Int(
                literal
                    .parse()
                    .map_err(|_| DataCenterError::Query(format!("`{literal}` is not an integer")))?,
            ),
            Some(BuildValue::Bool(_)) => BuildValue::Bool(parse_bool(literal)?),
            Some(BuildValue::Float(_)) => BuildValue::Float(
                literal
                    .parse()
                    .map_err(|_| DataCenterError::Query(format!("`{literal}` is not a float")))?,
            ),
            Some(BuildValue::Str(_)) => BuildValue::Str(self.values.intern(literal)),
            None => infer_value(self, literal),
        };
        let node = &mut self.nodes[id as usize];
        match node
            .attributes
            .iter_mut()
            .find(|(attribute, _)| *attribute == name_index)
        {
            Some(slot) => slot.1 = value,
            None => node.attributes.push((name_index, value)),
        }
        Ok(value)
    }

    pub fn remove_attribute(&mut self, id: u32, name: &str) -> bool {
        let Some(name_index) = self.names.index_of(name) else {
            return false;
        };
        let node = &mut self.nodes[id as usize];
        let before = node.attributes.len();
        node.attributes.retain(|(attribute, _)| *attribute != name_index);
        before != node.attributes.len()
    }

    pub fn pack(&self) -> Result<Vec<u8>> {
        Packer::new(self)?.run()
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_bool(literal: &str) -> Result<bool> {
    match literal {
        "true" | "True" | "1" => Ok(true),
        "false" | "False" | "0" => Ok(false),
        other => Err(DataCenterError::Query(format!("`{other}` is not a boolean"))),
    }
}

fn infer_value(builder: &mut Builder, literal: &str) -> BuildValue {
    if literal == "true" || literal == "false" {
        return BuildValue::Bool(literal == "true");
    }
    if let Ok(number) = literal.parse::<i32>() {
        return BuildValue::Int(number);
    }
    if literal.contains(['.', 'e', 'E']) {
        if let Ok(number) = literal.parse::<f32>() {
            return BuildValue::Float(number);
        }
    }
    BuildValue::Str(builder.values.intern(literal))
}

#[derive(Clone, Copy, Default)]
struct Hash128 {
    low: u64,
    high: u64,
}

impl Hash128 {
    fn new() -> Self {
        Self {
            low: 0xcbf2_9ce4_8422_2325,
            high: 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn feed(&mut self, value: u64) {
        self.low = (self.low ^ value).wrapping_mul(0x0000_0100_0000_01b3);
        self.low ^= self.low >> 29;
        self.high = (self.high.rotate_left(27) ^ value).wrapping_mul(0xff51_afd7_ed55_8ccd);
        self.high ^= self.high >> 33;
    }

    fn finish(self) -> u128 {
        (u128::from(self.high) << 64) | u128::from(self.low)
    }
}

struct StringTableWriter {
    segment_count: usize,
    data: Vec<Vec<u16>>,
    addresses: Vec<Address>,
    entries: Vec<(u32, u32, u32, Address)>,
}

impl StringTableWriter {
    fn new(segment_count: usize) -> Self {
        Self {
            segment_count,
            data: vec![Vec::with_capacity(SEGMENT_CAPACITY as usize)],
            addresses: Vec::new(),
            entries: Vec::new(),
        }
    }

    fn push(&mut self, value: &str) -> Address {
        let mut units: Vec<u16> = value.encode_utf16().collect();
        let hash = string_hash_units(units.iter().copied());
        units.push(0);
        let needed = units.len();
        if self.data.last().map(|segment| segment.len()).unwrap_or(0) + needed
            > SEGMENT_CAPACITY as usize
        {
            self.data
                .push(Vec::with_capacity(SEGMENT_CAPACITY as usize));
        }
        let segment_index = self.data.len() - 1;
        let segment = &mut self.data[segment_index];
        let element_index = segment.len();
        segment.extend_from_slice(&units);
        let address = Address {
            segment: segment_index as u16,
            element: element_index as u16,
        };
        self.addresses.push(address);
        self.entries
            .push((hash, needed as u32, self.addresses.len() as u32, address));
        address
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        let padding = vec![0u8; SEGMENT_CAPACITY as usize * 2];
        for segment in &self.data {
            out.extend_from_slice(&SEGMENT_CAPACITY.to_le_bytes());
            out.extend_from_slice(&(segment.len() as u32).to_le_bytes());
            for unit in segment {
                out.extend_from_slice(&unit.to_le_bytes());
            }
            out.extend_from_slice(&padding[..(SEGMENT_CAPACITY as usize - segment.len()) * 2]);
        }
        let mut buckets: Vec<Vec<&(u32, u32, u32, Address)>> = vec![Vec::new(); self.segment_count];
        for entry in &self.entries {
            let bucket = table_segment_index(entry.0, self.segment_count as u32) as usize;
            buckets[bucket].push(entry);
        }
        for bucket in &mut buckets {
            bucket.sort_by_key(|entry| entry.0);
            out.extend_from_slice(&(bucket.len() as u32).to_le_bytes());
            for entry in bucket.iter() {
                out.extend_from_slice(&entry.0.to_le_bytes());
                out.extend_from_slice(&entry.1.to_le_bytes());
                out.extend_from_slice(&entry.2.to_le_bytes());
                out.extend_from_slice(&entry.3.to_raw().to_le_bytes());
            }
        }
        out.extend_from_slice(&((self.addresses.len() + 1) as u32).to_le_bytes());
        for address in &self.addresses {
            out.extend_from_slice(&address.to_raw().to_le_bytes());
        }
    }
}

struct Packer<'a> {
    builder: &'a Builder,
    name_remap: Vec<u16>,
    ordered_names: Vec<&'a str>,
    value_addresses: Vec<Address>,
    value_hashes: Vec<u16>,
    values: StringTableWriter,
    node_segments: Vec<Vec<RawNode>>,
    attribute_segments: Vec<Vec<RawAttribute>>,
    signatures: Vec<u128>,
    sorted_children: Vec<Vec<u32>>,
    attribute_memo: HashMap<u128, Address>,
    group_memo: HashMap<u128, Address>,
}

impl<'a> Packer<'a> {
    fn new(builder: &'a Builder) -> Result<Self> {
        Ok(Self {
            builder,
            name_remap: Vec::new(),
            ordered_names: Vec::new(),
            value_addresses: Vec::new(),
            value_hashes: Vec::new(),
            values: StringTableWriter::new(VALUE_TABLE_SEGMENTS),
            node_segments: vec![Vec::with_capacity(SEGMENT_CAPACITY as usize)],
            attribute_segments: vec![Vec::with_capacity(SEGMENT_CAPACITY as usize)],
            signatures: vec![0; builder.nodes.len()],
            sorted_children: Vec::new(),
            attribute_memo: HashMap::new(),
            group_memo: HashMap::new(),
        })
    }

    fn run(mut self) -> Result<Vec<u8>> {
        self.prepare_names()?;
        self.prepare_values();
        self.prepare_signatures();
        let root_address = self.allocate_nodes(1);
        self.emit(self.builder.root, root_address);
        self.assemble()
    }

    fn prepare_names(&mut self) -> Result<()> {
        let total = self.builder.names.len();
        let mut remap = vec![0u16; total];
        let mut ordered: Vec<&str> = Vec::with_capacity(total);
        let root = self.builder.names.index_of(ROOT_NAME);
        let text = self.builder.names.index_of(TEXT_NAME);
        for index in 0..total as u32 {
            if Some(index) == root || Some(index) == text {
                continue;
            }
            ordered.push(self.builder.names.get(index));
            remap[index as usize] = ordered.len() as u16;
        }
        for special in [root, text].into_iter().flatten() {
            ordered.push(self.builder.names.get(special));
            remap[special as usize] = ordered.len() as u16;
        }
        if ordered.len() > u16::MAX as usize {
            return Err(DataCenterError::Query(format!(
                "{} names exceed the 65535 name limit",
                ordered.len()
            )));
        }
        self.name_remap = remap;
        self.ordered_names = ordered;
        Ok(())
    }

    fn prepare_values(&mut self) {
        self.value_addresses.reserve(self.builder.values.len());
        self.value_hashes.reserve(self.builder.values.len());
        for index in 0..self.builder.values.len() as u32 {
            let text = self.builder.values.get(index);
            let address = self.values.push(text);
            self.value_addresses.push(address);
            self.value_hashes
                .push(value_hash_units(text.encode_utf16()));
        }
    }

    fn attribute_records(&self, id: u32) -> Vec<RawAttribute> {
        let node = &self.builder.nodes[id as usize];
        let mut records: Vec<RawAttribute> = node
            .attributes
            .iter()
            .map(|(name, value)| {
                let name_index = self.name_remap[*name as usize];
                match value {
                    BuildValue::Int(number) => RawAttribute {
                        name_index,
                        type_info: TypeCode::Int.bits(),
                        value: *number as u32,
                    },
                    BuildValue::Bool(flag) => RawAttribute {
                        name_index,
                        type_info: TypeCode::Int.bits() | (1 << 2),
                        value: u32::from(*flag),
                    },
                    BuildValue::Float(number) => RawAttribute {
                        name_index,
                        type_info: TypeCode::Float.bits(),
                        value: number.to_bits(),
                    },
                    BuildValue::Str(index) => RawAttribute {
                        name_index,
                        type_info: TypeCode::String.bits()
                            | (self.value_hashes[*index as usize] << 2),
                        value: self.value_addresses[*index as usize].to_raw(),
                    },
                }
            })
            .collect();
        records.sort_by_key(|record| record.name_index);
        records
    }

    fn prepare_signatures(&mut self) {
        let count = self.builder.nodes.len();
        self.sorted_children = vec![Vec::new(); count];
        let mut order: Vec<u32> = Vec::with_capacity(count);
        let mut stack = vec![self.builder.root];
        while let Some(id) = stack.pop() {
            order.push(id);
            let mut children = self.builder.nodes[id as usize].children.clone();
            children.sort_by_key(|child| self.name_remap[self.builder.nodes[*child as usize].name as usize]);
            for child in &children {
                stack.push(*child);
            }
            self.sorted_children[id as usize] = children;
        }
        for id in order.into_iter().rev() {
            let node = &self.builder.nodes[id as usize];
            let mut hash = Hash128::new();
            hash.feed(u64::from(self.name_remap[node.name as usize]));
            hash.feed(u64::from(node.key_flags) << 32 | u64::from(node.key_index));
            for record in self.attribute_records(id) {
                hash.feed(u64::from(record.name_index) << 48 | u64::from(record.type_info) << 32);
                hash.feed(u64::from(record.value));
            }
            for child in &self.sorted_children[id as usize] {
                let signature = self.signatures[*child as usize];
                hash.feed(signature as u64);
                hash.feed((signature >> 64) as u64);
            }
            self.signatures[id as usize] = hash.finish();
        }
    }

    fn allocate_nodes(&mut self, count: usize) -> Address {
        if self.node_segments.last().map(Vec::len).unwrap_or(0) + count > SEGMENT_CAPACITY as usize {
            self.node_segments
                .push(Vec::with_capacity(SEGMENT_CAPACITY as usize));
        }
        let segment_index = self.node_segments.len() - 1;
        let segment = &mut self.node_segments[segment_index];
        let element_index = segment.len();
        segment.resize(element_index + count, RawNode::empty());
        Address {
            segment: segment_index as u16,
            element: element_index as u16,
        }
    }

    fn allocate_attributes(&mut self, count: usize) -> Address {
        if self.attribute_segments.last().map(Vec::len).unwrap_or(0) + count
            > SEGMENT_CAPACITY as usize
        {
            self.attribute_segments
                .push(Vec::with_capacity(SEGMENT_CAPACITY as usize));
        }
        let segment_index = self.attribute_segments.len() - 1;
        let segment = &mut self.attribute_segments[segment_index];
        let element_index = segment.len();
        segment.resize(element_index + count, RawAttribute::empty());
        Address {
            segment: segment_index as u16,
            element: element_index as u16,
        }
    }

    fn emit(&mut self, id: u32, address: Address) {
        let records = self.attribute_records(id);
        let attribute_address = if records.is_empty() {
            Address::NONE
        } else {
            let mut hash = Hash128::new();
            for record in &records {
                hash.feed(u64::from(record.name_index) << 48 | u64::from(record.type_info) << 32);
                hash.feed(u64::from(record.value));
            }
            let key = hash.finish();
            match self.attribute_memo.get(&key) {
                Some(existing) => *existing,
                None => {
                    let base = self.allocate_attributes(records.len());
                    let segment = &mut self.attribute_segments[base.segment as usize];
                    for (offset, record) in records.iter().enumerate() {
                        segment[base.element as usize + offset] = *record;
                    }
                    self.attribute_memo.insert(key, base);
                    base
                }
            }
        };

        let children = std::mem::take(&mut self.sorted_children[id as usize]);
        let (child_address, emit_children) = if children.is_empty() {
            (Address::NONE, false)
        } else {
            let mut hash = Hash128::new();
            for child in &children {
                let signature = self.signatures[*child as usize];
                hash.feed(signature as u64);
                hash.feed((signature >> 64) as u64);
            }
            let key = hash.finish();
            match self.group_memo.get(&key) {
                Some(existing) => (*existing, false),
                None => {
                    let base = self.allocate_nodes(children.len());
                    self.group_memo.insert(key, base);
                    (base, true)
                }
            }
        };

        let node = &self.builder.nodes[id as usize];
        self.node_segments[address.segment as usize][address.element as usize] = RawNode {
            name_index: self.name_remap[node.name as usize],
            key_flags: node.key_flags,
            key_index: node.key_index,
            attribute_count: records.len() as u16,
            child_count: children.len() as u16,
            attribute_address,
            child_address,
        };

        if emit_children {
            for (offset, child) in children.iter().enumerate() {
                let target = Address {
                    segment: child_address.segment,
                    element: child_address.element + offset as u16,
                };
                self.emit(*child, target);
            }
        }
        self.sorted_children[id as usize] = children;
    }

    fn assemble(self) -> Result<Vec<u8>> {
        let mut names = StringTableWriter::new(NAME_TABLE_SEGMENTS);
        for name in &self.ordered_names {
            names.push(name);
        }
        let mut out = Vec::with_capacity(1 << 26);
        out.extend_from_slice(&6u32.to_le_bytes());
        out.extend_from_slice(&self.builder.timestamp.to_le_bytes());
        out.extend_from_slice(&self.builder.revision.to_le_bytes());
        out.extend_from_slice(&0i16.to_le_bytes());
        out.extend_from_slice(&0i16.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());

        out.extend_from_slice(&(self.builder.keys.len() as u32).to_le_bytes());
        for key in &self.builder.keys {
            for name_index in key.name_indexes {
                out.extend_from_slice(&name_index.to_le_bytes());
            }
        }

        let attribute_padding = [0u8; ATTRIBUTE_SIZE];
        out.extend_from_slice(&(self.attribute_segments.len() as u32).to_le_bytes());
        for segment in &self.attribute_segments {
            out.extend_from_slice(&SEGMENT_CAPACITY.to_le_bytes());
            out.extend_from_slice(&(segment.len() as u32).to_le_bytes());
            for attribute in segment {
                attribute.write(&mut out);
            }
            for _ in segment.len()..SEGMENT_CAPACITY as usize {
                out.extend_from_slice(&attribute_padding);
            }
        }

        let node_padding = [0u8; NODE_SIZE];
        out.extend_from_slice(&(self.node_segments.len() as u32).to_le_bytes());
        for segment in &self.node_segments {
            out.extend_from_slice(&SEGMENT_CAPACITY.to_le_bytes());
            out.extend_from_slice(&(segment.len() as u32).to_le_bytes());
            for node in segment {
                node.write(&mut out);
            }
            for _ in segment.len()..SEGMENT_CAPACITY as usize {
                out.extend_from_slice(&node_padding);
            }
        }

        self.values.write(&mut out);
        names.write(&mut out);
        out.extend_from_slice(&0i32.to_le_bytes());
        Ok(out)
    }
}
