//! A whole-song picture of the optimal star power path, in the style of CHOpt's images: rows of
//! measures with the notes on a five-lane staff, star power phrases shaded green and
//! activations blue, and under each measure its base score, the running total and the bar.
//! It's laid out once as simple shapes, then painted in the app or written out as an SVG.

use std::fmt::Write as _;

use eframe::egui;
use egui::{Color32, Pos2, Rect, Stroke, pos2, vec2};

use crate::chart::{BAR, Parsed, time_signatures};
use crate::optimal::Outcome;
use crate::preview::thousands;

/// Layout units (the SVG's pixels); the app scales it to fit.
pub const WIDTH: f32 = 1024.0;
const LEFT: f32 = 36.0;
const RIGHT: f32 = 20.0;
/// Beats per row of the chart.
const ROW_BEATS: f64 = 16.0;
const HEADER: f32 = 150.0;
/// Space above the staff (tempo and measure numbers), between lanes, and below (scores).
const ABOVE: f32 = 40.0;
const LANE_GAP: f32 = 12.0;
const BELOW: f32 = 54.0;
const NOTE_R: f32 = 6.0;

const FRETS: [Color32; 5] = [
    Color32::from_rgb(40, 190, 70),
    Color32::from_rgb(225, 50, 55),
    Color32::from_rgb(240, 200, 40),
    Color32::from_rgb(50, 120, 235),
    Color32::from_rgb(245, 135, 30),
];
const OPEN: Color32 = Color32::from_rgb(175, 80, 225);
const PHRASE: Color32 = Color32::from_rgba_premultiplied(70, 150, 70, 90);
const ACTIVATION: Color32 = Color32::from_rgba_premultiplied(50, 70, 170, 110);
const INK: Color32 = Color32::from_rgb(20, 20, 20);
const GREY: Color32 = Color32::from_rgb(130, 130, 130);
const FAINT: Color32 = Color32::from_rgb(215, 215, 215);
const MEASURE_NO: Color32 = Color32::from_rgb(170, 20, 20);
const TOTAL: Color32 = Color32::from_rgb(20, 120, 20);
const BAR_TEXT: Color32 = Color32::from_rgb(0, 150, 170);

#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Centre,
    Right,
}

#[derive(Clone)]
pub enum Shape {
    Rect { rect: Rect, fill: Color32 },
    Line { from: Pos2, to: Pos2, width: f32, color: Color32 },
    Circle { centre: Pos2, radius: f32, fill: Color32 },
    Star { centre: Pos2, radius: f32, fill: Color32 },
    /// A drum cymbal: a triangle pointing up.
    Triangle { centre: Pos2, radius: f32, fill: Color32 },
    /// `at` is the left, centre or right end of the text's baseline.
    Text { at: Pos2, text: String, size: f32, color: Color32, align: Align },
}

#[derive(Clone)]
pub struct Layout {
    pub height: f32,
    pub shapes: Vec<Shape>,
}

/// What the header says.
pub struct Titles {
    pub name: String,
    pub artist: String,
    pub charter: String,
    pub part: String,
}

