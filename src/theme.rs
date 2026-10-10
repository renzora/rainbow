//! Rainbow's look: colour themes, rounded widgets, roomier spacing, and a few custom widgets.

use crate::n;
use std::sync::RwLock;

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

/// Bundled fonts that are also used outside the UI (e.g. for saving path charts as images).
pub const NOTO_SANS: &[u8] = include_bytes!("../assets/fonts/NotoSans.ttf");
pub const NOTO_EMOJI: &[u8] = include_bytes!("../assets/fonts/NotoEmoji.ttf");
pub const NOTO_SANS_JP: &[u8] = include_bytes!("../assets/fonts/NotoSansJP.ttf");

const GREEN: Color32 = Color32::from_rgb(74, 201, 120);
const RED: Color32 = Color32::from_rgb(235, 96, 96);
const AMBER: Color32 = Color32::from_rgb(240, 180, 70);

/// Colour-vision modes. They swap the status and difficulty colours for ones that stay
/// distinguishable (based on the Okabe-Ito palette); icons and numbers carry the meaning too.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub enum ColorBlind {
    #[default]
    Off,
    /// Protanopia / deuteranopia (the common red-green kinds).
    RedGreen,
    /// Tritanopia.
    BlueYellow,
}

impl ColorBlind {
    pub const ALL: [ColorBlind; 3] = [ColorBlind::Off, ColorBlind::RedGreen, ColorBlind::BlueYellow];

    pub fn name(self) -> &'static str {
        match self {
            ColorBlind::Off => "Off",
            ColorBlind::RedGreen => "Red-green",
            ColorBlind::BlueYellow => "Blue-yellow",
        }
    }
}

static COLOR_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn color_mode() -> ColorBlind {
    match COLOR_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => ColorBlind::RedGreen,
        2 => ColorBlind::BlueYellow,
        _ => ColorBlind::Off,
    }
}

/// "Good" status colour (downloaded, must-have).
pub fn good() -> Color32 {
    match color_mode() {
        ColorBlind::Off => GREEN,
        ColorBlind::RedGreen => rgb(86, 180, 233),
        ColorBlind::BlueYellow => rgb(42, 183, 202),
    }
}

/// "Bad" status colour (broken, failed, must-not-have).
pub fn bad() -> Color32 {
    match color_mode() {
        ColorBlind::Off => RED,
        ColorBlind::RedGreen => rgb(230, 159, 0),
        ColorBlind::BlueYellow => rgb(232, 80, 91),
    }
}

/// Warning colour (chart issues, official games, notices).
pub fn warn() -> Color32 {
    match color_mode() {
        ColorBlind::Off => AMBER,
        ColorBlind::RedGreen => rgb(240, 228, 66),
        ColorBlind::BlueYellow => rgb(242, 167, 195),
    }
}

/// Colour for a difficulty (0 easy .. 6 hardest).
pub fn difficulty_color(d: i8) -> Color32 {
    let i = d.clamp(0, 6) as usize;
    match color_mode() {
        ColorBlind::Off => RAINBOW[[3, 3, 2, 2, 1, 0, 0][i]],
        ColorBlind::RedGreen => [rgb(86, 180, 233), rgb(86, 180, 233), rgb(240, 228, 66), rgb(240, 228, 66), rgb(230, 159, 0), rgb(213, 94, 0), rgb(213, 94, 0)][i],
        ColorBlind::BlueYellow => [rgb(127, 211, 224), rgb(127, 211, 224), rgb(242, 167, 195), rgb(242, 167, 195), rgb(232, 80, 91), rgb(179, 0, 27), rgb(179, 0, 27)][i],
    }
}

/// Text colour that reads well on the accent colour (black on bright accents).
pub fn on_accent() -> Color32 {
    text_on(accent())
}

/// Black or white, whichever reads better on `fill`.
pub fn text_on(fill: Color32) -> Color32 {
    let luma = 0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
    if luma > 160.0 { Color32::BLACK } else { Color32::WHITE }
}

/// Settings that affect the look beyond the colour theme.
#[derive(Clone, Copy)]
pub struct Access {
    pub color_blind: ColorBlind,
    pub reduce_motion: bool,
}

/// Interface fonts, all bundled (SIL Open Font License).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub enum UiFont {
    /// Rainbow's default: a rounded, friendly face based on Doppio One.
    #[default]
    Rounded,
    NotoSans,
    /// Designed for low-vision readers.
    Atkinson,
    /// Designed to reduce visual stress and improve reading speed.
    Lexend,
    /// Weighted letter shapes that some dyslexic readers find easier.
    OpenDyslexic,
}

impl UiFont {
    pub const ALL: [UiFont; 5] = [UiFont::Rounded, UiFont::NotoSans, UiFont::Atkinson, UiFont::Lexend, UiFont::OpenDyslexic];

