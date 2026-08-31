use crate::jobs::{report, Job};
use crate::theme::{self, Palette};
use crate::view3d::{Camera, Raster, Scene, Texture as RasterTexture, Triangle};
use crate::Paths;
use eframe::egui;
use memmap2::Mmap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;
use tera_index::Index;
use tera_package::mesh::Mesh;
use tera_package::properties::read_export_properties;
use tera_package::{Bundle, Package, SoundNodeWave, Texture2D};

pub enum Preview {
    None,
    Texture {
        width: u32,
        height: u32,
        format: String,
        mips: usize,
        rgba: Vec<u8>,
        handle: Option<egui::TextureHandle>,
    },
    Mesh {
        mesh: Mesh,
        camera: Camera,
        handle: Option<egui::TextureHandle>,
        dirty: bool,
        texture: Option<(u32, u32, Vec<u8>)>,
        file: PathBuf,
        animations: Vec<tera_package::Animation>,
        anims_loaded: bool,
        anim: usize,
        playing: bool,
        time: f32,
    },
    Sound {
        ogg: Vec<u8>,
        duration: f32,
        rate: i32,
        channels: i32,
        peaks: Vec<(f32, f32)>,
    },
    Raw,
}

pub struct Loaded {
    pub package: String,
    pub path: String,
    pub class: String,
    pub file: PathBuf,
    pub package_offset: u64,
    pub export: usize,
    pub size: usize,
    pub properties: Vec<(String, String, String)>,
    pub preview: Preview,
    pub load_ms: f64,
}

pub struct Assets {
    index: Option<Arc<Index>>,
    index_error: Option<String>,
    query: String,
    class_filter: usize,
    classes: Vec<String>,
    hits: Vec<u32>,
    selected: Option<u32>,
    loaded: Option<Loaded>,
    search_ms: f64,
    search_rx: Option<Receiver<(Vec<u32>, f64)>>,
    dirty_search: bool,
    job: Option<Job>,
    audio: Option<(rodio::OutputStream, rodio::OutputStreamHandle)>,
    sink: Option<rodio::Sink>,
    new_package: String,
    new_object: String,
    pending_pick: Option<usize>,
}

impl Assets {
    pub fn new(paths: &Paths) -> Self {
        let mut assets = Self {
            index: None,
            index_error: None,
            query: String::new(),
            class_filter: 0,
            classes: Vec::new(),
            hits: Vec::new(),
            selected: None,
            loaded: None,
            search_ms: 0.0,
            search_rx: None,
            dirty_search: true,
            job: None,
            audio: rodio::OutputStream::try_default().ok(),
            sink: None,
            new_package: String::new(),
            new_object: String::new(),
            pending_pick: None,
        };
        assets.open_index(&paths.index);
        let arguments: Vec<String> = std::env::args().collect();
        for pair in arguments.windows(2) {
            match pair[0].as_str() {
                "--query" => assets.query = pair[1].clone(),
                "--pick" => assets.pending_pick = pair[1].parse().ok(),
                "--class" => {
                    if let Some(position) = assets
                        .classes
                        .iter()
                        .position(|name| name.eq_ignore_ascii_case(&pair[1]))
                    {
                        assets.class_filter = position;
                    }
                }
                _ => {}
            }
        }
        assets
    }

    fn open_index(&mut self, path: &Path) {
        match Index::open(path) {
            Ok(index) => {
                let mut classes = vec!["all classes".to_string()];
                let mut names: Vec<String> =
                    index.classes().iter().map(|name| name.to_string()).collect();
                names.sort();
                classes.extend(names);
                self.classes = classes;
                self.index = Some(Arc::new(index));
                self.index_error = None;
                self.dirty_search = true;
            }
            Err(error) => {
                self.index = None;
                self.index_error = Some(error.to_string());
            }
        }
    }

    pub fn request(&mut self, query: String) {
        self.query = query;
        self.class_filter = 0;
        self.pending_pick = Some(0);
        self.dirty_search = true;
    }

    fn start_search(&mut self) {
        let Some(index) = self.index.clone() else {
            return;
        };
        let query = self.query.clone();
        let class = if self.class_filter == 0 {
            None
        } else {
            self.classes.get(self.class_filter).cloned()
        };
        let (sender, receiver) = channel();
        self.search_rx = Some(receiver);
        std::thread::spawn(move || {
            let started = Instant::now();
            let hits = index.search_objects(&query, 4000, class.as_deref());
            let _ = sender.send((hits, started.elapsed().as_secs_f64() * 1000.0));
        });
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        palette: Palette,
        paths: &Paths,
        status: &mut String,
    ) {
        if let Some(receiver) = &self.search_rx {
            if let Ok((hits, elapsed)) = receiver.try_recv() {
                self.hits = hits;
                self.search_ms = elapsed;
                self.search_rx = None;
                if let Some(pick) = self.pending_pick.take() {
                    if let Some(hit) = self.hits.get(pick).copied() {
                        self.selected = Some(hit);
                        self.load(hit);
                    }
                }
            } else {
                context.request_repaint_after(std::time::Duration::from_millis(30));
            }
        }
        if self.dirty_search && self.search_rx.is_none() {
            self.dirty_search = false;
            self.start_search();
        }
        if let Some(job) = &mut self.job {
            if let Some(outcome) = job.poll() {
                *status = match outcome {
                    Ok(message) => message,
                    Err(error) => format!("error: {error}"),
                };
                self.job = None;
                self.open_index(&paths.index);
            } else {
                context.request_repaint_after(std::time::Duration::from_millis(80));
            }
        }

        if self.index.is_none() {
            self.index_prompt(ui, palette, paths, status);
            return;
        }

        egui::Panel::left("asset_list")
            .resizable(true)
            .default_size(470.0)
            .size_range(300.0..=900.0)
            .show(ui, |ui| self.list_ui(ui, palette));
        egui::CentralPanel::default().show(ui, |ui| {
            self.detail_ui(ui, context, palette, paths, status)
        });
    }

