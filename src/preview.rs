//! The chart preview: a window that plays a chart's notes falling down a highway, like in game.
//! Visual only (no audio).

use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Instant;

use anyhow::bail;
use eframe::egui;
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::i18n::t;
use crate::api::{Chart, Client, DIFFICULTIES, INSTRUMENTS};
use crate::chart::{self, BAR, Kind, Layout, Song, Track};
use crate::sng;

const GREEN: Color32 = Color32::from_rgb(40, 190, 70);
const RED: Color32 = Color32::from_rgb(225, 50, 55);
const YELLOW: Color32 = Color32::from_rgb(240, 200, 40);
const BLUE: Color32 = Color32::from_rgb(50, 120, 235);
const ORANGE: Color32 = Color32::from_rgb(245, 135, 30);
const PURPLE: Color32 = Color32::from_rgb(175, 80, 225);
const STAR: Color32 = Color32::from_rgb(130, 235, 255);
const SPEEDS: [f32; 5] = [0.5, 0.75, 1.0, 1.25, 1.5];
/// How far past the strike line notes stay visible, as a fraction of the highway.
const BEHIND: f32 = 0.12;

enum State {
    Loading(Receiver<Result<Song, String>>),
    Failed(String),
    Ready(Song),
}

pub struct Preview {
    title: String,
    state: State,
    inst: u8,
    diff: u8,
    /// Song position in seconds.
    time: f32,
    playing: bool,
    last_frame: Option<Instant>,
    speed: f32,
    /// Seconds of upcoming notes on the highway.
    window: f32,
}