    pub fn name(self) -> &'static str {
        match self {
            UiFont::Rounded => "Rainbow Rounded",
            UiFont::NotoSans => "Noto Sans",
            UiFont::Atkinson => "Atkinson Hyperlegible",
            UiFont::Lexend => "Lexend",
            UiFont::OpenDyslexic => "OpenDyslexic",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            UiFont::Rounded => "The default. Based on Doppio One by Sorkin Type.",
            UiFont::NotoSans => "Plain and neutral.",
            UiFont::Atkinson => "Made for low-vision readers: very distinct letters.",
            UiFont::Lexend => "Wider spacing to reduce visual stress.",
            UiFont::OpenDyslexic => "Heavier letter bottoms some dyslexic readers find easier.",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            UiFont::Rounded => "rainbow-rounded",
            UiFont::NotoSans => "noto-sans",
            UiFont::Atkinson => "atkinson",
            UiFont::Lexend => "lexend",
            UiFont::OpenDyslexic => "opendyslexic",
        }
    }
}

/// The logo's colours, for the title and difficulty dots.
pub const RAINBOW: [Color32; 7] = [
    Color32::from_rgb(255, 72, 72),
    Color32::from_rgb(255, 150, 50),
    Color32::from_rgb(255, 220, 60),
    Color32::from_rgb(80, 220, 100),
    Color32::from_rgb(60, 170, 255),
    Color32::from_rgb(100, 110, 255),
    Color32::from_rgb(190, 90, 255),
];

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub enum ThemeKind {
    #[default]
    Midnight,
    Daylight,
    Hotel,
    Synthwave,
    Ocean,
    Forest,
    Sunset,
    HighContrast,
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub panel: Color32,
    pub faint: Color32,
    pub raised: Color32,
    pub input: Color32,
    pub hover: Color32,
    pub active: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

const MIDNIGHT: Palette = Palette {
    dark: true,
    panel: rgb(19, 20, 29),
    faint: rgb(23, 24, 34),
    raised: rgb(28, 30, 42),
    input: rgb(10, 11, 16),
    hover: rgb(40, 43, 60),
    active: rgb(52, 56, 78),
    text: rgb(228, 230, 240),
    weak: rgb(140, 145, 165),
    accent: rgb(124, 108, 255),
    accent_hover: rgb(146, 133, 255),
};

impl ThemeKind {
    pub const ALL: [ThemeKind; 8] = [
        ThemeKind::Midnight,
        ThemeKind::Daylight,
        ThemeKind::Hotel,
        ThemeKind::Synthwave,
        ThemeKind::Ocean,
        ThemeKind::Forest,
        ThemeKind::Sunset,
        ThemeKind::HighContrast,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ThemeKind::Midnight => n("Midnight"),
            ThemeKind::Daylight => n("Daylight"),
            ThemeKind::Hotel => n("Hotel"),
            ThemeKind::Synthwave => n("Synthwave"),
            ThemeKind::Ocean => n("Ocean"),
            ThemeKind::Forest => n("Forest"),
            ThemeKind::Sunset => n("Sunset"),
            ThemeKind::HighContrast => n("High contrast"),
        }
    }

    pub fn palette(self) -> Palette {
        #[allow(clippy::too_many_arguments)]
        fn p(
            dark: bool,
            panel: Color32,
            faint: Color32,
            raised: Color32,
            input: Color32,
            hover: Color32,
            active: Color32,
            text: Color32,
            weak: Color32,
            accent: Color32,
            accent_hover: Color32,
        ) -> Palette {
            Palette { dark, panel, faint, raised, input, hover, active, text, weak, accent, accent_hover }
        }
        match self {
            ThemeKind::Midnight => MIDNIGHT,
            ThemeKind::Daylight => p(false, rgb(255, 255, 255), rgb(247, 247, 252), rgb(236, 237, 245), rgb(248, 248, 252), rgb(224, 226, 240), rgb(210, 212, 232), rgb(28, 30, 40), rgb(110, 114, 130), rgb(108, 92, 240), rgb(128, 113, 250)),
            // Retro light blue-grey with chunky blue accents, a nod to old hotel-game UIs.
            ThemeKind::Hotel => p(false, rgb(226, 236, 242), rgb(214, 228, 236), rgb(196, 214, 225), rgb(246, 250, 252), rgb(178, 203, 218), rgb(160, 190, 208), rgb(24, 40, 52), rgb(84, 106, 122), rgb(30, 120, 190), rgb(48, 146, 220)),
            ThemeKind::Synthwave => p(true, rgb(28, 16, 44), rgb(34, 20, 54), rgb(48, 28, 76), rgb(18, 10, 30), rgb(66, 38, 104), rgb(84, 48, 130), rgb(246, 234, 255), rgb(176, 150, 206), rgb(255, 62, 165), rgb(255, 110, 192)),
            ThemeKind::Ocean => p(true, rgb(14, 30, 46), rgb(17, 36, 55), rgb(24, 48, 72), rgb(8, 20, 32), rgb(32, 64, 94), rgb(42, 80, 116), rgb(226, 240, 248), rgb(130, 160, 182), rgb(34, 184, 207), rgb(72, 206, 226)),
            ThemeKind::Forest => p(true, rgb(18, 30, 22), rgb(22, 36, 27), rgb(30, 50, 37), rgb(10, 20, 14), rgb(40, 66, 49), rgb(52, 84, 63), rgb(228, 240, 230), rgb(138, 166, 146), rgb(76, 195, 107), rgb(108, 214, 136)),
            // Pure black and white with a bright yellow accent, for low vision.
            ThemeKind::HighContrast => p(true, rgb(0, 0, 0), rgb(10, 10, 10), rgb(32, 32, 32), rgb(0, 0, 0), rgb(60, 60, 60), rgb(85, 85, 85), rgb(255, 255, 255), rgb(215, 215, 215), rgb(255, 214, 0), rgb(255, 230, 90)),
            ThemeKind::Sunset => p(true, rgb(34, 22, 22), rgb(40, 26, 26), rgb(58, 36, 34), rgb(22, 14, 14), rgb(76, 46, 42), rgb(96, 58, 52), rgb(250, 236, 228), rgb(186, 152, 140), rgb(255, 138, 61), rgb(255, 166, 98)),
        }
    }
}

