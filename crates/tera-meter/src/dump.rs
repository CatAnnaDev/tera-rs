use std::collections::{HashMap, HashSet};
use std::io::Write;

use tera_protocol::{value, OpcodeMap, Packet, Registry, Value};

pub struct Dumper {
    seen: HashSet<u16>,
    counts: HashMap<u16, u64>,
    file: Option<std::fs::File>,
}

impl Dumper {
    pub fn new(path: &str) -> Self {
        let file = std::fs::File::create(path)
            .map_err(|e| eprintln!("[dump] impossible d'ecrire {path}: {e}"))
            .ok();
        eprintln!("[dump] actif -> {path} (un echantillon decode par opcode + compteurs)");
        Self { seen: HashSet::new(), counts: HashMap::new(), file }
    }

    fn emit(&mut self, line: &str) {
        eprint!("{line}");
        if let Some(f) = &mut self.file {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
    }

    pub fn record(&mut self, opcode: u16, name: Option<&str>, body: &[u8], registry: &Registry) {
        *self.counts.entry(opcode).or_default() += 1;
        if !self.seen.insert(opcode) {
            return;
        }
        let label = name.unwrap_or("?");
        let fields = match name.and_then(|n| registry.get(n)) {
            Some(def) => match value::read(def, &Packet::new(opcode, body.to_vec()).encode()) {
                Ok(obj) => describe(&obj),
                Err(_) => "<def presente mais decode KO>".to_string(),
            },
            None => "<pas de definition>".to_string(),
        };
        let line = format!("[dump] {label} (op {opcode}, {} o)\n        {fields}\n", body.len());
        self.emit(&line);
    }

    pub fn summary(&mut self, opcodes: &OpcodeMap) {
        let mut rows: Vec<(u16, u64)> = self.counts.iter().map(|(k, v)| (*k, *v)).collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        let mut out = String::from("\n[dump] ===== resume (opcode : occurrences) =====\n");
        for (op, n) in rows {
            let name = opcodes.name(op).unwrap_or("?");
            out.push_str(&format!("  {n:>7}  {name} ({op})\n"));
        }
        self.emit(&out);
    }
}

fn fmt(value: &Value) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Uint(u) => u.to_string(),
        Value::Float(f) => format!("{f:.2}"),
        Value::Vec3(v) => format!("({:.0},{:.0},{:.0})", v[0], v[1], v[2]),
        Value::Str(s) => format!("{s:?}"),
        Value::Bytes(b) => format!("[{} o]", b.len()),
        Value::Object(o) => format!("{{{}}}", describe(o)),
        Value::Array(a) => format!("[{} obj]", a.len()),
        Value::List(l) => format!("[{} vals]", l.len()),
    }
}

fn describe(obj: &value::Object) -> String {
    obj.fields.iter().map(|(k, v)| format!("{k}={}", fmt(v))).collect::<Vec<_>>().join("  ")
}
