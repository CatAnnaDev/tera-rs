use crate::jobs::{Job, Message, Progress};
use crate::theme::{self, Palette};
use crate::view3d::{Camera, Raster, Scene, Texture as RasterTexture, Triangle};
use crate::Paths;
use eframe::egui;
use std::sync::Arc;
use memmap2::Mmap;
use std::collections::HashMap;
use std::path::Path;
use tera_index::Index;
use tera_package::{
    mesh_materials_or_diffuse, parse_level, parse_static_mesh, rotator_to_quaternion, write_map_glb,
    MapInstance, Mesh, MaterialInput, Package,
};

struct MapPreviewData {
    scene: Scene,
    data: Arc<MapData>,
    label: String,
}

struct MapPreview {
    scene: Scene,
    data: Arc<MapData>,
    camera: Camera,
    handle: Option<egui::TextureHandle>,
    dirty: bool,
    textured: bool,
    label: String,
}

#[derive(Default)]
pub struct MapsTab {
    levels: Vec<(String, usize)>,
    loaded: bool,
    filter: String,
    selected: Option<usize>,
    job: Option<Job>,
    list_rx: Option<std::sync::mpsc::Receiver<Vec<(String, usize)>>>,
    preview: Option<MapPreview>,
    preview_rx: Option<std::sync::mpsc::Receiver<Result<MapPreviewData, String>>>,
    preview_pending: bool,
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

        if let Some(receiver) = &self.preview_rx {
            match receiver.try_recv() {
                Ok(Ok(data)) => {
                    *status = data.label.clone();
                    self.preview = Some(MapPreview {
                        scene: data.scene,
                        data: data.data,
                        camera: Camera::default(),
                        handle: None,
                        dirty: true,
                        textured: false,
                        label: data.label,
                    });
                    self.preview_rx = None;
                    self.preview_pending = false;
                }
                Ok(Err(error)) => {
                    *status = format!("error: {error}");
                    self.preview_rx = None;
                    self.preview_pending = false;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.preview_rx = None;
                    self.preview_pending = false;
                }
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
            } else if self.preview_pending {
                ui.horizontal(|ui| {
                    ui.spinner();
                    theme::eyebrow(ui, palette, "construction de la preview…");
                });
            } else if let Some(selected) = self.selected {
                if let Some((file, count)) = self.levels.get(selected).cloned() {
                    ui.horizontal(|ui| {
                        if ui.button(format!("prévisualiser ({count} placements)")).clicked() {
                            let level_file = paths.cooked().join(&file);
                            let index_path = paths.index.clone();
                            let cooked = paths.cooked();
                            let label = leaf(&file).to_string();
                            let (sender, receiver) = std::sync::mpsc::channel();
                            self.preview_rx = Some(receiver);
                            self.preview_pending = true;
                            std::thread::spawn(move || {
                                let (progress, _progress_rx) = std::sync::mpsc::channel();
                                let result = collect_map_data(&level_file, &index_path, &cooked, &progress)
                                    .map(|data| {
                                        let (scene, shown, total) =
                                            build_map_scene(&data.meshes, &data.instances, false);
                                        let label = if shown < total {
                                            format!(
                                                "{label} — {} placements, {shown}/{total} triangles",
                                                data.instances.len()
                                            )
                                        } else {
                                            format!(
                                                "{label} — {} placements, {total} triangles",
                                                data.instances.len()
                                            )
                                        };
                                        MapPreviewData {
                                            scene,
                                            data: Arc::new(data),
                                            label,
                                        }
                                    });
                                let _ = sender.send(result);
                            });
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(120));
                        }
                        if ui.button("export glTF").clicked() {
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
                    });
                }
            } else {
                theme::eyebrow(ui, palette, "choisis un niveau à gauche");
            }
            ui.add_space(4.0);
        });

        let mut close_preview = false;
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(preview) = &mut self.preview {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("← fermer la preview").clicked() {
                        close_preview = true;
                    }
                    if ui.checkbox(&mut preview.textured, "textures").changed() {
                        let (scene, _, _) = build_map_scene(
                            &preview.data.meshes,
                            &preview.data.instances,
                            preview.textured,
                        );
                        preview.scene = scene;
                        preview.dirty = true;
                    }
                    theme::eyebrow(ui, palette, &preview.label);
                });
                ui.add_space(4.0);
                let size = egui::vec2(ui.available_width(), ui.available_height().max(220.0));
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                if response.dragged() {
                    let delta = response.drag_delta();
                    preview.camera.yaw += delta.x * 0.01;
                    preview.camera.pitch = (preview.camera.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
                    preview.dirty = true;
                }
                let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                if response.hovered() && scroll.abs() > 0.1 {
                    preview.camera.distance =
                        (preview.camera.distance * (1.0 - scroll * 0.002)).clamp(1.0, 1.0e6);
                    preview.dirty = true;
                }
                if preview.handle.is_none() || preview.dirty {
                    let width = size.x as usize;
                    let height = size.y as usize;
                    let mut raster = Raster::new(width.max(16), height.max(16));
                    let accent = theme::colors(palette).panel;
                    raster.sky_top = [accent.r(), accent.g(), accent.b()];
                    let background = theme::colors(palette).background;
                    raster.sky_bottom = [background.r(), background.g(), background.b()];
                    raster.render(&preview.scene, &preview.camera.view_projection(size.x / size.y));
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [raster.width, raster.height],
                        &raster.color,
                    );
                    preview.handle = Some(ui.ctx().load_texture(
                        "map_preview",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                    preview.dirty = false;
                }
                if let Some(handle) = &preview.handle {
                    ui.painter().image(
                        handle.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                return;
            }
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
        if close_preview {
            self.preview = None;
        }
    }
}

fn leaf(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path).trim_end_matches(".gpk")
}

