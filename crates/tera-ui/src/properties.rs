use crate::jobs::Job;
use crate::theme::{self, Palette};
use crate::Paths;
use eframe::egui;
use std::path::{Path, PathBuf};
use tera_package::{object_properties, set_object_properties};

struct FieldEdit {
    name: String,
    type_name: String,
    original: String,
    value: String,
    editable: bool,
}

impl FieldEdit {
    fn changed(&self) -> bool {
        self.editable && self.value.trim() != self.original.trim()
    }
}

struct ObjectEdit {
    path: String,
    class: String,
    fields: Vec<FieldEdit>,
}

impl ObjectEdit {
    fn changed(&self) -> bool {
        self.fields.iter().any(FieldEdit::changed)
    }
}

#[derive(Default)]
pub struct PropertiesTab {
    file: Option<PathBuf>,
    objects: Vec<ObjectEdit>,
    error: Option<String>,
    filter: String,
    query: String,
    job: Option<Job>,
}

impl PropertiesTab {
    fn open(&mut self, file: PathBuf) {
        self.objects.clear();
        let bytes = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error = Some(format!("{}: {error}", file.display()));
                self.file = None;
                return;
            }
        };
        self.error = None;
        self.file = Some(file);
        for object in object_properties(&bytes, self.filter.trim()) {
            let fields = object
                .props
                .into_iter()
                .map(|property| FieldEdit {
                    name: property.name,
                    type_name: property.type_name,
                    original: property.value.clone(),
                    value: property.value,
                    editable: property.editable,
                })
                .collect();
            self.objects.push(ObjectEdit {
                path: object.path,
                class: object.class,
                fields,
            });
        }
    }

    fn pending(&self) -> Vec<(String, Vec<(String, String)>)> {
        self.objects
            .iter()
            .filter(|object| object.changed())
            .map(|object| {
                let edits = object
                    .fields
                    .iter()
                    .filter(|field| field.changed())
                    .map(|field| (field.name.clone(), field.value.trim().to_string()))
                    .collect();
                (object.path.clone(), edits)
            })
            .collect()
    }

    fn apply_edits(
        source: &Path,
        edits: &[(String, Vec<(String, String)>)],
    ) -> Result<Vec<u8>, String> {
        let mut bytes = std::fs::read(source).map_err(|error| error.to_string())?;
        for (path, set) in edits {
            bytes = set_object_properties(&bytes, path, set)
                .map_err(|error| error.to_string())?
                .bytes;
        }
        Ok(bytes)
    }

    fn spawn_write(&mut self, source: PathBuf, target: PathBuf, backup: bool, status: &mut String) {
        let edits = self.pending();
        *status = "writing…".into();
        self.job = Some(Job::spawn("write properties", move |_| {
            let touched = edits.len();
            let bytes = Self::apply_edits(&source, &edits)?;
            if backup {
                let mut clean = source.clone().into_os_string();
                clean.push(".clean");
                let clean = PathBuf::from(clean);
                if !clean.exists() {
                    std::fs::copy(&source, &clean).map_err(|error| error.to_string())?;
                }
            }
            std::fs::write(&target, &bytes).map_err(|error| error.to_string())?;
            Ok(format!("wrote {} ({} object(s))", target.display(), touched))
        }));
    }

    fn spawn_author(
        &mut self,
        source: PathBuf,
        cooked: PathBuf,
        container: String,
        status: &mut String,
    ) {
        let edits = self.pending();
        *status = "building composite mod…".into();
        self.job = Some(Job::spawn("author composite", move |_| {
            let bytes = Self::apply_edits(&source, &edits)?;
            let report = tera_mapper::author_composite(&cooked, &bytes, &container)
                .map_err(|error| error.to_string())?;
            Ok(format!(
                "composite mod installed: {} ({} route(s)) -> {}",
                report.container,
                report.registered.len(),
                report.destination.display()
            ))
        }));
    }

    fn tick(&mut self, ctx: &egui::Context, status: &mut String) {
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
                if let Some(file) = self.file.clone() {
                    self.open(file);
                }
            }
            None => ctx.request_repaint_after(std::time::Duration::from_millis(120)),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, palette: Palette, paths: &Paths, status: &mut String) {
        let context = ui.ctx().clone();
        self.tick(&context, status);
        let running = self.job.is_some();

        ui.add_space(8.0);
        ui.label(theme::display(palette, "Properties", 22.0));
        theme::eyebrow(
            ui,
            palette,
            "edit any scalar property (float/int/bool/byte) of any object in a .gpk",
        );
        theme::rule(ui, palette);

        ui.horizontal(|ui| {
            if ui.button("open a .gpk…").clicked() {
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("package", &["gpk", "upk"])
                    .set_directory(paths.cooked())
                    .pick_file()
                {
                    self.open(file);
                }
            }
            ui.label("objets contenant");
            let response =
                ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(180.0));
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                if let Some(file) = self.file.clone() {
                    self.open(file);
                }
            }
            if let Some(file) = &self.file {
                ui.label(file.display().to_string());
            }
        });

        if let Some(error) = &self.error {
            ui.colored_label(theme::colors(palette).accent_high, error);
        }
        if self.file.is_none() {
            return;
        }

        let pending = self.pending();
        let dirty = !pending.is_empty();
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} object(s) · {} changed",
                self.objects.len(),
                pending.len()
            ));
            ui.add_space(12.0);
            ui.label("display filter");
            ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(200.0));
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(dirty && !running, egui::Button::new("save a copy…"))
                .clicked()
            {
                if let Some(source) = self.file.clone() {
                    let default = source
                        .file_stem()
                        .map(|stem| format!("{}.modded.gpk", stem.to_string_lossy()))
                        .unwrap_or_else(|| "modded.gpk".into());
                    if let Some(target) = rfd::FileDialog::new()
                        .set_directory(source.parent().unwrap_or(Path::new(".")))
                        .set_file_name(default)
                        .save_file()
                    {
                        self.spawn_write(source, target, false, status);
                    }
                }
            }
            if ui
                .add_enabled(
                    dirty && !running,
                    egui::Button::new("apply to original (.clean backup)"),
                )
                .clicked()
            {
                if let Some(source) = self.file.clone() {
                    self.spawn_write(source.clone(), source, true, status);
                }
            }
            if ui
                .add_enabled(
                    dirty && !running,
                    egui::Button::new("create a composite mod (install)"),
                )
                .clicked()
            {
                if let Some(source) = self.file.clone() {
                    let container = source
                        .file_stem()
                        .map(|stem| format!("{}_mod", stem.to_string_lossy()))
                        .unwrap_or_else(|| "studio_mod".into());
                    self.spawn_author(source, paths.cooked(), container, status);
                }
            }
        });
        theme::rule(ui, palette);

        let needle = self.query.trim().to_ascii_lowercase();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for object in &mut self.objects {
                    if !needle.is_empty() && !object.path.to_ascii_lowercase().contains(&needle) {
                        continue;
                    }
                    if !object.fields.iter().any(|field| field.editable) {
                        continue;
                    }
                    egui::CollapsingHeader::new(format!("[{}] {}", object.class, object.path))
                        .id_salt(&object.path)
                        .show(ui, |ui| {
                            egui::Grid::new(&object.path)
                                .num_columns(3)
                                .striped(true)
                                .show(ui, |ui| {
                                    for field in &mut object.fields {
                                        if !field.editable {
                                            continue;
                                        }
                                        ui.label(format!("{} {}", field.type_name, field.name));
                                        ui.add(
                                            egui::TextEdit::singleline(&mut field.value)
                                                .desired_width(160.0),
                                        );
                                        if field.changed() && ui.small_button("↺").clicked() {
                                            field.value = field.original.clone();
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                }
            });
    }
}
