use crate::error::Result;
use crate::package::Bundle;
use crate::properties::read_export_properties;
use crate::reader::Reader;

pub struct Source {
    pub owner: String,
    pub text: String,
}

fn text_of(data: &[u8], start: usize) -> Option<String> {
    let mut reader = Reader::at(data, start);
    let _position = reader.i32().ok()?;
    let _top = reader.i32().ok()?;
    let text = reader.string().ok()?;
    (!text.is_empty()).then_some(text)
}

pub fn sources(data: &[u8]) -> Vec<Source> {
    let mut found = Vec::new();
    for package in Bundle::new(data) {
        let Ok(package) = package else { break };
        for (index, export) in package.exports.iter().enumerate() {
            if package.export_class(export) != "TextBuffer" {
                continue;
            }
            let Ok(blob) = package.export_data(export) else {
                continue;
            };
            let Ok((_, consumed)) = read_export_properties(&package, blob) else {
                continue;
            };
            let Some(text) = text_of(blob, consumed) else {
                continue;
            };
            let path = package.export_path(index);
            let owner = path
                .strip_suffix(".ScriptText")
                .unwrap_or(&path)
                .rsplit('.')
                .next()
                .unwrap_or(&path)
                .to_string();
            found.push(Source { owner, text });
        }
    }
    found
}

pub fn classes(data: &[u8]) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for package in Bundle::new(data) {
        let Ok(package) = package else { break };
        for (index, export) in package.exports.iter().enumerate() {
            if package.export_class(export) == "Class" {
                names.push(package.export_path(index));
            }
        }
    }
    Ok(names)
}

pub struct Function {
    pub path: String,
    pub line: i32,
    pub bytecode_size: i32,
    pub bytecode: Vec<u8>,
    pub flags: u32,
    pub friendly_name: String,
}

fn read_function(
    package: &crate::package::Package<'_>,
    blob: &[u8],
    consumed: usize,
) -> Option<Function> {
    let mut reader = Reader::at(blob, consumed);
    let _super_field = reader.i32().ok()?;
    let _next = reader.i32().ok()?;
    let _script_text = reader.i32().ok()?;
    let _children = reader.i32().ok()?;
    let _cpp_text = reader.i32().ok()?;
    let line = reader.i32().ok()?;
    let _text_position = reader.i32().ok()?;
    let bytecode_size = reader.i32().ok()?;
    let storage_size = reader.i32().ok()?;
    if storage_size < 0 || storage_size as usize > blob.len() {
        return None;
    }
    let bytecode = reader.take(storage_size as usize).ok()?.to_vec();
    let _native = reader.u16().ok()?;
    let _precedence = reader.u8().ok()?;
    let flags = reader.u32().ok()?;
    let friendly_name = match reader.i32() {
        Ok(index) => {
            let number = reader.i32().unwrap_or(0);
            package.name_text(crate::package::Name { index, number })
        }
        Err(_) => String::new(),
    };
    Some(Function {
        path: String::new(),
        line,
        bytecode_size,
        bytecode,
        flags,
        friendly_name,
    })
}

pub fn functions(data: &[u8]) -> Vec<Function> {
    let mut found = Vec::new();
    for package in Bundle::new(data) {
        let Ok(package) = package else { break };
        for (index, export) in package.exports.iter().enumerate() {
            if package.export_class(export) != "Function" {
                continue;
            }
            let Ok(blob) = package.export_data(export) else {
                continue;
            };
            let Ok((_, consumed)) = read_export_properties(&package, blob) else {
                continue;
            };
            if let Some(mut function) = read_function(&package, blob, consumed) {
                function.path = package.export_path(index);
                found.push(function);
            }
        }
    }
    found
}