    fn index_prompt(
        &mut self,
        ui: &mut egui::Ui,
        palette: Palette,
        paths: &Paths,
        status: &mut String,
    ) {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(theme::display(palette, "no index yet", 26.0));
            ui.add_space(8.0);
            theme::eyebrow(
                ui,
                palette,
                format!("target · {}", paths.cooked().display()),
            );
            if let Some(error) = &self.index_error {
                ui.add_space(4.0);
                ui.colored_label(theme::colors(palette).dim, error);
            }
            ui.add_space(16.0);
            if let Some(job) = &self.job {
                ui.add(egui::ProgressBar::new(job.fraction()).text(job.detail.clone()));
            } else if ui.button("build the asset index").clicked() {
                let root = paths.cooked();
                let out = paths.index.clone();
                *status = "indexing…".into();
                self.job = Some(Job::spawn("index", move |sender| {
                    if let Some(parent) = out.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let started = Instant::now();
                    let data = tera_index::build(&root, true, |done, total| {
                        report(sender, format!("{done}/{total} packages"), done, total);
                    })
                    .map_err(|error| error.to_string())?;
                    data.write(&out).map_err(|error| error.to_string())?;
                    Ok(format!(
                        "index built: {} packages, {} objects in {:.1}s",
                        data.packages.len(),
                        data.objects.len(),
                        started.elapsed().as_secs_f32()
                    ))
                }));
            }
        });
    }

    fn list_ui(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let Some(index) = self.index.clone() else {
            return;
        };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("search objects…")
                    .desired_width(240.0),
            );
            if response.changed() {
                self.dirty_search = true;
            }
            egui::ComboBox::from_id_salt("class")
                .selected_text(
                    self.classes
                        .get(self.class_filter)
                        .cloned()
                        .unwrap_or_default(),
                )
                .width(150.0)
                .show_ui(ui, |ui| {
                    for (position, name) in self.classes.iter().enumerate() {
                        if ui
                            .selectable_label(self.class_filter == position, name)
                            .clicked()
                        {
                            self.class_filter = position;
                            self.dirty_search = true;
                        }
                    }
                });
        });
        theme::eyebrow(
            ui,
            palette,
            format!(
                "{} hits · {} objects · {:.1} ms",
                self.hits.len(),
                index.object_count(),
                self.search_ms
            ),
        );
        theme::rule(ui, palette);
        let row_height = 18.0;
        let mut clicked = None;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show_rows(
            ui,
            row_height,
            self.hits.len(),
            |ui, range| {
                for row in range {
                    let hit = self.hits[row];
                    let entry = index.object(hit as usize);
                    let class = index.class_name(entry.class);
                    let selected = self.selected == Some(hit);
                    let name = index.object_name(hit as usize);
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), row_height),
                        egui::Sense::click(),
                    );
                    if selected {
                        ui.painter().rect_filled(
                            rect,
                            2.0,
                            theme::colors(palette).accent.gamma_multiply(0.45),
                        );
                    } else if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, 2.0, theme::colors(palette).raised);
                    }
                    let font = egui::FontId::monospace(11.5);
                    ui.painter().text(
                        rect.left_center() + egui::vec2(4.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        format!("{class:.18}"),
                        font.clone(),
                        theme::class_color(class),
                    );
                    let target = name.len().saturating_sub(64);
                    let cut = (target..=name.len())
                        .find(|index| name.is_char_boundary(*index))
                        .unwrap_or(name.len());
                    let short = if cut > 0 {
                        format!("…{}", &name[cut..])
                    } else {
                        name.to_string()
                    };
                    ui.painter().text(
                        rect.left_center() + egui::vec2(150.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        short,
                        font,
                        theme::colors(palette).bone,
                    );
                    if response.clicked() {
                        clicked = Some(hit);
                    }
                }
            },
        );
        if let Some(hit) = clicked {
            self.selected = Some(hit);
            self.load(hit);
        }
    }

    fn load(&mut self, hit: u32) {
        let Some(index) = self.index.clone() else {
            return;
        };
        let started = Instant::now();
        let object = index.object(hit as usize);
        let package_entry = index.package(object.package as usize);
        let root = crate::Paths::default().cooked();
        let file = root.join(index.file_name(package_entry.file as usize));
        self.loaded = match load_object(
            &file,
            package_entry.offset,
            object.export as usize,
            index.package_name(object.package as usize),
        ) {
            Ok(mut loaded) => {
                loaded.load_ms = started.elapsed().as_secs_f64() * 1000.0;
                self.new_package = loaded.package.clone();
                self.new_object = loaded
                    .path
                    .rsplit('.')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                Some(loaded)
            }
            Err(error) => {
                self.loaded = None;
                Some(Loaded {
                    package: index.package_name(object.package as usize).to_string(),
                    path: index.object_name(hit as usize).to_string(),
                    class: index.class_name(object.class).to_string(),
                    file,
                    package_offset: package_entry.offset,
                    export: object.export as usize,
                    size: 0,
                    properties: vec![("error".into(), String::new(), error)],
                    preview: Preview::None,
                    load_ms: started.elapsed().as_secs_f64() * 1000.0,
                })
            }
        };
    }

    fn detail_ui(
        &mut self,
        ui: &mut egui::Ui,
        context: &egui::Context,
        palette: Palette,
        paths: &Paths,
        status: &mut String,
    ) {
        let Some(loaded) = &mut self.loaded else {
            ui.add_space(60.0);
            ui.vertical_centered(|ui| {
                ui.label(theme::display(palette, "select an object", 22.0));
            });
            return;
        };
        ui.add_space(6.0);
        ui.label(theme::display(palette, &loaded.path, 20.0));
        theme::eyebrow(
            ui,
            palette,
            format!(
                "{} · {} · {} bytes · {} · loaded in {:.1} ms",
                loaded.class,
                loaded.package,
                loaded.size,
                loaded
                    .file
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default(),
                loaded.load_ms
            ),
        );
        theme::rule(ui, palette);

        let actions = preview_ui(
            ui,
            context,
            palette,
            loaded,
            &mut self.sink,
            &self.audio,
            self.index.clone(),
            paths.cooked(),
        );
        theme::rule(ui, palette);
        ui.horizontal_wrapped(|ui| {
            for action in actions {
                if ui.button(action.label).clicked() {
                    match (action.run)(loaded, paths) {
                        Ok(message) => *status = message,
                        Err(error) => *status = format!("error: {error}"),
                    }
                }
            }
            if matches!(loaded.preview, Preview::Texture { .. }) {
                if ui.button("replace with an image…").clicked() {
                    match replace_texture(loaded) {
                        Ok(message) => *status = message,
                        Err(error) => *status = format!("error: {error}"),
                    }
                }
                if ui.button("create a mod from an image…").clicked() {
                    match create_mod(loaded, paths, &self.new_package, &self.new_object) {
                        Ok(message) => *status = message,
                        Err(error) => *status = format!("error: {error}"),
                    }
                }
            }
        });

        theme::rule(ui, palette);
        egui::ScrollArea::vertical()
            .id_salt("props")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                egui::Grid::new("properties")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        for (name, kind, value) in &loaded.properties {
                            ui.label(name);
                            ui.colored_label(theme::colors(palette).dim, kind);
                            ui.label(value);
                            ui.end_row();
                        }
                    });
            });
    }
}

