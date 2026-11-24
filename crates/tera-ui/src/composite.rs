use crate::jobs::Job;
use crate::theme::{self, Palette};
use crate::Paths;
use eframe::egui;
use std::path::PathBuf;
use tera_mapper::{install_mod, mapper_path, restore_mapper, uninstall_mod, CompositeMapper};

#[derive(Default)]
pub struct CompositeTab {
    mapper: Option<CompositeMapper>,
    error: Option<String>,
    query: String,
    delete_file: bool,
    job: Option<Job>,
    loaded: bool,
}

impl CompositeTab {
    fn dat(paths: &Paths) -> PathBuf {
        mapper_path(&paths.cooked())
    }

    fn load(&mut self, paths: &Paths) {
        self.loaded = true;
        let dat = Self::dat(paths);
        match CompositeMapper::read(&dat) {
            Ok(mapper) => {
                self.mapper = Some(mapper);
                self.error = None;
            }
            Err(error) => {
                self.mapper = None;
                self.error = Some(format!("{dat}: {error}", dat = dat.display()));
            }
        }
    }

    fn tick(&mut self, ctx: &egui::Context, paths: &Paths, status: &mut String) {
        let Some(job) = &mut self.job else {
            return;
        };
        match job.poll() {
            Some(outcome) => {
                *status = match outcome {
                    Ok(message) => message,
                    Err(error) => format!("error: {error}"),
                };
                self.job = None;
                self.load(paths);
            }
            None => ctx.request_repaint_after(std::time::Duration::from_millis(120)),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, palette: Palette, paths: &Paths, status: &mut String) {
        if !self.loaded {
            self.load(paths);
        }
        let context = ui.ctx().clone();
        self.tick(&context, paths, status);
        let running = self.job.is_some();

        ui.add_space(8.0);
        ui.label(theme::display(palette, "Composite", 22.0));
        theme::eyebrow(
            ui,
            palette,
            "composite GPK mods (OmiMapper): each install backs up the .clean and can be restored",
        );
        theme::rule(ui, palette);

        let dat = Self::dat(paths);
        ui.horizontal(|ui| {
            if ui.button("refresh").clicked() {
                self.load(paths);
            }
            if ui
                .add_enabled(!running, egui::Button::new("install a composite mod…"))
                .clicked()
            {
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("package", &["gpk", "upk"])
                    .set_directory(paths.cooked())
                    .pick_file()
                {
                    let cooked = paths.cooked();
                    *status = "installing…".into();
                    self.job = Some(Job::spawn("install composite", move |_| {
                        let report = install_mod(&cooked, &file, None).map_err(|e| e.to_string())?;
                        Ok(format!(
                            "installed {} -> {} ({} route(s))",
                            report.container,
                            report.destination.display(),
                            report.registered.len()
                        ))
                    }));
                }
            }
            if ui
                .add_enabled(!running, egui::Button::new("restore all (.clean)"))
                .clicked()
            {
                let cooked = paths.cooked();
                *status = "restoring…".into();
                self.job = Some(Job::spawn("restore mapper", move |_| {
                    let dat = restore_mapper(&cooked).map_err(|e| e.to_string())?;
                    Ok(format!("mapper restored: {}", dat.display()))
                }));
            }
            ui.checkbox(&mut self.delete_file, "also delete the .gpk");
        });
        theme::eyebrow(ui, palette, dat.display().to_string());

        if let Some(error) = &self.error {
            ui.colored_label(theme::colors(palette).accent_high, error);
            return;
        }
        let Some(mapper) = &self.mapper else {
            return;
        };

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} route(s) dans {} shard(s)",
                mapper.len(),
                mapper.groups.len()
            ));
            ui.add_space(12.0);
            ui.label("filter");
            ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(220.0));
        });
        ui.add_space(4.0);

        let needle = self.query.trim().to_ascii_lowercase();
        let row_budget = 600usize;
        let mut uninstall_target = None;
        let mut truncated = false;
        egui::ScrollArea::vertical()
            .id_salt("composite-groups")
            .show(ui, |ui| {
                let mut rows = 0usize;
                for group in &mapper.groups {
                    let matches = needle.is_empty()
                        || group.filename.to_ascii_lowercase().contains(&needle)
                        || group.entries.iter().any(|entry| {
                            entry.composite_name.to_ascii_lowercase().contains(&needle)
                                || entry.object_path.to_ascii_lowercase().contains(&needle)
                        });
                    if !matches {
                        continue;
                    }
                    if rows >= row_budget {
                        truncated = true;
                        break;
                    }
                    ui.horizontal(|ui| {
                        ui.label(theme::display(palette, &group.filename, 14.0));
                        ui.label(
                            egui::RichText::new(format!("{} route(s)", group.entries.len()))
                                .color(theme::colors(palette).dim),
                        );
                        if ui
                            .add_enabled(!running, egui::Button::new("uninstall"))
                            .clicked()
                        {
                            uninstall_target = Some(group.filename.clone());
                        }
                    });
                    rows += 1;
                    for entry in group.entries.iter().take(row_budget.saturating_sub(rows)) {
                        ui.label(
                            egui::RichText::new(format!(
                                "    {}  ·  {}  ·  off {} size {}",
                                entry.composite_name, entry.object_path, entry.offset, entry.size
                            ))
                            .color(theme::colors(palette).dim),
                        );
                        rows += 1;
                    }
                    if group.entries.len() > row_budget {
                        ui.label(
                            egui::RichText::new(format!(
                                "    … +{} route(s), refine the filter",
                                group.entries.len() - row_budget
                            ))
                            .color(theme::colors(palette).accent_high),
                        );
                    }
                    ui.add_space(4.0);
                }
                if truncated {
                    ui.label(
                        egui::RichText::new("… list truncated, refine the filter")
                            .color(theme::colors(palette).accent_high),
                    );
                }
            });

        if let Some(target) = uninstall_target {
            let cooked = paths.cooked();
            let delete = self.delete_file;
            *status = format!("uninstalling {target}…");
            self.job = Some(Job::spawn("uninstall composite", move |_| {
                let report = uninstall_mod(&cooked, &target, delete).map_err(|e| e.to_string())?;
                Ok(format!(
                    "removed {} ({} route(s), {} restored)",
                    report.target,
                    report.removed.len(),
                    report.restored.len()
                ))
            }));
        }
    }
}
