//! Clone Hero leaderboards (leaderboards.clonehero.net) for the chart in the details panel.
//!
//! The leaderboards identify a chart by its track hash (see `trackhash`), so this fetches just
//! the .sng's header and notes file from Encore with range requests, hashes every track, then
//! asks the leaderboard API for the top scores on the chosen one:
//!   GET https://api.clonehero.net/v1/scores?trackHash=<hash>&instrument=<id>&difficulty=<id>&topScores=<n>
//! With no other filters that's the site's default view: 100% speed, no modifiers, any controller.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use eframe::egui;
use egui::{Color32, CornerRadius, RichText, Sense, vec2};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde_json::Value;

use crate::api::{self, Chart, DIFFICULTIES, FILES_URL, INSTRUMENTS, flag};
use crate::i18n::t;
use crate::preview::thousands;
use crate::trackhash::{self, Modifiers};
use crate::{sng, theme, tr};

const API_URL: &str = "https://api.clonehero.net/v1";
const SITE_URL: &str = "https://leaderboards.clonehero.net";
const TOP: usize = 10;
/// How much of the .sng to fetch for its header; the notes come in a second request.
const HEADER_BYTES: u64 = 64 * 1024;

/// (instrument, difficulty, track hash) for every track in a chart.
type Hashes = Vec<(&'static str, &'static str, String)>;

struct Entry {
    placement: u64,
    username: String,
    account: String,
    score: u64,
    accuracy: String,
    stars: u64,
    fc: bool,
    pfc: bool,
    controller: String,
}

struct Board {
    /// The site's id for the song, for linking to it.
    song_hash: Option<String>,
    players: u64,
    plays: u64,
    entries: Vec<Entry>,
}

enum Load<T> {
    Loading(Receiver<Result<T, String>>),
    Ready(T),
    Failed(String),
}

impl<T: Send + 'static> Load<T> {
    fn start(ctx: &egui::Context, work: impl FnOnce() -> Result<T> + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(work().map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        Load::Loading(rx)
    }

    fn poll(&mut self) {
        if let Load::Loading(rx) = self
            && let Ok(result) = rx.try_recv()
        {
            *self = match result {
                Ok(v) => Load::Ready(v),
                Err(e) => Load::Failed(e),
            };
        }
    }
}

/// The leaderboard section of the details panel.
pub struct Panel {
    http: Client,
    md5: String,
    /// Track hashes per chart md5, kept for the session.
    hashes: HashMap<String, Load<Hashes>>,
    /// Boards per (track hash, leaderboard instrument, difficulty).
    boards: HashMap<(String, &'static str, &'static str), Load<Board>>,
    /// Leaderboard instrument id (scan-chart's, plus "prodrums") and difficulty.
    instrument: &'static str,
    difficulty: &'static str,
}

