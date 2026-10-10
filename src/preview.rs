//! The chart preview: a window that plays a chart's notes falling down a highway, like in game.
//! Visual only (no audio).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Instant;

use anyhow::bail;
use eframe::egui;
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, pos2, vec2};

use crate::i18n::t;
use crate::tr;
use crate::api::{Chart, Client, DIFFICULTIES, INSTRUMENTS};
use crate::chart::{self, BAR, Kind, Layout, Parsed, Song, Track};
use crate::optimal::{self, Outcome};
use crate::pathchart;

const GREEN: Color32 = Color32::from_rgb(40, 190, 70);
const RED: Color32 = Color32::from_rgb(225, 50, 55);
const YELLOW: Color32 = Color32::from_rgb(240, 200, 40);
const BLUE: Color32 = Color32::from_rgb(50, 120, 235);
const ORANGE: Color32 = Color32::from_rgb(245, 135, 30);
const PURPLE: Color32 = Color32::from_rgb(175, 80, 225);
const STAR: Color32 = Color32::from_rgb(130, 235, 255);
const SPEEDS: [f32; 7] = [0.1, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5];
/// How far past the strike line notes stay visible, as a fraction of the highway.
const BEHIND: f32 = 0.12;

enum State {
    Loading(Receiver<Result<Loaded, String>>),
    Failed(String),
    Ready(Song),
}

/// Saving the path chart: in progress, or done (the path saved to, or an error).
enum SaveStatus {
    Saving(Receiver<Result<String, String>>),
    Done(Result<String, String>),
}

/// The optimal score for one instrument + difficulty, worked out in the background.
enum Optimal {
    /// Being worked out, and how far along (0 to 1000).
    Working(Receiver<Option<Outcome>>, Arc<AtomicU32>),
    Done(Option<Outcome>),
}

pub struct Preview {
    title: String,
    md5: String,
    /// How far the chart download has got (0 to 1000).
    load_progress: Arc<AtomicU32>,
    /// The Close button was clicked (it's in the controls, away from the app window's own ✖).
    closing: bool,
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
    chart: Option<(Arc<Parsed>, Option<i64>)>,
    optimal: HashMap<(u8, u8), Optimal>,
    /// Showing the path chart instead of the highway, and the charts built so far.
    path_view: bool,
    /// The path chart's zoom (1 = the whole width fits) and the top-left corner shown, in chart units.
    path_zoom: f32,
    path_pan: egui::Vec2,
    /// The section being repeated (index into the song's sections), if repeat is on.
    repeat: Option<usize>,
    /// The section the list last scrolled to (it only scrolls when this changes).
    shown_section: Option<usize>,
    /// The Clone Hero leaderboard beside the highway.
    board: crate::leaderboard::Panel,
    /// The chart, for the leaderboard.
    chart_info: Chart,
    path_charts: HashMap<(u8, u8), Option<pathchart::Layout>>,
    save_status: Option<SaveStatus>,
    titles: (String, String, String),
}

