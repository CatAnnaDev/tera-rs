use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use tera_protocol::value::{read, write};
use tera_protocol::{Definition, Object, Value};

const CONTRACT_DEF: &str = "ref name\nref data\nint32 type\nint64 target\nint32 value\nstring name\nbytes data\n";

const COPPER_PER_GOLD: u64 = 10000;
const SEARCH_STR_OFFSET: usize = 119;
const SAMPLES_PER_ITEM: usize = 24;

pub struct OutPacket {
    pub name: &'static str,
    pub bytes: Vec<u8>,
}

fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn framed(opcode: u16, body: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + body.len());
    put_u16(&mut buf, (4 + body.len()) as u16);
    put_u16(&mut buf, opcode);
    buf.extend_from_slice(body);
    buf
}

pub fn build_dialog_event(opcode: u16, dialog_id: u32, counter: i32) -> Vec<u8> {
    let mut body = Vec::with_capacity(12);
    body.extend_from_slice(&dialog_id.to_le_bytes());
    body.extend_from_slice(&counter.to_le_bytes());
    body.extend_from_slice(&0i32.to_le_bytes());
    framed(opcode, &body)
}

pub fn build_dialog_select(opcode: u16, dialog_id: u32, index: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(24);
    body.extend_from_slice(&8u32.to_le_bytes());
    body.extend_from_slice(&945799u32.to_le_bytes());
    body.extend_from_slice(&dialog_id.to_le_bytes());
    body.extend_from_slice(&index.to_le_bytes());
    body.extend_from_slice(&(-1i32).to_le_bytes());
    body.extend_from_slice(&(-1i32).to_le_bytes());
    framed(opcode, &body)
}

pub fn build_npc_contact(opcode: u16, game_id: u64) -> Vec<u8> {
    let mut buf = vec![0u8, 0];
    buf.extend_from_slice(&opcode.to_le_bytes());
    buf.extend_from_slice(&game_id.to_le_bytes());
    let len = buf.len() as u16;
    buf[0..2].copy_from_slice(&len.to_le_bytes());
    buf
}