static CURRENT: RwLock<Palette> = RwLock::new(MIDNIGHT);

/// The active theme's colours.
pub fn palette() -> Palette {
    *CURRENT.read().unwrap()
}

pub fn accent() -> Color32 {
    palette().accent
}

pub fn accent_hover() -> Color32 {
    palette().accent_hover
}

pub fn apply(ctx: &egui::Context, kind: ThemeKind, zoom: f32, access: Access) {
    let pal = kind.palette();
    *CURRENT.write().unwrap() = pal;
    COLOR_MODE.store(access.color_blind as u8, std::sync::atomic::Ordering::Relaxed);
    let theme = if pal.dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.set_theme(theme);
    ctx.set_zoom_factor(zoom);
    ctx.style_mut_of(theme, |s| {
        style(s, &pal);
        // Long enough to see sections slide open and switches slide across.
        s.animation_time = 0.2;
        if access.reduce_motion {
            s.animation_time = 0.0;
            s.scroll_animation = egui::style::ScrollAnimation::none();
        }
    });
}

fn style(style: &mut egui::Style, pal: &Palette) {
    let r = CornerRadius::same(8);
    let mut v = if pal.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = pal.panel;
    v.window_fill = pal.panel;
    v.extreme_bg_color = pal.input;
    v.faint_bg_color = pal.faint;
    v.code_bg_color = pal.raised;
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = r;
    v.window_stroke = Stroke::new(1.0, pal.raised);
    v.hyperlink_color = if pal.dark { pal.accent_hover } else { pal.accent };
    v.selection.bg_fill = pal.accent.linear_multiply(if pal.dark { 0.55 } else { 0.35 });
    v.selection.stroke = Stroke::new(1.0, pal.accent_hover);
    v.override_text_color = Some(pal.text);
    v.weak_text_color = Some(pal.weak);

    for (w, fill) in [
        (&mut v.widgets.noninteractive, pal.panel),
        (&mut v.widgets.inactive, pal.raised),
        (&mut v.widgets.hovered, pal.hover),
        (&mut v.widgets.active, pal.active),
        (&mut v.widgets.open, pal.hover),
    ] {
        w.corner_radius = r;
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::NONE;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, pal.raised);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, pal.text);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, pal.accent.linear_multiply(0.6));
    v.widgets.active.bg_stroke = Stroke::new(1.0, pal.accent);
    // Show the pointing-hand cursor over every clickable widget.
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    style.visuals = v;

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    style.spacing.combo_width = 140.0;
    // Built-in scroll bars (where the custom one isn't used): thicker and always visible.
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    style.spacing.scroll.bar_width = 12.0;
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(22.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Small, FontId::proportional(11.5)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
}

/// Width reserved for [`scrollbar`].
pub const SCROLLBAR_W: f32 = 18.0;

