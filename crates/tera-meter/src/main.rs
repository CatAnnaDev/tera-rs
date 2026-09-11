mod config;
mod data;
mod dump;
mod live;
mod model;
mod net;
mod notify;
mod sniff;

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use clap::Parser;
use data::{class_role_color, GameData};
use eframe::egui;
use model::{Meter, KIND_DAMAGE};
use net::{Config, Event};

#[derive(Parser)]
#[command(about = "Meter DPS natif (lit le flux external-interface de Noctenium, 100% local)")]
struct Cli {
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
    #[arg(long, default_value = "127.0.0.60:5301")]
    stream: String,
    #[arg(long, default_value = "http://127.0.0.61:5300")]
    control: String,
    #[arg(long, default_value = "data/opcodes/protocol.376012.map")]
    opcodes: PathBuf,
    #[arg(long, default_value = "data/definitions.classicplus")]
    definitions: PathBuf,
    #[arg(long, default_value_t = 100)]
    patch: u32,
    #[arg(long, help = "peuple le meter avec des donnees factices pour juger l'UI sans etre en jeu")]
    demo: bool,
    #[arg(
        long,
        help = "capture le reseau nativement (comme ShinraMeter, sans mod ni Noctenium) ; lancer avec sudo pour l'acces BPF"
    )]
    live: bool,
    #[arg(
        long,
        value_name = "IFACE",
        help = "interface(s) a capturer en --live (repetable) ; defaut = interface principale + lo0"
    )]
    iface: Vec<String>,
    #[arg(
        long,
        help = "lit un flux pcap sur stdin (sudo tcpdump -i lo0 -U -s0 -w - 'tcp and host 127.0.0.20' | tera-meter --sniff) au lieu de l'external-interface"
    )]
    sniff: bool,
    #[arg(
        long,
        value_name = "FICHIER",
        help = "dump tout ce qui transite (1 echantillon decode par opcode + compteurs) dans ce fichier ; en external-interface hooke TOUS les opcodes"
    )]
    dump: Option<String>,
}

fn seed_demo(meter: &mut Meter) {
    let now = Instant::now();
    meter.apply(Event::Me { game_id: 1, name: "Priest (moi)".into(), template_id: 10107 }, now);
    let players = [
        (1u64, "Priest (moi)", 10107u64),
        (2, "Lancer", 10102),
        (3, "Archer", 10106),
        (4, "Berserker", 10104),
        (5, "Mystic", 10108),
    ];
    for (gid, name, tpl) in players {
        meter.apply(
            Event::Player { game_id: gid, name: name.into(), template_id: tpl, server_id: 1, player_id: gid },
            now,
        );
    }
    meter.apply(Event::Npc { game_id: 100, template_id: 3013, hunting_zone: 152, max_hp: 5_000_000 }, now);
    meter.apply(Event::BossGage { id: 100, cur_hp: 3_640_000, max_hp: 5_000_000 }, now);
    meter.apply(
        Event::NpcStatus { game_id: 100, enraged: true, remaining_enrage_ms: 36_000, target: 2 },
        now,
    );
    let shares = [(1u64, 42u64, 31u64), (2, 28, 18), (3, 18, 61), (4, 8, 22), (5, 4, 15)];
    for (gid, share, crit) in shares {
        let total_hits = 40u64;
        let crit_hits = total_hits * crit / 100;
        for h in 0..total_hits {
            let value = (share as i64) * 12_000 / total_hits as i64 + (h as i64 % 5) * 300;
            meter.apply(
                Event::Hit {
                    source: gid,
                    target: 100,
                    value,
                    kind: model::KIND_DAMAGE,
                    crit: h < crit_hits,
                    skill: 10100 + (h as u32 % 4) * 100,
                },
                now,
            );
        }
    }
    for gid in [1u64, 5] {
        for h in 0..60u64 {
            meter.apply(
                Event::Hit {
                    source: gid,
                    target: 2 + (h % 4),
                    value: 8000 + (h as i64 % 7) * 500,
                    kind: model::KIND_HEAL,
                    crit: h % 3 == 0,
                    skill: 400000 + (h as u32 % 3) * 100,
                },
                now,
            );
        }
    }
    for h in 0..30u64 {
        meter.apply(
            Event::Hit {
                source: 100,
                target: 2,
                value: 15000 + (h as i64 % 5) * 2500,
                kind: model::KIND_DAMAGE,
                crit: false,
                skill: 0,
            },
            now,
        );
    }
    meter.apply(
        Event::Party { members: vec![(1, 1, 7), (1, 2, 2), (1, 3, 6), (1, 4, 4), (1, 5, 8)] },
        now,
    );
    let stats = [
        (1u64, 78i64, 100i64, 55i64, 100i64, true),
        (2, 40, 130, 20, 60, true),
        (3, 96, 92, 88, 100, true),
        (4, 12, 110, 70, 90, true),
        (5, 0, 95, 10, 100, false),
    ];
    for (pid, hp, maxhp, mp, maxmp, alive) in stats {
        meter.apply(
            Event::PartyStat {
                server_id: 1,
                player_id: pid,
                hp,
                max_hp: maxhp,
                mp,
                max_mp: maxmp,
                alive,
            },
            now,
        );
    }
    let started = now.checked_sub(Duration::from_secs(83)).unwrap_or(now);
    meter.total.begin = started;
    for b in meter.bosses.iter_mut() {
        b.begin = started;
    }
}

