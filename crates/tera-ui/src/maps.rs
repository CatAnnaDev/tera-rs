use crate::jobs::{Job, Message, Progress};
use crate::theme::{self, Palette};
use crate::view3d::{Camera, Matrix, Raster, Scene, Texture as RasterTexture, Triangle};
use crate::Paths;
use eframe::egui;
use memmap2::Mmap;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use tera_index::Index;
use tera_package::{
    level_actors, mesh_materials_or_diffuse, parse_static_mesh, rotator_to_quaternion,
    set_actor_transform, write_map_glb, MapInstance, MaterialInput, Mesh, Package, Transform,
};

const ROTATOR_TO_DEGREES: f32 = 360.0 / 65536.0;
const MAP_PREVIEW_TRI_BUDGET: usize = 1_400_000;

#[derive(Clone, Copy)]
struct MapView {
    center: [f32; 3],
    scale: f32,
    grid_half: f32,
    grid_step: f32,
}

impl MapView {
    fn convert(&self, world: [f32; 3]) -> [f32; 3] {
        [
            (world[0] - self.center[0]) * self.scale,
            (world[2] - self.center[2]) * self.scale,
            (world[1] - self.center[1]) * self.scale,
        ]
    }
}

struct EditorActor {
    object: String,
    class: String,
    file: usize,
    mesh: Option<usize>,
    location: [f32; 3],
    rotation: [i32; 3],
    scale: [f32; 3],
    origin_location: [f32; 3],
    origin_rotation: [i32; 3],
    origin_scale: [f32; 3],
    span: Range<usize>,
}

impl EditorActor {
    fn instance(&self) -> Option<MapInstance> {
        self.mesh.map(|mesh| MapInstance {
            mesh,
            translation: self.location,
            rotation: rotator_to_quaternion(self.rotation),
            scale: self.scale,
        })
    }

    fn edited(&self) -> bool {
        self.location != self.origin_location
            || self.rotation != self.origin_rotation
            || self.scale != self.origin_scale
    }

    fn leaf(&self) -> &str {
        self.object.rsplit('.').next().unwrap_or(&self.object)
    }
}

struct EditorBuild {
    files: Vec<PathBuf>,
    meshes: Vec<(Mesh, Vec<MaterialInput>)>,
    mesh_bounds: Vec<([f32; 3], [f32; 3])>,
    mesh_texture: Vec<i32>,
    actors: Vec<EditorActor>,
    view: MapView,
    scene: Scene,
    name: String,
    label: String,
}

struct MapEditor {
    files: Vec<PathBuf>,
    meshes: Vec<(Mesh, Vec<MaterialInput>)>,
    mesh_bounds: Vec<([f32; 3], [f32; 3])>,
    mesh_texture: Vec<i32>,
    actors: Vec<EditorActor>,
    view: MapView,
    scene: Scene,
    name: String,
    label: String,
    camera: Camera,
    handle: Option<egui::TextureHandle>,
    dirty: bool,
    textured: bool,
    selected: Option<usize>,
    search: String,
}

#[derive(Default)]
pub struct MapsTab {
    levels: Vec<(String, usize)>,
    loaded: bool,
    filter: String,
    selected: Option<usize>,
    job: Option<Job>,
    list_rx: Option<std::sync::mpsc::Receiver<Vec<(String, usize)>>>,
    editor: Option<MapEditor>,
    editor_rx: Option<std::sync::mpsc::Receiver<Result<EditorBuild, String>>>,
    editor_progress_rx: Option<std::sync::mpsc::Receiver<Message>>,
    editor_progress: Option<(String, usize, usize)>,
    editor_pending: bool,
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