/// A chunky, retro scroll bar: a sunken track and a bevelled gradient thumb with ridged grip
/// lines. Returns a new scroll offset if the user dragged the thumb or clicked the track.
pub fn scrollbar(ui: &mut egui::Ui, track: egui::Rect, id: egui::Id, offset: f32, content: f32, view: f32) -> Option<f32> {
    use egui::{Mesh, Shape, pos2, vec2};
    let pal = palette();
    let painter = ui.painter().clone();
    let max = (content - view).max(0.0);
    // Nothing to scroll (or no results): no bar at all.
    if max < 1.0 {
        return None;
    }

    // Sunken track: darker at the edges, lighter in the middle.
    let track_dark = pal.input;
    let track_light = pal.raised.lerp_to_gamma(pal.input, 0.4);
    let mut mesh = Mesh::default();
    let mid = track.center().x;
    for (x0, x1, c0, c1) in [(track.left(), mid, track_dark, track_light), (mid, track.right(), track_light, track_dark)] {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(pos2(x0, track.top()), c0);
        mesh.colored_vertex(pos2(x1, track.top()), c1);
        mesh.colored_vertex(pos2(x1, track.bottom()), c1);
        mesh.colored_vertex(pos2(x0, track.bottom()), c0);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(Shape::mesh(mesh));
    painter.rect_stroke(track, CornerRadius::same(3), Stroke::new(1.0, pal.raised), egui::StrokeKind::Inside);
    if max <= 0.5 {
        return None;
    }

    let inner = track.shrink(2.0);
    let thumb_h = (inner.height() * view / content).clamp(40.0_f32.min(inner.height()), inner.height());
    let travel = (inner.height() - thumb_h).max(1.0);
    let top = inner.top() + (offset / max).clamp(0.0, 1.0) * travel;
    let thumb = egui::Rect::from_min_size(pos2(inner.left(), top), vec2(inner.width(), thumb_h));

    let resp = ui.interact(track, id, egui::Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Scroll bar"));
    let grab_id = id.with("grab");
    let mut new_offset = None;
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.drag_started() {
            let grab = if thumb.contains(p) { p.y - thumb.top() } else { thumb_h / 2.0 };
            ui.data_mut(|d| d.insert_temp(grab_id, grab));
        }
        if resp.dragged() {
            let grab = ui.data(|d| d.get_temp::<f32>(grab_id)).unwrap_or(thumb_h / 2.0);
            new_offset = Some(((p.y - grab - inner.top()) / travel).clamp(0.0, 1.0) * max);
        } else if resp.clicked() {
            // Click on the track pages up or down.
            let page = view * 0.9;
            new_offset = Some(if p.y < thumb.top() { (offset - page).max(0.0) } else { (offset + page).min(max) });
        }
    }

    // Thumb: horizontal gradient from light to the accent, brighter while hovered or dragged.
    let lit = resp.hovered() || resp.dragged();
    let (light, dark) = if lit {
        (pal.accent_hover.lerp_to_gamma(Color32::WHITE, 0.35), pal.accent_hover)
    } else {
        (pal.accent_hover.lerp_to_gamma(Color32::WHITE, 0.2), pal.accent.lerp_to_gamma(Color32::BLACK, 0.15))
    };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(thumb.left_top(), light);
    mesh.colored_vertex(thumb.right_top(), dark);
    mesh.colored_vertex(thumb.right_bottom(), dark);
    mesh.colored_vertex(thumb.left_bottom(), light);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
    // Bevel: highlight on the top/left, shadow on the bottom/right, dark outline.
    let hi = Stroke::new(1.0, Color32::from_white_alpha(140));
    let lo = Stroke::new(1.0, Color32::from_black_alpha(110));
    let t = thumb.shrink(1.0);
    painter.line_segment([t.left_top(), t.right_top()], hi);
    painter.line_segment([t.left_top(), t.left_bottom()], hi);
    painter.line_segment([t.left_bottom(), t.right_bottom()], lo);
    painter.line_segment([t.right_top(), t.right_bottom()], lo);
    painter.rect_stroke(thumb, CornerRadius::ZERO, Stroke::new(1.0, pal.accent.lerp_to_gamma(Color32::BLACK, 0.45)), egui::StrokeKind::Inside);
    // Three embossed grip lines in the middle.
    if thumb_h > 24.0 {
        let c = thumb.center();
        let half = (thumb.width() * 0.28).max(3.0);
        for dy in [-4.0, 0.0, 4.0] {
            let y = c.y + dy;
            painter.line_segment([pos2(c.x - half, y - 0.5), pos2(c.x + half, y - 0.5)], lo);
            painter.line_segment([pos2(c.x - half, y + 0.5), pos2(c.x + half, y + 0.5)], hi);
        }
    }
    new_offset
}

/// A row of widgets: left to right, or right to left when the interface language is written
/// that way (Arabic). Use instead of `ui.horizontal`.
pub fn hrow<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    if crate::i18n::rtl() {
        // Like `ui.horizontal`: one row tall (it grows to fit), not the whole space left.
        let size = egui::vec2(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(size, egui::Layout::right_to_left(egui::Align::Center), add)
    } else {
        ui.horizontal(add)
    }
}

/// Like [`hrow`], wrapping onto more lines when it's too wide (`ui.horizontal_wrapped`).
pub fn hrow_wrapped<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    if crate::i18n::rtl() {
        let size = egui::vec2(ui.available_size_before_wrap().x, ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(size, egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true), add)
    } else {
        ui.horizontal_wrapped(add)
    }
}

/// A column of widgets lined up on the left, or on the right in a right-to-left language. Use
/// instead of `ui.vertical`.
pub fn vcol<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    let align = if crate::i18n::rtl() { egui::Align::Max } else { egui::Align::Min };
    ui.scope_builder(egui::UiBuilder::new().layout(egui::Layout::top_down(align)), add)
}

/// A progress bar that fills from the start of the line: egui's own, or in a right-to-left
/// language drawn here filling from the right. Built like `egui::ProgressBar`.
pub struct Bar {
    progress: f32,
    width: Option<f32>,
    height: Option<f32>,
    text: Option<String>,
    percent: bool,
    fill: Option<Color32>,
}

