use crate::error::Result;
use crate::node::{Node, Value};
use crate::TEXT_NAME;
use std::io::Write;

pub fn write_xml<W: Write>(out: &mut W, node: &Node<'_>, declaration: bool) -> Result<()> {
    if declaration {
        out.write_all(b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n")?;
    }
    write_xml_node(out, node, 0)?;
    out.write_all(b"\n")?;
    Ok(())
}

fn write_xml_node<W: Write>(out: &mut W, node: &Node<'_>, depth: usize) -> Result<()> {
    let name = node.name()?;
    for _ in 0..depth {
        out.write_all(b"  ")?;
    }
    out.write_all(b"<")?;
    out.write_all(name.as_bytes())?;
    let mut text: Option<Value<'_>> = None;
    for attribute in node.attributes() {
        let attribute_name = attribute.name()?;
        let value = attribute.value()?;
        if attribute_name == TEXT_NAME {
            text = Some(value);
            continue;
        }
        out.write_all(b" ")?;
        out.write_all(attribute_name.as_bytes())?;
        out.write_all(b"=\"")?;
        write_escaped(out, &value.to_text(), true)?;
        out.write_all(b"\"")?;
    }
    let has_children = node.children().next().is_some();
    if !has_children && text.is_none() {
        out.write_all(b" />")?;
        return Ok(());
    }
    out.write_all(b">")?;
    if let Some(value) = &text {
        write_escaped(out, &value.to_text(), false)?;
    }
    if has_children {
        for child in node.children() {
            out.write_all(b"\n")?;
            write_xml_node(out, &child, depth + 1)?;
        }
        out.write_all(b"\n")?;
        for _ in 0..depth {
            out.write_all(b"  ")?;
        }
    }
    out.write_all(b"</")?;
    out.write_all(name.as_bytes())?;
    out.write_all(b">")?;
    Ok(())
}

fn write_escaped<W: Write>(out: &mut W, text: &str, attribute: bool) -> Result<()> {
    let mut start = 0usize;
    for (index, byte) in text.bytes().enumerate() {
        let replacement: &[u8] = match byte {
            b'&' => b"&amp;",
            b'<' => b"&lt;",
            b'>' => b"&gt;",
            b'"' if attribute => b"&quot;",
            b'\n' | b'\r' | b'\t' if attribute => match byte {
                b'\n' => b"&#10;",
                b'\r' => b"&#13;",
                _ => b"&#9;",
            },
            _ => continue,
        };
        out.write_all(&text.as_bytes()[start..index])?;
        out.write_all(replacement)?;
        start = index + 1;
    }
    out.write_all(&text.as_bytes()[start..])?;
    Ok(())
}

pub fn write_json<W: Write>(out: &mut W, node: &Node<'_>, pretty: bool) -> Result<()> {
    write_json_node(out, node, pretty, 0)?;
    out.write_all(b"\n")?;
    Ok(())
}

fn write_json_node<W: Write>(
    out: &mut W,
    node: &Node<'_>,
    pretty: bool,
    depth: usize,
) -> Result<()> {
    let indent = |out: &mut W, level: usize| -> Result<()> {
        if pretty {
            out.write_all(b"\n")?;
            for _ in 0..level {
                out.write_all(b"  ")?;
            }
        }
        Ok(())
    };
    out.write_all(b"{")?;
    indent(out, depth + 1)?;
    out.write_all(b"\"name\":")?;
    write_json_string(out, node.name()?)?;
    let mut text = None;
    let mut attributes = Vec::new();
    for attribute in node.attributes() {
        let name = attribute.name()?;
        let value = attribute.value()?;
        if name == TEXT_NAME {
            text = Some(value);
        } else {
            attributes.push((name, value));
        }
    }
    if !attributes.is_empty() {
        out.write_all(b",")?;
        indent(out, depth + 1)?;
        out.write_all(b"\"attributes\":{")?;
        for (index, (name, value)) in attributes.iter().enumerate() {
            if index > 0 {
                out.write_all(b",")?;
            }
            indent(out, depth + 2)?;
            write_json_string(out, name)?;
            out.write_all(b":")?;
            write_json_value(out, value)?;
        }
        indent(out, depth + 1)?;
        out.write_all(b"}")?;
    }
    if let Some(value) = text {
        out.write_all(b",")?;
        indent(out, depth + 1)?;
        out.write_all(b"\"text\":")?;
        write_json_value(out, &value)?;
    }
    let mut children = node.children().peekable();
    if children.peek().is_some() {
        out.write_all(b",")?;
        indent(out, depth + 1)?;
        out.write_all(b"\"children\":[")?;
        let mut first = true;
        for child in children {
            if !first {
                out.write_all(b",")?;
            }
            first = false;
            indent(out, depth + 2)?;
            write_json_node(out, &child, pretty, depth + 2)?;
        }
        indent(out, depth + 1)?;
        out.write_all(b"]")?;
    }
    indent(out, depth)?;
    out.write_all(b"}")?;
    Ok(())
}

fn write_json_value<W: Write>(out: &mut W, value: &Value<'_>) -> Result<()> {
    match value {
        Value::Int(number) => write!(out, "{number}")?,
        Value::Bool(flag) => out.write_all(if *flag { b"true" } else { b"false" })?,
        Value::Float(number) => {
            if number.is_finite() {
                write!(out, "{}", crate::node::format_float(*number))?
            } else {
                write_json_string(out, &number.to_string())?
            }
        }
        Value::Str(text) => write_json_string(out, text)?,
    }
    Ok(())
}

fn write_json_string<W: Write>(out: &mut W, text: &str) -> Result<()> {
    out.write_all(b"\"")?;
    let mut start = 0usize;
    for (index, character) in text.char_indices() {
        let replacement: &[u8] = match character {
            '"' => b"\\\"",
            '\\' => b"\\\\",
            '\n' => b"\\n",
            '\r' => b"\\r",
            '\t' => b"\\t",
            control if (control as u32) < 0x20 => {
                out.write_all(&text.as_bytes()[start..index])?;
                write!(out, "\\u{:04x}", control as u32)?;
                start = index + control.len_utf8();
                continue;
            }
            _ => continue,
        };
        out.write_all(&text.as_bytes()[start..index])?;
        out.write_all(replacement)?;
        start = index + character.len_utf8();
    }
    out.write_all(&text.as_bytes()[start..])?;
    out.write_all(b"\"")?;
    Ok(())
}