impl Preview {
    /// Starts downloading the chart and opens the window. Prefers the instrument and difficulty
    /// being searched for, if the chart has them.
    pub fn open(client: &Client, c: &Chart, ctx: &egui::Context, inst: Option<u8>, diff: Option<u8>) -> Self {
        let (tx, rx) = channel();
        let (client, chart, ctx) = (client.clone(), c.clone(), ctx.clone());
        let progress = Arc::new(AtomicU32::new(0));
        let load_progress = progress.clone();
        thread::spawn(move || {
            let progress = load_progress;
            let _ = tx.send(load(&client, &chart, &progress).map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        let inst = inst.filter(|&i| c.diffs[i as usize] >= 0).or_else(|| (0..10).find(|&i| c.diffs[i as usize] >= 0)).unwrap_or(0);
        Self {
            title: format!("{} - {}", c.artist.clone().unwrap_or_else(|| t("Unknown Artist")), c.name.clone().unwrap_or_else(|| t("Unknown"))),
            state: State::Loading(rx),
            md5: c.md5.clone(),
            load_progress: progress,
            inst,
            diff: diff.unwrap_or(0),
            time: 0.0,
            playing: false,
            last_frame: None,
            speed: 1.0,
            window: 1.6,
            chart: None,
            optimal: HashMap::new(),
            path_view: false,
            shown_section: None,
            repeat: None,
            path_zoom: 1.0,
            path_pan: egui::Vec2::ZERO,
            board: crate::leaderboard::Panel::new(),
            chart_info: c.clone(),
            path_charts: HashMap::new(),
            save_status: None,
            closing: false,
            titles: (
                c.name.clone().unwrap_or_else(|| t("Unknown")),
                c.artist.clone().unwrap_or_else(|| t("Unknown Artist")),
                c.charter.clone().unwrap_or_else(|| t("Unknown Charter")),
            ),
        }
    }

    /// The optimal score for the shown track: starts working it out if needed, and returns it
    /// once ready (None while working; Some(None) if there isn't one).
    fn optimal(&mut self, ctx: &egui::Context) -> Option<Option<&Outcome>> {
        let key = (self.inst, self.diff);
        if !self.optimal.contains_key(&key) {
            let (chart, cutoff) = self.chart.clone()?;
            let (tx, rx) = channel();
            let (ctx, md5) = (ctx.clone(), self.md5.clone());
            let progress = Arc::new(AtomicU32::new(0));
            let shared = progress.clone();
            thread::spawn(move || {
                let report = |f: f32| shared.store((f * 1000.0) as u32, std::sync::atomic::Ordering::Relaxed);
                let _ = tx.send(optimal::optimise_cached(&md5, &chart, key.0, key.1, cutoff, &report));
                ctx.request_repaint();
            });
            self.optimal.insert(key, Optimal::Working(rx, progress));
        }
        let entry = self.optimal.get_mut(&key)?;
        if let Optimal::Working(rx, _) = entry
            && let Ok(result) = rx.try_recv()
        {
            *entry = Optimal::Done(result);
        }
        match entry {
            Optimal::Done(result) => Some(result.as_ref()),
            Optimal::Working(..) => None,
        }
    }

    /// Shows the window; returns false once it's been closed.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        if let State::Loading(rx) = &self.state
            && let Ok(result) = rx.try_recv()
        {
            self.state = match result {
                Ok(Loaded {
                    song,
                    chart,
                    sustain_cutoff,
                }) => {
                    self.chart = Some((chart, sustain_cutoff));
                    self.pick_track(&song);
                    let first = song.track(self.inst, self.diff).and_then(|t| t.notes.first()).map_or(0.0, |n| n.t);
                    self.time = (first - 2.0).max(-1.0);
                    self.playing = true;
                    State::Ready(song)
                }
                Err(e) => State::Failed(e),
            };
        }
        let screen = ctx.content_rect();
        let window = egui::Window::new(RichText::new(format!("▶  {}", self.title)).strong())
            .id(egui::Id::new("chart_preview"))
            // Above the blurred backdrop, which sits at the normal window level.
            .order(egui::Order::Foreground)
            .default_size([
                920.0f32.min(screen.width() - 40.0),
                680.0f32.min(screen.height() - 60.0),
            ])
            .default_width(920.0f32.min(screen.width() - 40.0));
        // Pinned to the middle of the whole window (not just the space between panels), so it
        // stays centred as it grows once the chart has loaded.
        let window = window.pivot(egui::Align2::CENTER_CENTER).fixed_pos(screen.center());
        window.min_size([280.0, 360.0]).collapsible(false).show(ctx, |ui| self.contents(ui));
        !self.closing
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
                    ui.label(t("Downloading the chart…"));
                    let done = self.load_progress.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
                    ui.add(crate::theme::Bar::new(done).desired_width(240.0).show_percentage().fill(crate::theme::accent()));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                    ui.add_space(8.0);
                    self.closing |= ui.button(t("Cancel")).clicked();
                });
                return;
            }
            State::Failed(e) => {
                ui.colored_label(RED, crate::tr!("Couldn't load this chart: {error}", error = e));
                self.closing |= ui.button(t("Close")).clicked();
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
            // Repeating a section: back to its start once it's over.
            if let Some(k) = self.repeat
                && let Some(&(start, _)) = song.sections.get(k)
            {
                let end = song.sections.get(k + 1).map_or(song.end, |s| s.0);
                if self.time >= end {
                    self.time = start;
                }
            }
            if self.time > song.end + 1.0 {
                self.playing = false;
            }
            ui.ctx().request_repaint();
        }
        self.last_frame = Some(now);
        if !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Space)) {
            self.toggle(song.end);
        }

        // Top: instrument and difficulty pickers (only what this chart has) and the optimal score,
        // centred on a background.
        let mut activations: Vec<(f32, f32)> = Vec::new();
        section(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        centered_row(ui, "pickers", |ui| {
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
            ui.separator();
            ui.selectable_value(&mut self.path_view, false, t("🎸 Highway"));
            ui.selectable_value(&mut self.path_view, true, t("📈 Path chart"))
                .on_hover_text(t("The whole song with the star power path, like a CHOpt image"));
        });

        // The best possible score, and where to activate star power for it (blue on the highway).
        centered_row(ui, "optimal", |ui| {
            let drums = self.inst == crate::api::DRUMS;
            match self.optimal(ui.ctx()) {
                None => {
                    ui.weak(t("Working out the optimal score…"));
                    let done = match self.optimal.get(&(self.inst, self.diff)) {
                        Some(Optimal::Working(_, p)) => p.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0,
                        _ => 0.0,
                    };
                    ui.add(crate::theme::Bar::new(done).desired_width(160.0).desired_height(8.0).fill(crate::theme::accent()));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                }
                Some(None) => {}
                Some(Some(o)) => {
                    activations = o.activations.iter().map(|a| (a.first_secs as f32, a.last_secs as f32)).collect();
                    let tip = crate::tr!(
                        "Every note hit, with star power activated where the blue sections show (squeezing and whammy included), as CHOpt works it out. Without star power: {score}",
                        score = thousands(o.no_sp)
                    );
                    ui.label(RichText::new(crate::tr!("⭐ Optimal score: {score}", score = thousands(o.total))).strong().color(STAR))
                        .on_hover_text(tip);
                    let steps: Vec<String> = o
                        .activations
                        .iter()
                        .enumerate()
                        .map(|(k, a)| {
                            crate::tr!(
                                "{n}. After {phrases} phrase(s), activate in measure {from}; it lasts to measure {to} (+{points})",
                                n = k + 1,
                                phrases = a.phrases_before,
                                from = a.first_measure.floor(),
                                to = a.last_measure.floor(),
                                points = thousands(a.bonus)
                            ) + &if a.phrases_during > 0 { crate::tr!(", collecting {count} more on the way", count = a.phrases_during) } else { String::new() }
                        })
                        .collect();
                    let about = if drums {
                        t("How many fills to let go by before each activation (0: the first fill that shows up). E: hit the note that fills the bar early so the fill shows up; L: hit it late so it doesn't.")
                    } else {
                        t("Phrases collected before each activation; (+n) are collected during it.")
                    };
                    let tip = format!("{about}\n\n{}", steps.join("\n"));
                    // Long paths are cut short to fit (the full one is in the tooltip), so the
                    // score beside it always stays in view.
                    let path = crate::tr!("Path {path}", path = o.path);
                    let tip = format!("{path}\n\n{tip}");
                    ui.add(egui::Label::new(RichText::new(path).weak()).truncate()).on_hover_text(tip);
                }
            }
        });
        });
        ui.add_space(6.0);
        if self.path_view {
            self.path_chart(ui);
            return;
        }
        let size = vec2(
            ui.available_width(),
            (ui.available_height() - 62.0).max(200.0),
        );
        // The highway, with the chart's Clone Hero leaderboard beside it when there's room
        // (on the right, or the left in a right-to-left language).
        let (full, _) = ui.allocate_exact_size(size, Sense::hover());
        let board_w = if full.width() >= 620.0 { (full.width() * 0.38).clamp(260.0, 380.0) } else { 0.0 };
        let gap = if board_w > 0.0 { 8.0 } else { 0.0 };
        let (rect, board_rect) = if crate::i18n::rtl() {
            let board = egui::Rect::from_min_size(full.min, vec2(board_w, full.height()));
            (egui::Rect::from_min_max(egui::pos2(full.left() + board_w + gap, full.top()), full.max), board)
        } else {
            let board = egui::Rect::from_min_max(egui::pos2(full.right() - board_w, full.top()), full.max);
            (egui::Rect::from_min_max(full.min, egui::pos2(full.right() - board_w - gap, full.bottom())), board)
        };
        let resp = ui.interact(rect, ui.id().with("highway"), Sense::click());
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
            draw(
                ui.painter_at(rect),
                rect,
                song,
                track,
                &activations,
                self.time,
                self.window,
            );
        }
        // The song's sections, listed down the left of the highway (over it, without taking
        // space in the layout): the current one highlighted, click one to jump there.
        if !song.sections.is_empty() {
            let current = song.sections.iter().rposition(|s| s.0 <= self.time + 0.05);
            let width = (rect.width() * 0.26).clamp(120.0, 200.0);
            let area = egui::Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(width, rect.height() - 16.0));
            let mut list = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
            list.set_clip_rect(area);
            egui::ScrollArea::vertical().id_salt("preview_sections").max_height(area.height()).auto_shrink([false, true]).show(&mut list, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (k, (secs, name)) in song.sections.iter().enumerate() {
                    let on = current == Some(k);
                    let text = RichText::new(name).size(12.5);
                    let text = if on { text.strong().color(Color32::WHITE) } else { text.color(Color32::from_gray(170)) };
                    let button = egui::Button::new(text)
                        .fill(if on { crate::theme::accent().linear_multiply(0.85) } else { Color32::from_black_alpha(140) })
                        .min_size(vec2(width - 12.0, 22.0));
                    let resp = ui.add(button).on_hover_text(crate::tr!("Jump to {time}", time = clock(*secs)));
                    // Bring a new current section into view once (not every frame, which shakes).
                    if on && self.shown_section != current {
                        resp.scroll_to_me(None);
                    }
                    if resp.clicked() {
                        // Its first notes are already on their way down the highway.
                        self.time = *secs;
                        if self.repeat.is_some() {
                            self.repeat = Some(k);
                        }
                    }
                }
            });
            self.shown_section = current;
        }
        if board_w > 0.0 {
            let (c, inst, diff) = (self.chart_info.clone(), self.inst, self.diff);
            let mut column = ui.new_child(egui::UiBuilder::new().max_rect(board_rect).layout(egui::Layout::top_down(egui::Align::Min)));
            section(&column).show(&mut column, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(board_rect.height() - 16.0);
                egui::ScrollArea::vertical()
                    .id_salt("preview_board")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.board.show(ui, &c, Some(inst), Some(diff)));
            });
        }

        ui.add_space(6.0);
        // Bottom: playback controls on a background.
        section(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            crate::theme::hrow(ui, |ui| {
                let icon = if self.playing { "⏸" } else { "▶" };
                if ui
                    .add(
                        egui::Button::new(RichText::new(icon).size(16.0))
                            .min_size(vec2(36.0, 28.0)),
                    )
                    .on_hover_text(t("Play/pause (Space)"))
                    .clicked()
                {
                    self.toggle(song.end);
                }
                ui.label(
                    RichText::new(format!("{} / {}", clock(self.time), clock(song.end)))
                        .monospace(),
                );
                // From the far end inwards: Close, note speed, playback speed, repeat; the
                // position slider takes the space left in the middle.
                ui.with_layout(crate::theme::trailing(), |ui| {
                    let close = ui.add(egui::Button::new(t("✖  Close")).min_size(vec2(0.0, 28.0)));
                    self.closing |= close.on_hover_text(t("Close the preview (Esc)")).clicked();
                    ui.spacing_mut().slider_width = 70.0;
                    // Note speed: a shorter window makes notes move faster.
                    let mut note_speed = 3.2 / self.window;
                    if ui
                        .add(egui::Slider::new(&mut note_speed, 1.0..=4.0).show_value(false))
                        .on_hover_text(t("Note speed"))
                        .changed()
                    {
                        self.window = 3.2 / note_speed;
                    }
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
                    if !song.sections.is_empty() {
                        let on = self.repeat.is_some();
                        let repeat = ui.add(egui::Button::new(RichText::new("🔁").size(15.0)).selected(on).min_size(vec2(32.0, 28.0)));
                        let tip = match self.repeat.and_then(|k| song.sections.get(k)) {
                            Some((_, name)) => crate::tr!("Repeating {section}", section = name),
                            None => t("Repeat the current section"),
                        };
                        if repeat.on_hover_text(tip).clicked() {
                            self.repeat = if on { None } else { song.sections.iter().rposition(|s| s.0 <= self.time + 0.05).or(Some(0)) };
                        }
                    }
                    ui.spacing_mut().slider_width = (ui.available_width() - 8.0).max(60.0);
                    ui.add(egui::Slider::new(&mut self.time, -1.0..=song.end).show_value(false));
                });
            });
        });
    }

    /// The path chart for the shown track, scaled to the window, with a button to save it.
    fn path_chart(&mut self, ui: &mut egui::Ui) {
        let key = (self.inst, self.diff);
        let ready = match self.optimal.get(&key) {
            Some(Optimal::Done(Some(o))) => Some(o),
            Some(Optimal::Done(None)) => {
                ui.weak(t("There's no path to show for this part"));
                return;
            }
            _ => None,
        };
        let Some(outcome) = ready else {
            crate::theme::hrow(ui, |ui| {
                ui.spinner();
                ui.weak(t("Working out the optimal score…"));
            });
            return;
        };
        if !self.path_charts.contains_key(&key)
            && let Some((chart, cutoff)) = &self.chart
        {
            let (name, artist, charter) = &self.titles;
            let titles = pathchart::Titles {
                name: name.clone(),
                artist: artist.clone(),
                charter: charter.clone(),
                part: format!("{} {}", t(INSTRUMENTS[key.0 as usize].1), t(DIFFICULTIES[key.1 as usize].1)),
            };
            let layout = pathchart::build(chart, key.0, key.1, *cutoff, outcome, &titles);
            self.path_charts.insert(key, layout);
        }
        let Some(Some(layout)) = self.path_charts.get(&key) else {
            ui.weak(t("There's no path to show for this part"));
            return;
        };
        // The chart in a viewport: drag to pan, scroll to move up and down, Ctrl+scroll (or
        // pinch) to zoom around the pointer.
        let footer = 52.0;
        let size = vec2(ui.available_width(), (ui.available_height() - footer).max(120.0));
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let fit = rect.width() / pathchart::WIDTH;
        let mut scale = fit * self.path_zoom;
        if resp.hovered() {
            let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
            if zoom != 1.0 {
                let pointer = resp.hover_pos().unwrap_or(rect.center());
                let at = self.path_pan + (pointer - rect.min) / scale; // chart point under the pointer
                self.path_zoom = (self.path_zoom * zoom).clamp(0.25, 8.0);
                scale = fit * self.path_zoom;
                self.path_pan = at - (pointer - rect.min) / scale;
            } else if scroll != egui::Vec2::ZERO {
                self.path_pan -= scroll / scale;
            }
        }
        if resp.dragged() {
            self.path_pan -= resp.drag_delta() / scale;
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if resp.double_clicked() {
            (self.path_zoom, self.path_pan) = (1.0, egui::Vec2::ZERO);
            scale = fit;
        }
        // Keep the chart in view (centred when it's smaller than the viewport).
        let view = rect.size() / scale;
        let content = vec2(pathchart::WIDTH, layout.height);
        for (pan, view, content) in [(&mut self.path_pan.x, view.x, content.x), (&mut self.path_pan.y, view.y, content.y)] {
            *pan = if content <= view { (content - view) / 2.0 } else { pan.clamp(0.0, content - view) };
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_gray(40));
        let origin = rect.min - self.path_pan * scale;
        painter.rect_filled(egui::Rect::from_min_size(origin, content * scale), 0.0, Color32::WHITE);
        layout.paint(&painter, origin, scale);
        ui.add_space(6.0);
        section(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            centered_row(ui, "path_save", |ui| {
                // Zoom controls.
                if ui.button("➖").on_hover_text(t("Zoom out (Ctrl+scroll)")).clicked() {
                    self.path_zoom = (self.path_zoom / 1.25).max(0.25);
                }
                ui.label(RichText::new(format!("{:.0}%", self.path_zoom * 100.0)).monospace());
                if ui.button("➕").on_hover_text(t("Zoom in (Ctrl+scroll)")).clicked() {
                    self.path_zoom = (self.path_zoom * 1.25).min(8.0);
                }
                if ui.button(t("Fit")).on_hover_text(t("Fit the width (or double-click the chart)")).clicked() {
                    (self.path_zoom, self.path_pan) = (1.0, egui::Vec2::ZERO);
                }
                ui.separator();
                let mut chosen = None;
                ui.menu_button(t("💾  Save as…"), |ui| {
                    for format in pathchart::Format::ALL {
                        if ui.button(format!("{}  (.{})", format.name(), format.extension())).clicked() {
                            chosen = Some(format);
                            ui.close();
                        }
                    }
                })
                .response
                .on_hover_text(t("Save the path chart as a picture"));
                if let Some(format) = chosen {
                    let (name, artist, _) = &self.titles;
                    let file = crate::api::sanitize_filename(&format!("{artist} - {name} ({})", t(INSTRUMENTS[key.0 as usize].1)));
                    let ext = format.extension();
                    if let Some(path) = rfd::FileDialog::new().set_file_name(format!("{file}.{ext}")).add_filter(format.name(), &[ext]).save_file() {
                        // Drawing a long chart into pixels takes a moment, so it's done in the background.
                        let (tx, rx) = channel();
                        let (layout, ctx) = (layout.clone(), ui.ctx().clone());
                        thread::spawn(move || {
                            let _ = tx.send(layout.save(&path, format).map(|()| path.display().to_string()).map_err(|e| format!("{e:#}")));
                            ctx.request_repaint();
                        });
                        self.save_status = Some(SaveStatus::Saving(rx));
                    }
                }
                if let Some(SaveStatus::Saving(rx)) = &self.save_status
                    && let Ok(result) = rx.try_recv()
                {
                    self.save_status = Some(SaveStatus::Done(result));
                }
                match &self.save_status {
                    Some(SaveStatus::Saving(_)) => {
                        ui.spinner();
                        ui.weak(t("Saving…"));
                    }
                    Some(SaveStatus::Done(Ok(path))) => {
                        ui.weak(t("✔ Saved")).on_hover_text(path);
                    }
                    Some(SaveStatus::Done(Err(e))) => {
                        ui.weak(tr!("Couldn't save: {error}", error = e));
                    }
                    None => {}
                }
            });
        });
    }

    fn toggle(&mut self, end: f32) {
        if !self.playing && self.time >= end {
            self.time = -1.0;
        }
        self.playing = !self.playing;
    }
}