impl Bar {
    pub fn new(progress: f32) -> Self {
        Self { progress: progress.clamp(0.0, 1.0), width: None, height: None, text: None, percent: false, fill: None }
    }
    pub fn desired_width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }
    pub fn desired_height(mut self, h: f32) -> Self {
        self.height = Some(h);
        self
    }
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }
    pub fn show_percentage(mut self) -> Self {
        self.percent = true;
        self
    }
    pub fn fill(mut self, color: Color32) -> Self {
        self.fill = Some(color);
        self
    }
}

impl egui::Widget for Bar {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        if !crate::i18n::rtl() {
            let mut bar = egui::ProgressBar::new(self.progress);
            if let Some(w) = self.width {
                bar = bar.desired_width(w);
            }
            if let Some(h) = self.height {
                bar = bar.desired_height(h);
            }
            if let Some(text) = self.text {
                bar = bar.text(text);
            }
            if self.percent {
                bar = bar.show_percentage();
            }
            if let Some(c) = self.fill {
                bar = bar.fill(c);
            }
            return ui.add(bar);
        }
        let height = self.height.unwrap_or(ui.spacing().interact_size.y);
        let width = self.width.unwrap_or(ui.available_size_before_wrap().x);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let radius = CornerRadius::same((height / 2.0).min(255.0) as u8);
        let visuals = ui.visuals();
        ui.painter().rect_filled(rect, radius, visuals.extreme_bg_color);
        let filled = egui::Rect::from_min_max(egui::pos2(rect.right() - rect.width() * self.progress, rect.top()), rect.max);
        if self.progress > 0.0 {
            ui.painter().rect_filled(filled, radius, self.fill.unwrap_or(visuals.selection.bg_fill));
        }
        let text = self.text.or_else(|| self.percent.then(|| format!("{}%", (self.progress * 100.0).round())));
        if let Some(text) = text {
            // Right-aligned inside the bar, the way egui left-aligns it.
            let at = egui::pos2(rect.right() - 6.0, rect.center().y);
            ui.painter().text(at, egui::Align2::RIGHT_CENTER, text, FontId::proportional((height * 0.6).clamp(9.0, 14.0)), visuals.text_color());
        }
        resp
    }
}

/// The layout for things pushed to the far end of a row (the right, or the left in a
/// right-to-left language).
pub fn trailing() -> egui::Layout {
    if crate::i18n::rtl() { egui::Layout::left_to_right(egui::Align::Center) } else { egui::Layout::right_to_left(egui::Align::Center) }
}

/// A rounded rectangle filled with a gradient from `top` to `bottom`.
pub fn vertical_gradient(rect: egui::Rect, radius: f32, top: Color32, bottom: Color32) -> egui::Shape {
    let r = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let mut outline = Vec::new();
    for (corner, start) in [
        (egui::pos2(rect.right() - r, rect.top() + r), -90.0f32),
        (egui::pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (egui::pos2(rect.left() + r, rect.bottom() - r), 90.0),
        (egui::pos2(rect.left() + r, rect.top() + r), 180.0),
    ] {
        for step in 0..=6 {
            let a = (start + step as f32 * 15.0).to_radians();
            outline.push(corner + egui::vec2(a.cos(), a.sin()) * r);
        }
    }
    let colour = |p: egui::Pos2| top.lerp_to_gamma(bottom, ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0));
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.center(), colour(rect.center()));
    for p in &outline {
        mesh.colored_vertex(*p, colour(*p));
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    egui::Shape::mesh(mesh)
}

/// Gives panel resize lines the look of the table's column bars (thick, in the theme's colours)
/// for the panels added next; returns the style to put back inside them.
pub fn thick_resize_bars(ui: &mut egui::Ui) -> std::sync::Arc<egui::Style> {
    let normal = ui.style().clone();
    let w = &mut ui.visuals_mut().widgets;
    w.noninteractive.bg_stroke.width = 3.0;
    w.hovered.fg_stroke = w.hovered.bg_stroke;
    w.hovered.fg_stroke.width = 4.0;
    w.active.fg_stroke = w.active.bg_stroke;
    w.active.fg_stroke.width = 4.0;
    ui.style_mut().interaction.resize_grab_radius_side = 7.0;
    normal
}

/// A filled accent button for primary actions.
pub fn primary(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.into()).color(on_accent()).strong()).fill(accent())
}