#[derive(Default)]
pub struct BrokerOut {
    pub messages: Vec<String>,
    pub packets: Vec<OutPacket>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Rule {
    item: u32,
    #[serde(default)]
    instant: u64,
    #[serde(default)]
    confirm: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Config {
    armed: bool,
    #[serde(default = "yes")]
    auto_start: bool,
    scan_interval_ms: u64,
    page_delay_ms: u64,
    daily_gold_cap: u64,
    confirm_ttl_ms: u64,
    dedup_ttl_ms: u64,
    cooldown_ms: u64,
    max_seen_cache: usize,
    max_refusals: u32,
    instant_ratio: f64,
    confirm_ratio: f64,
    min_samples: u32,
    min_deal_gold: u64,
    rules: Vec<Rule>,
}

fn yes() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            armed: false,
            auto_start: true,
            scan_interval_ms: 2500,
            page_delay_ms: 1200,
            daily_gold_cap: 2_000_000_000,
            confirm_ttl_ms: 120_000,
            dedup_ttl_ms: 300_000,
            cooldown_ms: 1500,
            max_seen_cache: 8000,
            max_refusals: 3,
            instant_ratio: 0.15,
            confirm_ratio: 0.5,
            min_samples: 4,
            min_deal_gold: 50,
            rules: Vec::new(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct State {
    serial: u32,
    token: u32,
    spent_today: u64,
    day: i64,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
struct PriceEntry {
    med: u64,
    min: u64,
    n: u32,
}

struct PendingBuy {
    listing: u32,
    name: String,
    cost: u64,
}

struct PendingConfirm {
    item: u32,
    name: String,
    cost: u64,
    at: Instant,
}

struct Scan {
    active: bool,
    expecting: bool,
    idx: usize,
    page: i32,
    page_count: i32,
    cycle: u64,
    deals_this_cycle: u32,
    next_page: Option<i32>,
    page_due: Instant,
    scan_due: Instant,
    watchdog: Option<Instant>,
}

pub struct BrokerBuy {
    data_dir: PathBuf,
    cfg: Config,
    state: State,
    items: HashMap<u32, String>,
    categories: Vec<String>,
    template: Vec<u8>,
    search_serial: u32,
    page_opcode: u16,
    buy_opcode: u16,
    contract_opcode: u16,
    contract_def: Definition,
    price_db: HashMap<u32, PriceEntry>,
    samples: HashMap<u32, VecDeque<u64>>,
    listings_cache: HashMap<u32, (u32, u64, String)>,
    seen_listings: HashMap<u32, Instant>,
    pending_confirm: HashMap<u32, PendingConfirm>,
    pending_buy: Option<PendingBuy>,
    last_buy_at: Option<Instant>,
    refusals: u32,
    cold_search_failed: bool,
    open_confirmed: bool,
    prices_dirty: bool,
    last_save: Instant,
    scan: Scan,
}

fn read_json(path: &Path) -> Option<Json> {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

fn epoch_day() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| (d.as_secs() / 86400) as i64).unwrap_or(0)
}

pub fn load_items(path: &Path) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    if let Some(Json::Array(rows)) = read_json(path) {
        for row in rows {
            if let (Some(id), Some(name)) = (row.get("id").and_then(Json::as_u64), row.get("name").and_then(Json::as_str)) {
                map.insert(id as u32, name.to_string());
            }
        }
    }
    map
}

fn gold(copper: u64) -> String {
    let g = copper / 10000;
    let s = (copper % 10000) / 100;
    let c = copper % 100;
    let mut parts = Vec::new();
    if g > 0 {
        parts.push(format!("{g}g"));
    }
    if s > 0 {
        parts.push(format!("{s}s"));
    }
    if c > 0 || (g == 0 && s == 0) {
        parts.push(format!("{c}c"));
    }
    parts.join(" ")
}

fn median(values: &VecDeque<u64>) -> u64 {
    if values.is_empty() {
        return 0;
    }
    let mut sorted: Vec<u64> = values.iter().copied().collect();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2
    }
}

impl BrokerBuy {
    pub fn new(data_dir: &Path, items_path: &Path, page_opcode: u16, buy_opcode: u16, contract_opcode: u16, armed_override: Option<bool>) -> Self {
        let now = Instant::now();
        let mut cfg: Config = read_json(&data_dir.join("config.json"))
            .and_then(|j| serde_json::from_value(j).ok())
            .unwrap_or_default();
        if let Some(armed) = armed_override {
            cfg.armed = armed;
        }
        let mut state: State = read_json(&data_dir.join("state.json"))
            .and_then(|j| serde_json::from_value(j).ok())
            .unwrap_or_default();
        let day = epoch_day();
        if state.day != day {
            state.day = day;
            state.spent_today = 0;
        }
        state.serial = state.serial.max(1);
        state.token = state.token.max(1);

        let categories = match read_json(&data_dir.join("categories.json")) {
            Some(Json::Object(obj)) => obj
                .get("top")
                .and_then(Json::as_array)
                .map(|a| a.iter().filter_map(Json::as_str).map(str::to_string).collect::<Vec<_>>())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let categories = if categories.is_empty() { vec!["magical;combat".to_string()] } else { categories };

        let template = fs::read(data_dir.join("search_template.bin")).unwrap_or_default();
        let search_serial = if template.len() >= 8 {
            u32::from_le_bytes([template[4], template[5], template[6], template[7]])
        } else {
            0
        };

        let mut price_db = HashMap::new();
        if let Some(Json::Object(obj)) = read_json(&data_dir.join("prices.json")) {
            for (id, entry) in obj {
                if let (Ok(id), Ok(entry)) = (id.parse::<u32>(), serde_json::from_value::<PriceEntry>(entry)) {
                    price_db.insert(id, entry);
                }
            }
        }

        Self {
            data_dir: data_dir.to_path_buf(),
            cfg,
            state,
            items: load_items(items_path),
            categories,
            template,
            search_serial,
            page_opcode,
            buy_opcode,
            contract_opcode,
            contract_def: tera_protocol::defs::parse(CONTRACT_DEF).expect("def contract v50 invalide"),
            price_db,
            samples: HashMap::new(),
            listings_cache: HashMap::new(),
            seen_listings: HashMap::new(),
            pending_confirm: HashMap::new(),
            pending_buy: None,
            last_buy_at: None,
            refusals: 0,
            cold_search_failed: false,
            open_confirmed: false,
            prices_dirty: false,
            last_save: now,
            scan: Scan {
                active: false,
                expecting: false,
                idx: 0,
                page: 0,
                page_count: 0,
                cycle: 0,
                deals_this_cycle: 0,
                next_page: None,
                page_due: now,
                scan_due: now,
                watchdog: None,
            },
        }
    }

    fn item_name(&self, id: u32) -> String {
        self.items.get(&id).cloned().unwrap_or_else(|| format!("#{id}"))
    }

    fn save_state(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.state) {
            let _ = fs::write(self.data_dir.join("state.json"), text);
        }
    }

