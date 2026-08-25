use std::path::PathBuf;
use tera_server::responses::{object_from_json, Context};

fn main() -> anyhow::Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    let [_, definition, opcode, json] = &arguments[..] else {
        eprintln!("usage: encode <def-file> <opcode> <json-file>");
        std::process::exit(2);
    };
    let file = tera_protocol::defs::read_file(PathBuf::from(definition))?;
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(json)?)?;
    let object = object_from_json(&file.definition, &value, &Context::default());
    let packet = tera_protocol::value::write(&file.definition, opcode.parse()?, &object)?;
    println!("{}", packet.iter().map(|byte| format!("{byte:02x}")).collect::<String>());
    Ok(())
}