/// "Rainbow" in the Rainbow Rounded face with a soft pink, lavender and sky-blue gradient across
/// it. `phase` (0..1, wrapping) slides the gradient along, for a gentle animation.
pub fn title(size: f32, phase: f32) -> egui::text::LayoutJob {
    // A loop of colours (back to the first), so sliding it along never jumps.
    const STOPS: [Color32; 4] = [rgb(255, 128, 182), rgb(184, 146, 255), rgb(112, 196, 255), rgb(255, 128, 182)];
    let mut job = egui::text::LayoutJob::default();
    let font = FontId::new(size, egui::FontFamily::Name(UiFont::Rounded.key().into()));
    let letters: Vec<char> = "RAINBOW".chars().collect();
    for (i, ch) in letters.iter().enumerate() {
        // Position along the word, over two thirds of the loop, moved by the phase.
        let at = (i as f32 / (letters.len() - 1) as f32 * 0.66 + phase).rem_euclid(1.0);
        let p = at * (STOPS.len() - 1) as f32;
        let k = (p.floor() as usize).min(STOPS.len() - 2);
        let color = STOPS[k].lerp_to_gamma(STOPS[k + 1], p - k as f32);
        job.append(&ch.to_string(), 0.0, egui::TextFormat { font_id: font.clone(), color, ..Default::default() });
    }
    job
}

/// A small rounded tag, e.g. for genres and chart features.
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    chip_ex(ui, text, color, true).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A tag that only shows information (no click, normal cursor).
pub fn tag(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    chip_ex(ui, text, color, false)
}

fn chip_ex(ui: &mut egui::Ui, text: &str, color: Color32, clickable: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(11.5), color);
    let pad = egui::vec2(8.0, 3.0);
    let sense = if clickable { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, sense);
    let hovered = clickable && resp.hovered();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, text));
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(10),
        color.linear_multiply(if hovered { 0.35 } else { 0.18 }),
        Stroke::new(1.0, color.linear_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + pad, galley, color);
    resp
}

/// A filter badge: the label plus a ✖ that removes it. Only the ✖ is clickable; it turns
/// red with a pointer cursor on hover. Returns true when the ✖ is clicked.
pub fn filter_chip(ui: &mut egui::Ui, text: &str, color: Color32, close_tip: &str) -> bool {
    let font = FontId::proportional(11.5);
    let label = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), color);
    let pad = egui::vec2(8.0, 3.0);
    // Close area is a little wider than the ✖, plus right padding to balance the left.
    let (close_w, gap, right_pad) = (18.0, 3.0, 5.0);
    let size = egui::vec2(pad.x + label.size().x + gap + close_w + right_pad, label.size().y + pad.y * 2.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    let close_rect = egui::Rect::from_min_size(egui::pos2(rect.right() - right_pad - close_w, rect.top()), egui::vec2(close_w, rect.height()));
    let close = ui.interact(close_rect, resp.id.with("close"), egui::Sense::click());
    close.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, close_tip));
    let close_color = if close.hovered() { bad() } else { color };
    let x = ui.painter().layout_no_wrap("✖".to_owned(), font, close_color);
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(10), color.linear_multiply(0.18), Stroke::new(1.0, color.linear_multiply(0.6)), egui::StrokeKind::Inside);
    if close.hovered() {
        painter.rect_filled(close_rect.shrink2(egui::vec2(1.0, 2.0)), CornerRadius::same(6), bad().linear_multiply(0.18));
    }
    painter.galley(rect.min + pad, label, color);
    painter.galley(close_rect.center() - x.size() / 2.0, x, close_color);
    close.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(close_tip).clicked()
}

/// Uses the bundled Noto fonts (assets/fonts, SIL Open Font License) on every OS: Noto Sans
/// for the UI, with Noto Emoji for icons and Noto Sans JP for Japanese/Chinese chart names.
/// egui's own fonts stay in the list as a fallback for its UI symbols.
/// `language` picks which CJK font comes first, since Chinese and Japanese share characters
/// but draw some of them differently.
pub fn install_fonts(ctx: &egui::Context, choice: UiFont, language: &str) {
    use std::sync::Arc;
    use egui::{FontData, FontFamily};

    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        ("noto-sans", NOTO_SANS),
        ("noto-sans-bold", &include_bytes!("../assets/fonts/NotoSans-Bold.ttf")[..]),
        ("noto-emoji", NOTO_EMOJI),
        ("noto-sans-jp", NOTO_SANS_JP),
        // Only the characters Rainbow's Korean and Chinese translations use (tools/subset_cjk.py).
        ("noto-sans-kr", &include_bytes!("../assets/fonts/NotoSansKR-UI.ttf")[..]),
        ("noto-sans-sc", &include_bytes!("../assets/fonts/NotoSansSC-UI.ttf")[..]),
        ("noto-sans-tc", &include_bytes!("../assets/fonts/NotoSansTC-UI.ttf")[..]),
        // Arabic, including the joined letter forms the right-to-left text is drawn with.
        ("noto-sans-arabic", &include_bytes!("../assets/fonts/NotoSansArabic.ttf")[..]),
        ("rainbow-rounded", &include_bytes!("../assets/fonts/RainbowRounded.ttf")[..]),
        ("atkinson", &include_bytes!("../assets/fonts/AtkinsonHyperlegible.ttf")[..]),
        ("lexend", &include_bytes!("../assets/fonts/Lexend.ttf")[..]),
        ("opendyslexic", &include_bytes!("../assets/fonts/OpenDyslexic.otf")[..]),
    ] {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    let cjk: [&str; 4] = match language {
        "zh-CN" => ["noto-sans-sc", "noto-sans-tc", "noto-sans-jp", "noto-sans-kr"],
        "zh-TW" => ["noto-sans-tc", "noto-sans-sc", "noto-sans-jp", "noto-sans-kr"],
        "ko" => ["noto-sans-kr", "noto-sans-jp", "noto-sans-sc", "noto-sans-tc"],
        _ => ["noto-sans-jp", "noto-sans-kr", "noto-sans-sc", "noto-sans-tc"],
    };
    let fallbacks: Vec<String> = ["noto-sans", "noto-emoji", "noto-sans-arabic"].into_iter().chain(cjk).map(str::to_owned).collect();

    // The chosen face first; Noto covers anything it lacks, then egui's own symbol fonts.
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    let mut order = vec![choice.key().to_owned()];
    order.extend(fallbacks.iter().filter(|f| f.as_str() != choice.key()).cloned());
    for (k, name) in order.into_iter().enumerate() {
        proportional.insert(k, name);
    }
    fonts.families.entry(FontFamily::Monospace).or_default().extend(fallbacks.iter().cloned());
    for face in UiFont::ALL {
        let mut family = vec![face.key().to_owned()];
        family.extend(fallbacks.iter().cloned());
        fonts.families.insert(FontFamily::Name(face.key().into()), family);
    }
    let mut bold = vec!["noto-sans-bold".to_owned()];
    bold.extend(fallbacks.iter().cloned());
    fonts.families.insert(FontFamily::Name("bold".into()), bold);
    ctx.set_fonts(fonts);
}

