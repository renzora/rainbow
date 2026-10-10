//! Little country flags for the language menus. egui can't draw colour emoji, so these are the
//! flag-icons pictures (MIT, assets/flags) drawn into textures the first time they're needed.

use std::collections::HashMap;

use eframe::egui;

/// The flag shown for each language.
const FLAGS: &[(&str, &[u8])] = &[
    ("en", include_bytes!("../assets/flags/gb.svg")),
    ("ar", include_bytes!("../assets/flags/sa.svg")),
    ("cs", include_bytes!("../assets/flags/cz.svg")),
    ("de", include_bytes!("../assets/flags/de.svg")),
    ("es", include_bytes!("../assets/flags/es.svg")),
    ("fr", include_bytes!("../assets/flags/fr.svg")),
    ("id", include_bytes!("../assets/flags/id.svg")),
    ("it", include_bytes!("../assets/flags/it.svg")),
    ("ja", include_bytes!("../assets/flags/jp.svg")),
    ("ko", include_bytes!("../assets/flags/kr.svg")),
    ("nl", include_bytes!("../assets/flags/nl.svg")),
    ("pl", include_bytes!("../assets/flags/pl.svg")),
    ("pt-BR", include_bytes!("../assets/flags/br.svg")),
    ("ru", include_bytes!("../assets/flags/ru.svg")),
    ("sv", include_bytes!("../assets/flags/se.svg")),
    ("tr", include_bytes!("../assets/flags/tr.svg")),
    ("uk", include_bytes!("../assets/flags/ua.svg")),
    ("vi", include_bytes!("../assets/flags/vn.svg")),
    ("zh-CN", include_bytes!("../assets/flags/cn.svg")),
    ("zh-TW", include_bytes!("../assets/flags/tw.svg")),
];

/// The flag for a language code, as a texture (4:3), or None if there isn't one.
pub fn flag(ctx: &egui::Context, code: &str) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("language_flags");
    if let Some(tex) = ctx.data(|d| d.get_temp::<HashMap<String, Option<egui::TextureHandle>>>(id).and_then(|m| m.get(code).cloned())) {
        return tex;
    }
    let tex = FLAGS.iter().find(|(c, _)| *c == code).and_then(|(_, svg)| render(svg)).map(|img| ctx.load_texture(format!("flag_{code}"), img, egui::TextureOptions::LINEAR));
    ctx.data_mut(|d| d.get_temp_mut_or_default::<HashMap<String, Option<egui::TextureHandle>>>(id).insert(code.to_owned(), tex.clone()));
    tex
}

/// Draws an SVG flag at 48×36 pixels.
fn render(svg: &[u8]) -> Option<egui::ColorImage> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let (w, h) = (48u32, 36u32);
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    let size = tree.size();
    resvg::render(&tree, tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height()), &mut pixmap.as_mut());
    Some(egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data()))
}
