use crate::jobs::Job;
use crate::theme::{self, Palette};
use crate::Paths;
use eframe::egui;
use std::path::{Path, PathBuf};
use tera_package::{color_nodes, materials, set_floats, set_parameters};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Instance,
    Expression,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParamKind {
    Colour,
    Scalar,
    Texture,
}

impl ParamKind {
    fn from_label(label: &str) -> Self {
        match label {
            "colour" => Self::Colour,
            "scalar" => Self::Scalar,
            _ => Self::Texture,
        }
    }
}

struct ParamEdit {
    name: String,
    kind: ParamKind,
    original: String,
    value: String,
    rgba: [f32; 4],
}

impl ParamEdit {
    fn changed(&self) -> bool {
        self.value.trim() != self.original.trim()
    }
}

struct MaterialEdit {
    path: String,
    source: Source,
    class: String,
    params: Vec<ParamEdit>,
}

impl MaterialEdit {
    fn changed(&self) -> bool {
        self.params.iter().any(ParamEdit::changed)
    }
}

#[derive(Default)]
pub struct MaterialsTab {
    file: Option<PathBuf>,
    materials: Vec<MaterialEdit>,
    error: Option<String>,
    selected: Option<usize>,
    query: String,
    job: Option<Job>,
}

fn parse_rgba(value: &str) -> [f32; 4] {
    let mut channels = [0.0f32; 4];
    for (slot, part) in channels.iter_mut().zip(value.split(',')) {
        *slot = part.trim().parse().unwrap_or(0.0);
    }
    channels
}

fn format_rgba(rgba: [f32; 4]) -> String {
    format!("{},{},{},{}", rgba[0], rgba[1], rgba[2], rgba[3])
}

