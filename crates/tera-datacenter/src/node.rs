use crate::error::Result;
use crate::file::DataCenter;
use crate::format::*;
use std::borrow::Cow;

#[derive(Clone, Copy)]
pub struct Node<'a> {
    dc: &'a DataCenter,
    address: Address,
    raw: RawNode,
}

#[derive(Clone, Copy)]
pub struct Attribute<'a> {
    dc: &'a DataCenter,
    raw: RawAttribute,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value<'a> {
    Int(i32),
    Bool(bool),
    Float(f32),
    Str(Cow<'a, str>),
}

impl<'a> Value<'a> {
    pub fn as_reference(&self) -> Option<i32> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Int(value) => Some(*value),
            Self::Bool(value) => Some(i32::from(*value)),
            Self::Float(value) => Some(*value as i32),
            Self::Str(text) => text.parse().ok(),
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Self::Int(value) => Some(*value as f32),
            Self::Bool(value) => Some(f32::from(u8::from(*value))),
            Self::Float(value) => Some(*value),
            Self::Str(text) => text.parse().ok(),
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Int(value) => Some(*value != 0),
            Self::Str(text) => match &**text {
                "true" | "True" | "1" => Some(true),
                "false" | "False" | "0" => Some(false),
                _ => None,
            },
            Self::Float(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    pub fn to_text(&self) -> Cow<'a, str> {
        match self {
            Self::Int(value) => Cow::Owned(value.to_string()),
            Self::Bool(value) => Cow::Borrowed(if *value { "true" } else { "false" }),
            Self::Float(value) => Cow::Owned(format_float(*value)),
            Self::Str(text) => text.clone(),
        }
    }
}

pub fn format_float(value: f32) -> String {
    if value == value.trunc() && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        let mut text = format!("{value}");
        if text.contains('e') {
            text = format!("{value:e}");
        }
        text
    }
}

impl<'a> Attribute<'a> {
    pub fn name(&self) -> Result<&'a str> {
        self.dc.name(self.raw.name_index)
    }

    pub fn name_index(&self) -> u16 {
        self.raw.name_index
    }

    pub fn value(&self) -> Result<Value<'a>> {
        Ok(match self.raw.type_code() {
            Some(TypeCode::Int) if self.raw.is_bool() => Value::Bool(self.raw.value != 0),
            Some(TypeCode::Int) => Value::Int(self.raw.value as i32),
            Some(TypeCode::Float) => Value::Float(f32::from_bits(self.raw.value)),
            Some(TypeCode::String) => {
                Value::Str(self.dc.value_string(Address::from_raw(self.raw.value))?)
            }
            None => Value::Int(self.raw.value as i32),
        })
    }

    pub fn raw(&self) -> RawAttribute {
        self.raw
    }
}

impl<'a> Node<'a> {
    pub fn new(dc: &'a DataCenter, address: Address) -> Result<Self> {
        Ok(Self {
            dc,
            address,
            raw: dc.raw_node(address)?,
        })
    }

    pub fn address(&self) -> Address {
        self.address
    }

    pub fn raw(&self) -> RawNode {
        self.raw
    }

    pub fn is_placeholder(&self) -> bool {
        self.raw.name_index == 0
    }

    pub fn name(&self) -> Result<&'a str> {
        self.dc.name(self.raw.name_index)
    }

    pub fn attribute_count(&self) -> u16 {
        self.raw.attribute_count
    }

    pub fn child_count(&self) -> u16 {
        self.raw.child_count
    }

    pub fn attributes(&self) -> AttributeIter<'a> {
        AttributeIter {
            dc: self.dc,
            address: self.raw.attribute_address,
            index: 0,
            count: self.raw.attribute_count,
        }
    }

    pub fn children(&self) -> NodeIter<'a> {
        NodeIter {
            dc: self.dc,
            address: self.raw.child_address,
            index: 0,
            count: self.raw.child_count,
        }
    }

    pub fn children_named(&self, name: &'a str) -> impl Iterator<Item = Node<'a>> + 'a {
        self.children()
            .filter(move |child| child.name().map(|value| value == name).unwrap_or(false))
    }

    pub fn attribute(&self, name: &str) -> Option<Attribute<'a>> {
        self.attributes()
            .find(|attribute| attribute.name().map(|value| value == name).unwrap_or(false))
    }

    pub fn get(&self, name: &str) -> Option<Value<'a>> {
        self.attribute(name).and_then(|attr| attr.value().ok())
    }

    pub fn text(&self) -> Option<Value<'a>> {
        self.get(TEXT_NAME)
    }

    pub fn key_definition(&self) -> Option<&'a KeyDefinition> {
        self.dc.keys.get(usize::from(self.raw.key_index))
    }
}

#[derive(Clone)]
pub struct AttributeIter<'a> {
    dc: &'a DataCenter,
    address: Address,
    index: u16,
    count: u16,
}

impl<'a> Iterator for AttributeIter<'a> {
    type Item = Attribute<'a>;

    fn next(&mut self) -> Option<Attribute<'a>> {
        while self.index < self.count {
            let index = self.index;
            self.index += 1;
            if let Ok(raw) = self.dc.raw_attribute(self.address, index) {
                return Some(Attribute { dc: self.dc, raw });
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::from(self.count - self.index);
        (0, Some(remaining))
    }
}

#[derive(Clone)]
pub struct NodeIter<'a> {
    dc: &'a DataCenter,
    address: Address,
    index: u16,
    count: u16,
}

impl<'a> Iterator for NodeIter<'a> {
    type Item = Node<'a>;

    fn next(&mut self) -> Option<Node<'a>> {
        while self.index < self.count {
            let index = self.index;
            self.index += 1;
            let element = u32::from(self.address.element) + u32::from(index);
            if element > u32::from(u16::MAX) {
                return None;
            }
            let address = Address {
                segment: self.address.segment,
                element: element as u16,
            };
            match Node::new(self.dc, address) {
                Ok(node) if !node.is_placeholder() => return Some(node),
                _ => continue,
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::from(self.count - self.index);
        (0, Some(remaining))
    }
}
