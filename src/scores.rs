//! Optimal scores in the details panel: once a song has been open there briefly, its chart is
//! downloaded and the optimal score worked out for each guitar-type part, in the background.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};

use crate::api::{Chart, Client, INSTRUMENTS};
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
    Working(Receiver<Result<Parts, String>>),
    Done(Result<Parts, String>),
}

pub struct Scores {
    client: Client,
    /// Keyed by (md5, difficulty), with the order they were added for trimming.
    entries: HashMap<(String, u8), Entry>,
    order: Vec<(String, u8)>,
    /// The song shown and when it was first shown.
    shown: Option<((String, u8), Instant)>,
}

impl Scores {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            entries: HashMap::new(),
            order: Vec::new(),
            shown: None,
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
        crate::section(ui, "optimal", &t("⭐ Optimal score"), Some(&tip), |ui| self.contents(ui, chart, diff, parts));
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
        if let Entry::Working(rx) = entry
            && let Ok(result) = rx.try_recv()
        {
            *entry = Entry::Done(result);
        }
        match entry {
            Entry::Working(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak(t("Working out the optimal score…"));
                });
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
        thread::spawn(move || {
            let result = preview::load(&client, &chart)
                .map_err(|e| format!("{e:#}"))
                .map(|loaded| {
                    parts
                        .into_iter()
                        .filter_map(|i| {
                            optimal::optimise(
                                &loaded.chart,
                                i,
                                diff,
                                loaded.sustain_cutoff,
                                optimal::Options::default(),
                            )
                            .map(|o| (i, o))
                        })
                        .collect()
                });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.entries.insert(key.clone(), Entry::Working(rx));
        self.order.push(key);
        while self.order.len() > KEEP {
            let old = self.order.remove(0);
            self.entries.remove(&old);
        }
    }
}