    fn open_editor(&mut self, ui: &egui::Ui, paths: &Paths, file: &str) {
        let roots = asset_roots(paths);
        let level_file = resolve_in(&roots, file);
        let index_path = paths.index.clone();
        let label = leaf(file).to_string();
        let (sender, receiver) = std::sync::mpsc::channel();
        let (progress, progress_rx) = std::sync::mpsc::channel();
        self.editor_rx = Some(receiver);
        self.editor_progress_rx = Some(progress_rx);
        self.editor_progress = Some(("starting…".to_string(), 0, 0));
        self.editor_pending = true;
        std::thread::spawn(move || {
            let result = collect_editor(&level_file, &index_path, &roots, &label, &progress);
            let _ = sender.send(result);
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
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

        if let Some(receiver) = &self.editor_progress_rx {
            while let Ok(message) = receiver.try_recv() {
                if let Message::Progress(progress) = message {
                    self.editor_progress = Some((progress.label, progress.done, progress.total));
                }
            }
        }

        if let Some(receiver) = &self.editor_rx {
            match receiver.try_recv() {
                Ok(Ok(build)) => {
                    *status = build.label.clone();
                    let mut camera = Camera::default();
                    camera.distance = (build.view.grid_half * 1.7).max(60.0);
                    self.editor = Some(MapEditor {
                        files: build.files,
                        meshes: build.meshes,
                        mesh_bounds: build.mesh_bounds,
                        mesh_texture: build.mesh_texture,
                        actors: build.actors,
                        view: build.view,
                        scene: build.scene,
                        name: build.name,
                        label: build.label,
                        camera,
                        handle: None,
                        dirty: true,
                        textured: false,
                        selected: None,
                        search: String::new(),
                    });
                    self.editor_rx = None;
                    self.editor_progress_rx = None;
                    self.editor_progress = None;
                    self.editor_pending = false;
                }
                Ok(Err(error)) => {
                    *status = format!("error: {error}");
                    self.editor_rx = None;
                    self.editor_progress_rx = None;
                    self.editor_progress = None;
                    self.editor_pending = false;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.editor_rx = None;
                    self.editor_progress_rx = None;
                    self.editor_progress = None;
                    self.editor_pending = false;
                }
            }
        }

        if self.editor.is_some() {
            self.editor_ui(ui, palette, paths, status);
        } else {
            self.browser_ui(ui, palette, paths, status);
        }
    }

    fn browser_ui(&mut self, ui: &mut egui::Ui, palette: Palette, paths: &Paths, _status: &mut String) {
        egui::Panel::top("maps_head").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(theme::display(palette, "levels", 15.0));
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("filter")
                        .desired_width(240.0),
                );
                theme::eyebrow(ui, palette, format!("{} zones", self.levels.len()));
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("maps_foot").show(ui, |ui| {
            ui.add_space(4.0);
            if self.editor_pending {
                let (label, done, total) = self
                    .editor_progress
                    .clone()
                    .unwrap_or_else(|| ("building editor…".to_string(), 0, 0));
                ui.horizontal(|ui| {
                    ui.spinner();
                    let bar = if total > 0 {
                        egui::ProgressBar::new(done as f32 / total as f32)
                            .desired_width(200.0)
                            .text(format!("{done}/{total}"))
                    } else {
                        egui::ProgressBar::new(0.0).desired_width(200.0).animate(true)
                    };
                    ui.add(bar);
                    theme::eyebrow(ui, palette, label);
                });
            } else if let Some(selected) = self.selected {
                if let Some((file, count)) = self.levels.get(selected).cloned() {
                    ui.horizontal(|ui| {
                        if ui.button(format!("open editor ({count} placements)")).clicked() {
                            self.open_editor(ui, paths, &file);
                        }
                        theme::eyebrow(ui, palette, leaf(&file));
                    });
                }
            } else {
                theme::eyebrow(ui, palette, "pick a level on the left");
            }
            ui.add_space(4.0);
        });

        let filter = self.filter.to_ascii_lowercase();
        egui::CentralPanel::default().show(ui, |ui| {
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

    fn editor_ui(&mut self, ui: &mut egui::Ui, palette: Palette, _paths: &Paths, status: &mut String) {
        let mut close = false;
        let mut save_edits = false;
        let mut export_gltf = false;
        let mut export_obj = false;
        let editing = self.job.is_some();

        {
            let editor = self.editor.as_ref().unwrap();
            let changed = editor.actors.iter().filter(|actor| actor.edited()).count();
            egui::Panel::top("maps_head").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("← levels").clicked() {
                        close = true;
                    }
                    ui.label(theme::display(palette, &editor.label, 15.0));
                    theme::eyebrow(
                        ui,
                        palette,
                        format!("{} actors · {} meshes", editor.actors.len(), editor.meshes.len()),
                    );
                });
                ui.add_space(4.0);
            });

            egui::Panel::bottom("maps_foot").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if editing {
                        ui.spinner();
                        if let Some(job) = &self.job {
                            theme::eyebrow(ui, palette, &job.label);
                        }
                    } else {
                        ui.add_enabled_ui(changed > 0, |ui| {
                            if ui.button(format!("save {changed} edit(s) to files")).clicked() {
                                save_edits = true;
                            }
                        });
                        if ui.button("export glTF").clicked() {
                            export_gltf = true;
                        }
                        if ui.button("export OBJ").clicked() {
                            export_obj = true;
                        }
                        if changed > 0 {
                            theme::eyebrow(ui, palette, format!("{changed} unsaved"));
                        }
                    }
                });
                ui.add_space(4.0);
            });
        }

        egui::Panel::left("maps_actors").exact_size(252.0).show(ui, |ui| {
            let editor = self.editor.as_mut().unwrap();
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::singleline(&mut editor.search)
                    .hint_text("filter actors")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(4.0);
            theme::rule(ui, palette);
            let needle = editor.search.to_ascii_lowercase();
            let rows: Vec<usize> = editor
                .actors
                .iter()
                .enumerate()
                .filter(|(_, actor)| {
                    needle.is_empty()
                        || actor.object.to_ascii_lowercase().contains(&needle)
                        || actor.class.to_ascii_lowercase().contains(&needle)
                })
                .map(|(index, _)| index)
                .collect();
            let colors = theme::colors(palette);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
                ui,
                18.0,
                rows.len(),
                |ui, range| {
                    for row in range {
                        let actor = &editor.actors[rows[row]];
                        let marker = if actor.edited() { "• " } else { "" };
                        let label = format!("{marker}{}", actor.leaf());
                        let (rect, response) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 18.0),
                            egui::Sense::click(),
                        );
                        let selected = editor.selected == Some(rows[row]);
                        if selected {
                            ui.painter().rect_filled(rect, 2.0, colors.accent.gamma_multiply(0.45));
                        } else if response.hovered() {
                            ui.painter().rect_filled(rect, 2.0, colors.raised);
                        }
                        ui.painter().text(
                            rect.left_center() + egui::vec2(6.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            label,
                            egui::FontId::monospace(11.5),
                            if actor.edited() { colors.accent_high } else { colors.bone },
                        );
                        ui.painter().text(
                            rect.right_center() - egui::vec2(6.0, 0.0),
                            egui::Align2::RIGHT_CENTER,
                            &actor.class,
                            egui::FontId::monospace(10.5),
                            theme::class_color(&actor.class),
                        );
                        if response.clicked() {
                            editor.selected = Some(rows[row]);
                        }
                        response.on_hover_text(&actor.object);
                    }
                },
            );
        });

        let mut frame_selected = false;
        egui::Panel::right("maps_inspector").exact_size(272.0).show(ui, |ui| {
            let editor = self.editor.as_mut().unwrap();
            ui.add_space(4.0);
            let Some(selected) = editor.selected else {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    theme::eyebrow(ui, palette, "pick an actor to edit its transform");
                });
                return;
            };
            let mut touched = false;
            {
                let actor = &mut editor.actors[selected];
                ui.label(theme::display(palette, actor.leaf(), 15.0));
                theme::eyebrow(ui, palette, &actor.class);
                theme::rule(ui, palette);

                theme::eyebrow(ui, palette, "location");
                ui.horizontal(|ui| {
                    for axis in 0..3 {
                        if ui
                            .add(egui::DragValue::new(&mut actor.location[axis]).speed(1.0))
                            .changed()
                        {
                            touched = true;
                        }
                    }
                });

                theme::eyebrow(ui, palette, "rotation (degrees)");
                ui.horizontal(|ui| {
                    for axis in 0..3 {
                        let mut degrees = actor.rotation[axis] as f32 * ROTATOR_TO_DEGREES;
                        if ui
                            .add(egui::DragValue::new(&mut degrees).speed(1.0).suffix("°"))
                            .changed()
                        {
                            actor.rotation[axis] = (degrees / ROTATOR_TO_DEGREES).round() as i32;
                            touched = true;
                        }
                    }
                });

                theme::eyebrow(ui, palette, "scale");
                ui.horizontal(|ui| {
                    for axis in 0..3 {
                        if ui
                            .add(egui::DragValue::new(&mut actor.scale[axis]).speed(0.01))
                            .changed()
                        {
                            touched = true;
                        }
                    }
                });

                theme::rule(ui, palette);
                ui.horizontal(|ui| {
                    if ui.button("frame").clicked() {
                        frame_selected = true;
                    }
                    if ui.add_enabled(actor.edited(), egui::Button::new("reset")).clicked() {
                        actor.location = actor.origin_location;
                        actor.rotation = actor.origin_rotation;
                        actor.scale = actor.origin_scale;
                        touched = true;
                    }
                });
            }
            if touched {
                rebuild_actor(editor, selected);
                editor.dirty = true;
            }
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let editor = self.editor.as_mut().unwrap();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut editor.textured, "textures").changed() {
                    let (scene, spans, mesh_texture) =
                        build_scene(&editor.meshes, &editor.mesh_bounds, &editor.actors, &editor.view, editor.textured);
                    for (actor, span) in editor.actors.iter_mut().zip(spans) {
                        actor.span = span;
                    }
                    editor.scene = scene;
                    editor.mesh_texture = mesh_texture;
                    editor.dirty = true;
                }
                theme::eyebrow(ui, palette, "drag orbit · scroll zoom · shift-drag pan · click to pick");
            });
            ui.add_space(4.0);
            let size = egui::vec2(ui.available_width(), ui.available_height().max(240.0));
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            let aspect = size.x / size.y.max(1.0);

            if response.dragged() {
                let delta = response.drag_delta();
                if ui.input(|input| input.modifiers.shift) {
                    let pan = editor.camera.distance * 0.0016;
                    let yaw = editor.camera.yaw;
                    editor.camera.target[0] -= (-delta.x * yaw.sin() ) * pan;
                    editor.camera.target[2] -= (delta.x * yaw.cos()) * pan;
                    editor.camera.target[1] += delta.y * pan;
                } else {
                    editor.camera.yaw += delta.x * 0.01;
                    editor.camera.pitch = (editor.camera.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
                }
                editor.dirty = true;
            }
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if response.hovered() && scroll.abs() > 0.1 {
                editor.camera.distance =
                    (editor.camera.distance * (1.0 - scroll * 0.002)).clamp(1.0, 1.0e6);
                editor.dirty = true;
            }

            if frame_selected {
                if let Some(selected) = editor.selected {
                    editor.camera.target = editor.view.convert(editor.actors[selected].location);
                    editor.dirty = true;
                }
            }

            let view_projection = editor.camera.view_projection(aspect);
            if response.clicked() {
                if let Some(pointer) = response.interact_pointer_pos() {
                    if let Some(index) = pick_actor(editor, &view_projection, rect, pointer) {
                        editor.selected = Some(index);
                    }
                }
            }

            if editor.handle.is_none() || editor.dirty {
                let mut raster = Raster::new((size.x as usize).max(16), (size.y as usize).max(16));
                let top = theme::colors(palette).panel;
                let bottom = theme::colors(palette).background;
                raster.sky_top = [top.r(), top.g(), top.b()];
                raster.sky_bottom = [bottom.r(), bottom.g(), bottom.b()];
                raster.render(&editor.scene, &view_projection);
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [raster.width, raster.height],
                    &raster.color,
                );
                editor.handle =
                    Some(ui.ctx().load_texture("map_editor", image, egui::TextureOptions::LINEAR));
                editor.dirty = false;
            }
            if let Some(handle) = &editor.handle {
                ui.painter().image(
                    handle.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }

            if let Some(selected) = editor.selected {
                if let Some(bounds) = selection_bounds(editor, selected, &view_projection, rect) {
                    let accent = theme::colors(palette).accent_high;
                    ui.painter().rect_stroke(
                        bounds,
                        2.0,
                        egui::Stroke::new(1.5, accent),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        });

        if frame_selected {
            if let Some(editor) = &mut self.editor {
                editor.dirty = true;
            }
        }

        if close {
            self.editor = None;
        }

        if save_edits {
            if let Some(editor) = &self.editor {
                let mut per_file: HashMap<usize, Vec<(String, Transform)>> = HashMap::new();
                for actor in &editor.actors {
                    if actor.edited() {
                        per_file.entry(actor.file).or_default().push((
                            actor.object.clone(),
                            Transform {
                                location: Some(actor.location),
                                rotation: Some(actor.rotation),
                                scale: Some(actor.scale),
                            },
                        ));
                    }
                }
                let files = editor.files.clone();
                self.job = Some(Job::spawn("save map edits".to_string(), move |_sender| {
                    save_map_edits(&files, per_file)
                }));
            }
            if let Some(editor) = &mut self.editor {
                for actor in &mut editor.actors {
                    actor.origin_location = actor.location;
                    actor.origin_rotation = actor.rotation;
                    actor.origin_scale = actor.scale;
                }
            }
        }

        if export_gltf || export_obj {
            if let Some(editor) = &self.editor {
                let extension = if export_obj { "obj" } else { "glb" };
                if let Some(target) = rfd::FileDialog::new()
                    .set_file_name(format!("{}.{extension}", editor.name))
                    .save_file()
                {
                    let meshes = editor.meshes.clone();
                    let instances: Vec<MapInstance> =
                        editor.actors.iter().filter_map(|actor| actor.instance()).collect();
                    let name = editor.name.clone();
                    let as_obj = export_obj;
                    self.job = Some(Job::spawn(format!("export {name}"), move |_sender| {
                        let bytes = if as_obj {
                            write_map_obj(&meshes, &instances, &name).into_bytes()
                        } else {
                            write_map_glb(&meshes, &instances, &name)
                        };
                        std::fs::write(&target, &bytes).map_err(|error| error.to_string())?;
                        Ok(format!(
                            "wrote {} — {} meshes, {} placements",
                            target.display(),
                            meshes.len(),
                            instances.len()
                        ))
                    }));
                }
            }
        }
        let _ = status;
    }
}

