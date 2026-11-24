mod assets;
mod composite;
mod data;
mod gfxtab;
mod jobs;
mod keys;
mod maps;
mod materials;
mod properties;
mod mods;
mod skinning;
mod theme;
mod view3d;

use eframe::egui;
use std::path::PathBuf;
use theme::Palette;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Maps,
    Assets,
    DataCenter,
    Mods,
    Composite,
    Materials,
    Properties,
    Gfx,
    Keys,
}

type NavItem = (Tab, &'static str, &'static str);

const NAV_GROUPS: &[(&str, &[NavItem])] = &[
    (
        "EXPLORE",
        &[
            (Tab::Assets, "Assets", "Browse and edit the client's packages"),
            (Tab::Maps, "Maps", "Levels and actor placement"),
            (Tab::DataCenter, "DataCenter", "Structured game data"),
        ],
    ),
    (
        "CREATE",
        &[
            (Tab::Materials, "Materials", "Recolor and tweak materials"),
            (Tab::Properties, "Properties", "Edit a package's properties"),
            (Tab::Composite, "Composite", "Build a composite mod"),
            (Tab::Mods, "Mods", "Install and manage mods"),
        ],
    ),
    (
        "TOOLS",
        &[
            (Tab::Gfx, "Interfaces", "Scaleform GFx and HUD"),
            (Tab::Keys, "Keys", "Client decryption keys"),
        ],
    ),
];

fn tab_meta(tab: Tab) -> (&'static str, &'static str) {
    for (_, items) in NAV_GROUPS {
        for (candidate, label, subtitle) in *items {
            if *candidate == tab {
                return (label, subtitle);
            }
        }
    }
    ("TERA Studio", "")
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
            .join("Library/Application Support/CrossOver/Bottles/tera/drive_c");
        let game = bottle.join("Games/TERA Asura/TERA");
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

    pub fn library(&self) -> PathBuf {
        self.game.join("S1Game")
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
    composite: composite::CompositeTab,
    materials: materials::MaterialsTab,
    properties: properties::PropertiesTab,
    gfx: gfxtab::GfxTab,
    keys: keys::KeysTab,
    maps: maps::MapsTab,
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
                        "maps" => Tab::Maps,
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
            composite: composite::CompositeTab::default(),
            materials: materials::MaterialsTab::default(),
            properties: properties::PropertiesTab::default(),
            gfx: gfxtab::GfxTab::default(),
            keys: keys::KeysTab::default(),
            maps: maps::MapsTab::default(),
            paths,
        }
    }
}

impl eframe::App for Studio {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        let palette = self.palette;
        let colors = theme::colors(palette);

        let flat_tabs: Vec<Tab> = NAV_GROUPS
            .iter()
            .flat_map(|(_, items)| items.iter().map(|(tab, _, _)| *tab))
            .collect();
        let shortcut = ui.input(|input| {
            if !input.modifiers.command {
                return None;
            }
            for (slot, key) in [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
                egui::Key::Num8,
                egui::Key::Num9,
            ]
            .into_iter()
            .enumerate()
            {
                if input.key_pressed(key) {
                    return Some(slot);
                }
            }
            None
        });
        if let Some(slot) = shortcut {
            if let Some(tab) = flat_tabs.get(slot) {
                self.tab = *tab;
            }
        }

        egui::Panel::left("nav")
            .exact_size(204.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(colors.background).inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 14,
                bottom: 12,
            }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(
                        egui::RichText::new("TERA")
                            .size(19.0)
                            .strong()
                            .color(colors.accent_high),
                    ));
                    ui.add(egui::Label::new(
                        egui::RichText::new("STUDIO")
                            .size(19.0)
                            .strong()
                            .color(colors.bone),
                    ));
                });
                ui.add(egui::Label::new(
                    egui::RichText::new("TERA asset workshop")
                        .size(10.5)
                        .color(colors.dim),
                ));
                ui.add_space(8.0);

                for (group, items) in NAV_GROUPS {
                    theme::nav_group(ui, palette, group);
                    for (tab, label, _) in *items {
                        if theme::nav_item(ui, palette, label, self.tab == *tab) {
                            self.tab = *tab;
                        }
                    }
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(2.0);
                    ui.add(egui::Label::new(
                        egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .size(10.0)
                            .color(colors.dim),
                    ));
                    ui.add_space(6.0);
                    let next = self.palette.toggled();
                    if ui
                        .add_sized(
                            [ui.available_width(), 28.0],
                            egui::Button::new(format!("theme · {}", self.palette.label())),
                        )
                        .clicked()
                    {
                        self.palette = next;
                        theme::apply(&context, self.palette);
                    }
                });
            });

        let (title, subtitle) = tab_meta(self.tab);
        egui::Panel::top("header")
            .frame(egui::Frame::new().fill(colors.panel).inner_margin(egui::Margin {
                left: 20,
                right: 16,
                top: 10,
                bottom: 10,
            }))
            .show(ui, |ui| {
                ui.label(theme::display(palette, title, 18.0));
                ui.add(egui::Label::new(
                    egui::RichText::new(subtitle).size(11.5).color(colors.dim),
                ));
            });

        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(colors.panel).inner_margin(egui::Margin {
                left: 20,
                right: 16,
                top: 5,
                bottom: 5,
            }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let center = ui.cursor().left_center() + egui::vec2(3.0, 0.0);
                    ui.painter().circle_filled(center, 3.0, colors.accent_high);
                    ui.add_space(12.0);
                    theme::eyebrow(ui, palette, &self.status);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::eyebrow(ui, palette, title.to_ascii_lowercase());
                    });
                });
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
            Tab::Composite => self
                .composite
                .ui(ui, self.palette, &self.paths, &mut self.status),
            Tab::Materials => self
                .materials
                .ui(ui, self.palette, &self.paths, &mut self.status),
            Tab::Properties => self
                .properties
                .ui(ui, self.palette, &self.paths, &mut self.status),
            Tab::Gfx => self
                .gfx
                .ui(ui, self.palette, &self.paths, &mut self.status),
            Tab::Keys => self
                .keys
                .ui(ui, self.palette, &self.paths, &mut self.status),
            Tab::Maps => self
                .maps
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
