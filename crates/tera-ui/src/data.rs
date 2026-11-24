use crate::jobs::Job;
use crate::theme::{self, Palette};
use crate::Paths;
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tera_datacenter::{Address, DataCenter, Node};

#[derive(Clone)]
pub enum Op {
    Set { attribute: String, value: String },
    RemoveAttribute { attribute: String },
    InsertChild { name: String },
    Clone,
    RemoveNode,
}

impl Op {
    fn describe(&self) -> String {
        match self {
            Op::Set { attribute, value } => format!("{attribute} = {value}"),
            Op::RemoveAttribute { attribute } => format!("remove attribute {attribute}"),
            Op::InsertChild { name } => format!("add child <{name}>"),
            Op::Clone => "clone node".into(),
            Op::RemoveNode => "delete node".into(),
        }
    }
}

#[derive(Clone)]
pub struct Edit {
    pub select: String,
    pub op: Op,
}

#[derive(Default)]
pub struct DataTab {
    pending_open: Option<PathBuf>,
    pending_query: Option<String>,
    file: Option<PathBuf>,
    center: Option<Arc<DataCenter>>,
    sheets: Vec<(String, u64, u64)>,
    query: String,
    results: Vec<Row>,
    selected: Option<usize>,
    edits: Vec<Edit>,
    edit_attribute: String,
    edit_value: String,
    job: Option<Job>,
    query_ms: f64,
    open_error: Option<String>,
    pending_asset: Option<String>,
}

#[derive(Clone)]
pub struct Row {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub address: Address,
}

impl DataTab {
    fn open(&mut self, path: PathBuf, status: &mut String) {
        let started = Instant::now();
        match DataCenter::open(&path) {
            Ok(center) => {
                let mut sheets: Vec<(String, u64, u64)> = Vec::new();
                if let Ok(root) = center.root() {
                    let mut counts: std::collections::BTreeMap<String, (u64, u64)> =
                        std::collections::BTreeMap::new();
                    for child in root.children() {
                        if let Ok(name) = child.name() {
                            let entry = counts.entry(name.to_string()).or_insert((0, 0));
                            entry.0 += 1;
                            entry.1 += u64::from(child.child_count());
                        }
                    }
                    sheets = counts
                        .into_iter()
                        .map(|(name, (nodes, records))| (name, nodes, records))
                        .collect();
                }
                *status = format!(
                    "{} loaded in {:.2}s · {} nodes · {} attributes",
                    path.display(),
                    started.elapsed().as_secs_f32(),
                    center.node_count(),
                    center.attribute_count()
                );
                self.sheets = sheets;
                let center = Arc::new(center);
                self.center = Some(center);
                self.file = Some(path);
                self.open_error = None;
            }
            Err(error) => {
                self.open_error = Some(error.to_string());
                *status = format!("error: {error}");
            }
        }
    }

    fn run_query(&mut self) {
        let Some(center) = &self.center else {
            return;
        };
        let started = Instant::now();
        self.results.clear();
        self.selected = None;
        let Ok(root) = center.root() else { return };
        if let Ok(nodes) = tera_datacenter::query(root, &self.query) {
            for node in nodes.iter().take(500) {
                let mut attributes = Vec::new();
                for attribute in node.attributes() {
                    if let (Ok(name), Ok(value)) = (attribute.name(), attribute.value()) {
                        attributes.push((name.to_string(), value.to_text().to_string()));
                    }
                }
                self.results.push(Row {
                    name: node.name().unwrap_or("?").to_string(),
                    attributes,
                    address: node.address(),
                });
            }
        }
        self.query_ms = started.elapsed().as_secs_f64() * 1000.0;
    }

    pub fn take_asset_request(&mut self) -> Option<String> {
        self.pending_asset.take()
    }