pub struct Action {
    pub label: &'static str,
    pub run: fn(&Loaded, &Paths) -> Result<String, String>,
}

fn preview_ui(
    ui: &mut egui::Ui,
    context: &egui::Context,
    palette: Palette,
    loaded: &mut Loaded,
    sink: &mut Option<rodio::Sink>,
    audio: &Option<(rodio::OutputStream, rodio::OutputStreamHandle)>,
    index: Option<Arc<Index>>,
    cooked: PathBuf,
) -> Vec<Action> {
    let mut actions = Vec::new();
    let mesh_leaf = loaded.path.rsplit('.').next().unwrap_or("mesh").to_string();
    match &mut loaded.preview {
        Preview::Texture {
            width,
            height,
            format,
            mips,
            rgba,
            handle,
        } => {
            if handle.is_none() {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [*width as usize, *height as usize],
                    rgba,
                );
                *handle = Some(context.load_texture(
                    "preview",
                    image,
                    egui::TextureOptions::NEAREST,
                ));
            }
            theme::eyebrow(
                ui,
                palette,
                format!("{format} · {width}×{height} · {mips} mip(s)"),
            );
            if let Some(handle) = handle {
                let available = ui.available_size_before_wrap();
                let budget = egui::vec2(available.x.min(760.0), 360.0);
                let ratio = (*width as f32 / *height as f32).max(0.01);
                let mut size = egui::vec2(budget.x, budget.x / ratio);
                if size.y > budget.y {
                    size = egui::vec2(budget.y * ratio, budget.y);
                }
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                theme::checkerboard(ui.painter(), rect, 12.0, palette);
                ui.painter().image(
                    handle.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            actions.push(Action {
                label: "export PNG",
                run: export_png,
            });
            actions.push(Action {
                label: "export DDS",
                run: export_dds_action,
            });
        }
        Preview::Mesh {
            mesh,
            camera,
            handle,
            dirty,
            texture,
            file,
            animations,
            anims_loaded,
            anim,
            playing,
            time,
        } => {
            theme::eyebrow(
                ui,
                palette,
                format!(
                    "{} sommets · {} triangles",
                    mesh.vertices.len(),
                    mesh.triangle_count()
                ),
            );
            let size = egui::vec2(ui.available_width().min(760.0), 360.0);
            let (rect, response) =
                ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            if response.dragged() {
                let delta = response.drag_delta();
                camera.yaw += delta.x * 0.01;
                camera.pitch = (camera.pitch + delta.y * 0.01).clamp(-1.5, 1.5);
                *dirty = true;
            }
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if response.hovered() && scroll.abs() > 0.1 {
                camera.distance = (camera.distance * (1.0 - scroll * 0.002)).clamp(1.0, 1.0e6);
                *dirty = true;
            }
            let has_anim = mesh.skin.is_some() && !animations.is_empty();
            if has_anim && *playing {
                let delta = ui.input(|input| input.stable_dt).min(0.1);
                let duration = animations.get(*anim).map(|clip| clip.duration).unwrap_or(0.0).max(0.001);
                *time = (*time + delta) % duration;
                *dirty = true;
                context.request_repaint();
            }
            if handle.is_none() || *dirty {
                let positions = if has_anim {
                    animations
                        .get(*anim)
                        .and_then(|clip| crate::skinning::pose_vertices(mesh, clip, *time))
                        .unwrap_or_else(|| mesh.vertices.clone())
                } else {
                    mesh.vertices.clone()
                };
                let width = size.x as usize;
                let height = size.y as usize;
                let mut raster = Raster::new(width.max(16), height.max(16));
                let accent = theme::colors(palette).panel;
                raster.sky_top = [accent.r(), accent.g(), accent.b()];
                let background = theme::colors(palette).background;
                raster.sky_bottom = [background.r(), background.g(), background.b()];
                let mut scene = build_scene(mesh, &positions, texture.as_ref());
                scene.shade();
                raster.render(&scene, &camera.view_projection(size.x / size.y));
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [raster.width, raster.height],
                    &raster.color,
                );
                *handle = Some(context.load_texture(
                    "mesh",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
                *dirty = false;
            }
            if let Some(handle) = handle {
                ui.painter().image(
                    handle.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            if mesh.skin.is_some() {
                ui.add_space(6.0);
                if !*anims_loaded {
                    if ui.button("charger les animations").clicked() {
                        let bones: std::collections::HashSet<String> =
                            mesh.skin.as_ref().unwrap().bones.iter().map(|bone| bone.name.clone()).collect();
                        let skeleton = bones.len().max(1);
                        let mut clips = find_animations(index.as_ref(), &cooked, &mesh_leaf, &bones, skeleton);
                        if clips.is_empty() {
                            let borrowed: std::collections::HashSet<&str> = bones.iter().map(String::as_str).collect();
                            clips = collect_animations(file.as_path(), &borrowed, skeleton, 60);
                        }
                        *animations = clips;
                        *anims_loaded = true;
                        *anim = 0;
                        *time = 0.0;
                        *dirty = true;
                    }
                } else if animations.is_empty() {
                    theme::eyebrow(ui, palette, "aucune animation dans ce fichier");
                } else {
                    ui.horizontal(|ui| {
                        let selected = animations.get(*anim).map(|clip| clip.name.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("anim_pick")
                            .selected_text(selected)
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for (index, clip) in animations.iter().enumerate() {
                                    if ui.selectable_label(*anim == index, &clip.name).clicked() {
                                        *anim = index;
                                        *time = 0.0;
                                        *dirty = true;
                                    }
                                }
                            });
                        if ui.button(if *playing { "pause" } else { "play" }).clicked() {
                            *playing = !*playing;
                        }
                        let duration = animations.get(*anim).map(|clip| clip.duration).unwrap_or(0.0).max(0.001);
                        if ui.add(egui::Slider::new(time, 0.0..=duration).suffix(" s")).changed() {
                            *dirty = true;
                        }
                    });
                    theme::eyebrow(
                        ui,
                        palette,
                        format!("{} clips · squelette {} os", animations.len(), mesh.skin.as_ref().unwrap().bones.len()),
                    );
                }
            }
            actions.push(Action {
                label: "export OBJ",
                run: export_obj,
            });
            actions.push(Action {
                label: "export glTF + animations",
                run: export_glb_anim,
            });
            actions.push(Action {
                label: "export glTF (Blender)",
                run: export_glb,
            });
        }
        Preview::Sound {
            ogg,
            duration,
            rate,
            channels,
            peaks,
        } => {
            theme::eyebrow(
                ui,
                palette,
                format!("ogg vorbis · {duration:.2}s · {rate} Hz · {channels} channel(s)"),
            );
            draw_waveform(ui, palette, peaks);
            ui.horizontal(|ui| {
                if ui.button("play").clicked() {
                    if let Some((_, handle)) = audio {
                        if let Ok(new_sink) = rodio::Sink::try_new(handle) {
                            if let Ok(decoder) =
                                rodio::Decoder::new(std::io::Cursor::new(ogg.clone()))
                            {
                                new_sink.append(decoder);
                                *sink = Some(new_sink);
                            }
                        }
                    }
                }
                if ui.button("stop").clicked() {
                    if let Some(active) = sink.take() {
                        active.stop();
                    }
                }
            });
            actions.push(Action {
                label: "export OGG",
                run: export_ogg,
            });
        }
        Preview::Raw | Preview::None => {
            theme::eyebrow(ui, palette, "no preview for this class");
        }
    }
    actions.push(Action {
        label: "export raw object",
        run: export_raw,
    });
    actions
}

fn decode_peaks(ogg: &[u8], buckets: usize) -> Vec<(f32, f32)> {
    use rodio::Source;
    let Ok(decoder) = rodio::Decoder::new(std::io::Cursor::new(ogg.to_vec())) else {
        return Vec::new();
    };
    let channels = decoder.channels().max(1) as usize;
    let samples: Vec<i16> = decoder.step_by(channels).collect();
    if samples.is_empty() {
        return Vec::new();
    }
    let step = samples.len().div_ceil(buckets.max(1));
    samples
        .chunks(step)
        .map(|chunk| {
            let low = chunk.iter().copied().min().unwrap_or(0) as f32 / 32768.0;
            let high = chunk.iter().copied().max().unwrap_or(0) as f32 / 32768.0;
            (low, high)
        })
        .collect()
}

fn draw_waveform(ui: &mut egui::Ui, palette: Palette, peaks: &[(f32, f32)]) {
    let size = egui::vec2(ui.available_width().min(760.0), 100.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, theme::colors(palette).background);
    if peaks.is_empty() {
        return;
    }
    let accent = theme::colors(palette).accent_high;
    painter.hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1.0, theme::colors(palette).line),
    );
    let half = rect.height() * 0.46;
    for (column, (low, high)) in peaks.iter().enumerate() {
        let x = rect.left() + column as f32 * rect.width() / peaks.len() as f32;
        let top = rect.center().y - high * half;
        let bottom = rect.center().y - low * half;
        painter.vline(
            x,
            egui::Rangef::new(top.min(bottom - 0.5), bottom.max(top + 0.5)),
            egui::Stroke::new(1.0, accent),
        );
    }
}

fn bounds_of(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut low = [f32::MAX; 3];
    let mut high = [f32::MIN; 3];
    for position in positions {
        for axis in 0..3 {
            low[axis] = low[axis].min(position[axis]);
            high[axis] = high[axis].max(position[axis]);
        }
    }
    if positions.is_empty() {
        return ([0.0; 3], [0.0; 3]);
    }
    (low, high)
}

fn build_scene(mesh: &Mesh, positions: &[[f32; 3]], texture: Option<&(u32, u32, Vec<u8>)>) -> Scene {
    let mut scene = Scene::default();
    let (low, high) = bounds_of(positions);
    let center = [
        (low[0] + high[0]) * 0.5,
        (low[1] + high[1]) * 0.5,
        (low[2] + high[2]) * 0.5,
    ];
    let extent = (0..3)
        .map(|axis| high[axis] - low[axis])
        .fold(1.0f32, f32::max);
    let scale = 200.0 / extent;
    let convert = |vertex: [f32; 3]| -> [f32; 3] {
        [
            (vertex[0] - center[0]) * scale,
            (vertex[2] - center[2]) * scale,
            (vertex[1] - center[1]) * scale,
        ]
    };
    let has_uv = mesh.uvs.len() == mesh.vertices.len() && texture.is_some();
    let texture_index = if has_uv { 0 } else { -1 };
    let uv_of = |vertex: usize| -> [f32; 2] {
        if has_uv {
            mesh.uvs[vertex]
        } else {
            [0.0, 0.0]
        }
    };
    let vertex = |index: usize| -> [f32; 3] { positions.get(index).copied().unwrap_or([0.0; 3]) };
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let points = [
            convert(vertex(triangle[0] as usize)),
            convert(vertex(triangle[1] as usize)),
            convert(vertex(triangle[2] as usize)),
        ];
        scene.triangles.push(Triangle {
            points,
            uv: [
                uv_of(triangle[0] as usize),
                uv_of(triangle[1] as usize),
                uv_of(triangle[2] as usize),
            ],
            texture: texture_index,
            color: [196, 186, 200],
            light: [1.0; 3],
        });
    }
    scene.add_grid(160.0, 20.0, [70, 60, 72]);
    if let Some((width, height, rgba)) = texture {
        scene.textures.push(RasterTexture {
            width: *width as usize,
            height: *height as usize,
            rgba: rgba.clone(),
        });
    }
    scene
}