/// Lays out the path for one instrument + difficulty. `sustain_cutoff` is song.ini's.
pub fn build(chart: &Parsed, inst: u8, diff: u8, sustain_cutoff: Option<i64>, outcome: &Outcome, titles: &Titles) -> Option<Layout> {
    let res = chart.res as f64;
    let beat = |tick: u32| tick as f64 / res;
    let ghl = (6..=9).contains(&inst);
    let drums = inst == crate::api::DRUMS;
    let lanes = if ghl { 3 } else if drums { 4 } else { 5 };

    // Notes as drawn: (beat, sustain end beat, lane), with short .mid sustains cut like scoring
    // does, and whether each is a cymbal. Drums are drawn as scored (five-lane as pro drums).
    let (notes, cymbals, star): (Vec<(f64, f64, u8)>, Vec<bool>, Vec<(u32, u32)>) = if drums {
        let track = chart.drum_track(diff)?;
        let gems = crate::drums::drawn(chart, &track);
        let notes = gems.iter().map(|&(tick, lane, _)| (beat(tick), beat(tick), lane.unwrap_or(BAR))).collect();
        (notes, gems.iter().map(|g| g.2).collect(), track.star)
    } else {
        let track = chart.track(inst, diff)?;
        let cutoff = sustain_cutoff.unwrap_or(chart.res as i64 / 3 + 1);
        let notes: Vec<(f64, f64, u8)> = track
            .notes
            .iter()
            .map(|&(tick, len, lane)| {
                let len = if track.midi && len as i64 <= cutoff { 0 } else { len };
                (beat(tick), beat(tick + len), lane)
            })
            .collect();
        let n = notes.len();
        (notes, vec![false; n], track.star)
    };
    let last = notes.iter().map(|n| n.1).fold(0.0, f64::max);
    let in_phrase = |b: f64| star.iter().any(|&(s, e)| b >= beat(s) && b < beat(e));
    let kick = if drums { FRETS[4] } else { OPEN };

    // Measures: (start beat, length in beats, time signature if it changes here).
    let sigs = time_signatures(&chart.sigs);
    let mut measures: Vec<(f64, f64, Option<(u32, u32)>)> = Vec::new();
    for (k, &(tick, num, den)) in sigs.iter().enumerate() {
        let len = num as f64 * 4.0 / den as f64;
        let stop = sigs.get(k + 1).map_or(f64::INFINITY, |s| beat(s.0));
        let mut b = beat(tick);
        let mut first = true;
        while b < stop - 1e-9 && b <= last {
            measures.push((b, len, first.then_some((num, den))));
            first = false;
            b += len;
        }
    }
    if measures.is_empty() {
        return None;
    }

    // Tempo changes worth labelling: (beat, BPM), skipping repeats and very close ones.
    let mut tempos: Vec<(f64, f64)> = Vec::new();
    let mut sorted = chart.tempos.clone();
    sorted.sort_by_key(|t| t.0);
    for (tick, us) in sorted {
        let bpm = 60_000_000.0 / us;
        let b = beat(tick);
        if tempos.last().is_none_or(|&(pb, pbpm)| ((bpm - pbpm).abs() >= 0.5 && b - pb >= 1.0) || b == pb) {
            if tempos.last().is_some_and(|&(pb, _)| pb == b) {
                tempos.pop();
            }
            tempos.push((b, bpm));
        }
    }
    if tempos.first().is_none_or(|t| t.0 > 0.0) {
        tempos.insert(0, (0.0, 120.0));
    }

    let mut shapes = Vec::new();
    let text = |shapes: &mut Vec<Shape>, x: f32, y: f32, t: String, size: f32, color: Color32, align: Align| {
        shapes.push(Shape::Text { at: pos2(x, y), text: t, size, color, align });
    };

    // Header.
    let mut y = 34.0;
    for (line, size) in [(&titles.name, 22.0), (&titles.artist, 18.0), (&titles.charter, 18.0), (&titles.part, 18.0)] {
        text(&mut shapes, LEFT, y, line.clone(), size, INK, Align::Left);
        y += size + 6.0;
    }
    text(&mut shapes, LEFT, y + 4.0, format!("Total score = {}", thousands(outcome.total)), 20.0, INK, Align::Left);

    // Rows of whole measures.
    let ppb = ((WIDTH - LEFT - RIGHT) as f64 / ROW_BEATS) as f32;
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut used = 0.0;
    for (i, m) in measures.iter().enumerate() {
        if rows.is_empty() || (used + m.1 > ROW_BEATS + 1e-9 && used > 0.0) {
            rows.push(Vec::new());
            used = 0.0;
        }
        rows.last_mut().unwrap().push(i);
        used += m.1;
    }

    let staff_h = LANE_GAP * (lanes - 1) as f32;
    let row_h = ABOVE + staff_h + BELOW;
    let mut running = 0u64;
    let mut point = 0; // next scoring point to add to the running total
    for (r, row) in rows.iter().enumerate() {
        let top = HEADER + r as f32 * row_h;
        let staff_top = top + ABOVE;
        let staff_bottom = staff_top + staff_h;
        let start = measures[row[0]].0;
        let end = measures[*row.last().unwrap()].0 + measures[*row.last().unwrap()].1;
        let x = |b: f64| LEFT + ((b - start) as f32) * ppb;
        let lane_y = |lane: u8| staff_top + LANE_GAP * (lane % lanes) as f32;
        let band = |from: f64, to: f64| Rect::from_min_max(pos2(x(from.max(start)), staff_top - 8.0), pos2(x(to.min(end)), staff_bottom + 8.0));

        // Shading: phrases (from their first note to their last) and activations.
        for &(s, e) in &star {
            let inside: Vec<f64> = notes.iter().map(|n| n.0).filter(|&b| b >= beat(s) && b < beat(e)).collect();
            if let (Some(&a), Some(&b)) = (inside.first(), inside.last())
                && b + 0.1 > start
                && a < end
            {
                shapes.push(Shape::Rect { rect: band(a - 0.1, b + 0.1), fill: PHRASE });
            }
        }
        for a in &outcome.activations {
            if a.last_beat + 0.1 > start && a.first_beat < end {
                shapes.push(Shape::Rect { rect: band(a.first_beat - 0.1, a.last_beat + 0.1), fill: ACTIVATION });
            }
        }

        // Staff: lanes, beat lines and measure lines.
        for lane in 0..lanes {
            let y = lane_y(lane);
            shapes.push(Shape::Line { from: pos2(x(start), y), to: pos2(x(end), y), width: 1.0, color: GREY });
        }
        for &i in row {
            let (m_start, m_len, sig) = measures[i];
            let mut b = m_start + 1.0;
            while b < m_start + m_len - 1e-9 {
                shapes.push(Shape::Line { from: pos2(x(b), staff_top), to: pos2(x(b), staff_bottom), width: 1.0, color: FAINT });
                b += 1.0;
            }
            shapes.push(Shape::Line { from: pos2(x(m_start), staff_top), to: pos2(x(m_start), staff_bottom), width: 1.5, color: INK });
            text(&mut shapes, x(m_start) + 2.0, staff_top - 10.0, (i + 1).to_string(), 12.0, MEASURE_NO, Align::Left);
            if let Some((num, den)) = sig {
                let cx = x(m_start) + 10.0;
                text(&mut shapes, cx, staff_top + staff_h * 0.45, num.to_string(), 20.0, GREY, Align::Centre);
                text(&mut shapes, cx, staff_bottom + 2.0, den.to_string(), 20.0, GREY, Align::Centre);
            }
        }
        shapes.push(Shape::Line { from: pos2(x(end), staff_top), to: pos2(x(end), staff_bottom), width: 1.5, color: INK });
        for &(b, bpm) in tempos.iter().filter(|t| t.0 >= start && t.0 < end) {
            text(&mut shapes, x(b), top + 14.0, format!("♩={}", bpm.round()), 11.0, GREY, Align::Left);
        }

        // Sustains under the notes, then the notes (stars inside star power phrases).
        for &(b, e, lane) in notes.iter().filter(|n| n.1 > start && n.0 < end && n.1 > n.0) {
            let (from, to) = (x(b.max(start)), x(e.min(end)));
            let (y, color) = if lane == BAR { (staff_top + staff_h / 2.0, kick) } else { (lane_y(lane), note_color(lane, ghl, drums)) };
            shapes.push(Shape::Line { from: pos2(from, y), to: pos2(to, y), width: 5.0, color: color.gamma_multiply(0.8) });
        }
        // Open notes and kicks first, so gems at the same time sit on top of them.
        for &(b, _, _) in notes.iter().filter(|n| n.2 == BAR && n.0 >= start && n.0 < end) {
            let r = Rect::from_min_max(pos2(x(b) - 3.0, staff_top - 4.0), pos2(x(b) + 3.0, staff_bottom + 4.0));
            shapes.push(Shape::Rect { rect: r, fill: kick });
        }
        for (&(b, _, lane), &cymbal) in notes.iter().zip(&cymbals).filter(|(n, _)| n.0 >= start && n.0 < end) {
            if lane == BAR {
                continue;
            }
            let centre = pos2(x(b), lane_y(lane));
            let fill = note_color(lane, ghl, drums);
            if cymbal {
                shapes.push(Shape::Triangle { centre, radius: NOTE_R + 2.0, fill: if in_phrase(b) { fill.lerp_to_gamma(Color32::WHITE, 0.45) } else { fill } });
            } else if in_phrase(b) {
                shapes.push(Shape::Star { centre, radius: NOTE_R + 2.5, fill });
            } else {
                shapes.push(Shape::Circle { centre, radius: NOTE_R, fill });
            }
        }

        // Under each measure: its base score, the running total, and the bar at its end.
        let pts = &outcome.points;
        for &i in row {
            let (m_start, m_len, _) = measures[i];
            let m_end = m_start + m_len;
            let mut base = 0;
            while point < pts.len() && pts[point].beat < m_end - 1e-9 {
                let p = &pts[point];
                base += p.base;
                running += p.value * if p.activation.is_some() { 2 } else { 1 } + p.extra;
                point += 1;
            }
            let rx = x(m_end) - 3.0;
            text(&mut shapes, rx, staff_bottom + 18.0, base.to_string(), 11.0, GREY, Align::Right);
            text(&mut shapes, rx, staff_bottom + 31.0, running.to_string(), 11.0, TOTAL, Align::Right);
            if let Some(&(_, bar)) = outcome.measure_bars.iter().find(|m| (m.0 - m_end).abs() < 1e-6) {
                text(&mut shapes, rx, staff_bottom + 44.0, format!("{:.2}%", bar * 100.0), 11.0, BAR_TEXT, Align::Right);
            }
        }
    }
    Some(Layout { height: HEADER + rows.len() as f32 * row_h + 20.0, shapes })
}