/// Difficulty as six dots, filled up to `d`, coloured by how hard it is.
pub fn difficulty_dots(d: i8) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    // Easy = green through hard = red.
    let filled = difficulty_color(d);
    for k in 0..6 {
        let color = if (k as i8) < d { filled } else { Color32::from_gray(80) };
        job.append("●", 0.0, egui::TextFormat { font_id: FontId::proportional(12.0), color, ..Default::default() });
    }
    job
}

/// A larger, square checkbox (egui's built-in one looks like a circle with our rounded style).
pub fn checkbox(ui: &mut egui::Ui, checked: &mut bool, text: &str) -> egui::Response {
    checkbox_ex(ui, checked, text, false)
}

/// Like [`checkbox`], optionally showing "some selected" (the knob in the middle).
/// Drawn as a switch whose knob slides across (instantly with Reduce motion on).
pub fn checkbox_ex(ui: &mut egui::Ui, checked: &mut bool, text: &str, indeterminate: bool) -> egui::Response {
    const W: f32 = 34.0;
    const H: f32 = 18.0;
    let galley = (!text.is_empty()).then(|| {
        ui.painter().layout_no_wrap(text.to_owned(), TextStyle::Body.resolve(ui.style()), ui.visuals().text_color())
    });
    let text_size = galley.as_ref().map_or(egui::Vec2::ZERO, |g| g.size());
    let width = W + if galley.is_some() { 8.0 + text_size.x } else { 0.0 };
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(width, H.max(text_size.y)), egui::Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    let (state, enabled) = (*checked, ui.is_enabled());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, state, text));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    // Animated with the style's animation time, which Reduce motion sets to zero.
    let how_on = ui.ctx().animate_bool_responsive(resp.id, *checked);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let visuals = ui.visuals();
    let fade = |c: Color32| if enabled { c } else { c.linear_multiply(0.4) };
    let pos = if indeterminate && !*checked { 0.5 } else { how_on };
    let off_fill = if resp.hovered() { visuals.widgets.hovered.bg_fill } else { visuals.extreme_bg_color };
    let on_fill = if resp.hovered() { accent_hover() } else { accent() };
    let fill = if indeterminate && !*checked { on_fill.linear_multiply(0.6) } else { off_fill.lerp_to_gamma(on_fill, how_on) };
    let stroke = visuals.weak_text_color().lerp_to_gamma(accent(), pos);
    let track = egui::Rect::from_min_size(egui::pos2(rect.left(), rect.center().y - H / 2.0), egui::vec2(W, H));
    let painter = ui.painter();
    painter.rect(track, CornerRadius::same((H / 2.0) as u8), fade(fill), Stroke::new(1.5, fade(stroke)), egui::StrokeKind::Inside);
    let r = H / 2.0 - 3.0;
    let x = egui::lerp((track.left() + 3.0 + r)..=(track.right() - 3.0 - r), pos);
    painter.circle_filled(egui::pos2(x, track.center().y), r, fade(Color32::WHITE));
    if let Some(g) = galley {
        let at = egui::pos2(track.right() + 8.0, rect.center().y - text_size.y / 2.0);
        painter.galley(at, g, fade(visuals.text_color()));
    }
    resp
}