fn load_object(
    file: &Path,
    package_offset: u64,
    export_index: usize,
    package_name: &str,
) -> Result<Loaded, String> {
    let handle = std::fs::File::open(file).map_err(|error| error.to_string())?;
    let map = unsafe { Mmap::map(&handle) }.map_err(|error| error.to_string())?;
    let mut package =
        Package::parse(&map, package_offset as usize).map_err(|error| error.to_string())?;
    package.name_hint = Some(package_name.to_string());
    let export = package
        .exports
        .get(export_index)
        .ok_or_else(|| "export not found".to_string())?;
    let class = package.export_class(export);
    let path = package.export_path(export_index);
    let blob = package.export_data(export).map_err(|error| error.to_string())?;
    let mut properties = Vec::new();
    if let Ok((parsed, _)) = read_export_properties(&package, blob) {
        for property in parsed {
            properties.push((
                property.name,
                property.type_name,
                property.value.describe(),
            ));
        }
    }
    let preview = match class.as_str() {
        "Texture2D" => match Texture2D::parse(&package, export) {
            Ok(texture) => match texture.decode_rgba() {
                Ok((width, height, rgba)) => Preview::Texture {
                    width,
                    height,
                    format: texture.format.clone(),
                    mips: texture.mips.len(),
                    rgba,
                    handle: None,
                },
                Err(_) => Preview::Raw,
            },
            Err(_) => Preview::Raw,
        },
        "SoundNodeWave" => match SoundNodeWave::parse(&package, export) {
            Ok(sound) => match sound.ogg.clone() {
                Some(ogg) => Preview::Sound {
                    peaks: decode_peaks(&ogg, 900),
                    ogg,
                    duration: sound.duration,
                    rate: sound.sample_rate,
                    channels: sound.channels,
                },
                None => Preview::Raw,
            },
            Err(_) => Preview::Raw,
        },
        "StaticMesh" | "SkeletalMesh" => {
            let parsed = if class == "SkeletalMesh" {
                tera_package::parse_skeletal_mesh(&package, export)
            } else {
                tera_package::parse_static_mesh(&package, export)
            };
            match parsed {
                Some(mesh) => {
                    let leaf = path.rsplit('.').next().unwrap_or("mesh");
                    let texture = tera_package::mesh_diffuse_rgba(
                        &package,
                        leaf,
                        &crate::Paths::default().cooked(),
                    );
                    Preview::Mesh {
                        mesh,
                        camera: Camera {
                            distance: 420.0,
                            ..Camera::default()
                        },
                        handle: None,
                        dirty: true,
                        texture,
                        file: file.to_path_buf(),
                        animations: Vec::new(),
                        anims_loaded: false,
                        anim: 0,
                        playing: false,
                        time: 0.0,
                    }
                }
                None => Preview::Raw,
            }
        }
        _ => Preview::Raw,
    };
    Ok(Loaded {
        package: package.package_name(),
        path,
        class,
        file: file.to_path_buf(),
        package_offset,
        export: export_index,
        size: blob.len(),
        properties,
        preview,
        load_ms: 0.0,
    })
}