fn note_color(lane: u8, ghl: bool, drums: bool) -> Color32 {
    if drums {
        // Red, yellow, blue, green.
        FRETS[[1, 2, 3, 0][(lane as usize).min(3)]]
    } else if ghl {
        if lane < 3 { Color32::from_gray(245) } else { Color32::from_gray(40) }
    } else {
        FRETS[(lane as usize).min(4)]
    }
}

/// The corners of a triangle pointing up.
fn triangle_points(c: Pos2, r: f32) -> Vec<Pos2> {
    vec![c + vec2(0.0, -r), c + vec2(r * 0.9, r * 0.6), c + vec2(-r * 0.9, r * 0.6)]
}

/// The five points and five inner corners of a star.
fn star_points(c: Pos2, r: f32) -> Vec<Pos2> {
    (0..10)
        .map(|k| {
            let a = std::f32::consts::PI * (k as f32 / 5.0 - 0.5);
            let len = if k % 2 == 0 { r } else { r * 0.45 };
            c + vec2(a.cos(), a.sin()) * len
        })
        .collect()
}

/// File formats the chart can be saved as.
#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Svg,
    Png,
    Jpeg,
    Webp,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Png, Format::Jpeg, Format::Webp, Format::Svg];

    pub fn name(self) -> &'static str {
        match self {
            Format::Svg => "SVG",
            Format::Png => "PNG",
            Format::Jpeg => "JPEG",
            Format::Webp => "WebP",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Svg => "svg",
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Webp => "webp",
        }
    }
}

