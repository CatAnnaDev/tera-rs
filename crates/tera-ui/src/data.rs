use crate::jobs::Job;
use crate::theme::{self, Palette};
use crate::Paths;
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tera_datacenter::{Address, DataCenter, Node, RefIndex};

#[derive(Clone)]
pub struct Edit {
    pub select: String,
    pub attribute: String,
    pub value: String,
}

struct NavState {
    query: String,
    results: Vec<Row>,
    selected: Option<usize>,
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
    refs: Option<Arc<RefIndex>>,
    history: Vec<NavState>,
}

#[derive(Clone)]
pub struct Row {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub address: Address,
    pub id: Option<i32>,
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
                self.refs = Some(Arc::new(RefIndex::build(&center)));
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
        self.history.clear();
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
                    id: node.get("id").and_then(|value| value.as_i32()),
                });
            }
        }
        self.query_ms = started.elapsed().as_secs_f64() * 1000.0;
    }

    fn row_from_address(center: &DataCenter, address: Address) -> Option<Row> {
        let node = Node::new(center, address).ok()?;
        let mut attributes = Vec::new();
        for attribute in node.attributes() {
            if let (Ok(name), Ok(value)) = (attribute.name(), attribute.value()) {
                attributes.push((name.to_string(), value.to_text().to_string()));
            }
        }
        Some(Row {
            name: node.name().unwrap_or("?").to_string(),
            attributes,
            address,
            id: node.get("id").and_then(|value| value.as_i32()),
        })
    }

    fn navigate_to(&mut self, address: Address) {
        let Some(center) = self.center.clone() else {
            return;
        };
        let Some(row) = Self::row_from_address(&center, address) else {
            return;
        };
        self.history.push(NavState {
            query: self.query.clone(),
            results: std::mem::take(&mut self.results),
            selected: self.selected.take(),
        });
        self.results = vec![row];
        self.selected = Some(0);
    }

    fn back(&mut self) {
        if let Some(state) = self.history.pop() {
            self.query = state.query;
            self.results = state.results;
            self.selected = state.selected;
        }
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

        let refs = self.refs.clone();
        let center = self.center.clone();
        let mut nav_target: Option<Address> = None;
        let mut go_back = false;
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
                    if !self.history.is_empty() && ui.button("← retour").clicked() {
                        go_back = true;
                    }
                    if let (Some(refs), Ok(node)) =
                        (refs.as_ref(), Node::new(center, row.address))
                    {
                        let outbound = refs.outbound(&node);
                        if !outbound.is_empty() {
                            theme::eyebrow(ui, palette, "references sortantes");
                            for reference in &outbound {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(format!("{} = {}", reference.attribute, reference.value));
                                    if reference.targets.is_empty() {
                                        ui.weak("(pas de cible)");
                                    }
                                    for target in &reference.targets {
                                        if ui
                                            .small_button(format!(
                                                "→ {}/{}",
                                                target.sheet, target.node_name
                                            ))
                                            .clicked()
                                        {
                                            nav_target = Some(target.address);
                                        }
                                    }
                                });
                            }
                        }
                        if let Some(id) = row.id {
                            let incoming = refs.incoming(id);
                            if !incoming.is_empty() {
                                theme::rule(ui, palette);
                                theme::eyebrow(
                                    ui,
                                    palette,
                                    format!("référencé par {} (id {})", incoming.len(), id),
                                );
                                egui::ScrollArea::vertical()
                                    .id_salt("dc_backlinks")
                                    .max_height(160.0)
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        for backlink in incoming.iter().take(1000) {
                                            if ui
                                                .small_button(format!(
                                                    "{}/{}  .{}",
                                                    backlink.sheet,
                                                    backlink.node_name,
                                                    backlink.attribute
                                                ))
                                                .clicked()
                                            {
                                                nav_target = Some(backlink.address);
                                            }
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
                        .hint_text("attribute")
                        .desired_width(160.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.edit_value)
                        .hint_text("new value")
                        .desired_width(260.0),
                );
                if ui.button("queue this edit").clicked()
                    && !self.edit_attribute.is_empty()
                {
                    self.edits.push(Edit {
                        select: self.query.clone(),
                        attribute: self.edit_attribute.clone(),
                        value: self.edit_value.clone(),
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
                    format!("{}  ·  {} = {}", edit.select, edit.attribute, edit.value),
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
        if go_back {
            self.back();
        }
        if let Some(address) = nav_target {
            self.navigate_to(address);
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
                    builder
                        .set_attribute(id, &edit.attribute, &edit.value)
                        .map_err(|error| error.to_string())?;
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