fn suggested_name(loaded: &Loaded, extension: &str) -> String {
    let leaf = loaded.path.rsplit('.').next().unwrap_or("objet");
    format!("{leaf}.{extension}")
}

fn save_dialog(loaded: &Loaded, extension: &str) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_file_name(suggested_name(loaded, extension))
        .save_file()
}

fn export_png(loaded: &Loaded, _paths: &Paths) -> Result<String, String> {
    let Preview::Texture {
        width,
        height,
        rgba,
        ..
    } = &loaded.preview
    else {
        return Err("not a texture".into());
    };
    let Some(target) = save_dialog(loaded, "png") else {
        return Ok("cancelled".into());
    };
    let bytes =
        tera_package::png::encode(rgba, *width, *height).map_err(|error| error.to_string())?;
    std::fs::write(&target, bytes).map_err(|error| error.to_string())?;
    Ok(format!("wrote {}", target.display()))
}

fn export_dds_action(loaded: &Loaded, _paths: &Paths) -> Result<String, String> {
    let Some(target) = save_dialog(loaded, "dds") else {
        return Ok("cancelled".into());
    };
    with_export(loaded, |package, export| {
        let texture =
            Texture2D::parse(package, export).map_err(|error| error.to_string())?;
        tera_package::export_dds(&texture).map_err(|error| error.to_string())
    })
    .and_then(|bytes| {
        std::fs::write(&target, bytes).map_err(|error| error.to_string())?;
        Ok(format!("wrote {}", target.display()))
    })
}