impl Layout {
    /// The chart as a file in `format`. Pictures are drawn at twice the layout size for
    /// sharpness, scaled down when that would pass the format's size limit.
    pub fn encode(&self, format: Format) -> anyhow::Result<Vec<u8>> {
        if format == Format::Svg {
            return Ok(self.svg().into_bytes());
        }
        // WebP images can't be taller than 16383 pixels; keep the others reasonable too.
        let max_height = if format == Format::Webp { 16_000.0 } else { 32_000.0 };
        let scale = (max_height / self.height.max(1.0)).min(2.0);
        let image = self.raster(scale)?;
        let mut out = std::io::Cursor::new(Vec::new());
        match format {
            Format::Png => image.write_to(&mut out, image::ImageFormat::Png)?,
            Format::Jpeg => image::DynamicImage::ImageRgba8(image).to_rgb8().write_to(&mut out, image::ImageFormat::Jpeg)?,
            Format::Webp => image.write_to(&mut out, image::ImageFormat::WebP)?,
            Format::Svg => unreachable!(),
        }
        Ok(out.into_inner())
    }

    /// Draws the SVG version into pixels, using Rainbow's bundled fonts.
    fn raster(&self, scale: f32) -> anyhow::Result<image::RgbaImage> {
        use resvg::{tiny_skia, usvg};
        let mut options = usvg::Options::default();
        let fonts = options.fontdb_mut();
        for data in [crate::theme::NOTO_SANS, crate::theme::NOTO_EMOJI].into_iter().chain(crate::theme::noto_sans_jp()) {
            fonts.load_font_data(data.to_vec());
        }
        fonts.set_sans_serif_family("Noto Sans");
        let tree = usvg::Tree::from_str(&self.svg(), &options)?;
        let size = tree.size().to_int_size().scale_by(scale).ok_or_else(|| anyhow::anyhow!("the chart is too big to draw"))?;
        let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height()).ok_or_else(|| anyhow::anyhow!("the chart is too big to draw"))?;
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        // tiny-skia stores premultiplied alpha; the background is opaque white, so it's the same.
        image::RgbaImage::from_raw(size.width(), size.height(), pixmap.take()).ok_or_else(|| anyhow::anyhow!("couldn't make the image"))
    }

    /// Paints it with its top-left at `origin`, scaled by `scale`.
    pub fn paint(&self, painter: &egui::Painter, origin: Pos2, scale: f32) {
        let at = |p: Pos2| origin + p.to_vec2() * scale;
        let clip = painter.clip_rect();
        for shape in &self.shapes {
            match shape {
                Shape::Rect { rect, fill } => {
                    let r = Rect::from_min_max(at(rect.min), at(rect.max));
                    if r.intersects(clip) {
                        painter.rect_filled(r, 0.0, *fill);
                    }
                }
                Shape::Line { from, to, width, color } => {
                    painter.line_segment([at(*from), at(*to)], Stroke::new(width * scale, *color));
                }
                Shape::Circle { centre, radius, fill } => {
                    painter.circle(at(*centre), radius * scale, *fill, Stroke::new(scale, INK));
                }
                Shape::Star { centre, radius, fill } => {
                    // egui fills convex shapes only, so draw the middle and then each point.
                    let p: Vec<Pos2> = star_points(at(*centre), radius * scale);
                    let inner: Vec<Pos2> = p.iter().skip(1).step_by(2).copied().collect();
                    painter.add(egui::Shape::convex_polygon(inner, *fill, Stroke::NONE));
                    for k in 0..5 {
                        let tri = vec![p[(2 * k + 9) % 10], p[2 * k], p[2 * k + 1]];
                        painter.add(egui::Shape::convex_polygon(tri, *fill, Stroke::NONE));
                    }
                    let mut outline = p.clone();
                    outline.push(p[0]);
                    painter.add(egui::Shape::line(outline, Stroke::new(scale, INK)));
                }
                Shape::Triangle { centre, radius, fill } => {
                    painter.add(egui::Shape::convex_polygon(triangle_points(at(*centre), radius * scale), *fill, Stroke::new(scale, INK)));
                }
                Shape::Text { at: pos, text, size, color, align } => {
                    let anchor = match align {
                        Align::Left => egui::Align2::LEFT_BOTTOM,
                        Align::Centre => egui::Align2::CENTER_BOTTOM,
                        Align::Right => egui::Align2::RIGHT_BOTTOM,
                    };
                    painter.text(at(*pos), anchor, text, egui::FontId::proportional(size * scale), *color);
                }
            }
        }
    }

    /// The same picture as an SVG document.
    pub fn svg(&self) -> String {
        let colour = |c: &Color32| {
            let [r, g, b, a] = c.to_srgba_unmultiplied();
            format!("rgb({r},{g},{b})\" fill-opacity=\"{:.3}", a as f32 / 255.0)
        };
        let mut s = String::new();
        let _ = writeln!(
            s,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" font-family=\"Noto Sans, Arial, sans-serif\">",
            w = WIDTH,
            h = self.height.ceil()
        );
        let _ = writeln!(s, "<rect width=\"100%\" height=\"100%\" fill=\"white\"/>");
        for shape in &self.shapes {
            match shape {
                Shape::Rect { rect, fill } => {
                    let _ = writeln!(
                        s,
                        "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{}\"/>",
                        rect.min.x,
                        rect.min.y,
                        rect.width(),
                        rect.height(),
                        colour(fill)
                    );
                }
                Shape::Line { from, to, width, color } => {
                    let [r, g, b, _] = color.to_srgba_unmultiplied();
                    let _ = writeln!(
                        s,
                        "<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"rgb({r},{g},{b})\" stroke-width=\"{width}\"/>",
                        from.x, from.y, to.x, to.y
                    );
                }
                Shape::Circle { centre, radius, fill } => {
                    let _ = writeln!(
                        s,
                        "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{radius}\" fill=\"{}\" stroke=\"black\"/>",
                        centre.x,
                        centre.y,
                        colour(fill)
                    );
                }
                Shape::Star { centre, radius, fill } => {
                    let pts: Vec<String> = star_points(*centre, *radius).iter().map(|p| format!("{:.1},{:.1}", p.x, p.y)).collect();
                    let _ = writeln!(s, "<polygon points=\"{}\" fill=\"{}\" stroke=\"black\"/>", pts.join(" "), colour(fill));
                }
                Shape::Triangle { centre, radius, fill } => {
                    let pts: Vec<String> = triangle_points(*centre, *radius).iter().map(|p| format!("{:.1},{:.1}", p.x, p.y)).collect();
                    let _ = writeln!(s, "<polygon points=\"{}\" fill=\"{}\" stroke=\"black\"/>", pts.join(" "), colour(fill));
                }
                Shape::Text { at, text, size, color, align } => {
                    let anchor = match align {
                        Align::Left => "start",
                        Align::Centre => "middle",
                        Align::Right => "end",
                    };
                    let escaped = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
                    let _ = writeln!(
                        s,
                        "<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"{size}\" text-anchor=\"{anchor}\" fill=\"{}\">{escaped}</text>",
                        at.x,
                        at.y,
                        colour(color)
                    );
                }
            }
        }
        s.push_str("</svg>\n");
        s
    }
}

