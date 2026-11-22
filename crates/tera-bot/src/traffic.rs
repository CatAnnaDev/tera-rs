use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Clone, Copy, Default)]
struct Counter {
    packets: u64,
    bytes: u64,
}

struct OpCounter {
    name: String,
    incoming: Counter,
    outgoing: Counter,
    last_len: usize,
    last_seen: Instant,
}

struct Traffic {
    start: Instant,
    incoming: Counter,
    outgoing: Counter,
    unknown_in: u64,
    unknown_out: u64,
    per_opcode: HashMap<u16, OpCounter>,
}

static IN_PACKETS: AtomicU64 = AtomicU64::new(0);
static IN_BYTES: AtomicU64 = AtomicU64::new(0);
static OUT_PACKETS: AtomicU64 = AtomicU64::new(0);
static OUT_BYTES: AtomicU64 = AtomicU64::new(0);

fn table() -> &'static Mutex<Traffic> {
    static TABLE: OnceLock<Mutex<Traffic>> = OnceLock::new();
    TABLE.get_or_init(|| {
        Mutex::new(Traffic {
            start: Instant::now(),
            incoming: Counter::default(),
            outgoing: Counter::default(),
            unknown_in: 0,
            unknown_out: 0,
            per_opcode: HashMap::new(),
        })
    })
}

pub fn record(incoming: bool, opcode: u16, name: &str, bytes: usize) {
    let len = bytes as u64;
    if incoming {
        IN_PACKETS.fetch_add(1, Ordering::Relaxed);
        IN_BYTES.fetch_add(len, Ordering::Relaxed);
    } else {
        OUT_PACKETS.fetch_add(1, Ordering::Relaxed);
        OUT_BYTES.fetch_add(len, Ordering::Relaxed);
    }
    let Ok(mut traffic) = table().lock() else {
        return;
    };
    let unknown = name == "INCONNU" || name.is_empty();
    if incoming {
        traffic.incoming.packets += 1;
        traffic.incoming.bytes += len;
        if unknown {
            traffic.unknown_in += 1;
        }
    } else {
        traffic.outgoing.packets += 1;
        traffic.outgoing.bytes += len;
        if unknown {
            traffic.unknown_out += 1;
        }
    }
    let now = Instant::now();
    let entry = traffic.per_opcode.entry(opcode).or_insert_with(|| OpCounter {
        name: name.to_string(),
        incoming: Counter::default(),
        outgoing: Counter::default(),
        last_len: bytes,
        last_seen: now,
    });
    if entry.name == "INCONNU" && !unknown {
        entry.name = name.to_string();
    }
    entry.last_len = bytes;
    entry.last_seen = now;
    if incoming {
        entry.incoming.packets += 1;
        entry.incoming.bytes += len;
    } else {
        entry.outgoing.packets += 1;
        entry.outgoing.bytes += len;
    }
}

#[derive(Clone, Default)]
pub struct OpcodeRow {
    pub opcode: u16,
    pub name: String,
    pub incoming: bool,
    pub packets: u64,
    pub bytes: u64,
    pub idle_ms: u64,
}

#[derive(Clone, Default)]
pub struct TrafficSnapshot {
    pub elapsed_s: f64,
    pub in_packets: u64,
    pub in_bytes: u64,
    pub out_packets: u64,
    pub out_bytes: u64,
    pub unknown_in: u64,
    pub unknown_out: u64,
    pub distinct_opcodes: usize,
    pub top: Vec<OpcodeRow>,
}

pub fn snapshot(top_n: usize) -> TrafficSnapshot {
    let Ok(traffic) = table().lock() else {
        return TrafficSnapshot::default();
    };
    let now = Instant::now();
    let mut rows: Vec<OpcodeRow> = traffic
        .per_opcode
        .iter()
        .map(|(opcode, entry)| {
            let incoming = entry.incoming.packets >= entry.outgoing.packets;
            let counter = if incoming { entry.incoming } else { entry.outgoing };
            OpcodeRow {
                opcode: *opcode,
                name: entry.name.clone(),
                incoming,
                packets: entry.incoming.packets + entry.outgoing.packets,
                bytes: counter.bytes + if incoming { entry.outgoing.bytes } else { entry.incoming.bytes },
                idle_ms: now.duration_since(entry.last_seen).as_millis() as u64,
            }
        })
        .collect();
    rows.sort_by(|a, b| b.packets.cmp(&a.packets).then_with(|| b.bytes.cmp(&a.bytes)));
    rows.truncate(top_n);
    TrafficSnapshot {
        elapsed_s: now.duration_since(traffic.start).as_secs_f64().max(0.001),
        in_packets: traffic.incoming.packets,
        in_bytes: traffic.incoming.bytes,
        out_packets: traffic.outgoing.packets,
        out_bytes: traffic.outgoing.bytes,
        unknown_in: traffic.unknown_in,
        unknown_out: traffic.unknown_out,
        distinct_opcodes: traffic.per_opcode.len(),
        top: rows,
    }
}
