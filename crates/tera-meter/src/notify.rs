use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::config::Config;
use crate::model::Meter;

const TOAST_TTL: Duration = Duration::from_secs(4);
const MISSING_GRACE: Duration = Duration::from_secs(3);

struct Toast {
    text: String,
    color: egui::Color32,
    born: Instant,
}

pub struct Notifier {
    toasts: Vec<Toast>,
    prev_enraged: HashMap<u64, bool>,
    missing_since: Option<Instant>,
    missing_alerted: bool,
}

impl Notifier {
    pub fn new() -> Self {
        Self { toasts: Vec::new(), prev_enraged: HashMap::new(), missing_since: None, missing_alerted: false }
    }

    fn push(&mut self, text: String, color: egui::Color32, now: Instant, cfg: &Config) {
        self.toasts.push(Toast { text, color, born: now });
        if cfg.sound {
            play_sound();
        }
    }

    pub fn tick(&mut self, meter: &Meter, cfg: &Config, now: Instant) {
        for (gid, name, _cur, _max, enraged, _ms) in meter.active_bosses() {
            let prev = self.prev_enraged.get(&gid).copied().unwrap_or(false);
            if enraged && !prev && cfg.notify_enrage {
                self.push(format!("⚠  {name}  —  ENRAGE"), egui::Color32::from_rgb(0xff, 0x44, 0x44), now, cfg);
            } else if !enraged && prev && cfg.notify_enrage_end {
                self.push(format!("✓  {name}  —  fin d'enrage"), egui::Color32::from_rgb(0x66, 0xdd, 0x77), now, cfg);
            }
            self.prev_enraged.insert(gid, enraged);
        }

        if cfg.notify_missing_debuff && !cfg.required_debuffs.is_empty() {
            let missing: Vec<u32> = match meter.current_boss() {
                Some(boss) => {
                    let active = meter.abnormals.active(boss);
                    cfg.required_debuffs.iter().copied().filter(|id| !active.contains(id)).collect()
                }
                None => Vec::new(),
            };
            if missing.is_empty() {
                self.missing_since = None;
                self.missing_alerted = false;
            } else {
                let since = *self.missing_since.get_or_insert(now);
                if !self.missing_alerted && now.duration_since(since) >= MISSING_GRACE {
                    self.missing_alerted = true;
                    let names: Vec<String> = missing.iter().map(|id| meter.abnormality_name(*id)).collect();
                    self.push(
                        format!("🛡  debuff manquant : {}", names.join(", ")),
                        egui::Color32::from_rgb(0xff, 0xd5, 0x4f),
                        now,
                        cfg,
                    );
                }
            }
        }

        self.toasts.retain(|t| now.duration_since(t.born) < TOAST_TTL);
    }

    pub fn draw(&self, ctx: &egui::Context, now: Instant) {
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new("tera-meter-toasts".into())
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 10.0))
            .interactable(false)
            .show(ctx, |ui| {
                for t in &self.toasts {
                    let age = now.duration_since(t.born).as_secs_f32();
                    let ttl = TOAST_TTL.as_secs_f32();
                    let alpha = (1.0 - (age / ttl)).clamp(0.0, 1.0);
                    let a = (alpha * 255.0) as u8;
                    let bg = egui::Color32::from_rgba_unmultiplied(0x10, 0x12, 0x16, (alpha * 235.0) as u8);
                    let fg = t.color.gamma_multiply(alpha);
                    egui::Frame::none()
                        .fill(bg)
                        .rounding(6.0)
                        .stroke(egui::Stroke::new(1.0_f32, fg.gamma_multiply(0.6)))
                        .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(&t.text)
                                    .size(15.0)
                                    .strong()
                                    .color(egui::Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, a)),
                            );
                            let _ = fg;
                        });
                    ui.add_space(4.0);
                }
            });
    }
}

#[cfg(target_os = "macos")]
fn play_sound() {
    let _ = std::process::Command::new("afplay").arg("/System/Library/Sounds/Glass.aiff").spawn();
}

#[cfg(not(target_os = "macos"))]
fn play_sound() {
    print!("\x07");
}