    pub fn request(&mut self, path: Option<PathBuf>, query: Option<String>) {
        self.pending_open = path;
        self.pending_query = query;
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        palette: Palette,
        paths: &Paths,
        status: &mut String,
    ) {
        if let Some(path) = self.pending_open.take() {
            self.open(path, status);
            if let Some(query) = self.pending_query.take() {
                self.query = query;
                self.run_query();
            }
        }
        if let Some(job) = &mut self.job {
            if let Some(outcome) = job.poll() {
                *status = match outcome {
                    Ok(message) => message,
                    Err(error) => format!("error: {error}"),
                };
                self.job = None;
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("open a DataCenter…").clicked() {
                let start = paths.datacenter.clone();
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("datacenter", &["dat"])
                    .set_directory(start.parent().unwrap_or(std::path::Path::new(".")))
                    .pick_file()
                {
                    self.open(path, status);
                }
            }
            if self.center.is_none() && paths.datacenter.exists() && ui.button("open EUR").clicked()
            {
                self.open(paths.datacenter.clone(), status);
            }
            if let Some(center) = &self.center {
                theme::eyebrow(
                    ui,
                    palette,
                    format!(
                        "revision {} · {} sheets · key {}",
                        center.header.revision,
                        self.sheets.len(),
                        center
                            .keyiv
                            .map(|keyiv| keyiv.key_hex())
                            .unwrap_or_else(|| "?".into())
                    ),
                );
            }
        });
        if let Some(error) = &self.open_error {
            ui.colored_label(theme::colors(palette).accent_high, error);
        }
        if self.center.is_none() {
            return;
        }
        theme::rule(ui, palette);

        egui::Panel::left("sheets")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| {
                theme::eyebrow(ui, palette, "sheets");
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let sheets = self.sheets.clone();
                        for (name, nodes, records) in &sheets {
                            let label = format!("{name}  ({records})");
                            if ui.selectable_label(false, label).clicked() {
                                self.query = format!("/{name}/*");
                                self.run_query();
                            }
                            let _ = nodes;
                        }
                    });
            });

        let center = self.center.clone();
        let mut asset_open: Option<String> = None;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("/ItemData/Item[@id=\"1\"]")
                        .desired_width(460.0),
                );
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    self.run_query();
                }
                if ui.button("run").clicked() {
                    self.run_query();
                }
                theme::eyebrow(
                    ui,
                    palette,
                    format!("{} results · {:.1} ms", self.results.len(), self.query_ms),
                );
            });
            theme::rule(ui, palette);
            egui::ScrollArea::vertical()
                .id_salt("dc_results")
                .max_height(240.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (position, row) in self.results.iter().enumerate() {
                        let summary = row
                            .attributes
                            .iter()
                            .take(4)
                            .map(|(name, value)| format!("{name}={value}"))
                            .collect::<Vec<_>>()
                            .join("  ");
                        if ui
                            .selectable_label(
                                self.selected == Some(position),
                                format!("{}  {summary}", row.name),
                            )
                            .clicked()
                        {
                            self.selected = Some(position);
                        }
                    }
                });
            theme::rule(ui, palette);
            if let Some(index) = self.selected {
                if let Some(row) = self.results.get(index) {
                    egui::ScrollArea::vertical()
                        .id_salt("dc_attributes")
                        .max_height(220.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Grid::new("attrs").num_columns(2).striped(true).show(
                                ui,
                                |ui| {
                                    for (name, value) in &row.attributes {
                                        if ui.selectable_label(false, name).clicked() {
                                            self.edit_attribute = name.clone();
                                            self.edit_value = value.clone();
                                        }
                                        ui.label(value);
                                        ui.end_row();
                                    }
                                },
                            );
                        });
                }
            }
            theme::rule(ui, palette);
            if let (Some(index), Some(center)) = (self.selected, center.as_ref()) {
                if let Some(row) = self.results.get(index) {
                    if let Ok(node) = Node::new(center, row.address) {
                        let assets = tera_datacenter::asset_references(&node);
                        if !assets.is_empty() {
                            theme::rule(ui, palette);
                            theme::eyebrow(ui, palette, "linked assets");
                            for (name, value) in &assets {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(format!("{name} ="));
                                    if ui.small_button(format!("→ {value}")).clicked() {
                                        asset_open = Some(value.clone());
                                    }
                                });
                            }
                        }
                    }
                }
                theme::rule(ui, palette);
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.edit_attribute)
                        .hint_text("attribute / child name")
                        .desired_width(160.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.edit_value)
                        .hint_text("value")
                        .desired_width(240.0),
                );
                let select = self.query.clone();
                if ui.button("set").clicked() && !self.edit_attribute.is_empty() {
                    self.edits.push(Edit {
                        select: select.clone(),
                        op: Op::Set {
                            attribute: self.edit_attribute.clone(),
                            value: self.edit_value.clone(),
                        },
                    });
                }
                if ui.button("remove attribute").clicked() && !self.edit_attribute.is_empty() {
                    self.edits.push(Edit {
                        select: select.clone(),
                        op: Op::RemoveAttribute {
                            attribute: self.edit_attribute.clone(),
                        },
                    });
                }
                if ui.button("add child").clicked() && !self.edit_attribute.is_empty() {
                    self.edits.push(Edit {
                        select: select.clone(),
                        op: Op::InsertChild {
                            name: self.edit_attribute.clone(),
                        },
                    });
                }
                if ui.button("clone node").clicked() {
                    self.edits.push(Edit {
                        select: select.clone(),
                        op: Op::Clone,
                    });
                }
                if ui.button("delete node").clicked() {
                    self.edits.push(Edit {
                        select,
                        op: Op::RemoveNode,
                    });
                }
                if !self.edits.is_empty() && ui.button("clear").clicked() {
                    self.edits.clear();
                }
            });
            for edit in &self.edits {
                theme::eyebrow(
                    ui,
                    palette,
                    format!("{}  ·  {}", edit.select, edit.op.describe()),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("export as XML…").clicked() {
                    if let Some(directory) = rfd::FileDialog::new().pick_folder() {
                        self.export(directory, true, status);
                    }
                }
                if ui.button("export as JSON…").clicked() {
                    if let Some(directory) = rfd::FileDialog::new().pick_folder() {
                        self.export(directory, false, status);
                    }
                }
                let can_save = !self.edits.is_empty() && self.job.is_none();
                if ui
                    .add_enabled(can_save, egui::Button::new("save a patched .dat…"))
                    .clicked()
                {
                    if let Some(target) = rfd::FileDialog::new()
                        .set_file_name("DataCenter_Final_EUR.dat")
                        .save_file()
                    {
                        self.save(target, status);
                    }
                }
                if let Some(job) = &self.job {
                    ui.add(egui::Spinner::new());
                    theme::eyebrow(ui, palette, job.label.clone());
                }
            });
        });
        if let Some(name) = asset_open {
            self.pending_asset = Some(name);
        }
    }

    fn export(&mut self, directory: PathBuf, xml: bool, status: &mut String) {
        let Some(center) = self.center.clone() else {
            return;
        };
        let query = self.query.clone();
        *status = "exporting…".into();
        self.job = Some(Job::spawn("export", move |_| {
            let root = center.root().map_err(|error| error.to_string())?;
            let nodes = if query.is_empty() {
                root.children().collect::<Vec<_>>()
            } else {
                tera_datacenter::query(root, &query).map_err(|error| error.to_string())?
            };
            let mut written = 0usize;
            for (position, node) in nodes.iter().enumerate() {
                let name = node.name().unwrap_or("node");
                let extension = if xml { "xml" } else { "json" };
                let path = directory.join(format!("{name}-{:05}.{extension}", position + 1));
                let file = std::fs::File::create(&path).map_err(|error| error.to_string())?;
                let mut out = std::io::BufWriter::new(file);
                if xml {
                    tera_datacenter::export::write_xml(&mut out, node, true)
                        .map_err(|error| error.to_string())?;
                } else {
                    tera_datacenter::export::write_json(&mut out, node, true)
                        .map_err(|error| error.to_string())?;
                }
                written += 1;
                if written >= 2000 {
                    break;
                }
            }
            Ok(format!("{written} file(s) written to {}", directory.display()))
        }));
    }

    fn save(&mut self, target: PathBuf, status: &mut String) {
        let Some(file) = self.file.clone() else {
            return;
        };
        let edits = self.edits.clone();
        *status = "rebuilding the DataCenter…".into();
        self.job = Some(Job::spawn("repack", move |_| {
            let center = DataCenter::open(&file).map_err(|error| error.to_string())?;
            let keyiv = center.keyiv;
            let mut builder =
                tera_datacenter::Builder::from_datacenter(&center).map_err(|error| error.to_string())?;
            let mut touched = 0usize;
            for edit in &edits {
                let targets = tera_datacenter::query_builder(&builder, &edit.select)
                    .map_err(|error| error.to_string())?;
                for id in targets {
                    match &edit.op {
                        Op::Set { attribute, value } => {
                            builder
                                .set_attribute(id, attribute, value)
                                .map_err(|error| error.to_string())?;
                        }
                        Op::RemoveAttribute { attribute } => {
                            builder.remove_attribute(id, attribute);
                        }
                        Op::InsertChild { name } => {
                            builder.insert(id, name);
                        }
                        Op::Clone => {
                            let copy = builder.duplicate(id);
                            if let Some(parent) = builder.parent_of(id) {
                                builder.attach(parent, copy);
                                builder.adopt_key(parent, copy);
                            }
                        }
                        Op::RemoveNode => {
                            builder.remove(id);
                        }
                    }
                    touched += 1;
                }
            }
            let image = builder.pack().map_err(|error| error.to_string())?;
            let bytes = match keyiv {
                Some(keyiv) => tera_datacenter::wrap(&image, &keyiv, 6)
                    .map_err(|error| error.to_string())?,
                None => image,
            };
            std::fs::write(&target, &bytes).map_err(|error| error.to_string())?;
            Ok(format!(
                "{touched} node(s) edited · {} bytes written to {}",
                bytes.len(),
                target.display()
            ))
        }));
    }
}
