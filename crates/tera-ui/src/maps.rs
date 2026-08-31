use crate::jobs::{Job, Message, Progress};
use crate::theme::{self, Palette};
use crate::Paths;
use eframe::egui;
use memmap2::Mmap;
use std::collections::HashMap;
use std::path::Path;
use tera_index::Index;
use tera_package::{
    mesh_materials_or_diffuse, parse_level, parse_static_mesh, rotator_to_quaternion, write_map_glb,
    MapInstance, Mesh, MaterialInput, Package,
};

#[derive(Default)]
pub struct MapsTab {
    levels: Vec<(String, usize)>,
    loaded: bool,
    filter: String,
    selected: Option<usize>,
    job: Option<Job>,
    list_rx: Option<std::sync::mpsc::Receiver<Vec<(String, usize)>>>,
}

fn build_level_list(index_path: &Path) -> Vec<(String, usize)> {
    let Ok(index) = Index::open(index_path) else {
        return Vec::new();
    };
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for class in ["StaticMeshActor", "InterpActor", "DynamicSMActor"] {
        for hit in index.search_objects("", 2_000_000, Some(class)) {
            let object = index.object(hit as usize);
            let entry = index.package(object.package as usize);
            *counts.entry(entry.file).or_default() += 1;
        }
    }
    let mut zones: HashMap<String, (String, usize)> = HashMap::new();
    for (file, count) in counts {
        let path = index.file_name(file as usize).to_string();
        let stem = Path::new(&path)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(&path);
        let directory = path.rsplit_once(['/', '\\']).map(|(head, _)| head).unwrap_or("");
        let key = format!("{directory}/{}", tera_package::zone_base(stem));
        let entry = zones.entry(key).or_insert_with(|| (path.clone(), 0));
        entry.1 += count;
        if path.len() < entry.0.len() {
            entry.0 = path.clone();
        }
    }
    let mut levels: Vec<(String, usize)> = zones.into_values().collect();
    levels.sort_by(|a, b| b.1.cmp(&a.1));
    levels
}

impl MapsTab {
    fn ensure_list(&mut self, ui: &egui::Ui, paths: &Paths) {
        if self.loaded {
            return;
        }
        if let Some(receiver) = &self.list_rx {
            if let Ok(levels) = receiver.try_recv() {
                self.levels = levels;
                self.loaded = true;
                self.list_rx = None;
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
            }
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        self.list_rx = Some(receiver);
        let index_path = paths.index.clone();
        std::thread::spawn(move || {
            let _ = sender.send(build_level_list(&index_path));
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, palette: Palette, paths: &Paths, status: &mut String) {
        self.ensure_list(ui, paths);

        if let Some(job) = &mut self.job {
            if let Some(outcome) = job.poll() {
                *status = match outcome {
                    Ok(message) => message,
                    Err(error) => format!("error: {error}"),
                };
                self.job = None;
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
            }
        }

        egui::Panel::top("maps_head").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(theme::display(palette, "niveaux", 15.0));
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("filtrer").desired_width(240.0));
                theme::eyebrow(ui, palette, format!("{} packages", self.levels.len()));
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("maps_foot").show(ui, |ui| {
            ui.add_space(4.0);
            if let Some(job) = &self.job {
                ui.horizontal(|ui| {
                    ui.spinner();
                    theme::eyebrow(ui, palette, &job.label);
                });
            } else if let Some(selected) = self.selected {
                if let Some((file, count)) = self.levels.get(selected).cloned() {
                    if ui.button(format!("export glTF — {} ({} placements)", leaf(&file), count)).clicked() {
                        if let Some(target) = rfd::FileDialog::new()
                            .set_file_name(format!("{}.glb", leaf(&file)))
                            .save_file()
                        {
                            let level_file = paths.cooked().join(&file);
                            let index_path = paths.index.clone();
                            let cooked = paths.cooked();
                            self.job = Some(Job::spawn(format!("export {}", leaf(&file)), move |sender| {
                                export_map(&level_file, &index_path, &cooked, &target, sender)
                            }));
                        }
                    }
                }
            } else {
                theme::eyebrow(ui, palette, "choisis un niveau à gauche");
            }
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let filter = self.filter.to_ascii_lowercase();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (index, (file, count)) in self.levels.iter().enumerate() {
                    if !filter.is_empty() && !file.to_ascii_lowercase().contains(&filter) {
                        continue;
                    }
                    let label = format!("{}   ·   {} placements", leaf(file), count);
                    if ui.selectable_label(self.selected == Some(index), label).clicked() {
                        self.selected = Some(index);
                    }
                }
            });
        });
    }
}