    fn save_prices(&mut self) {
        if !self.prices_dirty {
            return;
        }
        self.prices_dirty = false;
        let map: HashMap<String, PriceEntry> = self.price_db.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        if let Ok(text) = serde_json::to_string_pretty(&map) {
            let _ = fs::write(self.data_dir.join("prices.json"), text);
        }
    }

    fn save_cfg(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.cfg) {
            let _ = fs::write(self.data_dir.join("config.json"), text);
        }
    }

    pub fn has_template(&self) -> bool {
        self.template.len() >= SEARCH_STR_OFFSET
    }

    fn build_search(&mut self, name: &str) -> Option<Vec<u8>> {
        if !self.has_template() {
            return None;
        }
        let mut buf = self.template[..SEARCH_STR_OFFSET].to_vec();
        for unit in name.encode_utf16() {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
        buf.extend_from_slice(&[0, 0]);
        let len = buf.len() as u16;
        buf[0..2].copy_from_slice(&len.to_le_bytes());
        self.search_serial = self.search_serial.wrapping_add(1);
        buf[4..8].copy_from_slice(&self.search_serial.to_le_bytes());
        Some(buf)
    }

    fn build_page(&self, page: i32) -> Vec<u8> {
        let mut buf = vec![0u8, 0];
        buf.extend_from_slice(&self.page_opcode.to_le_bytes());
        buf.extend_from_slice(&(page as u32).to_le_bytes());
        let len = buf.len() as u16;
        buf[0..2].copy_from_slice(&len.to_le_bytes());
        buf
    }

    fn build_buy(&self, serial: u32, token: u32, listing: u32) -> Vec<u8> {
        let mut buf = vec![0u8, 0];
        buf.extend_from_slice(&self.buy_opcode.to_le_bytes());
        buf.extend_from_slice(&serial.to_le_bytes());
        buf.extend_from_slice(&token.to_le_bytes());
        buf.extend_from_slice(&listing.to_le_bytes());
        buf.push(0);
        let len = buf.len() as u16;
        buf[0..2].copy_from_slice(&len.to_le_bytes());
        buf
    }

    pub fn build_open(&self, contract_type: i64, target: i64, value: i64) -> Option<Vec<u8>> {
        let mut object = Object::new();
        object.set("name", Value::Str(String::new()));
        object.set("data", Value::Bytes((value as u32).to_le_bytes().to_vec()));
        object.set("type", Value::Int(contract_type));
        object.set("target", Value::Int(target));
        object.set("value", Value::Int(value));
        write(&self.contract_def, self.contract_opcode, &object).ok()
    }

    pub fn restart_scan(&mut self) -> BrokerOut {
        let mut out = BrokerOut::default();
        self.scan.active = true;
        self.scan.idx = 0;
        self.scan.expecting = false;
        self.scan.next_page = None;
        self.cold_search_failed = false;
        let now = Instant::now();
        self.scan.scan_due = now;
        if let Some(p) = self.next_scan(now) {
            out.packets.push(p);
        }
        out
    }

    fn scan_sources(&self) -> Vec<String> {
        self.categories.clone()
    }

    fn send_search(&mut self, name: &str, now: Instant) -> Option<OutPacket> {
        let bytes = self.build_search(name)?;
        self.scan.page = 0;
        self.scan.page_count = 0;
        self.scan.watchdog = Some(now + Duration::from_millis(4000));
        Some(OutPacket { name: "C_TRADE_BROKER_WAITING_ITEM_LIST_NEW", bytes })
    }

    fn next_scan(&mut self, now: Instant) -> Option<OutPacket> {
        if !self.scan.active || self.scan.expecting {
            return None;
        }
        let list = self.scan_sources();
        if list.is_empty() {
            self.scan.active = false;
            return None;
        }
        if self.scan.idx >= list.len() {
            self.scan.idx = 0;
            self.scan.cycle += 1;
            self.scan.deals_this_cycle = 0;
        }
        let name = list[self.scan.idx].clone();
        self.scan.idx += 1;
        self.scan.expecting = true;
        self.send_search(&name, now)
    }

    pub fn start(&mut self) -> BrokerOut {
        let mut out = BrokerOut::default();
        if !self.cfg.auto_start || self.scan.active || !self.has_template() {
            return out;
        }
        out.messages.push(format!(
            "broker-buy pret ({} categories, {}) - contact auto du PNJ broker a l'apparition",
            self.scan_sources().len(),
            if self.cfg.armed { "arme" } else { "simu" }
        ));
        out
    }

    pub fn tick(&mut self, now: Instant) -> BrokerOut {
        let mut out = BrokerOut::default();
        if now.duration_since(self.last_save) >= Duration::from_secs(30) {
            self.last_save = now;
            self.save_prices();
            self.prune_caches(now);
        }
        if let Some(due) = self.scan.watchdog {
            if self.scan.expecting && now >= due {
                self.scan.watchdog = None;
                self.scan.expecting = false;
                self.scan.active = false;
                self.scan.next_page = None;
                if !self.cold_search_failed {
                    self.cold_search_failed = true;
                    out.messages.push("recherche a froid refusee - scan en pause. Ouvre le broker (bbuy open <gameId>, voir bbuy npc) pour relancer.".to_string());
                }
            }
        }
        if let Some(page) = self.scan.next_page {
            if self.scan.expecting && now >= self.scan.page_due {
                self.scan.next_page = None;
                out.packets.push(OutPacket { name: "C_TRADE_BROKER_WAITING_ITEM_LIST_PAGE", bytes: self.build_page(page) });
            }
        }
        if self.scan.active && !self.scan.expecting && self.scan.next_page.is_none() && now >= self.scan.scan_due {
            if let Some(p) = self.next_scan(now) {
                out.packets.push(p);
            }
        }
        out
    }

    fn prune_caches(&mut self, now: Instant) {
        let dedup = Duration::from_millis(self.cfg.dedup_ttl_ms);
        self.seen_listings.retain(|_, at| now.duration_since(*at) <= dedup);
        let confirm = Duration::from_millis(self.cfg.confirm_ttl_ms);
        self.pending_confirm.retain(|_, p| now.duration_since(p.at) <= confirm);
        if self.listings_cache.len() > self.cfg.max_seen_cache * 2 {
            self.listings_cache.clear();
        }
    }

    fn record_price(&mut self, item: u32, buyout: u64) {
        let arr = self.samples.entry(item).or_default();
        arr.push_back(buyout);
        if arr.len() > SAMPLES_PER_ITEM {
            arr.pop_front();
        }
        let med = median(arr);
        let n = arr.len() as u32;
        let entry = self.price_db.entry(item).or_insert(PriceEntry { med: 0, min: u64::MAX, n: 0 });
        entry.med = med;
        entry.min = entry.min.min(buyout);
        entry.n = entry.n.max(n);
        self.prices_dirty = true;
    }

    fn rule_for(&self, item: u32) -> Option<Rule> {
        self.cfg.rules.iter().find(|r| r.item == item).cloned()
    }

    pub fn on_listings(&mut self, def: &Definition, packet: &[u8]) -> BrokerOut {
        let mut out = BrokerOut::default();
        let Ok(object) = read(def, packet) else {
            return out;
        };
        let Some(Value::Array(rows)) = object.get("listings") else {
            return out;
        };
        let rows = rows.clone();
        let now = Instant::now();
        if !self.open_confirmed {
            self.open_confirmed = true;
            self.cold_search_failed = false;
            out.messages.push(format!("BROKER OUVERT - le serveur repond ({} annonces sur cette page)", rows.len()));
        }

        for row in &rows {
            let id = uint(row, "listing") as u32;
            let item = uint(row, "item") as u32;
            let quantity = uint(row, "quantity") as u32;
            let buyout = uint(row, "buyout");
            let name = row.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| self.item_name(item));
            self.listings_cache.insert(id, (quantity, buyout, name));
        }

        if self.scan.expecting {
            self.scan.watchdog = None;
            self.scan.page = uint(&object, "page") as i32;
            self.scan.page_count = uint(&object, "pageCount") as i32;
            self.eval_listings(&rows, now, &mut out);
            if self.scan.page + 1 < self.scan.page_count {
                self.scan.next_page = Some(self.scan.page + 1);
                self.scan.page_due = now + Duration::from_millis(self.cfg.page_delay_ms);
            } else {
                self.scan.expecting = false;
                self.scan.scan_due = now + Duration::from_millis(self.cfg.scan_interval_ms);
            }
        }
        out
    }