fn leaf(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path).trim_end_matches(".gpk")
}

fn sane_vertex(vertex: [f32; 3]) -> bool {
    vertex.iter().all(|value| value.is_finite() && value.abs() < 1.0e5)
}

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

fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
    if length > 1e-6 {
        [vector[0] / length, vector[1] / length, vector[2] / length]
    } else {
        vector
    }
}

fn triangle_light(points: [[f32; 3]; 3]) -> [f32; 3] {
    let edge1 = [
        points[1][0] - points[0][0],
        points[1][1] - points[0][1],
        points[1][2] - points[0][2],
    ];
    let edge2 = [
        points[2][0] - points[0][0],
        points[2][1] - points[0][1],
        points[2][2] - points[0][2],
    ];
    let normal = normalize([
        edge1[1] * edge2[2] - edge1[2] * edge2[1],
        edge1[2] * edge2[0] - edge1[0] * edge2[2],
        edge1[0] * edge2[1] - edge1[1] * edge2[0],
    ]);
    let key = normalize([0.35, 0.9, 0.25]);
    let hemisphere = 0.5 + 0.5 * normal[1].clamp(-1.0, 1.0);
    let direct = (normal[0] * key[0] + normal[1] * key[1] + normal[2] * key[2]).max(0.0);
    let intensity = 0.40 + 0.32 * hemisphere + 0.30 * direct;
    [intensity * 0.93, intensity * 0.97, intensity * 1.07]
}