fn leaf(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path).trim_end_matches(".gpk")
}

fn export_map(
    level_file: &Path,
    index_path: &Path,
    cooked: &Path,
    target: &Path,
    sender: &std::sync::mpsc::Sender<Message>,
) -> Result<String, String> {
    let siblings = tera_package::zone_siblings(level_file);
    let mut maps = Vec::new();
    for sibling in &siblings {
        if let Ok(handle) = std::fs::File::open(sibling) {
            if let Ok(mapped) = unsafe { Mmap::map(&handle) } {
                maps.push(mapped);
            }
        }
    }
    let mut placements = Vec::new();
    for mapped in &maps {
        for package in tera_package::Bundle::new(mapped).flatten() {
            placements.extend(parse_level(&package));
        }
    }
    if placements.is_empty() {
        return Err("aucun placement dans cette zone".into());
    }
    let mut unique: Vec<String> = placements.iter().map(|p| p.mesh.clone()).collect();
    unique.sort();
    unique.dedup();

    let index = Index::open(index_path).map_err(|error| error.to_string())?;
    let mut meshes: Vec<(Mesh, Vec<MaterialInput>)> = Vec::new();
    let mut mesh_index: HashMap<String, usize> = HashMap::new();
    let total = unique.len();
    for (done, name) in unique.iter().enumerate() {
        let _ = sender.send(Message::Progress(Progress {
            label: format!("mesh {name}"),
            done,
            total,
        }));
        let Some(hit) = index
            .find_object_exact(name, Some("StaticMesh"))
            .or_else(|| index.search_objects(name, 1, Some("StaticMesh")).first().copied())
        else {
            continue;
        };
        let object = index.object(hit as usize);
        let entry = index.package(object.package as usize);
        let file = cooked.join(index.file_name(entry.file as usize));
        let Ok(mesh_handle) = std::fs::File::open(&file) else {
            continue;
        };
        let Ok(mesh_map) = (unsafe { Mmap::map(&mesh_handle) }) else {
            continue;
        };
        let Ok(package) = Package::parse(&mesh_map, entry.offset as usize) else {
            continue;
        };
        let Some(export) = package.exports.get(object.export as usize) else {
            continue;
        };
        let Some(mesh) = parse_static_mesh(&package, export) else {
            continue;
        };
        let materials = mesh_materials_or_diffuse(&package, &mesh, name, cooked);
        mesh_index.insert(name.clone(), meshes.len());
        meshes.push((mesh, materials));
    }

    let instances: Vec<MapInstance> = placements
        .iter()
        .filter_map(|placement| {
            let &mesh = mesh_index.get(&placement.mesh)?;
            Some(MapInstance {
                mesh,
                translation: placement.location,
                rotation: rotator_to_quaternion(placement.rotation),
                scale: placement.scale,
            })
        })
        .collect();

    let name = leaf(level_file.to_str().unwrap_or("level"));
    let glb = write_map_glb(&meshes, &instances, name);
    std::fs::write(target, &glb).map_err(|error| error.to_string())?;
    Ok(format!(
        "wrote {} — {} meshes, {} placements",
        target.display(),
        meshes.len(),
        instances.len()
    ))
}