impl Panel {
    pub fn new() -> Self {
        let http = Client::builder()
            .user_agent(concat!("rainbow/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self { http, md5: String::new(), hashes: HashMap::new(), boards: HashMap::new(), instrument: "guitar", difficulty: "expert" }
    }

    /// Shows the leaderboard section for `c`, starting on the instrument and difficulty being
    /// searched for when the chart has them. Nothing is downloaded while it's collapsed.
    pub fn show(&mut self, ui: &mut egui::Ui, c: &Chart, instrument: Option<u8>, difficulty: Option<u8>) {
        crate::section(ui, "leaderboard", &t("🏆  Clone Hero Leaderboard"), None, |ui| self.contents(ui, c, instrument, difficulty));
    }

    pub(crate) fn contents(&mut self, ui: &mut egui::Ui, c: &Chart, instrument: Option<u8>, difficulty: Option<u8>) {
        let ctx = ui.ctx().clone();
        let new_chart = self.md5 != c.md5;
        if new_chart {
            self.md5 = c.md5.clone();
        }
        let hashes = self.hashes.entry(c.md5.clone()).or_insert_with(|| {
            let (http, chart) = (self.http.clone(), c.clone());
            Load::start(&ctx, move || chart_hashes(&http, &chart))
        });
        hashes.poll();

        let hashes = match hashes {
            Load::Loading(_) => {
                loading(ui, &t("Reading the chart…"));
                return;
            }
            Load::Failed(e) => {
                ui.label(RichText::new(tr!("Couldn't read this chart's notes: {error}", error = e)).weak());
                if ui.small_button(format!("↻  {}", t("Retry"))).clicked() {
                    self.hashes.remove(&c.md5);
                }
                return;
            }
            Load::Ready(h) => h.clone(),
        };
        if hashes.is_empty() {
            ui.weak(t("This chart has no playable tracks."));
            return;
        }

        let choices = instrument_choices(&hashes);
        if new_chart || !choices.contains(&self.instrument) {
            self.instrument = default_instrument(&choices, c, instrument);
        }
        let difficulties = difficulty_choices(&hashes, self.instrument);
        if new_chart || !difficulties.contains(&self.difficulty) {
            let wanted = difficulty.map(|d| DIFFICULTIES[d as usize].0);
            self.difficulty = wanted.filter(|d| difficulties.contains(d)).unwrap_or(difficulties[0]);
        }

        crate::theme::hrow_wrapped(ui, |ui| {
            egui::ComboBox::from_id_salt("leaderboard_instrument")
                .selected_text(instrument_name(self.instrument))
                .show_ui(ui, |ui| {
                    for &i in &choices {
                        ui.selectable_value(&mut self.instrument, i, instrument_name(i));
                    }
                });
            // The instrument may have just changed.
            let difficulties = difficulty_choices(&hashes, self.instrument);
            if !difficulties.contains(&self.difficulty) {
                self.difficulty = difficulties[0];
            }
            for d in difficulties {
                let name = DIFFICULTIES.iter().find(|(id, _)| *id == d).map_or(d, |(_, name)| *name);
                ui.selectable_value(&mut self.difficulty, d, t(name));
            }
        });

        let track = if self.instrument == "prodrums" { "drums" } else { self.instrument };
        let Some(hash) = hashes.iter().find(|h| h.0 == track && h.1 == self.difficulty).map(|h| h.2.clone()) else {
            return;
        };
        let key = (hash.clone(), self.instrument, self.difficulty);
        let board = self.boards.entry(key.clone()).or_insert_with(|| {
            let (http, instrument, difficulty) = (self.http.clone(), self.instrument, self.difficulty);
            Load::start(&ctx, move || fetch_board(&http, &hash, instrument, difficulty))
        });
        board.poll();
        let mut refresh = false;
        match board {
            Load::Loading(_) => loading(ui, &t("Loading scores…")),
            Load::Failed(e) => {
                ui.label(RichText::new(tr!("Couldn't load the leaderboard: {error}", error = e)).weak());
                refresh = ui.small_button(format!("↻  {}", t("Retry"))).clicked();
            }
            Load::Ready(b) => refresh = show_board(ui, b, self.instrument, self.difficulty),
        }
        if refresh {
            self.boards.remove(&key);
        }
    }
}

fn loading(ui: &mut egui::Ui, text: &str) {
    crate::theme::hrow(ui, |ui| {
        ui.spinner();
        ui.weak(text);
    });
}

/// Draws the scores; returns true if Refresh was clicked.
fn show_board(ui: &mut egui::Ui, b: &Board, instrument: &str, difficulty: &str) -> bool {
    if b.entries.is_empty() {
        ui.weak(t("No scores yet. Be the first!"));
    }
    ui.spacing_mut().item_spacing.y = 2.0;
    for (row, e) in b.entries.iter().enumerate() {
        // The top three on gold, silver and bronze (painted behind once the row's size is known).
        let podium = crate::players::medal(e.placement).is_some();
        let fill = if row % 2 == 0 && !podium { ui.visuals().widgets.inactive.bg_fill } else { Color32::TRANSPARENT };
        let background = ui.painter().add(egui::Shape::Noop);
        let frame = egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            crate::theme::hrow(ui, |ui| {
                let medal = match e.placement {
                    1 => Color32::from_rgb(240, 190, 40),
                    2 => Color32::from_rgb(190, 195, 205),
                    3 => Color32::from_rgb(205, 125, 60),
                    _ => ui.visuals().weak_text_color(),
                };
                let (rect, place) = ui.allocate_exact_size(vec2(30.0, 18.0), Sense::hover());
                if e.placement <= 3 {
                    // A trophy in gold, silver or bronze for the top three.
                    ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, "🏆", egui::FontId::proportional(15.0), medal);
                    ui.painter().text(rect.right_center(), egui::Align2::RIGHT_CENTER, e.placement.to_string(), egui::FontId::proportional(12.0), medal);
                    place.on_hover_text(tr!("#{place}", place = e.placement));
                } else {
                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, format!("#{}", e.placement), egui::FontId::proportional(13.0), medal);
                }
                if let Ok(id) = e.account.parse::<u64>() {
                    crate::players::small_avatar(ui, id, &e.username, 26.0);
                }
                crate::theme::vcol(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let name = ui.add(egui::Label::new(RichText::new(&e.username).strong()).truncate().sense(Sense::click()));
                    if name.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t("Open this player's profile")).clicked() {
                        match e.account.parse::<u64>() {
                            // Rainbow's own profile window.
                            Ok(id) => ui.ctx().data_mut(|d| {
                                d.insert_temp(crate::players::open_profile_id(), (id, e.username.clone()));
                            }),
                            Err(_) => ui.ctx().open_url(egui::OpenUrl::new_tab(format!("{SITE_URL}/users/{}", e.account))),
                        }
                    }
                    let mut details = format!("{}%  ·  ★{}", e.accuracy, e.stars);
                    if e.controller == "Gamepad" {
                        details.push_str("  ·  🎮");
                    }
                    crate::theme::hrow(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(RichText::new(details).size(12.0).weak());
                        if e.pfc && instrument != "drums" && instrument != "prodrums" {
                            theme::tag(ui, "👑 PFC", theme::warn());
                        } else if e.fc {
                            theme::tag(ui, "FC", theme::good());
                        }
                    });
                });
                ui.with_layout(crate::theme::trailing(), |ui| {
                    ui.label(RichText::new(thousands(e.score)).monospace().strong());
                });
            });
        });
        if let Some(shape) = crate::players::podium_background(frame.response.rect, e.placement) {
            ui.painter().set(background, shape);
        }
    }
    ui.spacing_mut().item_spacing.y = 6.0;
    let mut refresh = false;
    crate::theme::hrow_wrapped(ui, |ui| {
        ui.weak(tr!("{players} players  ·  {plays} plays", players = b.players, plays = b.plays));
        refresh = ui.small_button("↻").on_hover_text(t("Refresh the scores")).clicked();
    });
    if let Some(song) = &b.song_hash {
        let url = format!("{SITE_URL}/scores/{song}?instrument={instrument}&difficulty={difficulty}");
        let link = ui.add(egui::Button::new(RichText::new(t("View the full leaderboard ↗")).color(ui.visuals().hyperlink_color)).frame(false));
        if link.on_hover_text(&url).clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
    }
    refresh
}