const MAP_PREVIEW_TRI_BUDGET: usize = 1_200_000;

fn quat_rotate(quaternion: [f32; 4], vertex: [f32; 3]) -> [f32; 3] {
    let (x, y, z, w) = (quaternion[0], quaternion[1], quaternion[2], quaternion[3]);
    let tx = 2.0 * (y * vertex[2] - z * vertex[1]);
    let ty = 2.0 * (z * vertex[0] - x * vertex[2]);
    let tz = 2.0 * (x * vertex[1] - y * vertex[0]);
    [
        vertex[0] + w * tx + (y * tz - z * ty),
        vertex[1] + w * ty + (z * tx - x * tz),
        vertex[2] + w * tz + (x * ty - y * tx),
    ]
}

fn sane_vertex(vertex: [f32; 3]) -> bool {
    vertex.iter().all(|value| value.is_finite() && value.abs() < 1.0e5)
}

fn instance_vertex(instance: &MapInstance, vertex: [f32; 3]) -> [f32; 3] {
    let scaled = [
        vertex[0] * instance.scale[0],
        vertex[1] * instance.scale[1],
        vertex[2] * instance.scale[2],
    ];
    let rotated = quat_rotate(instance.rotation, scaled);
    [
        rotated[0] + instance.translation[0],
        rotated[1] + instance.translation[1],
        rotated[2] + instance.translation[2],
    ]
}

fn downsample_texture(image: &tera_package::png::Image, max: u32) -> RasterTexture {
    let (width, height) = (image.width.max(1), image.height.max(1));
    if width <= max && height <= max {
        return RasterTexture {
            width: width as usize,
            height: height as usize,
            rgba: image.rgba.clone(),
        };
    }
    let step = (width.max(height) as f32 / max as f32).ceil() as u32;
    let new_width = (width / step).max(1);
    let new_height = (height / step).max(1);
    let mut rgba = vec![0u8; (new_width * new_height * 4) as usize];
    for y in 0..new_height {
        let source_y = (y * step).min(height - 1);
        for x in 0..new_width {
            let source_x = (x * step).min(width - 1);
            let source = ((source_y * width + source_x) * 4) as usize;
            let target = ((y * new_width + x) * 4) as usize;
            rgba[target..target + 4].copy_from_slice(&image.rgba[source..source + 4]);
        }
    }
    RasterTexture {
        width: new_width as usize,
        height: new_height as usize,
        rgba,
    }
}