/// A downloaded chart: what the highway draws, plus the notes in ticks and song.ini's
/// sustain cutoff for working out optimal scores.
pub(crate) struct Loaded {
    pub song: Song,
    pub chart: Arc<Parsed>,
    pub sustain_cutoff: Option<i64>,
}

/// Downloads (or reads from the cache) a chart's notes; `progress` goes from 0 to 1000.
pub(crate) fn load(client: &Client, chart: &Chart, progress: &AtomicU32) -> anyhow::Result<Loaded> {
    let (ini, name, bytes) = client.fetch_notes(chart, progress)?;
    let file = Some((name, bytes));
    let pro = chart.drum_type == Some(1) || ini_flag(&ini, "pro_drums");
    let mut parsed = match file {
        Some((name, bytes)) if name.ends_with(".mid") => chart::read_midi_chart(&bytes)?,
        Some((_, bytes)) => chart::read_chart(&String::from_utf8_lossy(&bytes)),
        None => bail!("it has no notes.chart or notes.mid"),
    };
    let sustain_cutoff = ini_value(&ini, "sustain_cutoff_threshold").and_then(|v| v.parse().ok());
    // Which kind of drum kit the chart is for changes how drums score.
    let ini_bool = |key| ini_value(&ini, key).map(|v| matches!(v.to_lowercase().as_str(), "true" | "1"));
    parsed.pro_drums = ini_bool("pro_drums");
    parsed.five_lane_drums = ini_bool("five_lane_drums");
    Ok(Loaded {
        song: parsed.song(pro)?,
        chart: Arc::new(parsed),
        sustain_cutoff,
    })
}

