use crate::jobs::{report, Job};
use crate::theme::{self, Palette};
use crate::Paths;
use eframe::egui;
use std::path::PathBuf;
use tera_crypto::{scan_bytes, KeyIv, ScanMode, ZlibOracle, ORACLE_PREFIX_LEN};

#[derive(Default)]
pub struct KeysTab {
    data: Option<PathBuf>,
    haystack: Option<PathBuf>,
    mode: usize,
    radius: usize,
    align: usize,
    job: Option<Job>,
    found: Vec<(String, String, String)>,
}

impl KeysTab {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        palette: Palette,
        paths: &Paths,
        status: &mut String,
    ) {
        if self.radius == 0 {
            self.radius = 256;
            self.align = 1;
        }
        if let Some(job) = &mut self.job {
            if let Some(outcome) = job.poll() {
                *status = match outcome {
                    Ok(message) => message,
                    Err(error) => format!("erreur : {error}"),
                };
                self.job = None;
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        ui.add_space(8.0);
        ui.label(theme::display(palette, "AES key recovery", 22.0));
        theme::eyebrow(
            ui,
            palette,
            "the encrypted .dat is the oracle: plausible size, valid zlib header, then a control inflate",
        );
        theme::rule(ui, palette);

        ui.horizontal(|ui| {
            if ui.button(".dat file…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("datacenter", &["dat"])
                    .set_directory(paths.datacenter.parent().unwrap_or(std::path::Path::new(".")))
                    .pick_file()
                {
                    self.data = Some(path);
                }
            }
            ui.label(
                self.data
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| paths.datacenter.display().to_string()),
            );
        });
        ui.horizontal(|ui| {
            if ui.button("target to scan…").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    self.haystack = Some(path);
                }
            }
            if ui.button("folder…").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.haystack = Some(path);
                }
            }
            ui.label(
                self.haystack
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "none".into()),
            );
        });
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("mode")
                .selected_text(["adjacent", "window", "exhaustive"][self.mode.min(2)])
                .show_ui(ui, |ui| {
                    for (index, label) in ["adjacent", "window", "exhaustive"].iter().enumerate() {
                        ui.selectable_value(&mut self.mode, index, *label);
                    }
                });
            ui.add(egui::DragValue::new(&mut self.radius).prefix("radius ").range(16..=4096));
            ui.add(egui::DragValue::new(&mut self.align).prefix("align ").range(1..=16));
        });

        ui.horizontal(|ui| {
            let ready = self.job.is_none();
            if ui.add_enabled(ready, egui::Button::new("try known keys")).clicked() {
                let path = self.data.clone().unwrap_or_else(|| paths.datacenter.clone());
                match std::fs::read(&path) {
                    Ok(bytes) => match tera_datacenter::detect_key(&bytes) {
                        Some(keyiv) => {
                            self.found.push((
                                "built-in table".into(),
                                keyiv.key_hex(),
                                keyiv.iv_hex(),
                            ));
                            *status = "known key verified".into();
                        }
                        None => *status = "no known key decrypts this file".into(),
                    },
                    Err(error) => *status = format!("error: {error}"),
                }
            }
            if ui.add_enabled(ready, egui::Button::new("scan")).clicked() {
                let data = self.data.clone().unwrap_or_else(|| paths.datacenter.clone());
                let Some(haystack) = self.haystack.clone() else {
                    *status = "pick a target to scan".into();
                    return;
                };
                let mode = match self.mode {
                    0 => ScanMode::Adjacent,
                    1 => ScanMode::Window(self.radius),
                    _ => ScanMode::Exhaustive,
                };
                let align = self.align.max(1);
                *status = "scanning…".into();
                self.job = Some(Job::spawn("scan", move |sender| {
                    let encrypted = std::fs::read(&data).map_err(|error| error.to_string())?;
                    let oracle = ZlibOracle::new(
                        &encrypted[..encrypted.len().min(ORACLE_PREFIX_LEN)],
                        encrypted.len() as u64,
                    );
                    let mut files = Vec::new();
                    if haystack.is_dir() {
                        let mut stack = vec![haystack.clone()];
                        while let Some(directory) = stack.pop() {
                            let Ok(entries) = std::fs::read_dir(&directory) else {
                                continue;
                            };
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.is_dir() {
                                    stack.push(path);
                                } else {
                                    files.push(path);
                                }
                            }
                        }
                    } else {
                        files.push(haystack.clone());
                    }
                    let total = files.len();
                    let mut hits: Vec<KeyIv> = Vec::new();
                    for (position, path) in files.iter().enumerate() {
                        report(
                            sender,
                            path.file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            position,
                            total,
                        );
                        let Ok(bytes) = std::fs::read(path) else {
                            continue;
                        };
                        for candidate in scan_bytes(&bytes, &oracle, mode, align) {
                            if candidate.verified {
                                hits.push(candidate.keyiv);
                            }
                        }
                    }
                    match hits.first() {
                        Some(keyiv) => Ok(format!(
                            "key found: {} / iv {}",
                            keyiv.key_hex(),
                            keyiv.iv_hex()
                        )),
                        None => Ok(format!("no key found in {total} file(s)")),
                    }
                }));
            }
            if let Some(job) = &self.job {
                ui.add(egui::ProgressBar::new(job.fraction()).desired_width(220.0));
                theme::eyebrow(ui, palette, job.detail.clone());
            }
        });

        theme::rule(ui, palette);
        for (source, key, iv) in &self.found {
            ui.horizontal(|ui| {
                theme::eyebrow(ui, palette, source);
                ui.monospace(format!("key {key}"));
                ui.monospace(format!("iv {iv}"));
                if ui.button("copy").clicked() {
                    ui.ctx().copy_text(format!("--key {key} --iv {iv}"));
                }
            });
        }
    }
}
