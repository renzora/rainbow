//! Rainbow's look: colour themes, rounded widgets, roomier spacing, and a few custom widgets.

use std::sync::RwLock;

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

pub const GREEN: Color32 = Color32::from_rgb(74, 201, 120);
pub const RED: Color32 = Color32::from_rgb(235, 96, 96);
pub const AMBER: Color32 = Color32::from_rgb(240, 180, 70);

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
    pub const ALL: [ThemeKind; 7] = [
        ThemeKind::Midnight,
        ThemeKind::Daylight,
        ThemeKind::Hotel,
        ThemeKind::Synthwave,
        ThemeKind::Ocean,
        ThemeKind::Forest,
        ThemeKind::Sunset,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ThemeKind::Midnight => "Midnight",
            ThemeKind::Daylight => "Daylight",
            ThemeKind::Hotel => "Hotel",
            ThemeKind::Synthwave => "Synthwave",
            ThemeKind::Ocean => "Ocean",
            ThemeKind::Forest => "Forest",
            ThemeKind::Sunset => "Sunset",
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

pub fn apply(ctx: &egui::Context, kind: ThemeKind, zoom: f32) {
    let pal = kind.palette();
    *CURRENT.write().unwrap() = pal;
    let theme = if pal.dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.set_theme(theme);
    ctx.set_zoom_factor(zoom);
    ctx.style_mut_of(theme, |s| style(s, &pal));
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

/// A filled accent button for primary actions.
pub fn primary(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.into()).color(Color32::WHITE).strong()).fill(accent())
}

/// "Rainbow" with each letter in a logo colour.
pub fn title(size: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for (ch, color) in "RAINBOW".chars().zip(RAINBOW) {
        job.append(
            &ch.to_string(),
            0.0,
            egui::TextFormat { font_id: FontId::proportional(size), color, extra_letter_spacing: 1.5, ..Default::default() },
        );
    }
    job
}

/// A small rounded tag, e.g. for genres and chart features.
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(11.5), color);
    let pad = egui::vec2(8.0, 3.0);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::click());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(10),
        color.linear_multiply(if resp.hovered() { 0.35 } else { 0.18 }),
        Stroke::new(1.0, color.linear_multiply(0.6)),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + pad, galley, color);
    resp
}

/// Uses the bundled Noto fonts (assets/fonts, SIL Open Font License) on every OS: Noto Sans
/// for the UI, with Noto Emoji for icons and Noto Sans JP for Japanese/Chinese chart names.
/// egui's own fonts stay in the list as a fallback for its UI symbols.
pub fn install_fonts(ctx: &egui::Context) {
    use std::sync::Arc;
    use egui::{FontData, FontFamily};

    let mut fonts = egui::FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add("noto-sans", include_bytes!("../assets/fonts/NotoSans.ttf"));
    add("noto-emoji", include_bytes!("../assets/fonts/NotoEmoji.ttf"));
    add("noto-sans-jp", include_bytes!("../assets/fonts/NotoSansJP.ttf"));

    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "noto-sans".to_owned());
    proportional.extend(["noto-emoji".to_owned(), "noto-sans-jp".to_owned()]);
    let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
    monospace.extend(["noto-sans".to_owned(), "noto-emoji".to_owned(), "noto-sans-jp".to_owned()]);
    ctx.set_fonts(fonts);
}

/// Difficulty as six dots, filled up to `d`, coloured by how hard it is.
pub fn difficulty_dots(d: i8) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    // Easy = green through hard = red.
    let filled = RAINBOW[[3, 3, 2, 2, 1, 0, 0][d.clamp(0, 6) as usize]];
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

/// Like [`checkbox`], optionally showing a "some selected" dash.
pub fn checkbox_ex(ui: &mut egui::Ui, checked: &mut bool, text: &str, indeterminate: bool) -> egui::Response {
    const SIZE: f32 = 18.0;
    let galley = (!text.is_empty()).then(|| {
        ui.painter().layout_no_wrap(text.to_owned(), TextStyle::Body.resolve(ui.style()), ui.visuals().text_color())
    });
    let text_size = galley.as_ref().map_or(egui::Vec2::ZERO, |g| g.size());
    let width = SIZE + if galley.is_some() { 8.0 + text_size.x } else { 0.0 };
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(width, SIZE.max(text_size.y)), egui::Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let visuals = ui.visuals();
    let enabled = ui.is_enabled();
    let fade = |c: Color32| if enabled { c } else { c.linear_multiply(0.4) };
    let on = *checked || indeterminate;
    let (fill, stroke) = if on {
        (if resp.hovered() { accent_hover() } else { accent() }, accent())
    } else if resp.hovered() {
        (visuals.widgets.hovered.bg_fill, accent_hover())
    } else {
        (visuals.extreme_bg_color, visuals.weak_text_color())
    };
    let b = egui::Rect::from_center_size(egui::pos2(rect.left() + SIZE / 2.0, rect.center().y), egui::vec2(SIZE, SIZE));
    let painter = ui.painter();
    painter.rect(b, CornerRadius::same(4), fade(fill), Stroke::new(1.5, fade(stroke)), egui::StrokeKind::Inside);
    let mark = Stroke::new(2.2, fade(Color32::WHITE));
    if indeterminate && !*checked {
        painter.line_segment([b.lerp_inside(egui::vec2(0.25, 0.5)), b.lerp_inside(egui::vec2(0.75, 0.5))], mark);
    } else if *checked {
        let pts = [egui::vec2(0.24, 0.52), egui::vec2(0.42, 0.70), egui::vec2(0.77, 0.31)].map(|p| b.lerp_inside(p));
        painter.add(egui::Shape::line(pts.to_vec(), mark));
    }
    if let Some(g) = galley {
        let pos = egui::pos2(b.right() + 8.0, rect.center().y - text_size.y / 2.0);
        painter.galley(pos, g, fade(visuals.text_color()));
    }
    resp
}

/// A floating "Back to top" pill with a drawn arrow.
pub fn back_to_top(ui: &mut egui::Ui) -> egui::Response {
    let galley = ui.painter().layout_no_wrap("Back to top".to_owned(), FontId::proportional(14.0), Color32::WHITE);
    let (pad, arrow_w, gap, height) = (18.0, 12.0, 8.0, 38.0);
    let size = egui::vec2(pad * 2.0 + arrow_w + gap + galley.size().x, height);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
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