impl Preview {
    /// Starts downloading the chart and opens the window. Prefers the instrument and difficulty
    /// being searched for, if the chart has them.
    pub fn open(client: &Client, c: &Chart, ctx: &egui::Context, inst: Option<u8>, diff: Option<u8>) -> Self {
        let (tx, rx) = channel();
        let (client, chart, ctx) = (client.clone(), c.clone(), ctx.clone());
        thread::spawn(move || {
            let _ = tx.send(load(&client, &chart).map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        let inst = inst.filter(|&i| c.diffs[i as usize] >= 0).or_else(|| (0..10).find(|&i| c.diffs[i as usize] >= 0)).unwrap_or(0);
        Self {
            title: format!("{} - {}", c.artist.clone().unwrap_or_else(|| t("Unknown Artist")), c.name.clone().unwrap_or_else(|| t("Unknown"))),
            state: State::Loading(rx),
            inst,
            diff: diff.unwrap_or(0),
            time: 0.0,
            playing: false,
            last_frame: None,
            speed: 1.0,
            window: 1.6,
        }
    }

    /// Shows the window; returns false once it's been closed.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        if let State::Loading(rx) = &self.state
            && let Ok(result) = rx.try_recv()
        {
            self.state = match result {
                Ok(song) => {
                    self.pick_track(&song);
                    let first = song.track(self.inst, self.diff).and_then(|t| t.notes.first()).map_or(0.0, |n| n.t);
                    self.time = (first - 2.0).max(-1.0);
                    self.playing = true;
                    State::Ready(song)
                }
                Err(e) => State::Failed(e),
            };
        }
        let mut open = true;
        let screen = ctx.content_rect();
        let window = egui::Window::new(RichText::new(format!("▶  {}", self.title)).strong())
            .id(egui::Id::new("chart_preview"))
            // Above the blurred backdrop, which sits at the normal window level.
            .order(egui::Order::Foreground)
            .open(&mut open)
            .default_size([360.0, 680.0f32.min(screen.height() - 60.0)])
            .default_width(360.0);
        // Pinned to the middle of the whole window (not just the space between panels), so it
        // stays centred as it grows once the chart has loaded.
        let window = window.pivot(egui::Align2::CENTER_CENTER).fixed_pos(screen.center());
        window.min_size([280.0, 360.0]).collapsible(false).show(ctx, |ui| self.contents(ui));
        open
    }

    /// Keeps the chosen instrument and difficulty if the song has them, else the nearest that exists.
    fn pick_track(&mut self, song: &Song) {
        if song.track(self.inst, self.diff).is_some() {
            return;
        }
        let same_inst = song.tracks.iter().filter(|t| t.inst == self.inst).min_by_key(|t| t.diff);
        if let Some(t) = same_inst.or_else(|| song.tracks.first()) {
            (self.inst, self.diff) = (t.inst, t.diff);
        }
    }

    fn contents(&mut self, ui: &mut egui::Ui) {
        match &self.state {
            State::Loading(_) => {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 2.0 - 30.0);
                    ui.spinner();
                    ui.label(t("Downloading the chart…"));
                });
                return;
            }
            State::Failed(e) => {
                ui.colored_label(RED, crate::tr!("Couldn't load this chart: {error}", error = e));
                return;
            }
            State::Ready(_) => {}
        }
        // Borrow the song separately from the controls that change `self`.
        let State::Ready(song) = std::mem::replace(&mut self.state, State::Failed(String::new())) else {
            return;
        };
        self.player(ui, &song);
        self.state = State::Ready(song);
    }

    fn player(&mut self, ui: &mut egui::Ui, song: &Song) {
        // Advance the clock.
        let now = Instant::now();
        if self.playing {
            let dt = self.last_frame.map_or(0.0, |t| (now - t).as_secs_f32());
            self.time += dt.min(0.1) * self.speed;
            if self.time > song.end + 1.0 {
                self.playing = false;
            }
            ui.ctx().request_repaint();
        }
        self.last_frame = Some(now);
        if !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle(song.end);
        }

        // Instrument and difficulty pickers: only what this chart has.
        ui.horizontal(|ui| {
            let mut insts: Vec<u8> = song.tracks.iter().map(|t| t.inst).collect();
            insts.dedup();
            egui::ComboBox::from_id_salt("preview_inst").selected_text(t(INSTRUMENTS[self.inst as usize].1)).show_ui(ui, |ui| {
                for i in insts {
                    ui.selectable_value(&mut self.inst, i, t(INSTRUMENTS[i as usize].1));
                }
            });
            let diffs: Vec<u8> = song.tracks.iter().filter(|t| t.inst == self.inst).map(|t| t.diff).collect();
            if !diffs.contains(&self.diff) {
                self.diff = diffs.iter().copied().min().unwrap_or(0);
            }
            egui::ComboBox::from_id_salt("preview_diff").selected_text(t(DIFFICULTIES[self.diff as usize].1)).show_ui(ui, |ui| {
                for d in diffs {
                    ui.selectable_value(&mut self.diff, d, t(DIFFICULTIES[d as usize].1));
                }
            });
            if let Some(t) = song.track(self.inst, self.diff) {
                let gems = t.notes.iter().filter(|n| !matches!(n.kind, Kind::DoubleKick)).count();
                ui.weak(crate::tr!("{count} notes", count = gems));
            }
        });
        ui.add_space(4.0);

        let size = vec2(ui.available_width(), (ui.available_height() - 44.0).max(200.0));
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        if resp.clicked() {
            self.toggle(song.end);
        }
        if resp.hovered() {
            // Scroll over the highway to scrub.
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.time = (self.time + scroll * 0.01).clamp(-1.0, song.end);
            }
        }
        if let Some(track) = song.track(self.inst, self.diff) {
            draw(ui.painter_at(rect), rect, song, track, self.time, self.window);
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let icon = if self.playing { "⏸" } else { "▶" };
            if ui.add(egui::Button::new(RichText::new(icon).size(16.0)).min_size(vec2(36.0, 28.0))).on_hover_text(t("Play/pause (Space)")).clicked() {
                self.toggle(song.end);
            }
            ui.label(RichText::new(format!("{} / {}", clock(self.time), clock(song.end))).monospace());
            let right = 190.0;
            ui.spacing_mut().slider_width = (ui.available_width() - right).max(60.0);
            ui.add(egui::Slider::new(&mut self.time, -1.0..=song.end).show_value(false));
            egui::ComboBox::from_id_salt("preview_speed")
                .selected_text(format!("{}×", self.speed))
                .width(64.0)
                .show_ui(ui, |ui| {
                    for s in SPEEDS {
                        ui.selectable_value(&mut self.speed, s, format!("{s}×"));
                    }
                })
                .response
                .on_hover_text(t("Playback speed"));
            ui.spacing_mut().slider_width = 70.0;
            // Note speed: a shorter window makes notes move faster.
            let mut note_speed = 3.2 / self.window;
            if ui.add(egui::Slider::new(&mut note_speed, 1.0..=4.0).show_value(false)).on_hover_text(t("Note speed")).changed() {
                self.window = 3.2 / note_speed;
            }
        });
    }

    fn toggle(&mut self, end: f32) {
        if !self.playing && self.time >= end {
            self.time = -1.0;
        }
        self.playing = !self.playing;
    }
}