const SELF_COLOR: egui::Color32 = egui::Color32::from_rgb(0xf4, 0xa4, 0x42);
const NAME_COLOR: egui::Color32 = egui::Color32::from_rgb(0xAD, 0xFF, 0x2F);
const CRIT_COLOR: egui::Color32 = egui::Color32::from_rgb(0xF0, 0x80, 0x80);
const DAMAGE_COLOR: egui::Color32 = egui::Color32::from_rgb(0xef, 0x53, 0x50);
const HEAL_COLOR: egui::Color32 = egui::Color32::from_rgb(0x66, 0xbb, 0x6a);
const MANA_COLOR: egui::Color32 = egui::Color32::from_rgb(0x26, 0xc6, 0xda);
const CAST_COLOR: egui::Color32 = egui::Color32::from_rgb(0xba, 0x68, 0xc8);

fn format_value(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.1}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}k", v / 1e3)
    } else {
        format!("{:.0}", v)
    }
}

fn format_duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn hp_color(pct: f32, alive: bool) -> egui::Color32 {
    if !alive {
        egui::Color32::from_rgb(0x55, 0x55, 0x55)
    } else if pct > 0.5 {
        egui::Color32::from_rgb(0x4c, 0xc0, 0x5a)
    } else if pct > 0.25 {
        egui::Color32::from_rgb(0xe0, 0xa0, 0x30)
    } else {
        egui::Color32::from_rgb(0xd0, 0x40, 0x40)
    }
}

fn metric(ui: &mut egui::Ui, label: &str, value: &str, color: egui::Color32) {
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(label)
                .text_style(egui::TextStyle::Small)
                .color(egui::Color32::from_gray(0x80)),
        );
        ui.label(egui::RichText::new(value).monospace().size(15.0).color(color).strong());
    });
}

fn encounter_label(enc: &model::Encounter) -> String {
    let dur = format_duration(enc.interval_secs());
    if enc.boss.is_none() {
        format!("TOTAL  ·  {dur}")
    } else {
        let mark = if enc.ended { "  ✝" } else { "" };
        format!("{}  ·  {}  ·  {}/s{}", enc.name, dur, format_value(enc.dps()), mark)
    }
}

struct App {
    meter: Meter,
    rx: Receiver<Event>,
    detail: Option<u64>,
    detail_tab: u8,
    icons: std::collections::HashMap<&'static str, egui::TextureHandle>,
    passthrough: bool,
    show_buffs: bool,
    show_party: bool,
    show_settings: bool,
    show_graph: bool,
    opacity: u8,
    view: u8,
    last_rows: usize,
    fitted_once: bool,
    request_fit: bool,
    debuff_text: String,
    dps_history: std::collections::VecDeque<f32>,
    last_sample: Instant,
    last_total: i64,
    graph_key: (usize, Option<u64>, u8),
    config: config::Config,
    notifier: notify::Notifier,
}

fn panel_fill() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(0x15, 0x18, 0x1e, 0xdc)
}
fn card_border() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(0x8a, 0x96, 0xa6, 0x40)
}

fn setup_style(ctx: &egui::Context) {
    use egui::{Color32, FontFamily, FontId, Rounding, Stroke, TextStyle};
    let mut style = (*ctx.style()).clone();

    style.text_styles = [
        (TextStyle::Heading, FontId::new(16.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(12.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
        (TextStyle::Small, FontId::new(10.5, FontFamily::Proportional)),
    ]
    .into();

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = panel_fill();
    v.window_stroke = Stroke::new(1.0_f32, card_border());
    v.window_rounding = Rounding::same(8.0);
    v.override_text_color = Some(Color32::from_rgb(0xe6, 0xea, 0xf0));
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_white_alpha(10));
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.rounding = Rounding::same(5.0);
        w.bg_stroke = Stroke::NONE;
    }
    v.widgets.inactive.weak_bg_fill = Color32::from_white_alpha(10);
    v.widgets.hovered.weak_bg_fill = Color32::from_white_alpha(22);
    v.widgets.active.weak_bg_fill = Color32::from_white_alpha(30);
    v.selection.bg_fill = Color32::from_rgb(0x2b, 0x3c, 0x4e);
    v.selection.stroke = Stroke::NONE;
    style.spacing.item_spacing = egui::vec2(7.0, 5.0);
    style.spacing.button_padding = egui::vec2(7.0, 3.0);
    style.spacing.window_margin = egui::Margin::same(0.0);
    ctx.set_style(style);
}

fn load_icons(ctx: &egui::Context) -> std::collections::HashMap<&'static str, egui::TextureHandle> {
    let mut map = std::collections::HashMap::new();
    let mut add = |name: &'static str, bytes: &[u8]| {
        if let Ok(img) = image::load_from_memory(bytes) {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            map.insert(name, ctx.load_texture(name, color, egui::TextureOptions::LINEAR));
        }
    };
    add("Warrior", include_bytes!("../assets/class-icons/warrior.png"));
    add("Lancer", include_bytes!("../assets/class-icons/lancer.png"));
    add("Slayer", include_bytes!("../assets/class-icons/slayer.png"));
    add("Berserker", include_bytes!("../assets/class-icons/berserker.png"));
    add("Sorcerer", include_bytes!("../assets/class-icons/sorcerer.png"));
    add("Archer", include_bytes!("../assets/class-icons/archer.png"));
    add("Priest", include_bytes!("../assets/class-icons/priest.png"));
    add("Mystic", include_bytes!("../assets/class-icons/mystic.png"));
    add("Reaper", include_bytes!("../assets/class-icons/reaper.png"));
    add("Gunner", include_bytes!("../assets/class-icons/gunner.png"));
    add("Brawler", include_bytes!("../assets/class-icons/brawler.png"));
    add("Ninja", include_bytes!("../assets/class-icons/ninja.png"));
    add("Valkyrie", include_bytes!("../assets/class-icons/valkyrie.png"));
    add("?", include_bytes!("../assets/class-icons/common.png"));
    map
}

