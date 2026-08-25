use crate::jobs::Job;
use crate::theme::{self, Palette};
use crate::Paths;
use eframe::egui;
use std::path::{Path, PathBuf};
use tera_mod::apply::{Install, Receipt};
use tera_mod::manifest::Manifest;

const MANIFEST: &str = "mod.toml";

struct Entry {
    directory: PathBuf,
    manifest: Manifest,
}

#[derive(Default)]
pub struct ModsTab {
    library: Option<PathBuf>,
    entries: Vec<Entry>,
    unreadable: Vec<(PathBuf, String)>,
    selected: Option<String>,
    applied: Vec<Receipt>,
    clashes: Vec<(String, PathBuf)>,
    plan: Vec<(PathBuf, String)>,
    job: Option<Job>,
    scanned: bool,
}

impl ModsTab {
    fn library(&self, paths: &Paths) -> PathBuf {
        self.library.clone().unwrap_or_else(|| paths.mod_library())
    }

    fn install(paths: &Paths) -> Install {
        Install::new(paths.game.clone(), paths.game.join(".tera-mods"))
    }

    fn scan(&mut self, paths: &Paths) {
        self.entries.clear();
        self.unreadable.clear();
        let library = self.library(paths);
        let mut directories = vec![library.clone()];
        if let Ok(children) = std::fs::read_dir(&library) {
            directories.extend(children.flatten().map(|child| child.path()).filter(|path| path.is_dir()));
        }
        for directory in directories {
            let file = directory.join(MANIFEST);
            if !file.exists() {
                continue;
            }
            match Manifest::read(&file) {
                Ok(manifest) => self.entries.push(Entry {
                    directory,
                    manifest,
                }),
                Err(error) => self.unreadable.push((file, format!("{error:#}"))),
            }
        }
        self.entries
            .sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
        self.applied = Self::install(paths).applied();
        if let Some(name) = self.selected.clone() {
            if !self.entries.iter().any(|entry| entry.manifest.name == name) {
                self.selected = None;
            }
        }
        self.scanned = true;
        self.refresh_detail(paths);
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.manifest.name == name)
    }

    fn refresh_detail(&mut self, paths: &Paths) {
        self.clashes.clear();
        self.plan.clear();
        let Some(index) = self.selected.as_deref().and_then(|name| self.index_of(name)) else {
            return;
        };
        let install = Self::install(paths);
        let manifest = &self.entries[index].manifest;
        self.clashes = install.conflicts_with(manifest, &paths.datacenter, &self.applied);
        self.plan = install.plan(manifest, &paths.datacenter);
    }

    pub fn tick(&mut self, ctx: &egui::Context, paths: &Paths, status: &mut String) -> bool {
        let Some(job) = &mut self.job else {
            return false;
        };
        match job.poll() {
            Some(outcome) => {
                *status = match outcome {
                    Ok(message) => message,
                    Err(error) => format!("erreur : {error}"),
                };
                self.job = None;
                self.scan(paths);
                true
            }
            None => {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
                false
            }
        }
    }

    fn is_applied(&self, name: &str) -> bool {
        self.applied.iter().any(|receipt| receipt.name == name)
    }

    fn start(&mut self, label: &str, work: impl FnOnce() -> Result<String, String> + Send + 'static) {
        self.job = Some(Job::spawn(label, move |_| work()));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, palette: Palette, paths: &Paths, status: &mut String) {
        if !self.scanned {
            self.scan(paths);
        }
        let context = ui.ctx().clone();
        self.tick(&context, paths, status);
        self.refresh_detail(paths);

        ui.add_space(8.0);
        ui.label(theme::display(palette, "Mods", 22.0));
        theme::eyebrow(
            ui,
            palette,
            "every file is backed up before it is touched, and revert puts it back byte for byte",
        );
        theme::rule(ui, palette);

        self.library_row(ui, palette, paths);
        ui.add_space(6.0);

        let running = self.job.is_some();
        ui.columns(2, |columns| {
            self.list(&mut columns[0], palette, running);
            self.detail(&mut columns[1], palette, paths, status, running);
        });
    }

    fn library_row(&mut self, ui: &mut egui::Ui, palette: Palette, paths: &Paths) {
        let library = self.library(paths);
        ui.horizontal(|ui| {
            if ui.button("mod folder…").clicked() {
                if let Some(path) = rfd::FileDialog::new().set_directory(&library).pick_folder() {
                    self.library = Some(path);
                    self.scan(paths);
                }
            }
            if ui.button("rescan").clicked() {
                self.scan(paths);
            }
            ui.label(library.display().to_string());
        });
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} mod(s) found, {} applied",
                self.entries.len(),
                self.applied.len()
            ));
            if !self.unreadable.is_empty() {
                ui.label(
                    egui::RichText::new(format!("{} unreadable", self.unreadable.len()))
                        .color(theme::colors(palette).accent_high),
                );
            }
        });
    }

    fn list(&mut self, ui: &mut egui::Ui, palette: Palette, running: bool) {
        ui.label(theme::display(palette, "library", 15.0));
        egui::ScrollArea::vertical()
            .id_salt("mod-library")
            .max_height(460.0)
            .show(ui, |ui| {
                let mut pick = None;
                for index in 0..self.entries.len() {
                    let name = self.entries[index].manifest.name.clone();
                    let version = self.entries[index].manifest.version.clone();
                    let changes = self.entries[index].manifest.changes.len();
                    let live = self.is_applied(&name);
                    let selected = self.selected.as_deref() == Some(name.as_str());
                    let label = format!(
                        "{}{name}  {version}  ({changes} change{})",
                        if live { "* " } else { "  " },
                        if changes == 1 { "" } else { "s" }
                    );
                    if ui.selectable_label(selected, label).clicked() && !running {
                        pick = Some(name);
                    }
                }
                if let Some(name) = pick {
                    self.selected = Some(name);
                }
                for (path, error) in &self.unreadable {
                    ui.label(
                        egui::RichText::new(format!("{}: {error}", path.display()))
                            .color(theme::colors(palette).accent_high),
                    );
                }
            });
    }

    fn detail(
        &mut self,
        ui: &mut egui::Ui,
        palette: Palette,
        paths: &Paths,
        status: &mut String,
        running: bool,
    ) {
        let Some(index) = self.selected.as_deref().and_then(|name| self.index_of(name)) else {
            ui.label(theme::display(palette, "pick a mod", 15.0));
            self.applied_list(ui, palette);
            return;
        };
        let entry = &self.entries[index];
        let manifest = entry.manifest.clone();
        let directory = entry.directory.clone();
        let install = Self::install(paths);
        let live = self.is_applied(&manifest.name);
        let clashes = self.clashes.clone();
        let name = manifest.name.clone();

        ui.label(theme::display(palette, &manifest.name, 18.0));
        if !manifest.description.is_empty() {
            ui.label(&manifest.description);
        }
        if !manifest.author.is_empty() {
            theme::eyebrow(ui, palette, format!("by {}", manifest.author));
        }
        ui.add_space(4.0);

        let missing_datacenter = manifest.touches_data_center() && !paths.datacenter.exists();
        if missing_datacenter {
            ui.label(
                egui::RichText::new(format!(
                    "this mod edits the data center but {} is not there",
                    paths.datacenter.display()
                ))
                .color(theme::colors(palette).accent_high),
            );
        }
        for (other, target) in &clashes {
            ui.label(
                egui::RichText::new(format!("{other} already changed {}", target.display()))
                    .color(theme::colors(palette).accent_high),
            );
        }

        ui.horizontal(|ui| {
            let can_apply = !running && !live && clashes.is_empty() && !missing_datacenter;
            if ui.add_enabled(can_apply, egui::Button::new("apply")).clicked() {
                let datacenter = paths.datacenter.clone();
                *status = format!("applying {name}…");
                self.start("apply", move || {
                    install
                        .apply(&manifest, &datacenter, &directory)
                        .map(|receipt| {
                            format!("{} applied, {} file(s) touched", receipt.name, receipt.applied.len())
                        })
                        .map_err(|error| format!("{error:#}"))
                });
            }
            if ui.add_enabled(!running && live, egui::Button::new("revert")).clicked() {
                let reverting = Self::install(paths);
                let name = name.clone();
                *status = format!("reverting {name}…");
                self.start("revert", move || {
                    reverting
                        .revert(&name)
                        .map(|receipt| {
                            format!("{} reverted, {} file(s) restored", receipt.name, receipt.applied.len())
                        })
                        .map_err(|error| format!("{error:#}"))
                });
            }
            if running {
                ui.spinner();
            }
        });

        ui.add_space(6.0);
        ui.label(theme::display(palette, "what it will change", 15.0));
        let plan = self.plan.clone();
        egui::ScrollArea::vertical()
            .id_salt("mod-plan")
            .max_height(300.0)
            .show(ui, |ui| {
                for (target, summary) in plan {
                    ui.label(summary);
                    theme::eyebrow(ui, palette, short(&target, &paths.game));
                    ui.add_space(2.0);
                }
            });
        self.applied_list(ui, palette);
    }

    fn applied_list(&self, ui: &mut egui::Ui, palette: Palette) {
        if self.applied.is_empty() {
            return;
        }
        ui.add_space(6.0);
        theme::rule(ui, palette);
        ui.label(theme::display(palette, "applied right now", 15.0));
        for receipt in &self.applied {
            ui.label(format!(
                "{} {} — {} file(s)",
                receipt.name,
                receipt.version,
                receipt.applied.len()
            ));
        }
    }
}

fn short(target: &Path, root: &Path) -> String {
    target
        .strip_prefix(root)
        .unwrap_or(target)
        .display()
        .to_string()
}