impl MaterialsTab {
    fn open(&mut self, file: PathBuf) {
        self.selected = None;
        self.materials.clear();
        let bytes = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error = Some(format!("{}: {error}", file.display()));
                self.file = None;
                return;
            }
        };
        for material in materials(&bytes) {
            let params = material
                .parameters
                .iter()
                .map(|parameter| {
                    let kind = ParamKind::from_label(parameter.kind.label());
                    let rgba = if kind == ParamKind::Colour {
                        parse_rgba(&parameter.value)
                    } else {
                        [0.0; 4]
                    };
                    ParamEdit {
                        name: parameter.name.clone(),
                        kind,
                        original: parameter.value.clone(),
                        value: parameter.value.clone(),
                        rgba,
                    }
                })
                .collect();
            self.materials.push(MaterialEdit {
                path: material.path.clone(),
                source: Source::Instance,
                class: "MaterialInstance".into(),
                params,
            });
        }
        for node in color_nodes(&bytes) {
            let params = node
                .channels
                .iter()
                .map(|(name, value)| ParamEdit {
                    name: name.clone(),
                    kind: ParamKind::Scalar,
                    original: value.to_string(),
                    value: value.to_string(),
                    rgba: [0.0; 4],
                })
                .collect();
            self.materials.push(MaterialEdit {
                path: node.path.clone(),
                source: Source::Expression,
                class: node.class.clone(),
                params,
            });
        }
        self.error = if self.materials.is_empty() {
            Some("no editable material in this file".into())
        } else {
            None
        };
        self.file = Some(file);
    }

    fn pending(&self) -> Vec<(Source, String, Vec<(String, String)>)> {
        self.materials
            .iter()
            .filter(|material| material.changed())
            .map(|material| {
                let edits = material
                    .params
                    .iter()
                    .filter(|parameter| parameter.changed())
                    .map(|parameter| (parameter.name.clone(), parameter.value.trim().to_string()))
                    .collect();
                (material.source, material.path.clone(), edits)
            })
            .collect()
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
        ui.label(theme::display(palette, "Materials", 22.0));
        theme::eyebrow(
            ui,
            palette,
            "edit colors (MaterialInstance + MaterialExpressionConstant), scalars and textures, then rewrite the .gpk",
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
                "{} material(s) · {} changed",
                self.materials.len(),
                pending.len()
            ));
            ui.add_space(12.0);
            ui.label("filter");
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
            if ui.add_enabled(!running, egui::Button::new("reload")).clicked() {
                if let Some(file) = self.file.clone() {
                    self.open(file);
                }
            }
        });
        ui.add_space(4.0);

        let needle = self.query.trim().to_ascii_lowercase();
        ui.columns(2, |columns| {
            self.list(&mut columns[0], palette, &needle);
            self.detail(&mut columns[1], palette);
        });
    }

    fn list(&mut self, ui: &mut egui::Ui, palette: Palette, needle: &str) {
        ui.label(theme::display(palette, "materials", 15.0));
        egui::ScrollArea::vertical()
            .id_salt("material-list")
            .max_height(520.0)
            .show(ui, |ui| {
                let mut pick = None;
                for index in 0..self.materials.len() {
                    let material = &self.materials[index];
                    if !needle.is_empty() && !material.path.to_ascii_lowercase().contains(needle) {
                        continue;
                    }
                    let leaf = material.path.rsplit('.').next().unwrap_or(&material.path);
                    let tag = match material.source {
                        Source::Instance => "MI",
                        Source::Expression => "expr",
                    };
                    let label = format!(
                        "{}{leaf}  [{tag}] ({} param)",
                        if material.changed() { "* " } else { "  " },
                        material.params.len()
                    );
                    if ui.selectable_label(self.selected == Some(index), label).clicked() {
                        pick = Some(index);
                    }
                }
                if pick.is_some() {
                    self.selected = pick;
                }
            });
    }

    fn detail(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let Some(index) = self.selected else {
            ui.label(theme::display(palette, "pick a material", 15.0));
            return;
        };
        let material = &mut self.materials[index];
        ui.label(theme::display(palette, "parameters", 15.0));
        theme::eyebrow(ui, palette, format!("{}  ·  {}", material.class, material.path));
        ui.add_space(4.0);

        if material.source == Source::Expression {
            let mut rgb = [0.0f32; 3];
            for (channel, slot) in ["R", "G", "B"].iter().zip(rgb.iter_mut()) {
                if let Some(parameter) = material.params.iter().find(|p| p.name == *channel) {
                    *slot = parameter.value.trim().parse().unwrap_or(0.0);
                }
            }
            ui.horizontal(|ui| {
                ui.label("color");
                if ui.color_edit_button_rgb(&mut rgb).changed() {
                    for (channel, value) in ["R", "G", "B"].iter().zip(rgb.iter()) {
                        if let Some(parameter) =
                            material.params.iter_mut().find(|p| p.name == *channel)
                        {
                            parameter.value = value.to_string();
                        }
                    }
                }
            });
            ui.add_space(2.0);
        }

        egui::ScrollArea::vertical()
            .id_salt("material-params")
            .max_height(480.0)
            .show(ui, |ui| {
                for parameter in &mut material.params {
                    ui.horizontal(|ui| {
                        let tint = if parameter.changed() {
                            theme::colors(palette).accent_high
                        } else {
                            theme::colors(palette).dim
                        };
                        ui.label(egui::RichText::new(&parameter.name).color(tint));
                        match parameter.kind {
                            ParamKind::Colour => {
                                let mut rgba = egui::Rgba::from_rgba_premultiplied(
                                    parameter.rgba[0],
                                    parameter.rgba[1],
                                    parameter.rgba[2],
                                    parameter.rgba[3],
                                );
                                if egui::color_picker::color_edit_button_rgba(
                                    ui,
                                    &mut rgba,
                                    egui::color_picker::Alpha::OnlyBlend,
                                )
                                .changed()
                                {
                                    parameter.rgba = [rgba[0], rgba[1], rgba[2], rgba[3]];
                                    parameter.value = format_rgba(parameter.rgba);
                                }
                                if ui
                                    .add(egui::TextEdit::singleline(&mut parameter.value).desired_width(200.0))
                                    .changed()
                                {
                                    parameter.rgba = parse_rgba(&parameter.value);
                                }
                            }
                            _ => {
                                ui.add(
                                    egui::TextEdit::singleline(&mut parameter.value)
                                        .desired_width(240.0),
                                );
                            }
                        }
                        if parameter.changed() && ui.small_button("↺").clicked() {
                            parameter.value = parameter.original.clone();
                            if parameter.kind == ParamKind::Colour {
                                parameter.rgba = parse_rgba(&parameter.value);
                            }
                        }
                    });
                }
            });
    }

    fn apply_edits(source: &Path, edits: &[(Source, String, Vec<(String, String)>)]) -> Result<Vec<u8>, String> {
        let mut bytes = std::fs::read(source).map_err(|error| error.to_string())?;
        for (kind, path, set) in edits {
            let replaced = match kind {
                Source::Instance => set_parameters(&bytes, path, set),
                Source::Expression => set_floats(&bytes, path, set),
            }
            .map_err(|error| error.to_string())?;
            bytes = replaced.bytes;
        }
        Ok(bytes)
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

    fn spawn_write(&mut self, source: PathBuf, target: PathBuf, backup: bool, status: &mut String) {
        let edits = self.pending();
        *status = "writing…".into();
        self.job = Some(Job::spawn("write materials", move |_| {
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
            Ok(format!("wrote {} ({} material(s))", target.display(), touched))
        }));
    }
}