fn build_map_scene(
    meshes: &[(Mesh, Vec<MaterialInput>)],
    instances: &[MapInstance],
    textured: bool,
) -> (Scene, usize, usize) {
    let mut scene = Scene::default();
    let mut mesh_texture: Vec<i32> = Vec::with_capacity(meshes.len());
    let mut mesh_bounds: Vec<([f32; 3], [f32; 3])> = Vec::with_capacity(meshes.len());
    for (mesh, materials) in meshes {
        let index = if textured {
            materials
                .iter()
                .find_map(|material| material.diffuse.as_ref())
                .and_then(|(_, _, png)| tera_package::png::decode(png).ok())
                .map(|image| {
                    scene.textures.push(downsample_texture(&image, 128));
                    (scene.textures.len() - 1) as i32
                })
                .unwrap_or(-1)
        } else {
            -1
        };
        mesh_texture.push(index);
        let mut low = [f32::MAX; 3];
        let mut high = [f32::MIN; 3];
        for vertex in mesh.vertices.iter().filter(|vertex| sane_vertex(**vertex)) {
            for axis in 0..3 {
                low[axis] = low[axis].min(vertex[axis]);
                high[axis] = high[axis].max(vertex[axis]);
            }
        }
        if low[0] > high[0] {
            low = [0.0; 3];
            high = [0.0; 3];
        }
        mesh_bounds.push((low, high));
    }

    let mut low = [f32::MAX; 3];
    let mut high = [f32::MIN; 3];
    for instance in instances {
        let Some((bound_low, bound_high)) = mesh_bounds.get(instance.mesh) else {
            continue;
        };
        for corner in 0..8 {
            let local = [
                if corner & 1 == 0 { bound_low[0] } else { bound_high[0] },
                if corner & 2 == 0 { bound_low[1] } else { bound_high[1] },
                if corner & 4 == 0 { bound_low[2] } else { bound_high[2] },
            ];
            let world = instance_vertex(instance, local);
            for axis in 0..3 {
                low[axis] = low[axis].min(world[axis]);
                high[axis] = high[axis].max(world[axis]);
            }
        }
    }
    if low[0] > high[0] {
        return (scene, 0, 0);
    }
    let mut center = [0.0f32; 3];
    let mut placed = 0.0f32;
    for instance in instances {
        if mesh_bounds.get(instance.mesh).is_some() {
            for axis in 0..3 {
                center[axis] += instance.translation[axis];
            }
            placed += 1.0;
        }
    }
    if placed > 0.0 {
        for axis in 0..3 {
            center[axis] /= placed;
        }
    } else {
        for axis in 0..3 {
            center[axis] = (low[axis] + high[axis]) * 0.5;
        }
    }
    let footprint = (high[0] - low[0]).max(high[1] - low[1]).max(1.0);
    let view_scale = 300.0 / footprint;
    let grid_half = footprint * view_scale * 0.5 * 1.15;
    let grid_step = (grid_half / 8.0).max(1.0);
    let convert = |world: [f32; 3]| -> [f32; 3] {
        [
            (world[0] - center[0]) * view_scale,
            (world[2] - center[2]) * view_scale,
            (world[1] - center[1]) * view_scale,
        ]
    };

    let mut total = 0usize;
    let mut shown = 0usize;
    let mut order: Vec<usize> = (0..instances.len()).collect();
    order.sort_by_key(|&index| {
        let count = meshes
            .get(instances[index].mesh)
            .map(|(mesh, _)| mesh.indices.len())
            .unwrap_or(0);
        std::cmp::Reverse(count)
    });
    for &index in &order {
        let instance = &instances[index];
        let Some((mesh, _)) = meshes.get(instance.mesh) else {
            continue;
        };
        let triangles = mesh.indices.len() / 3;
        total += triangles;
        if shown + triangles > MAP_PREVIEW_TRI_BUDGET {
            continue;
        }
        let texture = mesh_texture.get(instance.mesh).copied().unwrap_or(-1);
        let has_uv = texture >= 0 && mesh.uvs.len() == mesh.vertices.len();
        for face in mesh.indices.as_chunks::<3>().0 {
            let corners = [
                mesh.vertices[face[0] as usize],
                mesh.vertices[face[1] as usize],
                mesh.vertices[face[2] as usize],
            ];
            if !corners.iter().all(|corner| sane_vertex(*corner)) {
                continue;
            }
            let points = [
                convert(instance_vertex(instance, corners[0])),
                convert(instance_vertex(instance, corners[1])),
                convert(instance_vertex(instance, corners[2])),
            ];
            let uv = if has_uv {
                [
                    mesh.uvs[face[0] as usize],
                    mesh.uvs[face[1] as usize],
                    mesh.uvs[face[2] as usize],
                ]
            } else {
                [[0.0, 0.0]; 3]
            };
            scene.triangles.push(Triangle {
                points,
                uv,
                texture,
                color: [190, 178, 198],
                light: [1.0; 3],
            });
        }
        shown += triangles;
    }
    scene.add_grid(grid_half, grid_step, [58, 52, 64]);
    scene.shade();
    (scene, shown, total)
}

struct MapData {
    meshes: Vec<(Mesh, Vec<MaterialInput>)>,
    instances: Vec<MapInstance>,
    name: String,
}

fn export_map(
    level_file: &Path,
    index_path: &Path,
    cooked: &Path,
    target: &Path,
    sender: &std::sync::mpsc::Sender<Message>,
) -> Result<String, String> {
    let data = collect_map_data(level_file, index_path, cooked, sender)?;
    let glb = write_map_glb(&data.meshes, &data.instances, &data.name);
    std::fs::write(target, &glb).map_err(|error| error.to_string())?;
    Ok(format!(
        "wrote {} — {} meshes, {} placements",
        target.display(),
        data.meshes.len(),
        data.instances.len()
    ))
}

fn collect_map_data(
    level_file: &Path,
    index_path: &Path,
    cooked: &Path,
    sender: &std::sync::mpsc::Sender<Message>,
) -> Result<MapData, String> {
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
    let placements = tera_package::dedup_placements(placements);
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

    let name = leaf(level_file.to_str().unwrap_or("level")).to_string();
    Ok(MapData {
        meshes,
        instances,
        name,
    })
}