fn load(client: &Client, chart: &Chart) -> anyhow::Result<Song> {
    let data = client.fetch_sng(chart)?;
    let (ini, file) = sng::read_file(&data, &["notes.chart", "notes.mid"])?;
    let pro = chart.drum_type == Some(1) || ini_flag(&ini, "pro_drums");
    match file {
        Some((name, bytes)) if name.ends_with(".mid") => chart::parse_midi(&bytes, pro),
        Some((_, bytes)) => chart::parse_chart(&String::from_utf8_lossy(&bytes), pro),
        None => bail!("it has no notes.chart or notes.mid"),
    }
}

fn ini_flag(ini: &str, key: &str) -> bool {
    ini.lines().filter_map(|l| l.split_once('=')).any(|(k, v)| {
        k.trim().eq_ignore_ascii_case(key) && matches!(v.trim().to_lowercase().as_str(), "true" | "1")
    })
}

fn clock(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

fn lane_colors(layout: Layout) -> &'static [Color32] {
    match layout {
        Layout::Five => &[GREEN, RED, YELLOW, BLUE, ORANGE],
        Layout::Drums4 => &[RED, YELLOW, BLUE, GREEN],
        Layout::Drums5 => &[RED, YELLOW, BLUE, ORANGE, GREEN],
        Layout::Ghl => &[Color32::WHITE, Color32::WHITE, Color32::WHITE],
    }
}