fn export_ogg(loaded: &Loaded, _paths: &Paths) -> Result<String, String> {
    let Preview::Sound { ogg, .. } = &loaded.preview else {
        return Err("not a sound".into());
    };
    let Some(target) = save_dialog(loaded, "ogg") else {
        return Ok("cancelled".into());
    };
    std::fs::write(&target, ogg).map_err(|error| error.to_string())?;
    Ok(format!("wrote {}", target.display()))
}

fn export_obj(loaded: &Loaded, _paths: &Paths) -> Result<String, String> {
    let Preview::Mesh { mesh, .. } = &loaded.preview else {
        return Err("not a mesh".into());
    };
    let Some(target) = save_dialog(loaded, "obj") else {
        return Ok("cancelled".into());
    };
    let leaf = loaded.path.rsplit('.').next().unwrap_or("mesh");
    std::fs::write(&target, mesh.to_obj(leaf)).map_err(|error| error.to_string())?;
    Ok(format!("wrote {}", target.display()))
}

fn export_glb(loaded: &Loaded, paths: &Paths) -> Result<String, String> {
    let Preview::Mesh { mesh, texture, .. } = &loaded.preview else {
        return Err("not a mesh".into());
    };
    let Some(target) = save_dialog(loaded, "glb") else {
        return Ok("cancelled".into());
    };
    let leaf = loaded.path.rsplit('.').next().unwrap_or("mesh");
    let _ = texture;
    let materials = resolve_materials(loaded, mesh, leaf, &paths.cooked());
    let diffuse = materials.iter().filter(|m| m.diffuse.is_some()).count();
    let normal = materials.iter().filter(|m| m.normal.is_some()).count();
    let glb = tera_package::write_glb_multi(mesh, leaf, &materials);
    std::fs::write(&target, glb).map_err(|error| error.to_string())?;
    Ok(format!(
        "wrote {} ({} matériaux, {diffuse} diffuse, {normal} normal)",
        target.display(),
        materials.len()
    ))
}

fn export_glb_anim(loaded: &Loaded, paths: &Paths) -> Result<String, String> {
    let Preview::Mesh { mesh, .. } = &loaded.preview else {
        return Err("not a mesh".into());
    };
    let Some(skin) = &mesh.skin else {
        return Err("mesh has no skeleton to animate".into());
    };
    let Some(target) = save_dialog(loaded, "glb") else {
        return Ok("cancelled".into());
    };
    let leaf = loaded.path.rsplit('.').next().unwrap_or("mesh");
    let materials = resolve_materials(loaded, mesh, leaf, &paths.cooked());
    let owned: std::collections::HashSet<String> = skin.bones.iter().map(|bone| bone.name.clone()).collect();
    let skeleton = owned.len().max(1);
    let index = Index::open(&paths.index).ok().map(Arc::new);
    let mut animations = find_animations(index.as_ref(), &paths.cooked(), leaf, &owned, skeleton);
    if animations.is_empty() {
        let borrowed: std::collections::HashSet<&str> = owned.iter().map(String::as_str).collect();
        animations = collect_animations(&loaded.file, &borrowed, skeleton, 60);
    }
    let glb = tera_package::write_glb_animated(mesh, leaf, &materials, &animations);
    std::fs::write(&target, glb).map_err(|error| error.to_string())?;
    let diffuse = materials.iter().filter(|m| m.diffuse.is_some()).count();
    let normal = materials.iter().filter(|m| m.normal.is_some()).count();
    let emissive = materials.iter().filter(|m| m.emissive.is_some()).count();
    Ok(format!(
        "wrote {} — {} matériaux ({diffuse} diff, {normal} norm, {emissive} emis), {} clips",
        target.display(),
        materials.len(),
        animations.len()
    ))
}