    fn eval_listings(&mut self, rows: &[Object], now: Instant, out: &mut BrokerOut) {
        let dedup = Duration::from_millis(self.cfg.dedup_ttl_ms);
        let min_deal = self.cfg.min_deal_gold * COPPER_PER_GOLD;
        for row in rows {
            let item = uint(row, "item") as u32;
            let price = uint(row, "buyout");
            if price == 0 {
                continue;
            }
            self.record_price(item, price);

            let listing = uint(row, "listing") as u32;
            if let Some(seen) = self.seen_listings.get(&listing) {
                if now.duration_since(*seen) < dedup {
                    continue;
                }
            }
            self.seen_listings.insert(listing, now);

            let label = row.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| self.item_name(item));
            let mut tier: Option<&str> = None;
            let mut reference = 0u64;

            if let Some(rule) = self.rule_for(item) {
                let inst = rule.instant * COPPER_PER_GOLD;
                let conf = rule.confirm * COPPER_PER_GOLD;
                if inst > 0 && price <= inst {
                    tier = Some("instant");
                } else if conf > 0 && price <= conf {
                    tier = Some("confirm");
                }
                reference = if conf > 0 { conf } else { inst };
            } else if let Some(db) = self.price_db.get(&item) {
                if db.n >= self.cfg.min_samples && db.med > 0 {
                    reference = db.med;
                    let instant_cut = (db.med as f64 * self.cfg.instant_ratio) as u64;
                    let confirm_cut = (db.med as f64 * self.cfg.confirm_ratio) as u64;
                    if price <= instant_cut && price >= min_deal {
                        tier = Some("instant");
                    } else if price <= confirm_cut && price >= min_deal {
                        tier = Some("confirm");
                    }
                }
            }

            let Some(tier) = tier else {
                continue;
            };
            let pct = if reference > 0 { (100.0 * (1.0 - price as f64 / reference as f64)).round() as i64 } else { 0 };
            self.scan.deals_this_cycle += 1;

            if tier == "instant" {
                out.messages.push(format!("SNIPE {label} @ {} (-{pct}% vs {}) listing {listing}", gold(price), gold(reference)));
                if let Some(p) = self.do_buy(listing, "snipe", Some((item, label.clone(), price)), now, out) {
                    out.packets.push(p);
                }
            } else {
                self.pending_confirm.insert(listing, PendingConfirm { item, name: label.clone(), cost: price, at: now });
                out.messages.push(format!("DEAL? {label} @ {} (-{pct}% vs {}) listing {listing} - bbuy ok {listing}", gold(price), gold(reference)));
            }
        }
    }

    fn do_buy(&mut self, listing: u32, reason: &str, info: Option<(u32, String, u64)>, now: Instant, out: &mut BrokerOut) -> Option<OutPacket> {
        if let Some(last) = self.last_buy_at {
            if now.duration_since(last) < Duration::from_millis(self.cfg.cooldown_ms) {
                return None;
            }
        }
        let (name, cost) = match info {
            Some((_, name, cost)) => (name, cost),
            None => match self.listings_cache.get(&listing) {
                Some((_, cost, name)) => (name.clone(), *cost),
                None => {
                    out.messages.push(format!("Bloque: listing {listing} inconnu."));
                    return None;
                }
            },
        };
        if cost == 0 {
            out.messages.push(format!("Bloque: prix inconnu listing {listing}."));
            return None;
        }
        if self.state.spent_today + cost > self.cfg.daily_gold_cap {
            out.messages.push(format!("Plafond journalier atteint ({}).", gold(self.cfg.daily_gold_cap)));
            return None;
        }

        self.last_buy_at = Some(now);
        let serial = self.state.serial + 1;
        let token = self.state.token + 1;

        if !self.cfg.armed {
            out.messages.push(format!("[SIMU] acheterait {name} @ {} (listing {listing}) [{reason}]. bbuy arm pour activer.", gold(cost)));
            return None;
        }

        self.state.serial = serial;
        self.state.token = token;
        self.save_state();
        self.pending_buy = Some(PendingBuy { listing, name, cost });
        Some(OutPacket { name: "C_TRADE_BROKER_BUY_IT_NOW", bytes: self.build_buy(serial, token, listing) })
    }

    pub fn on_buy_result(&mut self) -> Option<String> {
        let pending = self.pending_buy.take()?;
        self.refusals = 0;
        self.state.spent_today += pending.cost;
        self.save_state();
        Some(format!("ACHETE {} pour {} (listing {})", pending.name, gold(pending.cost), pending.listing))
    }

    pub fn on_system_message(&mut self) -> Option<String> {
        self.pending_buy.take()?;
        self.refusals += 1;
        let mut msg = "Broker a refuse l'achat.".to_string();
        if self.refusals >= self.cfg.max_refusals && self.cfg.armed {
            self.cfg.armed = false;
            self.save_cfg();
            msg.push_str(&format!(" Securite: {} refus d'affilee -> DESARME auto. bbuy arm pour reactiver.", self.refusals));
        }
        Some(msg)
    }

    pub fn command(&mut self, args: &str) -> BrokerOut {
        let mut out = BrokerOut::default();
        let mut parts = args.split_whitespace();
        let sub = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();
        match sub {
            "" | "status" => {
                out.messages.push(format!(
                    "broker-buy | {} | scan {} | {} items appris | regles {} | /jour {}",
                    if self.cfg.armed { "ARME" } else { "SIMU" },
                    if self.scan.active { "ON" } else { "off" },
                    self.price_db.len(),
                    self.cfg.rules.len(),
                    gold(self.state.spent_today)
                ));
                out.messages.push(format!(
                    "snipe <= {}% du marche, confirm <= {}% | arm|disarm | ok|no <listing> | price <nom> | ratio <i%> <c%> | rule ... | scan on|off",
                    (self.cfg.instant_ratio * 100.0).round() as i64,
                    (self.cfg.confirm_ratio * 100.0).round() as i64
                ));
            }
            "arm" => {
                self.cfg.armed = true;
                self.save_cfg();
                out.messages.push("ARME - achats reels.".to_string());
            }
            "disarm" => {
                self.cfg.armed = false;
                self.save_cfg();
                out.messages.push("SIMU - notifications seulement.".to_string());
            }
            "cap" => {
                if let Some(g) = rest.first().and_then(|s| s.parse::<u64>().ok()) {
                    self.cfg.daily_gold_cap = g * COPPER_PER_GOLD;
                    self.save_cfg();
                }
                out.messages.push(format!("Plafond/jour = {}", gold(self.cfg.daily_gold_cap)));
            }
            "ratio" => {
                if let Some(i) = rest.first().and_then(|s| s.parse::<f64>().ok()) {
                    self.cfg.instant_ratio = (i / 100.0).clamp(0.0, 1.0);
                }
                if let Some(c) = rest.get(1).and_then(|s| s.parse::<f64>().ok()) {
                    self.cfg.confirm_ratio = (c / 100.0).clamp(0.0, 1.0);
                }
                self.save_cfg();
                out.messages.push(format!(
                    "snipe <= {}% | confirm <= {}% du marche",
                    (self.cfg.instant_ratio * 100.0).round() as i64,
                    (self.cfg.confirm_ratio * 100.0).round() as i64
                ));
            }
            "price" => {
                let query = rest.join(" ").to_lowercase();
                let mut shown = 0;
                for (id, db) in &self.price_db {
                    let name = self.item_name(*id);
                    if name.to_lowercase().contains(&query) {
                        out.messages.push(format!("{name} (#{id}): marche ~{} | min vu {} | {} obs", gold(db.med), gold(db.min), db.n));
                        shown += 1;
                        if shown >= 15 {
                            break;
                        }
                    }
                }
                if shown == 0 {
                    out.messages.push("Aucun prix appris pour ca encore.".to_string());
                }
            }
            "find" => {
                let query = rest.join(" ").to_lowercase();
                if query.is_empty() {
                    out.messages.push("Usage: bbuy find <nom>".to_string());
                } else {
                    let mut shown = 0;
                    for (id, name) in &self.items {
                        if name.to_lowercase().contains(&query) {
                            out.messages.push(format!("{id} - {name}"));
                            shown += 1;
                            if shown >= 20 {
                                break;
                            }
                        }
                    }
                    if shown == 0 {
                        out.messages.push("Aucun item.".to_string());
                    }
                }
            }
            "rule" => self.rule_command(&rest, &mut out),
            "ok" => {
                if let Some(listing) = rest.first().and_then(|s| s.parse::<u32>().ok()) {
                    if let Some(p) = self.pending_confirm.remove(&listing) {
                        let now = Instant::now();
                        if let Some(packet) = self.do_buy(listing, "confirme", Some((p.item, p.name, p.cost)), now, &mut out) {
                            out.packets.push(packet);
                        }
                    } else {
                        out.messages.push(format!("Rien en attente pour {listing}."));
                    }
                }
            }
            "no" => {
                if let Some(listing) = rest.first().and_then(|s| s.parse::<u32>().ok()) {
                    self.pending_confirm.remove(&listing);
                    out.messages.push(format!("Ignore {listing}."));
                }
            }
            "scan" => match rest.first().copied() {
                Some("on") => {
                    if !self.has_template() {
                        out.messages.push("Pas de template. Ouvre le broker et cherche une fois.".to_string());
                    } else {
                        self.scan.active = true;
                        self.scan.idx = 0;
                        self.cold_search_failed = false;
                        out.messages.push(format!("Scan de fond ON ({} categories).", self.scan_sources().len()));
                        let now = Instant::now();
                        if let Some(p) = self.next_scan(now) {
                            out.packets.push(p);
                        }
                    }
                }
                Some("off") => {
                    self.scan.active = false;
                    self.scan.expecting = false;
                    self.scan.next_page = None;
                    out.messages.push("Scan OFF.".to_string());
                }
                _ => out.messages.push(format!(
                    "scan {} | template {} | {} categories | {} items appris",
                    if self.scan.active { "ON" } else { "off" },
                    if self.has_template() { "ok" } else { "MANQUANT" },
                    self.categories.len(),
                    self.price_db.len()
                )),
            },
            _ => out.messages.push("bbuy status|arm|disarm|cap|ratio|price|find|rule|ok|no|scan".to_string()),
        }
        out
    }

    fn rule_command(&mut self, rest: &[&str], out: &mut BrokerOut) {
        match rest.first().copied() {
            Some("list") => {
                if self.cfg.rules.is_empty() {
                    out.messages.push("Aucune regle (le scan auto couvre deja tout le broker).".to_string());
                } else {
                    for r in &self.cfg.rules {
                        out.messages.push(format!("#{} \"{}\" instant<={}g confirm<={}g", r.item, self.item_name(r.item), r.instant, r.confirm));
                    }
                }
            }
            Some("del") => {
                if let Some(id) = rest.get(1).and_then(|s| s.parse::<u32>().ok()) {
                    self.cfg.rules.retain(|r| r.item != id);
                    self.save_cfg();
                    out.messages.push("Regle supprimee.".to_string());
                }
            }
            Some("add") => {
                let id = rest.get(1).and_then(|s| s.parse::<u32>().ok());
                let Some(id) = id.filter(|v| *v != 0) else {
                    out.messages.push("Usage: bbuy rule add <itemId> <instantGold> <confirmGold>".to_string());
                    return;
                };
                let instant = rest.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                let confirm = rest.get(3).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                self.cfg.rules.retain(|r| r.item != id);
                self.cfg.rules.push(Rule { item: id, instant, confirm });
                self.save_cfg();
                out.messages.push(format!("Regle: {} instant<={instant}g confirm<={confirm}g", self.item_name(id)));
            }
            _ => out.messages.push("bbuy rule add|del|list".to_string()),
        }
    }

    pub fn flush(&mut self) {
        self.save_prices();
        self.save_state();
    }
}

