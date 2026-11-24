use crate::jobs::Job;
use crate::theme::{self, Palette};
use crate::Paths;
use eframe::egui;
use std::path::PathBuf;
use tera_package::{movies, replace_movie};

struct MovieRow {
    path: String,
    kind: String,
    extension: String,
    size: usize,
    source_file: String,
    data: Vec<u8>,
}

#[derive(Default)]
pub struct GfxTab {
    file: Option<PathBuf>,
    movies: Vec<MovieRow>,
    error: Option<String>,
    selected: Option<usize>,
    query: String,
    job: Option<Job>,
}

impl GfxTab {
    fn open(&mut self, file: PathBuf) {
        self.selected = None;
        self.movies.clear();
        let bytes = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.error = Some(format!("{}: {error}", file.display()));
                self.file = None;
                return;
            }
        };
        for movie in movies(&bytes) {
            self.movies.push(MovieRow {
                path: movie.path.clone(),
                kind: movie.kind().to_string(),
                extension: movie.extension().to_string(),
                size: movie.data.len(),
                source_file: movie.source_file.clone(),
                data: movie.data,
            });
        }
        self.error = if self.movies.is_empty() {
            Some("no GFxMovieInfo in this file".into())
        } else {
            None
        };
        self.file = Some(file);
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
        ui.label(theme::display(palette, "GFx", 22.0));
        theme::eyebrow(
            ui,
            palette,
            "extract and replace a .gpk's Scaleform movies (UI, chat balloon, HUD)",
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

        ui.horizontal(|ui| {
            ui.label(format!("{} movie(s)", self.movies.len()));
            ui.add_space(12.0);
            ui.label("filter");
            ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(220.0));
        });
        ui.add_space(4.0);

        let needle = self.query.trim().to_ascii_lowercase();
        ui.columns(2, |columns| {
            self.list(&mut columns[0], palette, &needle);
            self.detail(&mut columns[1], palette, running, status);
        });
    }

    fn list(&mut self, ui: &mut egui::Ui, palette: Palette, needle: &str) {
        ui.label(theme::display(palette, "movies", 15.0));
        egui::ScrollArea::vertical()
            .id_salt("gfx-list")
            .max_height(520.0)
            .show(ui, |ui| {
                let mut pick = None;
                for index in 0..self.movies.len() {
                    let movie = &self.movies[index];
                    if !needle.is_empty() && !movie.path.to_ascii_lowercase().contains(needle) {
                        continue;
                    }
                    let leaf = movie.path.rsplit('.').next().unwrap_or(&movie.path);
                    let label = format!("{leaf}  [{}] {} o", movie.kind, movie.size);
                    if ui.selectable_label(self.selected == Some(index), label).clicked() {
                        pick = Some(index);
                    }
                }
                if pick.is_some() {
                    self.selected = pick;
                }
            });
    }

    fn detail(&mut self, ui: &mut egui::Ui, palette: Palette, running: bool, status: &mut String) {
        let Some(index) = self.selected else {
            ui.label(theme::display(palette, "pick a movie", 15.0));
            return;
        };
        let (path, kind, extension, size, source_file, data) = {
            let movie = &self.movies[index];
            (
                movie.path.clone(),
                movie.kind.clone(),
                movie.extension.clone(),
                movie.size,
                movie.source_file.clone(),
                movie.data.clone(),
            )
        };
        ui.label(theme::display(palette, "movie", 15.0));
        theme::eyebrow(ui, palette, &path);
        ui.label(
            egui::RichText::new(format!("{kind} · {size} bytes"))
                .color(theme::colors(palette).dim),
        );
        if !source_file.is_empty() {
            ui.label(
                egui::RichText::new(format!("source {source_file}"))
                    .color(theme::colors(palette).dim),
            );
        }
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            if ui.button("extract…").clicked() {
                let leaf = path.rsplit(['.', '/', '\\', ':']).next().unwrap_or("movie");
                if let Some(target) = rfd::FileDialog::new()
                    .set_file_name(format!("{leaf}.{extension}"))
                    .save_file()
                {
                    match std::fs::write(&target, &data) {
                        Ok(()) => *status = format!("extrait {}", target.display()),
                        Err(error) => *status = format!("error: {error}"),
                    }
                }
            }
            if ui
                .add_enabled(!running, egui::Button::new("replace with a file…"))
                .clicked()
            {
                if let Some(movie_file) = rfd::FileDialog::new()
                    .add_filter("movie", &["gfx", "swf"])
                    .pick_file()
                {
                    if let Some(source) = self.file.clone() {
                        self.spawn_replace(source, path.clone(), movie_file, status);
                    }
                }
            }
        });
    }

    fn spawn_replace(
        &mut self,
        source: PathBuf,
        object: String,
        movie_file: PathBuf,
        status: &mut String,
    ) {
        *status = "replacing…".into();
        self.job = Some(Job::spawn("replace movie", move |_| {
            let bytes = std::fs::read(&source).map_err(|error| error.to_string())?;
            let movie = std::fs::read(&movie_file).map_err(|error| error.to_string())?;
            let replaced =
                replace_movie(&bytes, &object, &movie).map_err(|error| error.to_string())?;
            let mut clean = source.clone().into_os_string();
            clean.push(".clean");
            let clean = PathBuf::from(clean);
            if !clean.exists() {
                std::fs::copy(&source, &clean).map_err(|error| error.to_string())?;
            }
            std::fs::write(&source, &replaced.bytes).map_err(|error| error.to_string())?;
            Ok(format!(
                "replaced {} from {}",
                object.rsplit('.').next().unwrap_or(&object),
                movie_file
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            ))
        }));
    }
}