fn ini_value<'a>(ini: &'a str, key: &str) -> Option<&'a str> {
    ini.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        .map(|(_, v)| v.trim())
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

/// A score with thousands separators, e.g. 1,091,597.
/// The background behind the preview's top and bottom sections.
fn section(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
}

/// A row of widgets centred horizontally. egui can't centre a row it hasn't measured, so this
/// remembers the row's width from the previous frame.
fn centered_row(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    let id = ui.id().with(id);
    let width: f32 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(0.0);
    crate::theme::hrow(ui, |ui| {
        ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
        let used = crate::theme::hrow(ui, add).response.rect.width();
        if (used - width).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(id, used));
            ui.ctx().request_repaint();
        }
    });
}

pub(crate) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Paints the highway and every note within view at song time `now`, with star power
/// activations (start, end seconds) shaded blue.
fn draw(
    p: egui::Painter,
    rect: Rect,
    song: &Song,
    track: &Track,
    activations: &[(f32, f32)],
    now: f32,
    window: f32,
) {
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
    let board = vec![
        at(-edge, 1.0),
        at(edge, 1.0),
        at(edge, -BEHIND),
        at(-edge, -BEHIND),
    ];
    p.add(egui::Shape::convex_polygon(
        board,
        Color32::from_rgb(20, 20, 30),
        Stroke::new(2.0, Color32::from_gray(90)),
    ));
    for &(from, to) in activations {
        let (z0, z1) = (z_of(from).max(-BEHIND), z_of(to).min(1.0));
        if z1 > z0 {
            let band = vec![at(-edge, z1), at(edge, z1), at(edge, z0), at(-edge, z0)];
            p.add(egui::Shape::convex_polygon(
                band,
                Color32::from_rgba_unmultiplied(60, 110, 255, 60),
                Stroke::NONE,
            ));
        }
    }
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
            match load(&client, c, &AtomicU32::new(0)) {
                Err(e) => println!("FAILED {}: {e:#}", c.title()),
                Ok(Loaded { song, .. }) => {
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