fn uint(object: &Object, field: &str) -> u64 {
    object.get(field).and_then(Value::as_uint).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE_HEX: &str = "9900c78d15000000bbaa1c007500770095009700010000004600000000000000000000000000000000000044020000ffffffff000000000000000000000000000000001a000000000000000f0000000100000000000000000781421700000000000100000000000000804a5d05000000000000000000006d00610067006900630061006c003b0063006f006d00620061007400000000000000";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn fixture(dir: &Path) -> BrokerBuy {
        let template: Vec<u8> = (0..TEMPLATE_HEX.len() / 2).map(|i| u8::from_str_radix(&TEMPLATE_HEX[i * 2..i * 2 + 2], 16).unwrap()).collect();
        fs::write(dir.join("search_template.bin"), &template).unwrap();
        BrokerBuy::new(dir, &dir.join("absent_items.json"), 36091, 63085, 34899, Some(false))
    }

    #[test]
    fn page_packet_matches_capture() {
        let dir = std::env::temp_dir().join(format!("broker_page_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let broker = fixture(&dir);
        assert_eq!(hex(&broker.build_page(1)), "0800fb8c01000000");
    }

    #[test]
    fn buy_packet_matches_capture() {
        let dir = std::env::temp_dir().join(format!("broker_buy_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let broker = fixture(&dir);
        assert_eq!(hex(&broker.build_buy(8, 790316, 1870588)), "11006df6080000002c0f0c00fc8a1c0000");
    }

    #[test]
    fn open_contract_roundtrips() {
        let dir = std::env::temp_dir().join(format!("broker_open_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let broker = fixture(&dir);
        let bytes = broker.build_open(28, 0, 0).unwrap();
        assert_eq!([bytes[2], bytes[3]], 34899u16.to_le_bytes());
        let object = read(&broker.contract_def, &bytes).unwrap();
        assert_eq!(object.get("type").and_then(Value::as_int), Some(28));
        assert_eq!(object.get("data").and_then(Value::as_bytes).map(<[u8]>::to_vec), Some(vec![0, 0, 0, 0]));
    }

    #[test]
    fn search_rewrites_len_and_appends_utf16() {
        let dir = std::env::temp_dir().join(format!("broker_search_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let mut broker = fixture(&dir);
        let before = broker.search_serial;
        let buf = broker.build_search("magical;combat").unwrap();
        assert_eq!(u16::from_le_bytes([buf[0], buf[1]]) as usize, buf.len());
        assert_eq!([buf[2], buf[3]], [0xc7, 0x8d]);
        assert_eq!(u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]), before + 1);
        let tail: Vec<u8> = "magical;combat\u{0}".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(&buf[SEARCH_STR_OFFSET..], &tail[..]);
    }
}
