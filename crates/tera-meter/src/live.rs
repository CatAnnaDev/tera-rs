use std::path::PathBuf;
use std::sync::mpsc::Sender;

use anyhow::{bail, Context, Result};

use crate::dump::Dumper;
use crate::net::Event;
use crate::sniff::{load_tables, Tracker};

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
    tracker.finish();
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

    let dump_iface = cfg.dump.is_some().then(|| ifaces[0].clone());
    if cfg.dump.is_some() && ifaces.len() > 1 {
        eprintln!("[live] dump limite a {} (une seule interface)", ifaces[0]);
    }

    let mut handles = Vec::with_capacity(ifaces.len());
    for iface in ifaces {
        let tables = tables.clone();
        let tx = tx.clone();
        let dumper = (Some(&iface) == dump_iface.as_ref())
            .then(|| cfg.dump.as_deref().map(Dumper::new))
            .flatten();
        let tracker = Tracker::labelled(tables, dumper, tx, "live");
        handles.push(std::thread::spawn(move || {
            if let Err(e) = capture(&iface, tracker) {
                eprintln!("[live] {iface}: {e:#}");
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}
