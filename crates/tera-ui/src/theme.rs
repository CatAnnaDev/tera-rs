use egui::{Color32, CornerRadius, FontId, Margin, Stroke, TextStyle, Vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    Dark,
    Light,
}

impl Palette {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }
}

pub struct Colors {
    pub background: Color32,
    pub panel: Color32,
    pub raised: Color32,
    pub line: Color32,
    pub bone: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub accent_high: Color32,

}

pub const DARK: Colors = Colors {
    background: Color32::from_rgb(0x0e, 0x0f, 0x11),
    panel: Color32::from_rgb(0x16, 0x18, 0x1b),
    raised: Color32::from_rgb(0x20, 0x23, 0x28),
    line: Color32::from_rgb(0x2e, 0x32, 0x38),
    bone: Color32::from_rgb(0xe3, 0xe6, 0xec),
    dim: Color32::from_rgb(0x82, 0x8b, 0x96),
    accent: Color32::from_rgb(0xcf, 0x8a, 0x3c),
    accent_high: Color32::from_rgb(0xe7, 0xa9, 0x58),
};

pub const LIGHT: Colors = Colors {
    background: Color32::from_rgb(0xf4, 0xf5, 0xf7),
    panel: Color32::from_rgb(0xe9, 0xeb, 0xee),
    raised: Color32::from_rgb(0xdd, 0xe0, 0xe5),
    line: Color32::from_rgb(0xc3, 0xc8, 0xcf),
    bone: Color32::from_rgb(0x1a, 0x1d, 0x21),
    dim: Color32::from_rgb(0x5a, 0x62, 0x6c),
    accent: Color32::from_rgb(0xb2, 0x6c, 0x1c),
    accent_high: Color32::from_rgb(0x8f, 0x53, 0x0f),
};

pub fn colors(palette: Palette) -> &'static Colors {
    match palette {
        Palette::Dark => &DARK,
        Palette::Light => &LIGHT,
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let candidates = [
        ("wide_coverage", "/System/Library/Fonts/Supplemental/Arial Unicode.ttf"),
        ("korean", "/System/Library/Fonts/Supplemental/AppleGothic.ttf"),
        ("gothic", "/System/Library/Fonts/Supplemental/NotoSansGothic-Regular.ttf"),
        ("cjk", "/System/Library/Fonts/Supplemental/Songti.ttc"),
    ];
    let mut installed = Vec::new();
    for (key, path) in candidates {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        fonts
            .font_data
            .insert(key.to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        installed.push(key.to_owned());
    }
    if installed.is_empty() {
        return;
    }
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let entry = fonts.families.entry(family).or_default();
        for key in &installed {
            if !entry.contains(key) {
                entry.push(key.clone());
            }
        }
    }
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context, palette: Palette) {
    let c = colors(palette);
    let theme = match palette {
        Palette::Dark => egui::Theme::Dark,
        Palette::Light => egui::Theme::Light,
    };
    let mut style = (*ctx.style_of(theme)).clone();
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(18.0));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(13.5));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(13.0));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(12.0));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(11.0));

    let visuals = &mut style.visuals;
    visuals.dark_mode = palette == Palette::Dark;
    visuals.override_text_color = Some(c.bone);
    visuals.panel_fill = c.panel;
    visuals.window_fill = c.panel;
    visuals.extreme_bg_color = c.background;
    visuals.faint_bg_color = c.raised;
    visuals.hyperlink_color = c.accent_high;
    visuals.error_fg_color = Color32::from_rgb(0xc0, 0x50, 0x50);
    visuals.window_stroke = Stroke::new(1.0, c.line);
    visuals.selection.bg_fill = c.accent.gamma_multiply(0.55);
    visuals.selection.stroke = Stroke::new(1.0, c.accent_high);

    let radius = CornerRadius::same(5);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = radius;
        widget.fg_stroke = Stroke::new(1.0, c.bone);
        widget.bg_stroke = Stroke::new(1.0, c.line);
    }
    visuals.widgets.noninteractive.bg_fill = c.panel;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, c.dim);
    visuals.widgets.inactive.bg_fill = c.raised;
    visuals.widgets.inactive.weak_bg_fill = c.raised;
    visuals.widgets.hovered.bg_fill = c.line;
    visuals.widgets.hovered.weak_bg_fill = c.line;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, c.accent);
    visuals.widgets.active.bg_fill = c.accent;
    visuals.widgets.active.weak_bg_fill = c.accent;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, c.accent_high);

    style.spacing.item_spacing = Vec2::new(9.0, 7.0);
    style.spacing.button_padding = Vec2::new(11.0, 5.0);
    style.spacing.window_margin = Margin::same(10);
    style.spacing.menu_margin = Margin::same(7);
    style.spacing.indent = 16.0;
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(5);

    let style = std::sync::Arc::new(style);
    ctx.set_style_of(theme, style);
    ctx.set_theme(theme);
}