fn push_actor_triangles(
    out: &mut Vec<Triangle>,
    mesh: &Mesh,
    instance: &MapInstance,
    view: &MapView,
    texture: i32,
) {
    let has_uv = texture >= 0 && mesh.uvs.len() == mesh.vertices.len();
    for face in mesh.indices.as_chunks::<3>().0 {
        let locals = [
            mesh.vertices[face[0] as usize],
            mesh.vertices[face[1] as usize],
            mesh.vertices[face[2] as usize],
        ];
        if !locals.iter().all(|corner| sane_vertex(*corner)) {
            continue;
        }
        let points = [
            view.convert(instance_vertex(instance, locals[0])),
            view.convert(instance_vertex(instance, locals[1])),
            view.convert(instance_vertex(instance, locals[2])),
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
        out.push(Triangle {
            points,
            uv,
            texture,
            color: [190, 178, 198],
            light: triangle_light(points),
        });
    }
}

fn mesh_bounds_of(meshes: &[(Mesh, Vec<MaterialInput>)]) -> Vec<([f32; 3], [f32; 3])> {
    meshes
        .iter()
        .map(|(mesh, _)| {
            let mut low = [f32::MAX; 3];
            let mut high = [f32::MIN; 3];
            for vertex in mesh.vertices.iter().filter(|vertex| sane_vertex(**vertex)) {
                for axis in 0..3 {
                    low[axis] = low[axis].min(vertex[axis]);
                    high[axis] = high[axis].max(vertex[axis]);
                }
            }
            if low[0] > high[0] {
                ([0.0; 3], [0.0; 3])
            } else {
                (low, high)
            }
        })
        .collect()
}

fn make_view(
    actors: &[EditorActor],
    mesh_bounds: &[([f32; 3], [f32; 3])],
) -> MapView {
    let mut low = [f32::MAX; 3];
    let mut high = [f32::MIN; 3];
    let mut center = [0.0f32; 3];
    let mut placed = 0.0f32;
    for actor in actors {
        let Some(mesh) = actor.mesh else { continue };
        let Some((bound_low, bound_high)) = mesh_bounds.get(mesh) else { continue };
        let Some(instance) = actor.instance() else { continue };
        for corner in 0..8 {
            let local = [
                if corner & 1 == 0 { bound_low[0] } else { bound_high[0] },
                if corner & 2 == 0 { bound_low[1] } else { bound_high[1] },
                if corner & 4 == 0 { bound_low[2] } else { bound_high[2] },
            ];
            let world = instance_vertex(&instance, local);
            for axis in 0..3 {
                low[axis] = low[axis].min(world[axis]);
                high[axis] = high[axis].max(world[axis]);
            }
        }
        for axis in 0..3 {
            center[axis] += actor.location[axis];
        }
        placed += 1.0;
    }
    if placed > 0.0 {
        for axis in 0..3 {
            center[axis] /= placed;
        }
    } else {
        low = [0.0; 3];
        high = [0.0; 3];
    }
    let footprint = (high[0] - low[0]).max(high[1] - low[1]).max(1.0);
    let scale = 300.0 / footprint;
    let grid_half = footprint * scale * 0.5 * 1.15;
    let grid_step = (grid_half / 8.0).max(1.0);
    MapView {
        center,
        scale,
        grid_half,
        grid_step,
    }
}

fn build_scene(
    meshes: &[(Mesh, Vec<MaterialInput>)],
    mesh_bounds: &[([f32; 3], [f32; 3])],
    actors: &[EditorActor],
    view: &MapView,
    textured: bool,
) -> (Scene, Vec<Range<usize>>, Vec<i32>) {
    let mut scene = Scene::default();
    let mut mesh_texture = vec![-1i32; meshes.len()];
    if textured {
        for (index, (_, materials)) in meshes.iter().enumerate() {
            if let Some(image) = materials
                .iter()
                .find_map(|material| material.diffuse.as_ref())
                .and_then(|(_, _, png)| tera_package::png::decode(png).ok())
            {
                scene.textures.push(downsample_texture(&image, 128));
                mesh_texture[index] = (scene.textures.len() - 1) as i32;
            }
        }
    }
    let _ = mesh_bounds;
    let mut spans = Vec::with_capacity(actors.len());
    for actor in actors {
        let start = scene.triangles.len();
        if start < MAP_PREVIEW_TRI_BUDGET {
            if let (Some(instance), Some(mesh_index)) = (actor.instance(), actor.mesh) {
                if let Some((mesh, _)) = meshes.get(mesh_index) {
                    let texture = mesh_texture.get(mesh_index).copied().unwrap_or(-1);
                    push_actor_triangles(&mut scene.triangles, mesh, &instance, view, texture);
                }
            }
        }
        spans.push(start..scene.triangles.len());
    }
    scene.add_grid(view.grid_half, view.grid_step, [58, 52, 64]);
    (scene, spans, mesh_texture)
}

fn rebuild_actor(editor: &mut MapEditor, index: usize) {
    let (Some(instance), Some(mesh_index)) =
        (editor.actors[index].instance(), editor.actors[index].mesh)
    else {
        return;
    };
    let Some((mesh, _)) = editor.meshes.get(mesh_index) else {
        return;
    };
    let span = editor.actors[index].span.clone();
    if span.is_empty() {
        return;
    }
    let texture = editor.mesh_texture.get(mesh_index).copied().unwrap_or(-1);
    let mut fresh = Vec::with_capacity(span.len());
    push_actor_triangles(&mut fresh, mesh, &instance, &editor.view, texture);
    if fresh.len() == span.len() {
        editor.scene.triangles[span].clone_from_slice(&fresh);
    }
}

fn project(matrix: &Matrix, point: [f32; 3]) -> Option<(f32, f32)> {
    let x = matrix[0] * point[0] + matrix[4] * point[1] + matrix[8] * point[2] + matrix[12];
    let y = matrix[1] * point[0] + matrix[5] * point[1] + matrix[9] * point[2] + matrix[13];
    let w = matrix[3] * point[0] + matrix[7] * point[1] + matrix[11] * point[2] + matrix[15];
    if w <= 1e-4 {
        return None;
    }
    Some((x / w, y / w))
}

fn to_screen(ndc: (f32, f32), rect: egui::Rect) -> egui::Pos2 {
    egui::pos2(
        rect.left() + (ndc.0 * 0.5 + 0.5) * rect.width(),
        rect.top() + (1.0 - (ndc.1 * 0.5 + 0.5)) * rect.height(),
    )
}

fn pick_actor(
    editor: &MapEditor,
    view_projection: &Matrix,
    rect: egui::Rect,
    pointer: egui::Pos2,
) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (index, actor) in editor.actors.iter().enumerate() {
        let Some(ndc) = project(view_projection, editor.view.convert(actor.location)) else {
            continue;
        };
        let screen = to_screen(ndc, rect);
        let distance = screen.distance(pointer);
        if distance <= 16.0 && best.map(|(_, d)| distance < d).unwrap_or(true) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

fn selection_bounds(
    editor: &MapEditor,
    index: usize,
    view_projection: &Matrix,
    rect: egui::Rect,
) -> Option<egui::Rect> {
    let actor = &editor.actors[index];
    let instance = actor.instance();
    let bounds = actor.mesh.and_then(|mesh| editor.mesh_bounds.get(mesh)).copied();
    let mut min = egui::pos2(f32::MAX, f32::MAX);
    let mut max = egui::pos2(f32::MIN, f32::MIN);
    let mut found = false;
    let consider = |world: [f32; 3], min: &mut egui::Pos2, max: &mut egui::Pos2, found: &mut bool| {
        if let Some(ndc) = project(view_projection, editor.view.convert(world)) {
            let screen = to_screen(ndc, rect);
            if rect.contains(screen) {
                min.x = min.x.min(screen.x);
                min.y = min.y.min(screen.y);
                max.x = max.x.max(screen.x);
                max.y = max.y.max(screen.y);
                *found = true;
            }
        }
    };
    match (bounds, instance) {
        (Some((low, high)), Some(instance)) => {
            for corner in 0..8 {
                let local = [
                    if corner & 1 == 0 { low[0] } else { high[0] },
                    if corner & 2 == 0 { low[1] } else { high[1] },
                    if corner & 4 == 0 { low[2] } else { high[2] },
                ];
                consider(instance_vertex(&instance, local), &mut min, &mut max, &mut found);
            }
        }
        _ => consider(actor.location, &mut min, &mut max, &mut found),
    }
    if found {
        Some(egui::Rect::from_min_max(min - egui::vec2(3.0, 3.0), max + egui::vec2(3.0, 3.0)))
    } else {
        None
    }
}

fn asset_roots(paths: &Paths) -> Vec<PathBuf> {
    vec![paths.library(), paths.cooked(), paths.library().join("CookedPC")]
}

fn resolve_in(roots: &[PathBuf], relative: &str) -> PathBuf {
    roots
        .iter()
        .map(|root| root.join(relative))
        .find(|candidate| candidate.exists())
        .unwrap_or_else(|| roots[0].join(relative))
}

fn collect_editor(
    level_file: &Path,
    index_path: &Path,
    roots: &[PathBuf],
    label: &str,
    sender: &std::sync::mpsc::Sender<Message>,
) -> Result<EditorBuild, String> {
    let siblings = tera_package::zone_siblings(level_file);
    let sibling_total = siblings.len();
    let mut files = Vec::new();
    let mut raw_actors: Vec<(usize, tera_package::LevelActor)> = Vec::new();
    for (scanned, sibling) in siblings.iter().enumerate() {
        let _ = sender.send(Message::Progress(Progress {
            label: format!("reading {} actors", leaf(sibling.to_str().unwrap_or(""))),
            done: scanned,
            total: sibling_total,
        }));
        let Ok(handle) = std::fs::File::open(sibling) else {
            continue;
        };
        let Ok(mapped) = (unsafe { Mmap::map(&handle) }) else {
            continue;
        };
        let actors = level_actors(&mapped);
        if actors.is_empty() {
            continue;
        }
        let file_index = files.len();
        files.push(sibling.clone());
        for actor in actors {
            raw_actors.push((file_index, actor));
        }
    }
    if raw_actors.is_empty() {
        return Err("no actor in this zone".into());
    }

    let mut unique: Vec<String> = raw_actors
        .iter()
        .map(|(_, actor)| actor.placement.mesh.clone())
        .filter(|mesh| !mesh.is_empty())
        .collect();
    unique.sort();
    unique.dedup();

    let index = Index::open(index_path).map_err(|error| error.to_string())?;
    let mut meshes: Vec<(Mesh, Vec<MaterialInput>)> = Vec::new();
    let mut mesh_index: HashMap<String, usize> = HashMap::new();
    let total = unique.len();
    for (done, name) in unique.iter().enumerate() {
        let _ = sender.send(Message::Progress(Progress {
            label: format!("loading mesh {}", name.rsplit('.').next().unwrap_or(name)),
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
        let file = resolve_in(roots, index.file_name(entry.file as usize));
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
        let materials = mesh_materials_or_diffuse(&package, &mesh, name, &roots[0]);
        mesh_index.insert(name.clone(), meshes.len());
        meshes.push((mesh, materials));
    }

    let mut actors: Vec<EditorActor> = raw_actors
        .into_iter()
        .map(|(file, actor)| {
            let placement = actor.placement;
            EditorActor {
                object: actor.object,
                class: actor.class,
                file,
                mesh: mesh_index.get(&placement.mesh).copied(),
                location: placement.location,
                rotation: placement.rotation,
                scale: placement.scale,
                origin_location: placement.location,
                origin_rotation: placement.rotation,
                origin_scale: placement.scale,
                span: 0..0,
            }
        })
        .collect();

    let _ = sender.send(Message::Progress(Progress {
        label: format!("building scene · {} actors", actors.len()),
        done: total,
        total,
    }));
    let mesh_bounds = mesh_bounds_of(&meshes);
    let view = make_view(&actors, &mesh_bounds);
    let (scene, spans, mesh_texture) = build_scene(&meshes, &mesh_bounds, &actors, &view, false);
    for (actor, span) in actors.iter_mut().zip(spans) {
        actor.span = span;
    }
    let placements = actors.iter().filter(|actor| actor.mesh.is_some()).count();
    let label = format!(
        "{label} — {} actors, {placements} placed, {} meshes",
        actors.len(),
        meshes.len()
    );
    let name = leaf(level_file.to_str().unwrap_or("level")).to_string();
    Ok(EditorBuild {
        files,
        meshes,
        mesh_bounds,
        mesh_texture,
        actors,
        view,
        scene,
        name,
        label,
    })
}

fn save_map_edits(
    files: &[PathBuf],
    per_file: HashMap<usize, Vec<(String, Transform)>>,
) -> Result<String, String> {
    let mut saved_files = 0usize;
    let mut saved_actors = 0usize;
    for (file_index, edits) in per_file {
        let Some(path) = files.get(file_index) else {
            continue;
        };
        let mut data = std::fs::read(path).map_err(|error| error.to_string())?;
        let mut applied = 0usize;
        for (object, transform) in &edits {
            match set_actor_transform(&data, object, *transform) {
                Ok(replaced) => {
                    data = replaced.bytes;
                    applied += 1;
                }
                Err(error) => return Err(format!("{object}: {error}")),
            }
        }
        if applied == 0 {
            continue;
        }
        let backup = path.with_extension("clean");
        if !backup.exists() {
            std::fs::copy(path, &backup).map_err(|error| error.to_string())?;
        }
        std::fs::write(path, &data).map_err(|error| error.to_string())?;
        saved_files += 1;
        saved_actors += applied;
    }
    Ok(format!(
        "saved {saved_actors} actor(s) across {saved_files} file(s)"
    ))
}

fn write_map_obj(
    meshes: &[(Mesh, Vec<MaterialInput>)],
    instances: &[MapInstance],
    name: &str,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "o {name}");
    let mut base = 1usize;
    for instance in instances {
        let Some((mesh, _)) = meshes.get(instance.mesh) else {
            continue;
        };
        for vertex in &mesh.vertices {
            let world = instance_vertex(instance, *vertex);
            let _ = writeln!(out, "v {} {} {}", world[0], world[2], world[1]);
        }
        for face in mesh.indices.as_chunks::<3>().0 {
            let _ = writeln!(
                out,
                "f {} {} {}",
                base + face[0] as usize,
                base + face[1] as usize,
                base + face[2] as usize
            );
        }
        base += mesh.vertices.len();
    }
    out
}
