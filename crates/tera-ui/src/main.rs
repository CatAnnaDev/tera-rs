mod assets;
mod data;
mod jobs;
mod keys;
mod mods;
mod skinning;
mod theme;
mod view3d;

use eframe::egui;
use std::path::PathBuf;
use theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Assets,
    DataCenter,
    Mods,
    Keys,
}

pub struct Paths {
    pub game: PathBuf,
    pub index: PathBuf,
    pub datacenter: PathBuf,
    pub mods: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        let bottle = PathBuf::from(&home)
            .join("Library/Application Support/CrossOver/Bottles/Tera/drive_c");
        let game = bottle.join("Games/TERA Europe Classic");
        Self {
            index: PathBuf::from(&home).join(".tera-studio/assets.idx"),
            datacenter: game.join("S1Game/S1Data/DataCenter_Final_EUR.dat"),
            mods: bottle.join("users/crossover/AppData/Roaming/Crazy-eSports-ClassicPlus/mods"),
            game,
        }
    }
}

impl Paths {
    pub fn cooked(&self) -> PathBuf {
        self.game.join("S1Game/CookedPC")
    }

    pub fn mod_library(&self) -> PathBuf {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".tera-studio/mods")
    }

    pub fn pkg_mapper(&self) -> PathBuf {
        self.game.join("S1Game/PkgMapper.re")
    }
}

pub struct Studio {
    tab: Tab,
    palette: Palette,
    paths: Paths,
    status: String,
    assets: assets::Assets,
    data: data::DataTab,
    mods: mods::ModsTab,
    keys: keys::KeysTab,
}

impl Studio {
    fn new(context: &egui::Context) -> Self {
        theme::install_fonts(context);
        theme::apply(context, Palette::Dark);
        let paths = Paths::default();
        let arguments: Vec<String> = std::env::args().collect();
        let mut tab = Tab::Assets;
        let mut datacenter = None;
        let mut datacenter_query = None;
        for pair in arguments.windows(2) {
            match pair[0].as_str() {
                "--tab" => {
                    tab = match pair[1].as_str() {
                        "datacenter" => Tab::DataCenter,
                        "mods" => Tab::Mods,
                        "keys" => Tab::Keys,
                        _ => Tab::Assets,
                    }
                }
                "--dc" => datacenter = Some(PathBuf::from(&pair[1])),
                "--dc-query" => datacenter_query = Some(pair[1].clone()),
                _ => {}
            }
        }
        if datacenter_query.is_some() && datacenter.is_none() {
            datacenter = Some(paths.datacenter.clone());
        }
        let mut data = data::DataTab::default();
        if datacenter.is_some() {
            data.request(datacenter, datacenter_query);
            tab = Tab::DataCenter;
        }
        Self {
            tab,
            palette: Palette::Dark,
            status: "ready".into(),
            assets: assets::Assets::new(&paths),
            data,
            mods: mods::ModsTab::default(),
            keys: keys::KeysTab::default(),
            paths,
        }
    }
}

impl eframe::App for Studio {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(theme::display(self.palette, "TERA Studio", 16.0));
                ui.add_space(14.0);
                for (tab, label) in [
                    (Tab::Assets, "assets"),
                    (Tab::DataCenter, "datacenter"),
                    (Tab::Mods, "mods"),
                    (Tab::Keys, "keys"),
                ] {
                    if ui.selectable_label(self.tab == tab, label).clicked() {
                        self.tab = tab;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let next = self.palette.toggled();
                    if ui.button(format!("theme · {}", self.palette.label())).clicked() {
                        self.palette = next;
                        theme::apply(&context, self.palette);
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                theme::eyebrow(ui, self.palette, &self.status);
            });
            ui.add_space(3.0);
        });

        self.mods.tick(&context, &self.paths, &mut self.status);

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Assets => self
                .assets
                .ui(ui, &context, self.palette, &self.paths, &mut self.status),
            Tab::DataCenter => self
                .data
                .ui(ui, self.palette, &self.paths, &mut self.status),
            Tab::Mods => self
                .mods
                .ui(ui, self.palette, &self.paths, &mut self.status),
            Tab::Keys => self
                .keys
                .ui(ui, self.palette, &self.paths, &mut self.status),
        });
        if let Some(name) = self.data.take_asset_request() {
            self.assets.request(name);
            self.tab = Tab::Assets;
        }
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_title("TERA Studio"),
        ..Default::default()
    };
    eframe::run_native(
        "TERA Studio",
        options,
        Box::new(|context| Ok(Box::new(Studio::new(&context.egui_ctx)))),
    )
}