impl App {
    fn drain(&mut self) {
        let now = Instant::now();
        while let Ok(event) = self.rx.try_recv() {
            self.meter.apply(event, now);
        }
    }

    fn sample_dps(&mut self, now: Instant) {
        let (metric, key) = {
            let enc = self.meter.current();
            let m = match self.view {
                1 => enc.total_heal,
                2 => enc.total_taken,
                _ => enc.total_damage,
            };
            (m, (self.meter.selected, enc.boss, self.view))
        };
        if key != self.graph_key {
            self.graph_key = key;
            self.dps_history.clear();
            self.last_total = metric;
            self.last_sample = now;
            return;
        }
        let dt = now.saturating_duration_since(self.last_sample).as_secs_f32();
        if dt >= 1.0 {
            let per_s = (metric - self.last_total).max(0) as f32 / dt;
            self.dps_history.push_back(per_s);
            while self.dps_history.len() > 90 {
                self.dps_history.pop_front();
            }
            self.last_total = metric;
            self.last_sample = now;
        }
    }

    fn graph_window(&mut self, ctx: &egui::Context) {
        if !self.show_graph {
            return;
        }
        let mut open = true;
        egui::Window::new("Graphe").open(&mut open).default_width(360.0).show(ctx, |ui| {
            let label = match self.view {
                1 => "HPS",
                2 => "Subis/s",
                _ => "DPS",
            };
            let color = match self.view {
                1 => HEAL_COLOR,
                2 => DAMAGE_COLOR,
                _ => SELF_COLOR,
            };
            if self.dps_history.len() < 2 {
                ui.label("En attente de combat…");
                return;
            }
            let peak = self.dps_history.iter().copied().fold(0.0_f32, f32::max);
            let scale = peak.max(1.0);
            let cur = *self.dps_history.back().unwrap();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{label} {}/s", format_value(cur as f64)))
                        .color(color)
                        .strong(),
                );
                ui.label(
                    egui::RichText::new(format!("pic {}/s", format_value(peak as f64)))
                        .color(egui::Color32::from_gray(0xaa)),
                );
            });
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width().max(220.0), 120.0),
                egui::Sense::hover(),
            );
            let painter = ui.painter();
            painter.rect_filled(rect, 4.0, egui::Color32::from_black_alpha(0x50));
            let n = self.dps_history.len();
            let dx = rect.width() / (n - 1) as f32;
            let line: Vec<egui::Pos2> = self
                .dps_history
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    egui::pos2(rect.left() + dx * i as f32, rect.bottom() - (v / scale) * rect.height())
                })
                .collect();
            let mut mesh = egui::Mesh::default();
            for w in line.windows(2) {
                let base = mesh.vertices.len() as u32;
                mesh.colored_vertex(w[0], color.gamma_multiply(0.28));
                mesh.colored_vertex(w[1], color.gamma_multiply(0.28));
                mesh.colored_vertex(egui::pos2(w[1].x, rect.bottom()), egui::Color32::TRANSPARENT);
                mesh.colored_vertex(egui::pos2(w[0].x, rect.bottom()), egui::Color32::TRANSPARENT);
                mesh.add_triangle(base, base + 1, base + 2);
                mesh.add_triangle(base, base + 2, base + 3);
            }
            painter.add(egui::Shape::mesh(mesh));
            painter.add(egui::Shape::line(line, egui::Stroke::new(1.6_f32, color)));
        });
        self.show_graph = open;
    }

    fn boss_panel(&self, ui: &mut egui::Ui) {
        let bosses = self.meter.active_bosses();
        if bosses.is_empty() {
            return;
        }
        let width = ui.available_width();
        for (gid, name, cur, max, enraged, enrage_ms) in bosses {
            let pct = if max > 0 { cur as f64 / max as f64 } else { 0.0 };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 2.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 0x40));
            let mut fill = rect;
            fill.set_width(rect.width() * pct as f32);
            let bar_color = if enraged {
                egui::Color32::from_rgb(0xff, 0x00, 0x00)
            } else {
                egui::Color32::from_rgb(0x00, 0x97, 0xce)
            };
            painter.rect_filled(fill, 2.0, bar_color);
            for i in 1..10 {
                let x = rect.left() + rect.width() * (i as f32 / 10.0);
                painter.line_segment(
                    [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                    egui::Stroke::new(1.0_f32, egui::Color32::from_black_alpha(0x90)),
                );
            }
            let y = rect.center().y;
            painter.text(
                egui::pos2(rect.left() + 6.0, y),
                egui::Align2::LEFT_CENTER,
                &name,
                egui::FontId::proportional(13.0),
                egui::Color32::from_rgb(0xbb, 0xff, 0xff),
            );
            let center_text = if enraged {
                format!("ENRAGE {:.0}s", (enrage_ms as f64 / 1000.0).max(0.0))
            } else {
                format!("{:.1} %", pct * 100.0)
            };
            painter.text(
                egui::pos2(rect.center().x, y),
                egui::Align2::CENTER_CENTER,
                center_text,
                egui::FontId::proportional(13.0),
                if enraged { egui::Color32::from_rgb(0xff, 0xaa, 0x00) } else { egui::Color32::WHITE },
            );
            painter.text(
                egui::pos2(rect.right() - 6.0, y),
                egui::Align2::RIGHT_CENTER,
                format!("{} / {}", format_value(cur as f64), format_value(max as f64)),
                egui::FontId::proportional(10.0),
                egui::Color32::WHITE,
            );
            if let Some(holder) = self.meter.aggro_target(gid).filter(|t| self.meter.is_player(*t)) {
                let (arect, _) =
                    ui.allocate_exact_size(egui::vec2(width, 14.0), egui::Sense::hover());
                ui.painter().text(
                    egui::pos2(arect.left() + 6.0, arect.center().y),
                    egui::Align2::LEFT_CENTER,
                    format!("aggro ▶ {}", self.meter.player_name(holder)),
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_rgb(0xff, 0x8a, 0x3a),
                );
            }
            ui.add_space(2.0);
        }
        ui.add_space(2.0);
    }

    fn summary_bar(&self, ui: &mut egui::Ui) {
        let enc = self.meter.current();
        let interval = format_duration(enc.interval_secs());
        let (l2, v2, l3, v3) = match self.view {
            1 => {
                let hps = enc.total_heal as f64 / enc.interval_secs();
                ("HPS", format!("{}/s", format_value(hps)), "HEAL", format_value(enc.total_heal as f64))
            }
            2 => {
                let dtps = enc.total_taken as f64 / enc.interval_secs();
                ("SUBIS/s", format!("{}/s", format_value(dtps)), "SUBIS", format_value(enc.total_taken as f64))
            }
            _ => ("DPS", format!("{}/s", format_value(enc.dps())), "TOTAL", format_value(enc.total_damage as f64)),
        };
        ui.columns(3, |c| {
            metric(&mut c[0], "TEMPS", &interval, egui::Color32::from_gray(0xdd));
            metric(&mut c[1], l2, &v2, egui::Color32::WHITE);
            metric(&mut c[2], l3, &v3, egui::Color32::from_gray(0xdd));
        });
    }

    fn player_rows(&mut self, ui: &mut egui::Ui) {
        let view = self.view;
        let enc = self.meter.current();
        let (total_raw, ranked) = match view {
            1 => (enc.total_heal, enc.ranked_heal()),
            2 => (enc.total_taken, enc.ranked_taken()),
            _ => (enc.total_damage, enc.ranked()),
        };
        let total = total_raw.max(1) as f64;
        let interval = enc.interval_secs();
        let width = ui.available_width();
        let party_only = self.meter.party_only;
        let aggro = self.meter.current_boss().and_then(|b| self.meter.aggro_target(b));
        let rows: Vec<(u64, i64, f64, String, bool)> = ranked
            .iter()
            .filter(|e| self.meter.is_player(e.source))
            .filter(|e| !party_only || self.meter.is_party(e.source))
            .map(|e| {
                let amount = match view {
                    1 => e.heal,
                    2 => e.damage_taken,
                    _ => e.damage,
                };
                let third = match view {
                    1 => format!("{:.0}%", e.heal_crit_rate()),
                    2 => format!("☠{}", self.meter.deaths(e.source)),
                    _ => format!("{:.0}%", e.crit_rate()),
                };
                (e.source, amount, amount as f64 / total, third, e.source == self.meter.me)
            })
            .collect();

        self.last_rows = rows.len();
        let mut clicked: Option<u64> = None;
        for (source, damage, share, third, is_me) in rows {
            let class = self.meter.player_class(source);
            let role = if is_me { SELF_COLOR } else { class_role_color(class) };
            let name = self.meter.player_name(source);
            let dps = damage as f64 / interval;

            let (rect, resp) =
                ui.allocate_exact_size(egui::vec2(width, 29.0), egui::Sense::click());
            let painter = ui.painter();
            // share-of-total background bar: horizontal gradient (role -> transparent)
            let bar_right = rect.left() + rect.width() * share as f32;
            let left_color = role.gamma_multiply(0.42);
            let mut mesh = egui::Mesh::default();
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(egui::pos2(rect.left(), rect.top()), left_color);
            mesh.colored_vertex(egui::pos2(bar_right, rect.top()), egui::Color32::TRANSPARENT);
            mesh.colored_vertex(egui::pos2(bar_right, rect.bottom()), egui::Color32::TRANSPARENT);
            mesh.colored_vertex(egui::pos2(rect.left(), rect.bottom()), left_color);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
            painter.add(egui::Shape::mesh(mesh));
            painter.line_segment(
                [egui::pos2(rect.left(), rect.bottom() - 0.5), egui::pos2(bar_right, rect.bottom() - 0.5)],
                egui::Stroke::new(2.0_f32, role),
            );
            if resp.hovered() {
                painter.rect_filled(rect, 0.0, egui::Color32::from_white_alpha(14));
            }
            if Some(source) == aggro {
                painter.rect_filled(
                    egui::Rect::from_min_size(rect.left_top(), egui::vec2(3.0, rect.height())),
                    0.0,
                    egui::Color32::from_rgb(0xff, 0x3a, 0x3a),
                );
            }

            let y = rect.center().y;
            let class_label = class.unwrap_or("?");
            if let Some(tex) = self.icons.get(class_label) {
                let icon = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 3.0, y - 12.5),
                    egui::vec2(25.0, 25.0),
                );
                painter.image(
                    tex.id(),
                    icon,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                painter.text(
                    egui::pos2(rect.left() + 6.0, y),
                    egui::Align2::LEFT_CENTER,
                    class_label,
                    egui::FontId::proportional(11.0),
                    role,
                );
            }
            let num = egui::FontId::monospace(12.5);
            painter.text(
                egui::pos2(rect.left() + 34.0, y),
                egui::Align2::LEFT_CENTER,
                &name,
                egui::FontId::proportional(14.0),
                NAME_COLOR,
            );
            painter.text(
                egui::pos2(rect.right() - 148.0, y),
                egui::Align2::RIGHT_CENTER,
                format!("{:.0}%", share * 100.0),
                num.clone(),
                egui::Color32::from_gray(0xcc),
            );
            painter.text(
                egui::pos2(rect.right() - 54.0, y),
                egui::Align2::RIGHT_CENTER,
                format!("{}/s", format_value(dps)),
                num.clone(),
                egui::Color32::WHITE,
            );
            painter.text(
                egui::pos2(rect.right() - 6.0, y),
                egui::Align2::RIGHT_CENTER,
                third,
                num,
                CRIT_COLOR,
            );
            if resp.clicked() {
                clicked = Some(source);
            }
        }
        if let Some(source) = clicked {
            self.detail = Some(source);
            self.detail_tab = match data::class_role(self.meter.player_class(source)) {
                data::Role::Healer => 1,
                _ => 0,
            };
        }
    }

    fn detail_window(&mut self, ctx: &egui::Context) {
        let Some(source) = self.detail else { return };
        let title = self.meter.player_name(source);
        let mut open = true;
        let mut tab = self.detail_tab;
        egui::Window::new(format!("Skills — {title}"))
            .open(&mut open)
            .default_width(580.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut tab, 0, "Dps");
                    ui.selectable_value(&mut tab, 1, "Heal");
                    ui.selectable_value(&mut tab, 2, "Mana");
                    ui.selectable_value(&mut tab, 3, "Casts");
                    let deaths = self.meter.deaths(source);
                    if deaths > 0 {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(format!("☠ {deaths}")).color(DAMAGE_COLOR).strong(),
                        );
                    }
                });
                ui.separator();
                let enc = self.meter.current();
                let Some(entity) = enc.entities.get(&source) else {
                    ui.label("aucune donnée");
                    return;
                };
                if tab == 3 {
                    let mut casts: Vec<(u32, u64)> =
                        entity.cast_counts.iter().map(|(id, n)| (*id, *n)).collect();
                    casts.sort_by(|a, b| b.1.cmp(&a.1));
                    egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
                        egui::Grid::new("casts").striped(true).num_columns(2).spacing([14.0, 4.0]).show(
                            ui,
                            |ui| {
                                ui.label(egui::RichText::new("Skill").color(CAST_COLOR).strong());
                                ui.label(egui::RichText::new("Casts").color(CAST_COLOR).strong());
                                ui.end_row();
                                for (id, n) in casts {
                                    ui.label(self.meter.skill_name(source, id));
                                    ui.label(format!("{n}"));
                                    ui.end_row();
                                }
                            },
                        );
                    });
                    return;
                }
                let (map, total, value_header, color) = match tab {
                    1 => (&entity.heal_skills, entity.heal, "Heal", HEAL_COLOR),
                    2 => (&entity.mana_skills, 0_i64, "Mana", MANA_COLOR),
                    _ => (&entity.damage_skills, entity.damage, "Damage", DAMAGE_COLOR),
                };
                let base = if total > 0 {
                    total as f64
                } else {
                    map.values().map(|s| s.amount).sum::<i64>().max(1) as f64
                };
                let mut skills: Vec<(u32, &model::SkillStats)> =
                    map.iter().map(|(id, s)| (*id, s)).collect();
                skills.sort_by(|a, b| b.1.amount.cmp(&a.1.amount));

                egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
                    egui::Grid::new("skills")
                        .striped(true)
                        .num_columns(10)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            for h in [
                                value_header, "%", "% Crit", "Max crit", "Avg crit", "Avg white",
                                "Avg", "Hits", "Crits",
                            ] {
                                ui.label(egui::RichText::new(h).color(color).strong());
                            }
                            ui.label(egui::RichText::new("Skill").color(color).strong());
                            ui.end_row();
                            for (id, s) in skills {
                                let name = self.meter.skill_name(source, id);
                                ui.label(format_value(s.amount as f64));
                                ui.label(format!("{:.0}%", s.amount as f64 * 100.0 / base));
                                ui.label(format!("{:.0}%", s.crit_rate()));
                                ui.label(format_value(s.biggest_crit as f64));
                                ui.label(format_value(s.avg_crit()));
                                ui.label(format_value(s.avg_white()));
                                ui.label(format_value(s.avg()));
                                ui.label(format!("{}", s.hits));
                                ui.label(format!("{}", s.crits));
                                ui.label(name);
                                ui.end_row();
                            }
                        });
                });
            });
        self.detail_tab = tab;
        if !open {
            self.detail = None;
        }
    }

    fn party_window(&mut self, ctx: &egui::Context) {
        if !self.show_party {
            return;
        }
        let mut open = true;
        egui::Window::new("Groupe").open(&mut open).default_width(250.0).show(ctx, |ui| {
            let mut members: Vec<(&'static str, String, f32, f32, i64, i64, bool)> = self
                .meter
                .party_members
                .iter()
                .map(|((s, p), m)| {
                    let name = m.name.clone().unwrap_or_else(|| format!("{s}-{p}"));
                    let hpp = if m.max_hp > 0 { m.cur_hp as f32 / m.max_hp as f32 } else { 0.0 };
                    let mpp = if m.max_mp > 0 { m.cur_mp as f32 / m.max_mp as f32 } else { 0.0 };
                    (m.class.unwrap_or("?"), name, hpp, mpp, m.cur_hp, m.max_hp, m.alive)
                })
                .collect();
            if members.is_empty() {
                ui.label("aucun membre (hors groupe ?)");
                return;
            }
            members.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
            let width = ui.available_width();
            for (class, name, hpp, mpp, cur, max, alive) in members {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 32.0), egui::Sense::hover());
                let painter = ui.painter();
                if let Some(tex) = self.icons.get(class) {
                    let icon = egui::Rect::from_min_size(egui::pos2(rect.left(), rect.top() + 3.0), egui::vec2(18.0, 18.0));
                    painter.image(tex.id(), icon, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                }
                let bar_left = rect.left() + 22.0;
                let bar_w = rect.right() - bar_left;
                let hp_rect = egui::Rect::from_min_size(egui::pos2(bar_left, rect.top() + 1.0), egui::vec2(bar_w, 16.0));
                painter.rect_filled(hp_rect, 2.0, egui::Color32::from_black_alpha(0x60));
                let mut hp_fill = hp_rect;
                hp_fill.set_width(bar_w * hpp.clamp(0.0, 1.0));
                painter.rect_filled(hp_fill, 2.0, hp_color(hpp, alive));
                painter.text(egui::pos2(bar_left + 4.0, hp_rect.center().y), egui::Align2::LEFT_CENTER, &name, egui::FontId::proportional(12.0), egui::Color32::WHITE);
                painter.text(egui::pos2(hp_rect.right() - 4.0, hp_rect.center().y), egui::Align2::RIGHT_CENTER, format!("{}%", (hpp * 100.0) as i32), egui::FontId::monospace(11.0), egui::Color32::from_gray(0xe0));
                let mp_rect = egui::Rect::from_min_size(egui::pos2(bar_left, rect.top() + 19.0), egui::vec2(bar_w, 6.0));
                painter.rect_filled(mp_rect, 1.0, egui::Color32::from_black_alpha(0x60));
                let mut mp_fill = mp_rect;
                mp_fill.set_width(bar_w * mpp.clamp(0.0, 1.0));
                painter.rect_filled(mp_fill, 1.0, egui::Color32::from_rgb(0x33, 0x77, 0xcc));
                if !alive {
                    painter.text(rect.center(), egui::Align2::CENTER_CENTER, "☠ MORT", egui::FontId::proportional(12.0), egui::Color32::from_rgb(0xff, 0x55, 0x55));
                }
                let _ = (cur, max);
                ui.add_space(3.0);
            }
        });
        self.show_party = open;
    }

    fn resize_grip(&mut self, ctx: &egui::Context) {
        egui::Area::new("resize-grip".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-3.0, -3.0))
            .show(ctx, |ui| {
                let (rect, grip) = ui.allocate_exact_size(egui::vec2(15.0, 15.0), egui::Sense::drag());
                let p = ui.painter();
                let col = if grip.hovered() || grip.dragged() {
                    egui::Color32::from_white_alpha(0xcc)
                } else {
                    egui::Color32::from_white_alpha(0x55)
                };
                for k in 1..=3 {
                    let o = k as f32 * 3.6;
                    p.line_segment(
                        [egui::pos2(rect.right() - o, rect.bottom() - 1.0), egui::pos2(rect.right() - 1.0, rect.bottom() - o)],
                        egui::Stroke::new(1.2, col),
                    );
                }
                let grip = grip.on_hover_cursor(egui::CursorIcon::ResizeNwSe);
                if grip.dragged() {
                    self.config.auto_height = false;
                    if let Some(ir) = ctx.input(|i| i.viewport().inner_rect) {
                        let d = grip.drag_delta();
                        let new = egui::vec2(
                            (ir.width() + d.x).max(260.0),
                            (ir.height() + d.y).max(90.0),
                        );
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(new));
                    }
                }
            });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        egui::Window::new("Réglages").open(&mut open).default_width(340.0).show(ctx, |ui| {
            ui.label(egui::RichText::new("Notifications").strong());
            ui.checkbox(&mut self.config.notify_enrage, "Alerte début d'enrage");
            ui.checkbox(&mut self.config.notify_enrage_end, "Alerte fin d'enrage");
            ui.checkbox(&mut self.config.notify_missing_debuff, "Alerte debuff manquant sur le boss");
            ui.checkbox(&mut self.config.sound, "Son (afplay)");
            ui.horizontal(|ui| {
                ui.label("Debuffs requis (ids) :");
                if ui.text_edit_singleline(&mut self.debuff_text).changed() {
                    self.config.required_debuffs =
                        self.debuff_text.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                }
            });
            ui.separator();
            if ui
                .checkbox(&mut self.config.auto_reset, "Auto-reset au pull")
                .on_hover_text("Repart sur un combat neuf quand tu frappes après une pause")
                .changed()
            {
                self.meter.auto_reset = self.config.auto_reset;
            }
            if self.config.auto_reset {
                ui.horizontal(|ui| {
                    ui.label("Inactivité avant reset (s)");
                    if ui
                        .add(egui::Slider::new(&mut self.config.reset_idle_secs, 3.0..=30.0))
                        .changed()
                    {
                        self.meter.reset_idle =
                            Duration::from_secs_f64(self.config.reset_idle_secs.max(1.0));
                    }
                });
            }
            ui.separator();
            ui.checkbox(&mut self.config.auto_height, "Hauteur auto (colle au contenu)");
            ui.horizontal(|ui| {
                ui.label("Opacité");
                let mut op = self.opacity as f32;
                if ui.add(egui::Slider::new(&mut op, 40.0..=255.0).show_value(false)).changed() {
                    self.opacity = op as u8;
                }
            });
            ui.horizontal(|ui| {
                ui.label(format!("Config : {}", config::path().display()));
            });
            if ui.button("💾 Sauver").clicked() {
                self.config.opacity = self.opacity;
                self.config.view = self.view;
                self.config.party_only = self.meter.party_only;
                self.config.save();
            }
        });
        self.show_settings = open;
    }

    fn buffs_window(&mut self, ctx: &egui::Context) {
        if !self.show_buffs {
            return;
        }
        let mut open = true;
        egui::Window::new("Buffs / Debuffs").open(&mut open).default_width(320.0).show(ctx, |ui| {
            let Some(boss) = self.meter.current_boss() else {
                ui.label("Sélectionne un boss (pas TOTAL) pour voir l'uptime.");
                return;
            };
            let window = self.meter.current().interval_secs();
            let rows = self.meter.abnormals.uptime(boss, Instant::now(), window);
            if rows.is_empty() {
                ui.label("aucune abnormality observée sur ce boss.");
                return;
            }
            egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
                egui::Grid::new("buffs").striped(true).num_columns(2).spacing([14.0, 4.0]).show(
                    ui,
                    |ui| {
                        ui.label(egui::RichText::new("Abnormality").strong());
                        ui.label(egui::RichText::new("Uptime").strong());
                        ui.end_row();
                        for (id, pct) in rows {
                            ui.label(self.meter.abnormality_name(id));
                            ui.label(format!("{pct:.0}%"));
                            ui.end_row();
                        }
                    },
                );
            });
        });
        self.show_buffs = open;
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::from_black_alpha(0.0).to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain();
        self.sample_dps(Instant::now());
        ctx.request_repaint_after(Duration::from_millis(150));

        if ctx.input(|i| i.key_pressed(egui::Key::F9)) {
            self.passthrough = !self.passthrough;
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(self.passthrough));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F7)) {
            self.opacity = self.opacity.saturating_sub(20).max(40);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F8)) {
            self.opacity = self.opacity.saturating_add(20);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F6)) {
            self.request_fit = true;
        }

        let frame = egui::Frame::none()
            .fill(egui::Color32::from_rgba_unmultiplied(0x15, 0x18, 0x1e, self.opacity))
            .rounding(egui::Rounding::same(8.0))
            .stroke(egui::Stroke::new(1.0_f32, card_border()))
            .inner_margin(egui::Margin::same(9.0))
            .outer_margin(egui::Margin::same(6.0))
            .shadow(egui::epaint::Shadow {
                offset: egui::vec2(0.0, 3.0),
                blur: 14.0,
                spread: 0.0,
                color: egui::Color32::from_black_alpha(0x66),
            });
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            // Title bar: drag handle + right-aligned controls
            ui.horizontal(|ui| {
                let handle = ui.add(
                    egui::Label::new(egui::RichText::new("⠿ tera-meter").weak())
                        .sense(egui::Sense::drag()),
                );
                if handle.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .selectable_label(self.passthrough, "👁")
                        .on_hover_text("Click-through (F9 pour rebasculer)")
                        .clicked()
                    {
                        self.passthrough = !self.passthrough;
                        ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(
                            self.passthrough,
                        ));
                    }
                    ui.toggle_value(&mut self.show_settings, "⚙");
                    ui.toggle_value(&mut self.show_party, "Groupe");
                    ui.toggle_value(&mut self.show_graph, "Graphe");
                    ui.toggle_value(&mut self.show_buffs, "Buffs");
                    ui.checkbox(&mut self.meter.party_only, "Party");
                    if ui.button("⟳").on_hover_text("Reset tout").clicked() {
                        self.meter.reset(Instant::now());
                        self.detail = None;
                    }
                });
            });

            // Encounter selector with duration + dead marker
            let selected_label = encounter_label(self.meter.current());
            let boss_labels: Vec<String> =
                self.meter.bosses.iter().map(encounter_label).collect();
            egui::ComboBox::from_id_salt("encounter")
                .width(ui.available_width())
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.meter.selected, 0, "TOTAL (zone entière)".to_string());
                    for (i, label) in boss_labels.into_iter().enumerate() {
                        ui.selectable_value(&mut self.meter.selected, i + 1, label);
                    }
                });

            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.view, 0u8, "⚔ Dégâts");
                ui.selectable_value(&mut self.view, 1u8, "✚ Heal");
                ui.selectable_value(&mut self.view, 2u8, "🛡 Tank");
            });
            ui.add_space(4.0);
            egui::Frame::none()
                .fill(egui::Color32::from_white_alpha(8))
                .rounding(5.0)
                .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                .show(ui, |ui| self.summary_bar(ui));
            ui.add_space(3.0);
            self.boss_panel(ui);
            egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(348.0).show(ui, |ui| {
                self.player_rows(ui);
            });
        });

        self.resize_grip(ctx);
        self.detail_window(ctx);
        self.graph_window(ctx);
        self.buffs_window(ctx);
        self.party_window(ctx);
        self.settings_window(ctx);

        let now = Instant::now();
        self.notifier.tick(&self.meter, &self.config, now);
        self.notifier.draw(ctx, now);

        if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
            self.config.window = Some([r.min.x, r.min.y, r.width(), r.height()]);
        }

        let want_fit =
            self.config.auto_height || self.request_fit || (!self.fitted_once && self.last_rows > 0);
        if want_fit {
            let bosses = self.meter.active_bosses().len();
            let rows = self.last_rows.clamp(1, 12);
            let desired_h = 124.0 + bosses as f32 * 24.0 + rows as f32 * 29.0;
            if let Some(ir) = ctx.input(|i| i.viewport().inner_rect) {
                if (ir.height() - desired_h).abs() > 6.0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        ir.width(),
                        desired_h,
                    )));
                }
            }
            if self.last_rows > 0 {
                self.fitted_once = true;
            }
            self.request_fit = false;
        }
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.config.opacity = self.opacity;
        self.config.view = self.view;
        self.config.party_only = self.meter.party_only;
        self.config.save();
    }
}