fn anim_stem(mesh_leaf: &str) -> String {
    let lower = mesh_leaf.to_ascii_lowercase();
    let base = match lower.find("_skel") {
        Some(position) => lower[..position].to_string(),
        None => lower,
    };
    let base = base.trim_end_matches("_dup").to_string();
    match base.rsplit_once('_') {
        Some((head, tail)) if !tail.is_empty() && tail.len() <= 3 && head.len() >= 4 => head.to_string(),
        _ => base,
    }
}

fn find_animations(
    index: Option<&Arc<Index>>,
    cooked: &Path,
    mesh_leaf: &str,
    bones: &std::collections::HashSet<String>,
    skeleton: usize,
) -> Vec<tera_package::Animation> {
    let Some(index) = index else {
        return Vec::new();
    };
    let stem = anim_stem(mesh_leaf);
    if stem.len() < 3 {
        return Vec::new();
    }
    let hits = index.search_objects(&stem, 8000, Some("AnimSet"));
    let mut visited: std::collections::HashSet<(u32, u64)> = std::collections::HashSet::new();
    let mut scored: Vec<(usize, tera_package::Animation)> = Vec::new();
    for hit in hits {
        if visited.len() > 200 {
            break;
        }
        let object = index.object(hit as usize);
        let entry = index.package(object.package as usize);
        if !visited.insert((entry.file, entry.offset)) {
            continue;
        }
        let file = cooked.join(index.file_name(entry.file as usize));
        let Ok(handle) = std::fs::File::open(&file) else {
            continue;
        };
        let Ok(map) = (unsafe { Mmap::map(&handle) }) else {
            continue;
        };
        let Ok(package) = Package::parse(&map, entry.offset as usize) else {
            continue;
        };
        for animation in tera_package::animations(&package) {
            let overlap = animation.tracks.iter().filter(|track| bones.contains(track.bone.as_str())).count();
            if overlap * 100 >= skeleton * 30 {
                scored.push((overlap, animation));
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen = std::collections::HashSet::new();
    scored
        .into_iter()
        .filter(|(_, animation)| seen.insert(animation.name.to_ascii_lowercase()))
        .take(60)
        .map(|(_, animation)| animation)
        .collect()
}

fn collect_animations(
    file: &Path,
    bones: &std::collections::HashSet<&str>,
    skeleton: usize,
    limit: usize,
) -> Vec<tera_package::Animation> {
    let Ok(handle) = std::fs::File::open(file) else {
        return Vec::new();
    };
    let Ok(map) = (unsafe { Mmap::map(&handle) }) else {
        return Vec::new();
    };
    let mut scored: Vec<(usize, tera_package::Animation)> = Vec::new();
    for package in Bundle::new(&map).flatten() {
        for animation in tera_package::animations(&package) {
            let hit = animation.tracks.iter().filter(|track| bones.contains(track.bone.as_str())).count();
            if hit * 100 >= skeleton * 40 {
                scored.push((hit, animation));
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen = std::collections::HashSet::new();
    scored
        .into_iter()
        .filter(|(_, animation)| seen.insert(animation.name.to_ascii_lowercase()))
        .take(limit)
        .map(|(_, animation)| animation)
        .collect()
}

fn resolve_materials(
    loaded: &Loaded,
    mesh: &Mesh,
    leaf: &str,
    cooked: &Path,
) -> Vec<tera_package::MaterialInput> {
    let Ok(handle) = std::fs::File::open(&loaded.file) else {
        return vec![tera_package::MaterialInput::default()];
    };
    let Ok(map) = (unsafe { Mmap::map(&handle) }) else {
        return vec![tera_package::MaterialInput::default()];
    };
    let Ok(mut package) = Package::parse(&map, loaded.package_offset as usize) else {
        return vec![tera_package::MaterialInput::default()];
    };
    package.name_hint = Some(loaded.package.clone());
    tera_package::mesh_materials_or_diffuse(&package, mesh, leaf, cooked)
}


fn export_raw(loaded: &Loaded, _paths: &Paths) -> Result<String, String> {
    let Some(target) = save_dialog(loaded, "bin") else {
        return Ok("cancelled".into());
    };
    let bytes = with_export(loaded, |package, export| {
        package
            .export_data(export)
            .map(|data| data.to_vec())
            .map_err(|error| error.to_string())
    })?;
    std::fs::write(&target, bytes).map_err(|error| error.to_string())?;
    Ok(format!("wrote {}", target.display()))
}

fn with_export<T>(
    loaded: &Loaded,
    action: impl FnOnce(&Package<'_>, &tera_package::Export) -> Result<T, String>,
) -> Result<T, String> {
    let handle = std::fs::File::open(&loaded.file).map_err(|error| error.to_string())?;
    let map = unsafe { Mmap::map(&handle) }.map_err(|error| error.to_string())?;
    let package = Package::parse(&map, loaded.package_offset as usize)
        .map_err(|error| error.to_string())?;
    let export = package
        .exports
        .get(loaded.export)
        .ok_or_else(|| "export not found".to_string())?;
    action(&package, export)
}

fn load_image(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    if bytes.starts_with(b"DDS ") {
        let dds = tera_package::Dds::parse(&bytes).map_err(|error| error.to_string())?;
        let format = match dds.four_cc {
            Some(code) => match &code {
                b"DXT1" => tera_package::BlockFormat::Bc1,
                b"DXT3" => tera_package::BlockFormat::Bc2,
                b"DXT5" => tera_package::BlockFormat::Bc3,
                _ => return Err("unsupported dds format".into()),
            },
            None => return Err("uncompressed dds is not handled here".into()),
        };
        let rgba = tera_package::decode_blocks(
            format,
            dds.mips.first().ok_or("empty dds")?,
            dds.width as usize,
            dds.height as usize,
        )
        .ok_or("dds decode failed")?;
        return Ok((dds.width, dds.height, rgba));
    }
    let image = tera_package::png::decode(&bytes).map_err(|error| error.to_string())?;
    Ok((image.width, image.height, image.rgba))
}

fn replace_texture(loaded: &Loaded) -> Result<String, String> {
    let Preview::Texture {
        width,
        height,
        format,
        ..
    } = &loaded.preview
    else {
        return Err("not a texture".into());
    };
    let Some(source) = rfd::FileDialog::new()
        .add_filter("image", &["png", "dds"])
        .pick_file()
    else {
        return Ok("cancelled".into());
    };
    let (image_width, image_height, rgba) = load_image(&source)?;
    if image_width != *width || image_height != *height {
        return Err(format!(
            "the image is {image_width}×{image_height}, the texture expects {width}×{height}"
        ));
    }
    let block = match format.as_str() {
        "PF_DXT1" => tera_package::BlockFormat::Bc1,
        "PF_DXT5" => tera_package::BlockFormat::Bc3,
        other => return Err(format!("replacement not handled for {other}")),
    };
    let payload = tera_package::encode_blocks(
        block,
        &rgba,
        image_width as usize,
        image_height as usize,
    );
    let Some(target) = rfd::FileDialog::new()
        .set_file_name(
            loaded
                .file
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "package.gpk".into()),
        )
        .save_file()
    else {
        return Ok("cancelled".into());
    };
    let handle = std::fs::File::open(&loaded.file).map_err(|error| error.to_string())?;
    let map = unsafe { Mmap::map(&handle) }.map_err(|error| error.to_string())?;
    let mut out = Vec::new();
    let mut replaced = false;
    for package in Bundle::new(&map) {
        let package = package.map_err(|error| error.to_string())?;
        let mut overrides = std::collections::BTreeMap::new();
        if package.base as u64 == loaded.package_offset {
            let export = package
                .exports
                .get(loaded.export)
                .ok_or_else(|| "export not found".to_string())?;
            let texture =
                Texture2D::parse(&package, export).map_err(|error| error.to_string())?;
            let blob = package
                .export_data(export)
                .map_err(|error| error.to_string())?;
            let mip = texture
                .largest_inline_mip()
                .ok_or_else(|| "no inline mip".to_string())?;
            if payload.len() != mip.data.size_on_disk.max(0) as usize {
                return Err(format!(
                    "the encoded texture is {} bytes, the original is {}",
                    payload.len(),
                    mip.data.size_on_disk
                ));
            }
            let mut patched = blob.to_vec();
            let start = mip.data.payload_offset;
            patched[start..start + payload.len()].copy_from_slice(&payload);
            overrides.insert(loaded.export, patched);
            replaced = true;
        }
        out.extend_from_slice(
            &tera_package::rebuild(&package, &overrides).map_err(|error| error.to_string())?,
        );
    }
    if !replaced {
        return Err("package not found in this file".into());
    }
    std::fs::write(&target, out).map_err(|error| error.to_string())?;
    Ok(format!("wrote {}", target.display()))
}

fn create_mod(
    loaded: &Loaded,
    paths: &Paths,
    package_name: &str,
    object_name: &str,
) -> Result<String, String> {
    let Preview::Texture { format, .. } = &loaded.preview else {
        return Err("not a texture".into());
    };
    let Some(source) = rfd::FileDialog::new()
        .add_filter("image", &["png", "dds"])
        .pick_file()
    else {
        return Ok("cancelled".into());
    };
    let (width, height, rgba) = load_image(&source)?;
    if !width.is_power_of_two() || !height.is_power_of_two() {
        return Err("dimensions must be powers of two".into());
    }
    let block = match format.as_str() {
        "PF_DXT1" => tera_package::BlockFormat::Bc1,
        _ => tera_package::BlockFormat::Bc3,
    };
    let payload =
        tera_package::encode_blocks(block, &rgba, width as usize, height as usize);
    let mut spec = tera_package::TextureSpec::new(package_name, object_name);
    spec.width = width;
    spec.height = height;
    spec.format = if block == tera_package::BlockFormat::Bc1 {
        "PF_DXT1".into()
    } else {
        "PF_DXT5".into()
    };
    spec.source_path = source.to_string_lossy().to_string();
    spec.mips = vec![payload];
    let bytes = tera_package::build_texture_package(&spec).map_err(|error| error.to_string())?;
    let default = paths.mods.join("gpk").join(format!("{object_name}.gpk"));
    let Some(target) = rfd::FileDialog::new()
        .set_file_name(
            default
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
        )
        .set_directory(default.parent().unwrap_or(Path::new(".")))
        .save_file()
    else {
        return Ok("cancelled".into());
    };
    std::fs::write(&target, bytes).map_err(|error| error.to_string())?;
    Ok(format!(
        "mod created: {} ({}×{})",
        target.display(),
        width,
        height
    ))
}