#[cfg(test)]
mod tests {
    /// Draws a chart's path to an SVG. Run with:
    /// CHART_DIR=<folder with song.ini + notes> SVG_OUT=<file.svg> cargo test path_chart_svg -- --ignored
    #[test]
    #[ignore]
    fn path_chart_svg() {
        let dir = std::path::PathBuf::from(std::env::var("CHART_DIR").expect("set CHART_DIR"));
        let ini = std::fs::read_to_string(dir.join("song.ini")).unwrap_or_default();
        let cutoff = ini
            .lines()
            .filter_map(|l| l.split_once('='))
            .find(|(k, _)| k.trim() == "sustain_cutoff_threshold")
            .and_then(|(_, v)| v.trim().parse().ok());
        let chart = match std::fs::read(dir.join("notes.mid")) {
            Ok(mid) => crate::chart::read_midi_chart(&mid).unwrap(),
            Err(_) => crate::chart::read_chart(&std::fs::read_to_string(dir.join("notes.chart")).unwrap()),
        };
        // INST picks the instrument (Rainbow's numbering: 0 lead guitar, 4 drums).
        let inst = std::env::var("INST").ok().and_then(|i| i.parse().ok()).unwrap_or(0);
        let outcome = crate::optimal::optimise(&chart, inst, 0, cutoff, Default::default()).unwrap();
        let titles = super::Titles { name: "Test".into(), artist: "Artist".into(), charter: "Charter".into(), part: "Expert".into() };
        let layout = super::build(&chart, inst, 0, cutoff, &outcome, &titles).unwrap();
        let out = std::path::PathBuf::from(std::env::var("SVG_OUT").expect("set SVG_OUT"));
        for format in super::Format::ALL {
            let started = web_time::Instant::now();
            std::fs::write(out.with_extension(format.extension()), layout.encode(format).unwrap()).unwrap();
            println!("{} in {:.2}s", format.name(), started.elapsed().as_secs_f64());
        }
    }
}