pub fn nav_item(ui: &mut egui::Ui, palette: Palette, label: &str, active: bool) -> bool {
    let c = colors(palette);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), egui::Sense::click());
    let painter = ui.painter();
    if active {
        painter.rect_filled(rect, CornerRadius::same(6), c.accent.gamma_multiply(0.16));
        let bar = egui::Rect::from_min_max(
            rect.left_top() + Vec2::new(0.0, 5.0),
            egui::pos2(rect.left() + 3.0, rect.bottom() - 5.0),
        );
        painter.rect_filled(bar, CornerRadius::same(2), c.accent_high);
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), c.raised);
    }
    let color = if active {
        c.bone
    } else if response.hovered() {
        c.bone
    } else {
        c.dim
    };
    painter.text(
        rect.left_center() + Vec2::new(16.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.5),
        color,
    );
    response.clicked()
}

pub fn nav_group(ui: &mut egui::Ui, palette: Palette, text: &str) {
    ui.add_space(10.0);
    ui.add(egui::Label::new(
        egui::RichText::new(text)
            .size(10.0)
            .color(colors(palette).dim)
            .extra_letter_spacing(2.0),
    ));
    ui.add_space(2.0);
}

pub fn eyebrow(ui: &mut egui::Ui, palette: Palette, text: impl Into<String>) {
    ui.add(egui::Label::new(
        egui::RichText::new(text.into())
            .size(11.0)
            .color(colors(palette).dim),
    ));
}

pub fn display(palette: Palette, text: impl Into<String>, size: f32) -> egui::RichText {
    egui::RichText::new(text.into())
        .size(size.min(18.0))
        .strong()
        .color(colors(palette).bone)
}

pub fn rule(ui: &mut egui::Ui, palette: Palette) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, colors(palette).line),
    );
}

pub fn checkerboard(painter: &egui::Painter, rect: egui::Rect, cell: f32, palette: Palette) {
    let (light, dark) = match palette {
        Palette::Dark => (
            Color32::from_rgb(0x2a, 0x2d, 0x31),
            Color32::from_rgb(0x1e, 0x20, 0x23),
        ),
        Palette::Light => (
            Color32::from_rgb(0xff, 0xff, 0xff),
            Color32::from_rgb(0xd8, 0xdc, 0xe1),
        ),
    };
    painter.rect_filled(rect, 0.0, dark);
    let mut y = rect.top();
    let mut row = 0;
    while y < rect.bottom() {
        let mut x = rect.left();
        let mut column = 0;
        while x < rect.right() {
            if (row + column) % 2 == 0 {
                let cell_rect = egui::Rect::from_min_max(
                    egui::pos2(x, y),
                    egui::pos2((x + cell).min(rect.right()), (y + cell).min(rect.bottom())),
                );
                painter.rect_filled(cell_rect, 0.0, light);
            }
            x += cell;
            column += 1;
        }
        y += cell;
        row += 1;
    }
}

pub fn class_color(class: &str) -> Color32 {
    let mut hash: u32 = 2166136261;
    for byte in class.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(16777619);
    }
    let hue = (hash % 360) as f32 / 360.0;
    let (red, green, blue) = hsv(hue, 0.30, 0.78);
    Color32::from_rgb(red, green, blue)
}

fn hsv(hue: f32, saturation: f32, value: f32) -> (u8, u8, u8) {
    let sector = (hue * 6.0).floor();
    let fraction = hue * 6.0 - sector;
    let p = value * (1.0 - saturation);
    let q = value * (1.0 - fraction * saturation);
    let t = value * (1.0 - (1.0 - fraction) * saturation);
    let (red, green, blue) = match (sector as i32) % 6 {
        0 => (value, t, p),
        1 => (q, value, p),
        2 => (p, value, t),
        3 => (p, q, value),
        4 => (t, p, value),
        _ => (value, p, q),
    };
    (
        (red * 255.0) as u8,
        (green * 255.0) as u8,
        (blue * 255.0) as u8,
    )
}