/// Paints the highway and every note within view at song time `now`.
fn draw(p: egui::Painter, rect: Rect, song: &Song, track: &Track, now: f32, window: f32) {
    let layout = track.layout;
    let lanes = layout.lanes() as f32;
    let colors = lane_colors(layout);
    let horizon = rect.top() + 10.0;
    let strike = rect.bottom() - rect.height() * 0.1;
    // Keep the highway slim: narrower than the window, and never wider than about half its height.
    let half_w = (rect.width() * 0.4).min(rect.height() * 0.26);
    let cx = rect.center().x;
    // Depth z: 0 at the strike line, 1 at the far end. Far things are smaller and bunched up.
    let depth = |z: f32| (1.0 - z) / (1.0 + 1.6 * z);
    let scale = |z: f32| 0.3 + 0.7 * depth(z);
    let at = |x: f32, z: f32| -> Pos2 { pos2(cx + x * half_w * scale(z), horizon + (strike - horizon) * depth(z)) };
    let lane_x = |lane: u8| -1.0 + (2.0 * (lane % layout.lanes()) as f32 + 1.0) / lanes;
    let gem_r = half_w / lanes * 0.72;
    let z_of = |t: f32| (t - now) / window;

    // Highway surface, lane dividers and edges.
    let edge = 1.06;
    let board = vec![at(-edge, 1.0), at(edge, 1.0), at(edge, -BEHIND), at(-edge, -BEHIND)];
    p.add(egui::Shape::convex_polygon(board, Color32::from_rgb(20, 20, 30), Stroke::new(2.0, Color32::from_gray(90))));
    for i in 1..layout.lanes() {
        let x = -1.0 + 2.0 * i as f32 / lanes;
        p.line_segment([at(x, 1.0), at(x, -BEHIND)], Stroke::new(1.0, Color32::from_gray(45)));
    }
    for &(t, measure) in &song.beats {
        let z = z_of(t);
        if (-BEHIND..=1.0).contains(&z) {
            let (w, c) = if measure { (2.0, Color32::from_gray(120)) } else { (1.0, Color32::from_gray(60)) };
            p.line_segment([at(-edge, z), at(edge, z)], Stroke::new(w * scale(z), c));
        }
    }

    // Notes in view (sustains up to 30 s long can reach in from before it).
    let start = track.notes.partition_point(|n| n.t < now - 30.0);
    let end = track.notes.partition_point(|n| n.t <= now + window);
    let visible = &track.notes[start.min(end)..end];

    // Strike line targets, lit briefly when a note passes them.
    let lit = |lane: u8| visible.iter().any(|n| n.lane == lane && now >= n.t && (now - n.t < 0.1 || now < n.end));
    p.line_segment([at(-edge, 0.0), at(edge, 0.0)], Stroke::new(3.0, Color32::from_gray(150)));
    let drums = matches!(layout, Layout::Drums4 | Layout::Drums5);
    if drums && lit(BAR) {
        p.line_segment([at(-1.0, 0.0), at(1.0, 0.0)], Stroke::new(gem_r * 0.6, ORANGE.gamma_multiply(0.5)));
    }
    for lane in 0..layout.lanes() {
        let c = at(lane_x(lane), 0.0);
        let color = colors[lane as usize];
        let on = lit(lane) || (layout == Layout::Ghl && lit(lane + 3));
        if on {
            p.circle_filled(c, gem_r * 1.25, color.gamma_multiply(0.45));
        }
        p.circle(c, gem_r * 0.9, Color32::from_gray(25), Stroke::new(3.0, if layout == Layout::Ghl { Color32::from_gray(170) } else { color }));
    }

    // Notes, far ones first so nearer ones draw on top.

    // Sustain tails underneath the gems.
    for n in visible.iter().filter(|n| n.end > n.t + 0.01) {
        let (z0, z1) = (z_of(n.t).max(0.0), z_of(n.end).min(1.0));
        if z1 <= z0 {
            continue;
        }
        let color = if n.star { STAR } else if n.lane == BAR { PURPLE } else { colors[(n.lane % layout.lanes()) as usize] };
        let x = if n.lane == BAR { 0.0 } else { lane_x(n.lane) };
        let steps = 8;
        for s in 0..steps {
            let (a, b) = (z0 + (z1 - z0) * s as f32 / steps as f32, z0 + (z1 - z0) * (s + 1) as f32 / steps as f32);
            let w = if n.lane == BAR { half_w * 1.6 * scale(a) } else { gem_r * 0.35 * scale(a) };
            p.line_segment([at(x, a), at(x, b)], Stroke::new(w, color.gamma_multiply(0.8)));
        }
    }

    for n in visible.iter().rev() {
        if n.t < now {
            continue; // already "hit"
        }
        let z = z_of(n.t);
        let s = scale(z);
        let r = gem_r * s;
        if n.lane == BAR {
            let color = match n.kind {
                Kind::Kick => ORANGE,
                Kind::DoubleKick => Color32::from_rgb(255, 90, 160),
                _ => PURPLE,
            };
            let color = if n.star { STAR } else { color };
            let (a, b) = (at(-0.96, z), at(0.96, z));
            let bar = Rect::from_min_max(pos2(a.x, a.y - r * 0.28), pos2(b.x, b.y + r * 0.28));
            p.rect(bar, CornerRadius::same(4), color, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);
            continue;
        }
        let c = at(lane_x(n.lane), z);
        let (fill, rim) = if layout == Layout::Ghl {
            // White frets are light gems, black frets dark ones.
            if n.lane < 3 { (Color32::from_gray(235), Color32::from_gray(60)) } else { (Color32::from_gray(30), Color32::from_gray(220)) }
        } else {
            (colors[n.lane as usize], Color32::WHITE)
        };
        let fill = if n.star { STAR } else { fill };
        match n.kind {
            Kind::Cymbal => {
                let pts = vec![
                    c + vec2(-r * 1.1, 0.0),
                    c + vec2(-r * 0.55, -r * 0.55),
                    c + vec2(r * 0.55, -r * 0.55),
                    c + vec2(r * 1.1, 0.0),
                    c + vec2(r * 0.55, r * 0.55),
                    c + vec2(-r * 0.55, r * 0.55),
                ];
                p.add(egui::Shape::convex_polygon(pts, fill, Stroke::new(1.5, rim)));
            }
            Kind::Hopo => {
                p.circle(c, r * 0.85, fill, Stroke::new(1.5, rim));
                p.circle_filled(c, r * 0.38, Color32::WHITE);
            }
            Kind::Tap => {
                p.circle(c, r * 0.85, fill, Stroke::new(2.5, PURPLE));
                p.circle_filled(c, r * 0.38, Color32::from_gray(20));
            }
            _ => {
                p.circle(c, r, fill, Stroke::new(1.5, rim));
                p.circle_filled(c, r * 0.45, fill.gamma_multiply(0.55));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Downloads a mix of charts and compares note counts with what Encore reports.
    /// Run with: cargo test note_counts -- --ignored --nocapture
    #[test]
    #[ignore]
    fn note_counts_match_encore() {
        let cat = crate::catalogue::Catalogue::load().expect("no catalogue");
        let client = Client::new().unwrap();
        let ok = |c: &&Chart| !c.has(crate::api::flag::BROKEN) && !c.notes.is_empty();
        let mut picks: Vec<&Chart> = Vec::new();
        picks.extend(cat.charts.iter().filter(ok).filter(|c| crate::games::game_key(c.icon.as_deref()).is_some()).step_by(397).take(8));
        picks.extend(cat.charts.iter().filter(ok).filter(|c| c.drum_type == Some(1)).take(2));
        picks.extend(cat.charts.iter().filter(ok).filter(|c| c.diffs[6] >= 0).take(2));
        picks.extend(cat.charts.iter().filter(ok).step_by(9973).take(5));
        let (mut same, mut total) = (0, 0);
        for c in picks {
            match load(&client, c) {
                Err(e) => println!("FAILED {}: {e:#}", c.title()),
                Ok(song) => {
                    println!("{}", c.title());
                    for s in &c.notes {
                        let Some(t) = song.track(s.i, s.d) else {
                            println!("    {} {}: missing (Encore says {})", INSTRUMENTS[s.i as usize].1, DIFFICULTIES[s.d as usize].1, s.n);
                            total += 1;
                            continue;
                        };
                        let mut times: Vec<u32> = t.notes.iter().map(|n| (n.t * 1000.0).round() as u32).collect();
                        // Encore counts each drum hit, but a guitar chord as one note.
                        if s.i != crate::api::DRUMS {
                            times.dedup();
                        }
                        total += 1;
                        if times.len() as u32 == s.n {
                            same += 1;
                        } else {
                            println!("    {} {}: {} groups, Encore says {}", INSTRUMENTS[s.i as usize].1, DIFFICULTIES[s.d as usize].1, times.len(), s.n);
                        }
                    }
                }
            }
        }
        println!("{same}/{total} tracks match");
    }
}