/// Leaderboard instruments the chart has, in Rainbow's instrument order. Drums has two:
/// pro drums is its own leaderboard, on the same track.
fn instrument_choices(hashes: &Hashes) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (id, ..) in INSTRUMENTS {
        if hashes.iter().any(|h| h.0 == id) {
            out.push(id);
            if id == "drums" {
                out.push("prodrums");
            }
        }
    }
    out
}

fn default_instrument(choices: &[&'static str], c: &Chart, wanted: Option<u8>) -> &'static str {
    let wanted = wanted.map(|i| INSTRUMENTS[i as usize].0);
    let pick = wanted.filter(|w| choices.contains(w)).or_else(|| choices.iter().copied().find(|&i| i == "guitar")).unwrap_or(choices[0]);
    // Charts with cymbals are played as pro drums.
    if pick == "drums" && c.drum_type == Some(1) { "prodrums" } else { pick }
}

fn difficulty_choices(hashes: &Hashes, instrument: &str) -> Vec<&'static str> {
    let track = if instrument == "prodrums" { "drums" } else { instrument };
    trackhash::DIFFICULTIES.into_iter().filter(|d| hashes.iter().any(|h| h.0 == track && h.1 == *d)).collect()
}

pub(crate) fn instrument_name(id: &str) -> String {
    if id == "prodrums" {
        return t("Pro Drums");
    }
    INSTRUMENTS.iter().find(|(i, ..)| *i == id).map_or_else(|| id.to_owned(), |(_, name, _)| t(name))
}

/// Gets the .sng's header and notes with range requests, and hashes every track.
fn chart_hashes(http: &Client, c: &Chart) -> Result<Hashes> {
    let novideo = if c.has(flag::VIDEO) { "_novideo" } else { "" };
    let url = format!("{FILES_URL}/{}{novideo}.sng", c.md5);
    let fetch = |start: u64, len: u64| -> Result<Vec<u8>> {
        let resp = http.get(&url).header(reqwest::header::RANGE, format!("bytes={start}-{}", start + len.max(1) - 1)).send()?;
        let partial = resp.status() == StatusCode::PARTIAL_CONTENT;
        let bytes = resp.error_for_status()?.bytes()?;
        // A server that ignores the range sends the whole file.
        Ok(if partial { bytes.to_vec() } else { bytes.get(start as usize..).unwrap_or_default().iter().take(len as usize).copied().collect() })
    };
    let head = fetch(0, HEADER_BYTES)?;
    let contents = sng::contents(&head).or_else(|_| sng::contents(&fetch(0, 16 * 1024 * 1024)?)).context("not a readable .sng")?;

    // Like scan-chart: notes.mid, then notes.chart, then any other .mid/.chart.
    let mut candidates: Vec<&(String, usize, usize)> = contents
        .files
        .iter()
        .filter(|f| {
            let n = f.0.to_lowercase();
            n.ends_with(".mid") || n.ends_with(".chart")
        })
        .collect();
    candidates.sort_by_key(|f| {
        let n = f.0.to_lowercase();
        (!(n == "notes.mid" || n == "notes.chart"), !n.ends_with(".mid"))
    });
    let Some((name, offset, len)) = candidates.first() else { bail!("it has no notes.chart or notes.mid") };
    let notes = contents.unmask(&fetch(*offset as u64, *len as u64)?);
    if notes.len() != *len {
        bail!("couldn't download the notes");
    }
    let values: HashMap<String, String> = contents.values.iter().map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())).collect();
    trackhash::track_hashes(&notes, name.to_lowercase().ends_with(".mid"), &Modifiers::from_ini(&values))
}

