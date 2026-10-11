//! Optimal scores in the details panel: once a song has been open there briefly, its chart is
//! downloaded and the optimal score worked out for each guitar-type part, in the background.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, channel};
use web_time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};

use crate::api::{Chart, Client, DIFFICULTIES, INSTRUMENTS};
use crate::i18n::t;
use crate::optimal::{self, Outcome};
use crate::preview::{self, thousands};

/// Wait this long after a song is opened before downloading, so clicking through the list
/// doesn't download every chart passed on the way.
const SETTLE: Duration = Duration::from_millis(500);
/// How many songs' results to keep.
const KEEP: usize = 30;
const STAR: Color32 = Color32::from_rgb(130, 235, 255);

/// (instrument, optimal score) for each part at the difficulty asked for.
type Parts = Vec<(u8, Outcome)>;

enum Entry {
    /// Being worked out, and how far along (0 to 1000: downloading, then each part in turn).
    Working(Receiver<Result<Parts, String>>, Arc<AtomicU32>),
    Done(Result<Parts, String>),
}

pub struct Scores {
    client: Client,
    /// Keyed by (md5, difficulty), with the order they were added for trimming.
    entries: HashMap<(String, u8), Entry>,
    order: Vec<(String, u8)>,
    /// The song shown and when it was first shown.
    shown: Option<((String, u8), Instant)>,
    /// The difficulty picked in the section (otherwise the search's, or Expert).
    picked: Option<u8>,
}

impl Scores {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            entries: HashMap::new(),
            order: Vec::new(),
            shown: None,
            picked: None,
        }
    }

    /// The details panel's optimal score section for `chart` at `diff` (0 = Expert).
    pub fn show(&mut self, ui: &mut egui::Ui, chart: &Chart, diff: u8) {
        let parts: Vec<u8> = (0..INSTRUMENTS.len() as u8)
            .filter(|&i| chart.diffs[i as usize] >= 0)
            .collect();
        if parts.is_empty() {
            return;
        }
        ui.add_space(10.0);
        // Collapsed, nothing is downloaded or worked out.
        let tip = t("Every note hit, with star power used on the best path (squeezing and whammy included), as CHOpt works it out");
        crate::section(ui, "optimal", &t("⭐ Optimal score"), Some(&tip), |ui| {
            // Difficulties the chart has, to pick from.
            // Easiest to hardest, so Expert is on the right (DIFFICULTIES runs Expert first).
            let have: Vec<u8> = (0..4u8).rev().filter(|&d| chart.notes.iter().any(|s| s.d == d)).collect();
            let mut diff = self.picked.filter(|d| have.contains(d)).unwrap_or(diff);
            if have.len() > 1 {
                crate::theme::hrow_wrapped(ui, |ui| {
                    for &d in &have {
                        if crate::pill(ui, d == diff, &t(DIFFICULTIES[d as usize].1)).clicked() {
                            self.picked = Some(d);
                            diff = d;
                        }
                    }
                });
                ui.add_space(4.0);
            }
            self.contents(ui, chart, diff, parts)
        });
    }

    fn contents(&mut self, ui: &mut egui::Ui, chart: &Chart, diff: u8, parts: Vec<u8>) {
        let key = (chart.md5.clone(), diff);
        if self.shown.as_ref().is_none_or(|(k, _)| *k != key) {
            self.shown = Some((key.clone(), Instant::now()));
        }
        if !self.entries.contains_key(&key) {
            let since = self
                .shown
                .as_ref()
                .map_or(Duration::ZERO, |(_, at)| at.elapsed());
            if since < SETTLE {
                ui.ctx().request_repaint_after(SETTLE - since);
                ui.weak(t("Working out the optimal score…"));
                return;
            }
            self.start(ui.ctx(), chart, diff, parts);
        }
        let Some(entry) = self.entries.get_mut(&key) else {
            return;
        };
        if let Entry::Working(rx, _) = entry
            && let Ok(result) = rx.try_recv()
        {
            *entry = Entry::Done(result);
        }
        match entry {
            Entry::Working(_, progress) => {
                ui.weak(t("Working out the optimal score…"));
                let done = progress.load(Relaxed) as f32 / 1000.0;
                ui.add(crate::theme::Bar::new(done).desired_height(8.0).fill(crate::theme::accent()));
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
            Entry::Done(Err(e)) => {
                ui.weak(crate::tr!("Couldn't work it out: {error}", error = e));
            }
            Entry::Done(Ok(parts)) => {
                egui::Grid::new("optimal_scores")
                    .num_columns(3)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("");
                        ui.label(RichText::new(t("Optimal")).small().weak());
                        ui.label(RichText::new(t("No star power")).small().weak())
                            .on_hover_text(t("Every note hit, but never activating star power"));
                        ui.end_row();
                        for (inst, o) in parts.iter() {
                            ui.label(t(INSTRUMENTS[*inst as usize].1));
                            ui.label(RichText::new(thousands(o.total)).strong().color(STAR))
                                .on_hover_text(crate::tr!("Path {path}", path = o.path));
                            ui.label(RichText::new(thousands(o.no_sp)).weak());
                            ui.end_row();
                        }
                    });
            }
        }
    }

    fn start(&mut self, ctx: &egui::Context, chart: &Chart, diff: u8, parts: Vec<u8>) {
        let key = (chart.md5.clone(), diff);
        let (tx, rx) = channel();
        let (client, chart, ctx) = (self.client.clone(), chart.clone(), ctx.clone());
        let progress = Arc::new(AtomicU32::new(0));
        let shared = progress.clone();
        crate::task::spawn(move || async move {
            // The first fifth of the bar is the download, the rest each part in turn.
            let download = AtomicU32::new(0);
            let result = match preview::load(&client, &chart, &download).await {
                Ok(loaded) => {
                    let count = parts.len().max(1) as f32;
                    let mut found = Vec::new();
                    for (k, &i) in parts.iter().enumerate() {
                        let report = |f: f32| shared.store((200.0 + 800.0 * (k as f32 + f) / count) as u32, Relaxed);
                        report(0.0);
                        // (On the web, lets the bar move between parts.)
                        crate::task::breathe().await;
                        if let Some(o) = optimal::optimise_cached(&chart.md5, &loaded.chart, i, diff, loaded.sustain_cutoff, &report) {
                            found.push((i, o));
                        }
                    }
                    Ok(found.into_iter().collect())
                }
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.entries.insert(key.clone(), Entry::Working(rx, progress));
        self.order.push(key);
        while self.order.len() > KEEP {
            let old = self.order.remove(0);
            self.entries.remove(&old);
        }
    }
}