/// A tick box, for picking songs in the results table (settings use switches).
pub fn select_box(ui: &mut egui::Ui, checked: &mut bool, indeterminate: bool) -> egui::Response {
    const SIZE: f32 = 18.0;
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(SIZE, SIZE), egui::Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    let (state, enabled) = (*checked, ui.is_enabled());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, state, ""));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let visuals = ui.visuals();
    let on = *checked || indeterminate;
    let (fill, stroke) = if on {
        (if resp.hovered() { accent_hover() } else { accent() }, accent())
    } else if resp.hovered() {
        (visuals.widgets.hovered.bg_fill, accent_hover())
    } else {
        (visuals.extreme_bg_color, visuals.weak_text_color())
    };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(4), fill, Stroke::new(1.5, stroke), egui::StrokeKind::Inside);
    let mark = Stroke::new(2.2, Color32::WHITE);
    if indeterminate && !*checked {
        painter.line_segment([rect.lerp_inside(egui::vec2(0.25, 0.5)), rect.lerp_inside(egui::vec2(0.75, 0.5))], mark);
    } else if *checked {
        let pts = [egui::vec2(0.24, 0.52), egui::vec2(0.42, 0.70), egui::vec2(0.77, 0.31)].map(|p| rect.lerp_inside(p));
        painter.add(egui::Shape::line(pts.to_vec(), mark));
    }
    resp
}

/// A floating "Back to top" pill with a drawn arrow.
pub fn back_to_top(ui: &mut egui::Ui) -> egui::Response {
    let galley = ui.painter().layout_no_wrap("Back to top".to_owned(), FontId::proportional(14.0), Color32::WHITE);
    let (pad, arrow_w, gap, height) = (18.0, 12.0, 8.0, 38.0);
    let size = egui::vec2(pad * 2.0 + arrow_w + gap + galley.size().x, height);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Back to top"));
    let painter = ui.painter();
    let radius = CornerRadius::same((height / 2.0) as u8);
    painter.add(ui.visuals().popup_shadow.as_shape(rect, radius));
    painter.rect_filled(rect, radius, if resp.hovered() { accent_hover() } else { accent() });

    let c = egui::pos2(rect.left() + pad + arrow_w / 2.0, rect.center().y);
    let stroke = Stroke::new(2.2, Color32::WHITE);
    painter.line_segment([c + egui::vec2(0.0, 7.0), c + egui::vec2(0.0, -7.0)], stroke);
    painter.add(egui::Shape::line(
        vec![c + egui::vec2(-6.0, -1.0), c + egui::vec2(0.0, -7.0), c + egui::vec2(6.0, -1.0)],
        stroke,
    ));
    let text_pos = egui::pos2(rect.left() + pad + arrow_w + gap, rect.center().y - galley.size().y / 2.0);
    painter.galley(text_pos, galley, Color32::WHITE);
    resp
}

/// A soft, heavily blurred copy of a screenshot, for frosted-glass backdrops. The image is
/// shrunk first (which blurs and keeps it cheap), then box-blurred; drawing it stretched
/// with linear filtering smooths it further.
pub fn blurred(image: &egui::ColorImage) -> egui::ColorImage {
    const SHRINK: usize = 8;
    const RADIUS: usize = 3;
    let [w, h] = image.size;
    let (sw, sh) = ((w / SHRINK).max(1), (h / SHRINK).max(1));
    let mut small = vec![[0f32; 3]; sw * sh];
    for y in 0..sh {
        for x in 0..sw {
            let mut acc = [0f32; 3];
            let mut n = 0.0f32;
            for yy in y * SHRINK..((y + 1) * SHRINK).min(h) {
                for xx in x * SHRINK..((x + 1) * SHRINK).min(w) {
                    let p = image.pixels[yy * w + xx];
                    acc[0] += p.r() as f32;
                    acc[1] += p.g() as f32;
                    acc[2] += p.b() as f32;
                    n += 1.0;
                }
            }
            small[y * sw + x] = acc.map(|v| v / n.max(1.0));
        }
    }
    // Three box-blur passes in each direction approximate a Gaussian.
    let mut tmp = small.clone();
    for _ in 0..3 {
        for y in 0..sh {
            for x in 0..sw {
                let (lo, hi) = (x.saturating_sub(RADIUS), (x + RADIUS).min(sw - 1));
                let mut acc = [0f32; 3];
                for xx in lo..=hi {
                    for c in 0..3 {
                        acc[c] += small[y * sw + xx][c];
                    }
                }
                tmp[y * sw + x] = acc.map(|v| v / (hi - lo + 1) as f32);
            }
        }
        for y in 0..sh {
            for x in 0..sw {
                let (lo, hi) = (y.saturating_sub(RADIUS), (y + RADIUS).min(sh - 1));
                let mut acc = [0f32; 3];
                for yy in lo..=hi {
                    for c in 0..3 {
                        acc[c] += tmp[yy * sw + x][c];
                    }
                }
                small[y * sw + x] = acc.map(|v| v / (hi - lo + 1) as f32);
            }
        }
    }
    let pixels = small.iter().map(|p| Color32::from_rgb(p[0] as u8, p[1] as u8, p[2] as u8)).collect();
    egui::ColorImage { size: [sw, sh], source_size: egui::vec2(sw as f32, sh as f32), pixels }
}
