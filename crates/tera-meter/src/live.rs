use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};

use crate::dump::Dumper;
use crate::net::Event;
use crate::sniff::{load_tables, SharedDumper, Tracker};

pub struct Config {
    pub opcodes: PathBuf,
    pub definitions: PathBuf,
    pub patch: u32,
    pub dump: Option<String>,
    pub ifaces: Vec<String>,
}

fn default_ifaces() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(Some(dev)) = pcap::Device::lookup() {
        out.push(dev.name);
    }
    let lo = "lo0".to_string();
    if !out.contains(&lo) {
        out.push(lo);
    }
    out
}

fn capture(iface: &str, mut tracker: Tracker) -> Result<()> {
    let cap = pcap::Capture::from_device(iface)
        .with_context(|| format!("ouverture de l'interface {iface}"))?
        .immediate_mode(true)
        .snaplen(65535)
        .timeout(250);
    let mut cap = cap.open().with_context(|| {
        format!("capture sur {iface} (lance le meter avec sudo pour l'acces BPF)")
    })?;
    let linktype = cap.get_datalink().0 as u32;
    let _ = cap.filter("tcp", true);
    eprintln!("[live] capture active sur {iface} (datalink {linktype})");

    loop {
        match cap.next_packet() {
            Ok(packet) => {
                if !tracker.feed(linktype, packet.data) {
                    break;
                }
            }
            Err(pcap::Error::TimeoutExpired) => continue,
            Err(pcap::Error::NoMorePackets) => break,
            Err(e) => {
                eprintln!("[live] {iface}: {e}");
                break;
            }
        }
    }
    Ok(())
}

pub fn run(cfg: Config, tx: Sender<Event>) -> Result<()> {
    let tables = load_tables(&cfg.opcodes, &cfg.definitions, cfg.patch)?;
    let mut ifaces = cfg.ifaces;
    if ifaces.is_empty() {
        ifaces = default_ifaces();
    }
    if ifaces.is_empty() {
        bail!("aucune interface reseau a capturer");
    }
    eprintln!("[live] interfaces: {}", ifaces.join(", "));

    let dumper: SharedDumper = cfg.dump.as_deref().map(|p| Arc::new(Mutex::new(Dumper::new(p))));

    let mut handles = Vec::with_capacity(ifaces.len());
    for iface in ifaces {
        let tracker = Tracker::labelled(tables.clone(), dumper.clone(), tx.clone(), "live");
        handles.push(std::thread::spawn(move || {
            if let Err(e) = capture(&iface, tracker) {
                eprintln!("[live] {iface}: {e:#}");
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    if let Some(d) = &dumper {
        d.lock().unwrap().summary(&tables.0);
    }
    Ok(())
}