fn main() -> eframe::Result<()> {
    let cli = Cli::parse();
    let demo = cli.demo;
    let dump = cli.dump;
    let data = GameData::load(&cli.data_dir.join("skills.json"), &cli.data_dir.join("npcs.json"))
        .expect("chargement skills.json / npcs.json");

    let (tx, rx) = channel();
    if cli.live {
        let lcfg = live::Config {
            opcodes: cli.opcodes,
            definitions: cli.definitions,
            patch: cli.patch,
            dump,
            ifaces: cli.iface,
        };
        std::thread::spawn(move || {
            if let Err(e) = live::run(lcfg, tx.clone()) {
                eprintln!("[meter] live: {e:#}");
            }
            let _ = tx.send(Event::Disconnected);
        });
    } else if cli.sniff {
        let scfg = sniff::Config {
            opcodes: cli.opcodes,
            definitions: cli.definitions,
            patch: cli.patch,
            dump,
        };
        std::thread::spawn(move || {
            if let Err(e) = sniff::run(scfg, tx.clone()) {
                eprintln!("[meter] sniff: {e:#}");
            }
            let _ = tx.send(Event::Disconnected);
        });
    } else {
        let cfg = Config {
            data: cli.stream,
            control: cli.control,
            opcodes: cli.opcodes,
            definitions: cli.definitions,
            patch: cli.patch,
            dump,
        };
        std::thread::spawn(move || loop {
            if let Err(e) = net::run(cfg.clone(), tx.clone()) {
                eprintln!("[meter] reader: {e:#}");
            }
            if tx.send(Event::Disconnected).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_secs(3));
        });
    }

    let cfg = config::Config::load();
    let mut viewport = egui::ViewportBuilder::default()
        .with_min_inner_size([260.0, 90.0])
        .with_always_on_top()
        .with_decorations(false)
        .with_resizable(true)
        .with_transparent(true);
    match cfg.window {
        Some([x, y, w, h]) => {
            viewport = viewport.with_inner_size([w, h]).with_position([x, y]);
        }
        None => viewport = viewport.with_inner_size([540.0, 360.0]),
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    eframe::run_native(
        "tera-meter",
        options,
        Box::new(move |cc| {
            setup_style(&cc.egui_ctx);
            let icons = load_icons(&cc.egui_ctx);
            let mut meter = Meter::new(data, Instant::now());
            meter.party_only = cfg.party_only;
            meter.auto_reset = cfg.auto_reset;
            meter.reset_idle = Duration::from_secs_f64(cfg.reset_idle_secs.max(1.0));
            if demo {
                seed_demo(&mut meter);
            }
            let debuff_text =
                cfg.required_debuffs.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            Ok(Box::new(App {
                opacity: cfg.opacity,
                view: cfg.view,
                last_rows: 0,
                fitted_once: false,
                request_fit: false,
                debuff_text,
                meter,
                rx,
                detail: None,
                detail_tab: 0,
                icons,
                passthrough: false,
                show_buffs: false,
                show_party: false,
                show_settings: false,
                show_graph: false,
                dps_history: std::collections::VecDeque::with_capacity(90),
                last_sample: Instant::now(),
                last_total: 0,
                graph_key: (usize::MAX, None, 0),
                config: cfg,
                notifier: notify::Notifier::new(),
            }))
        }),
    )
}

#[allow(dead_code)]
fn is_damage(kind: u8) -> bool {
    kind == KIND_DAMAGE
}