fn fetch_board(http: &Client, track_hash: &str, instrument: &str, difficulty: &str) -> Result<Board> {
    let url = format!("{API_URL}/scores?trackHash={track_hash}&instrument={instrument}&difficulty={difficulty}&topScores={TOP}");
    let v: Value = http.get(url).send()?.error_for_status()?.json()?;
    let num = |v: &Value| v.as_u64().or_else(|| v.as_str()?.parse().ok()).unwrap_or(0);
    let text = |v: &Value| v.as_str().map(str::to_owned).unwrap_or_else(|| if v.is_null() { String::new() } else { v.to_string() });
    let summary = &v["summary"];
    let entries = v["top"]["scoreData"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Entry {
            placement: num(&s["placement"]),
            username: api::strip_tags(&text(&s["username"])),
            account: text(&s["accountId"]),
            score: num(&s["totalScore"]),
            accuracy: text(&s["accuracy"]),
            stars: num(&s["stars"]),
            fc: s["isFc"].as_bool().unwrap_or(false),
            pfc: s["isPfc"].as_bool().unwrap_or(false),
            controller: text(&s["controllerType"]),
        })
        .collect();
    Ok(Board {
        song_hash: summary["song"]["songHash"].as_str().map(str::to_owned),
        players: num(&summary["totalPlayerCount"]),
        plays: num(&summary["totalPlayCount"]),
        entries,
    })
}

#[cfg(test)]
mod tests {
    /// Fetches the leaderboards for real charts. Run with:
    /// LEADERBOARD_MD5=<md5>[,<md5>...] cargo test leaderboard -- --ignored --nocapture
    #[test]
    #[ignore]
    fn fetch_live() {
        let http = super::Client::new();
        for md5 in std::env::var("LEADERBOARD_MD5").expect("set LEADERBOARD_MD5").split(',') {
            let chart = crate::api::Chart { md5: md5.to_owned(), ..Default::default() };
            let hashes = super::chart_hashes(&http, &chart).unwrap();
            let (instrument, difficulty, hash) = hashes.iter().find(|h| h.0 == "guitar" && h.1 == "expert").or(hashes.first()).unwrap();
            let board = super::fetch_board(&http, hash, instrument, difficulty).unwrap();
            println!("{md5}: {} tracks; {instrument} {difficulty} {hash}: {} players, {} plays", hashes.len(), board.players, board.plays);
            for e in &board.entries {
                println!("  #{} {} {} {}% ★{} fc={} {}", e.placement, e.username, e.score, e.accuracy, e.stars, e.fc, e.controller);
            }
        }
    }
}
