//! Rainbow's look: dark/light palettes with a purple accent, rounded widgets, roomier spacing.

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Visuals};

pub const ACCENT: Color32 = Color32::from_rgb(124, 108, 255);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(146, 133, 255);
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

pub fn apply(ctx: &egui::Context, dark: bool, zoom: f32) {
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.set_zoom_factor(zoom);
    ctx.style_mut_of(egui::Theme::Dark, |s| style(s, true));
    ctx.style_mut_of(egui::Theme::Light, |s| style(s, false));
}

fn style(style: &mut egui::Style, dark: bool) {
    let r = CornerRadius::same(8);
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let (bg, panel, raised, input, hover, active, text, weak) = if dark {
        (
            Color32::from_rgb(13, 14, 20),
            Color32::from_rgb(19, 20, 29),
            Color32::from_rgb(28, 30, 42),
            Color32::from_rgb(10, 11, 16),
            Color32::from_rgb(40, 43, 60),
            Color32::from_rgb(52, 56, 78),
            Color32::from_rgb(228, 230, 240),
            Color32::from_rgb(140, 145, 165),
        )
    } else {
        (
            Color32::from_rgb(244, 245, 250),
            Color32::from_rgb(255, 255, 255),
            Color32::from_rgb(236, 237, 245),
            Color32::from_rgb(248, 248, 252),
            Color32::from_rgb(224, 226, 240),
            Color32::from_rgb(210, 212, 232),
            Color32::from_rgb(28, 30, 40),
            Color32::from_rgb(110, 114, 130),
        )
    };
    v.panel_fill = panel;
    v.window_fill = panel;
    v.extreme_bg_color = input;
    v.faint_bg_color = if dark { Color32::from_rgb(23, 24, 34) } else { Color32::from_rgb(247, 247, 252) };
    v.code_bg_color = raised;
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = r;
    v.window_stroke = Stroke::new(1.0, raised);
    v.hyperlink_color = ACCENT_HOVER;
    v.selection.bg_fill = ACCENT.linear_multiply(if dark { 0.55 } else { 0.35 });
    v.selection.stroke = Stroke::new(1.0, ACCENT_HOVER);
    v.override_text_color = Some(text);
    v.weak_text_color = Some(weak);
    let _ = bg;

    for (w, fill) in [
        (&mut v.widgets.noninteractive, panel),
        (&mut v.widgets.inactive, raised),
        (&mut v.widgets.hovered, hover),
        (&mut v.widgets.active, active),
        (&mut v.widgets.open, hover),
    ] {
        w.corner_radius = r;
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = Stroke::NONE;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, raised);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.linear_multiply(0.6));
    v.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    // Show the pointing-hand cursor over every clickable widget.
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    style.visuals = v;

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    style.spacing.interact_size.y = 26.0;
    style.spacing.combo_width = 140.0;
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(22.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Small, FontId::proportional(11.5)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
}

/// A filled accent button for primary actions.
pub fn primary(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.into()).color(Color32::WHITE).strong()).fill(ACCENT)
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
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(10),
        color.linear_multiply(0.18),
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
        (if resp.hovered() { ACCENT_HOVER } else { ACCENT }, ACCENT)
    } else if resp.hovered() {
        (visuals.widgets.hovered.bg_fill, ACCENT_HOVER)
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
    painter.rect_filled(rect, radius, if resp.hovered() { ACCENT_HOVER } else { ACCENT });

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
