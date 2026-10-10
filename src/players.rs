//! Clone Hero players, from Clone Hero's public leaderboard API: finding players by name,
//! their profile (stats, how they rank, what they play, their latest scores) with charts, and
//! comparing two players.
//!   GET /v1/search?q=<text>                 -> matching songs and players
//!   GET /v1/account/<id>/profile            -> stats, ranks, top artists/instruments, scores
//!   GET /v1/stats/site-totals               -> how many players there are, for percentiles
//!   GET /v1/scores?songHash=..&accountId=.. -> a player's placement on a song

use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use eframe::egui;
use egui::{Color32, CornerRadius, RichText, Sense, pos2, vec2};
use reqwest::blocking::Client;
use serde_json::Value;

use crate::api::strip_tags;
use crate::i18n::t;
use crate::n;
use crate::settings::Density;
use crate::preview::thousands;
use crate::{theme, tr};

const API_URL: &str = "https://api.clonehero.net/v1";
const SITE_URL: &str = "https://leaderboards.clonehero.net";
/// Wait this long after typing stops before searching for players.
const SEARCH_DELAY: Duration = Duration::from_millis(600);

/// Asks the app to open a player's profile: put (account id, name) here in egui's memory.
pub fn open_profile_id() -> egui::Id {
    egui::Id::new("rainbow_open_player")
}

/// Asks the app to search the catalogue for this text.
pub fn search_request_id() -> egui::Id {
    egui::Id::new("rainbow_search_request")
}

/// Chart colours by job, in a fixed order (validated for colour-blind separation on Rainbow's
/// light and dark surfaces): first player / notes, second player / sustains, solos, combo,
/// star power, other.
fn series(k: usize) -> Color32 {
    const LIGHT: [Color32; 6] = [
        Color32::from_rgb(0x2a, 0x78, 0xd6),
        Color32::from_rgb(0xeb, 0x68, 0x34),
        Color32::from_rgb(0x1b, 0xaf, 0x7a),
        Color32::from_rgb(0xed, 0xa1, 0x00),
        Color32::from_rgb(0xe8, 0x7b, 0xa4),
        Color32::from_rgb(0x00, 0x83, 0x00),
    ];
    const DARK: [Color32; 6] = [
        Color32::from_rgb(0x39, 0x87, 0xe5),
        Color32::from_rgb(0xd9, 0x59, 0x26),
        Color32::from_rgb(0x19, 0x9e, 0x70),
        Color32::from_rgb(0xc9, 0x85, 0x00),
        Color32::from_rgb(0xd5, 0x51, 0x81),
        Color32::from_rgb(0x00, 0x83, 0x00),
    ];
    if theme::palette().dark { DARK[k % 6] } else { LIGHT[k % 6] }
}

// ─────────────────────────────── Loading ───────────────────────────────

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

    fn poll(&mut self) -> &mut Self {
        if let Load::Loading(rx) = self
            && let Ok(result) = rx.try_recv()
        {
            *self = match result {
                Ok(v) => Load::Ready(v),
                Err(e) => Load::Failed(e),
            };
        }
        self
    }

    fn ready(&mut self) -> Option<&T> {
        match self.poll() {
            Load::Ready(v) => Some(v),
            _ => None,
        }
    }
}

fn get(http: &Client, path: &str, query: &[(&str, String)]) -> Result<Value> {
    let query: Vec<String> = query.iter().map(|(k, v)| format!("{k}={}", encode(v))).collect();
    let url = if query.is_empty() { format!("{API_URL}{path}") } else { format!("{API_URL}{path}?{}", query.join("&")) };
    let resp = http.get(url).send()?.error_for_status()?;
    Ok(resp.json()?)
}

/// Percent-encodes a query value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn num(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)).or_else(|| v.as_str()?.parse().ok()).unwrap_or(0)
}

fn text(v: &Value) -> String {
    v.as_str().map(strip_tags).unwrap_or_default()
}

// ─────────────────────────────── Data ───────────────────────────────

#[derive(Clone)]
struct Found {
    id: u64,
    name: String,
}

fn search_players(http: &Client, query: &str) -> Result<Vec<Found>> {
    let v = get(http, "/search", &[("q", query.to_owned()), ("songsPage", "1".into()), ("usersPage", "1".into()), ("perPage", "8".into())])?;
    Ok(v["users"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|u| Found { id: num(&u["accountId"]), name: text(&u["username"]) })
        .filter(|f| f.id > 0)
        .collect())
}

/// One of a profile's stats, with its rank among all players (1 = best) if it has one.
struct Stat {
    icon: &'static str,
    label: &'static str,
    value: u64,
    shown: String,
    rank: Option<u64>,
}

#[derive(Clone)]
struct Score {
    song_hash: String,
    name: String,
    artist: String,
    instrument: String,
    difficulty: String,
    total: u64,
    accuracy: String,
    stars: u64,
    fc: bool,
    pfc: bool,
    max_combo: u64,
    sp_hit: u64,
    sp_count: u64,
    squeezes: u64,
    when: String,
    /// Score from notes, sustains, solos, combo, star power and everything else.
    parts: [u64; 6],
}

struct Profile {
    id: u64,
    name: String,
    joined: String,
    stats: Vec<Stat>,
    instruments: Vec<(String, u64)>,
    artists: Vec<(String, u64)>,
    most_played: Vec<(String, String, u64)>,
    latest: Vec<Score>,
    /// How many players have scores, for percentiles.
    players: u64,
    /// Their picture (e.g. from Discord), or the colour of their default avatar.
    avatar: Option<egui::ColorImage>,
    avatar_color: Color32,
}

const PARTS: [&str; 6] = [n("Notes"), n("Sustains"), n("Solos"), n("Combo"), n("Star power"), n("Other")];

fn fetch_profile(http: &Client, id: u64) -> Result<Profile> {
    let v = get(http, &format!("/account/{id}/profile"), &[])?;
    let players = get(http, "/stats/site-totals", &[]).map(|t| num(&t["users"])).unwrap_or(0);
    let (s, r) = (&v["stats"], &v["ranks"]);
    let rank = |k: &str| Some(num(&r[k])).filter(|&n| n > 0);
    let hours = num(&s["totalTimePlayedMs"]) / 3_600_000;
    let stats = vec![
        Stat { icon: "💯", label: n("Total score"), value: num(&s["totalScore"]), shown: thousands(num(&s["totalScore"])), rank: rank("totalScore") },
        Stat { icon: "🎵", label: n("Songs played"), value: num(&s["songsPlayed"]), shown: thousands(num(&s["songsPlayed"])), rank: rank("submissions") },
        Stat { icon: "🎯", label: n("Notes hit"), value: num(&s["notesHit"]), shown: thousands(num(&s["notesHit"])), rank: rank("notesHit") },
        Stat { icon: "🔥", label: n("Full combos"), value: num(&s["totalFcs"]), shown: thousands(num(&s["totalFcs"])), rank: rank("totalFcs") },
        Stat { icon: "💎", label: n("Perfect FCs"), value: num(&s["totalPfcs"]), shown: thousands(num(&s["totalPfcs"])), rank: rank("totalPfcs") },
        Stat { icon: "⭐", label: n("Stars earned"), value: num(&s["totalStarsEarned"]), shown: thousands(num(&s["totalStarsEarned"])), rank: rank("starsEarned") },
        Stat { icon: "⏱", label: n("Hours played"), value: hours, shown: thousands(hours), rank: rank("hoursPlayed") },
        Stat { icon: "⚡", label: n("Highest combo"), value: num(&s["highestCombo"]), shown: thousands(num(&s["highestCombo"])), rank: None },
    ];
    let pairs = |key: &str, name: &str| -> Vec<(String, u64)> {
        v[key].as_array().into_iter().flatten().map(|e| (text(&e[name]), num(&e["plays"]))).collect()
    };
    let latest = v["latestSubmissions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            let part = |k: &str| num(&e[k]);
            let known = part("noteScore") + part("sustainScore") + part("soloScore") + part("comboScore") + part("spScore");
            Score {
                song_hash: text(&e["songHash"]),
                name: text(&e["name"]),
                artist: text(&e["artist"]),
                instrument: text(&e["instrument"]),
                difficulty: text(&e["difficulty"]),
                total: part("totalScore"),
                accuracy: text(&e["accuracy"]),
                stars: part("stars"),
                fc: e["isFc"].as_bool().unwrap_or(false),
                pfc: e["isPfc"].as_bool().unwrap_or(false),
                max_combo: part("maxCombo"),
                sp_hit: part("spPhrasesHit"),
                sp_count: part("spPhraseCount"),
                squeezes: part("squeezes"),
                when: text(&e["createdAt"]).get(..10).unwrap_or_default().to_owned(),
                parts: [
                    part("noteScore"),
                    part("sustainScore"),
                    part("soloScore"),
                    part("comboScore"),
                    part("spScore"),
                    part("totalScore").saturating_sub(known),
                ],
            }
        })
        .collect();
    Ok(Profile {
        id,
        name: text(&v["username"]),
        joined: text(&v["createdAt"]).get(..10).unwrap_or_default().to_owned(),
        stats,
        instruments: pairs("topInstruments", "instrument"),
        artists: pairs("topArtists", "artist"),
        most_played: v["mostPlayed"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| (text(&e["name"]), text(&e["artist"]), num(&e["plays"])))
            .collect(),
        latest,
        players,
        avatar: v["profilePictureUrl"].as_str().and_then(|url| fetch_picture(http, url).ok()),
        avatar_color: default_avatar_color(&text(&v["avatarSourceKey"])),
    })
}

/// Downloads a profile picture, shrunk to avatar size.
fn fetch_picture(http: &Client, url: &str) -> Result<egui::ColorImage> {
    let bytes = http.get(url).send()?.error_for_status()?.bytes()?;
    let img = image::load_from_memory(&bytes)?.resize_to_fill(128, 128, image::imageops::FilterType::Triangle).to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

/// The colour of a default avatar like "default-blue".
fn default_avatar_color(key: &str) -> Color32 {
    match key.rsplit('-').next().unwrap_or_default() {
        "red" => Color32::from_rgb(226, 72, 72),
        "orange" => Color32::from_rgb(235, 125, 50),
        "yellow" => Color32::from_rgb(225, 180, 40),
        "green" => Color32::from_rgb(60, 175, 90),
        "purple" | "violet" => Color32::from_rgb(140, 100, 220),
        "pink" => Color32::from_rgb(220, 100, 160),
        "teal" | "cyan" => Color32::from_rgb(40, 170, 180),
        _ => Color32::from_rgb(60, 130, 220),
    }
}
/// A player's placement on one song's leaderboard, and how many players it has.
fn fetch_placement(http: &Client, id: u64, s: &Score) -> Result<(u64, u64)> {
    let v = get(
        http,
        "/scores",
        &[
            ("songHash", s.song_hash.clone()),
            ("instrument", s.instrument.clone()),
            ("difficulty", s.difficulty.clone()),
            ("topScores", "1".into()),
            ("accountId", id.to_string()),
            ("nearHigher", "0".into()),
            ("nearLower", "0".into()),
        ],
    )?;
    let mine = v["near"]["scoreData"].as_array().into_iter().flatten().chain(v["top"]["scoreData"].as_array().into_iter().flatten()).find(|e| num(&e["accountId"]) == id);
    let placement = mine.map(|e| num(&e["placement"])).context("not on this leaderboard")?;
    Ok((placement, num(&v["summary"]["totalPlayerCount"])))
}

/// "Top 3%" style text for a rank among `of` players.
fn top_percent(rank: u64, of: u64) -> String {
    if of == 0 {
        return tr!("#{rank}", rank = thousands(rank));
    }
    let p = rank as f64 / of as f64 * 100.0;
    let shown = if p < 1.0 { format!("{p:.2}") } else if p < 10.0 { format!("{p:.1}") } else { format!("{p:.0}") };
    tr!("Top {percent}%", percent = shown)
}

// ─────────────────────────────── State and UI ───────────────────────────────

/// Leaderboard stats: API name, icon and label.
const METRICS: [(&str, &str, &str); 7] = [
    ("totalScore", "💯", n("Total score")),
    ("submissions", "🎵", n("Songs played")),
    ("totalFcs", "🔥", n("Full combos")),
    ("totalPfcs", "💎", n("Perfect FCs")),
    ("notesHit", "🎯", n("Notes hit")),
    ("hoursPlayed", "⏱", n("Hours played")),
    ("starsEarned", "⭐", n("Stars earned")),
];
/// Song chart periods: API name and label.
const PERIODS: [(&str, &str); 5] = [("trending", n("Trending")), ("topWeek", n("This week")), ("topMonth", n("This month")), ("topYear", n("This year")), ("topAllTime", n("All time"))];
/// Icons for the site totals, in `fetch_totals` order.
const TOTAL_ICONS: [&str; 6] = ["👤", "📝", "🎵", "🎯", "🔥", "⏱"];

#[derive(Clone)]
struct Ranked {
    id: u64,
    name: String,
    value: u64,
}

#[derive(Clone)]
struct Popular {
    name: String,
    artist: String,
    plays: u64,
}

#[derive(Clone)]
struct Recent {
    id: u64,
    player: String,
    name: String,
    artist: String,
    instrument: String,
    difficulty: String,
    score: u64,
    accuracy: String,
    stars: u64,
    fc: bool,
    pfc: bool,
    when: String,
}

struct Totals(Vec<(&'static str, u64)>);

fn fetch_ranked(http: &Client, metric: &str) -> Result<Vec<Ranked>> {
    let v = get(http, "/stats/player-leaderboard", &[("metric", metric.to_owned())])?;
    Ok(v.as_array().into_iter().flatten().map(|e| Ranked { id: num(&e["accountId"]), name: text(&e["username"]), value: num(&e["value"]) }).collect())
}

fn fetch_popular(http: &Client, period: &str) -> Result<Vec<Popular>> {
    let v = get(http, "/stats/song-leaderboard", &[("tab", period.to_owned())])?;
    let mut out: Vec<Popular> =
        v.as_array().into_iter().flatten().map(|e| Popular { name: text(&e["name"]), artist: text(&e["artist"]), plays: num(&e["plays"]) }).collect();
    out.sort_by(|a, b| b.plays.cmp(&a.plays));
    Ok(out)
}

fn fetch_recent(http: &Client) -> Result<Vec<Recent>> {
    let v = get(http, "/scores/latest-scores", &[])?;
    Ok(v.as_array()
        .into_iter()
        .flatten()
        .map(|e| Recent {
            id: num(&e["accountId"]),
            player: text(&e["username"]),
            name: text(&e["name"]),
            artist: text(&e["artist"]),
            instrument: text(&e["instrument"]),
            difficulty: text(&e["difficulty"]),
            score: num(&e["totalScore"]),
            accuracy: text(&e["accuracy"]),
            stars: num(&e["stars"]),
            fc: e["isFc"].as_bool().unwrap_or(false),
            pfc: e["isPfc"].as_bool().unwrap_or(false),
            when: text(&e["createdAt"]).replace('T', " ").get(..16).unwrap_or_default().to_owned(),
        })
        .collect())
}

fn fetch_totals(http: &Client) -> Result<Totals> {
    let v = get(http, "/stats/site-totals", &[])?;
    Ok(Totals(vec![
        ("Players", num(&v["users"])),
        ("Scores submitted", num(&v["submissions"])),
        ("Songs", num(&v["songs"])),
        ("Notes hit", num(&v["notesHit"])),
        ("Full combos", num(&v["fcs"])),
        ("Hours played", num(&v["hoursPlayed"])),
    ]))
}

/// Sizes that follow the Row size setting (Compact / Default / Large).
#[derive(Clone, Copy)]
struct Sizes {
    row: f32,
    text: f32,
    value: f32,
    cover: f32,
    tile: f32,
}

fn sizes(d: Density) -> Sizes {
    match d {
        Density::Compact => Sizes { row: 28.0, text: 12.5, value: 18.0, cover: 30.0, tile: 170.0 },
        Density::Default => Sizes { row: 34.0, text: 13.5, value: 21.0, cover: 40.0, tile: 195.0 },
        Density::Large => Sizes { row: 44.0, text: 15.5, value: 25.0, cover: 56.0, tile: 230.0 },
    }
}

/// The profile shown on the Players tab.
struct Window {
    id: u64,
    name: String,
    profile: Load<Profile>,
    placements: Vec<Option<Load<(u64, u64)>>>,
    avatar: Option<egui::TextureHandle>,
}

/// The player being compared with, and their picture.
struct Other {
    name: String,
    profile: Load<Profile>,
    avatar: Option<egui::TextureHandle>,
}

pub struct Players {
    http: Client,
    /// The Charts search's player chips: the last searched text and the players it found.
    found: Option<(String, Load<Vec<Found>>)>,
    typed: (String, Instant),
    /// The Players tab's own search, and the compare search.
    find: Finder,
    compare_find: Finder,
    window: Option<Window>,
    other: Option<Other>,
    /// Profiles viewed before this one, for Back.
    history: Vec<(u64, String)>,
    /// Set when a profile was opened, so the app switches to the Players tab.
    opened: bool,
    sizes: Sizes,
    // Leaderboards.
    metric: usize,
    ranked: Vec<Option<Load<Vec<Ranked>>>>,
    period: usize,
    popular: Vec<Option<Load<Vec<Popular>>>>,
    recent: Option<Load<Vec<Recent>>>,
    totals: Option<Load<Totals>>,
    /// The newest charts in the catalogue, worked out when the catalogue's size changes.
    newest: (usize, Vec<crate::api::Chart>),
}

/// A search box for player names, with its results.
struct Finder {
    text: String,
    typed: Instant,
    found: Option<(String, Load<Vec<Found>>)>,
}

impl Finder {
    fn new() -> Self {
        Self { text: String::new(), typed: Instant::now(), found: None }
    }

    /// The box itself; returns the players found for what's typed, once they're in.
    fn show(&mut self, ui: &mut egui::Ui, http: &Client, hint: &str) -> Option<Vec<Found>> {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.text)
                .hint_text(hint)
                .font(egui::FontId::proportional(15.0))
                .margin(egui::Margin::symmetric(10, 6))
                .desired_width(f32::INFINITY),
        );
        if resp.changed() {
            self.typed = Instant::now();
        }
        let query = self.text.trim().to_owned();
        if query.chars().count() < 2 {
            return None;
        }
        if self.found.as_ref().is_none_or(|(q, _)| *q != query) {
            let waited = self.typed.elapsed();
            if waited < SEARCH_DELAY {
                ui.ctx().request_repaint_after(SEARCH_DELAY - waited);
                return None;
            }
            let (http, q) = (http.clone(), query.clone());
            self.found = Some((query.clone(), Load::start(ui.ctx(), move || search_players(&http, &q))));
        }
        let (_, load) = self.found.as_mut()?;
        match load.poll() {
            Load::Loading(_) => {
                ui.spinner();
                None
            }
            Load::Failed(e) => {
                ui.weak(tr!("Couldn't search: {error}", error = e));
                None
            }
            Load::Ready(found) => {
                if found.is_empty() {
                    ui.weak(t("No players found"));
                }
                Some(found.clone())
            }
        }
    }
}

impl Players {
    pub fn new() -> Self {
        let http = Client::builder()
            .user_agent(concat!("rainbow/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(40))
            .build()
            .unwrap_or_default();
        Self {
            http,
            found: None,
            typed: (String::new(), Instant::now()),
            find: Finder::new(),
            compare_find: Finder::new(),
            window: None,
            other: None,
            history: Vec::new(),
            opened: false,
            sizes: sizes(Density::Default),
            metric: 0,
            ranked: (0..METRICS.len()).map(|_| None).collect(),
            period: 0,
            popular: (0..PERIODS.len()).map(|_| None).collect(),
            recent: None,
            totals: None,
            newest: (0, Vec::new()),
        }
    }

    /// Follows the Row size setting.
    pub fn set_density(&mut self, d: Density) {
        self.sizes = sizes(d);
    }

    pub fn open(&mut self, ctx: &egui::Context, id: u64, name: String) {
        self.opened = true;
        if self.window.as_ref().is_some_and(|w| w.id == id) {
            return;
        }
        if let Some(w) = &self.window {
            self.history.push((w.id, w.name.clone()));
        }
        self.show_profile(ctx, id, name);
    }

    fn show_profile(&mut self, ctx: &egui::Context, id: u64, name: String) {
        let http = self.http.clone();
        self.window = Some(Window { id, name, profile: Load::start(ctx, move || fetch_profile(&http, id)), placements: Vec::new(), avatar: None });
        if self.other.as_ref().is_some_and(|o| matches!(&o.profile, Load::Ready(p) if p.id == id)) {
            self.other = None;
        }
    }

    fn compare_with(&mut self, ctx: &egui::Context, id: u64, name: String) {
        let http = self.http.clone();
        self.other = Some(Other { name, profile: Load::start(ctx, move || fetch_profile(&http, id)), avatar: None });
    }

    /// Whether a profile was opened since the last call (so the app can show the Players tab).
    pub fn take_opened(&mut self) -> bool {
        std::mem::take(&mut self.opened)
    }

    /// Players whose names match the search text, as chips under the search bar.
    pub fn search_strip(&mut self, ui: &mut egui::Ui, query: &str) {
        let query = query.trim();
        if query != self.typed.0 {
            self.typed = (query.to_owned(), Instant::now());
        }
        // Names are at least 2 characters; links and hashes aren't names.
        let searchable = query.chars().count() >= 2 && !query.contains("://") && !query.contains('/');
        if searchable && self.found.as_ref().is_none_or(|(q, _)| q != query) {
            let waited = self.typed.1.elapsed();
            if waited < SEARCH_DELAY {
                ui.ctx().request_repaint_after(SEARCH_DELAY - waited);
            } else {
                let (http, q) = (self.http.clone(), query.to_owned());
                self.found = Some((q.clone(), Load::start(ui.ctx(), move || search_players(&http, &q))));
            }
        }
        if !searchable {
            return;
        }
        let Some((q, load)) = &mut self.found else { return };
        if q != query {
            return;
        }
        let Some(found) = load.ready().cloned() else { return };
        if found.is_empty() {
            return;
        }
        let mut open = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(t("👤 Players:")).weak());
            for f in &found {
                if theme::chip(ui, &f.name, theme::accent()).on_hover_text(t("Open this player's profile")).clicked() {
                    open = Some(f.clone());
                }
            }
        });
        if let Some(f) = open {
            self.open(ui.ctx(), f.id, f.name);
        }
    }

    /// Picks up requests from elsewhere (e.g. a leaderboard name) to open a profile.
    pub fn take_requests(&mut self, ctx: &egui::Context) {
        if let Some((id, name)) = ctx.data_mut(|d| d.remove_temp::<(u64, String)>(open_profile_id())) {
            self.open(ctx, id, name);
        }
    }

    /// The Players tab: finding and comparing players on the left, the profile on the right.
    /// `can_return` says there's another tab to go back to; returns true when Back asks for it.
    pub fn players_tab(&mut self, ui: &mut egui::Ui, can_return: bool) -> bool {
        let ctx = ui.ctx().clone();
        let z = self.sizes;
        egui::Panel::left("player_search").resizable(true).default_size(290.0).size_range(230.0..=440.0).show(ui, |ui| {
            crate::fancy_scroll(ui, "players_side", |ui| {
                ui.add_space(8.0);
                if let Some(found) = self.find.show(ui, &self.http.clone(), &t("🔍  Find a player…")) {
                    let current = self.window.as_ref().map(|w| w.id);
                    let mut pick = None;
                    for (k, f) in found.iter().enumerate() {
                        let resp = row(ui, k, z.row, true, current == Some(f.id), |ui| {
                            plain(ui, RichText::new(format!("👤  {}", f.name)).size(z.text).strong());
                        });
                        if resp.on_hover_text(t("Open this player's profile")).clicked() {
                            pick = Some(f.clone());
                        }
                    }
                    if let Some(f) = pick {
                        self.open(&ctx, f.id, f.name);
                        self.opened = false;
                    }
                }

                // Compare: right under the search, since it's a main feature.
                if self.window.is_some() {
                    ui.add_space(10.0);
                    crate::section(ui, "side_compare", &t("⚖  Compare"), Some(&t("Put another player's stats side by side with this profile")), |ui| {
                        if let Some(name) = self.other.as_ref().map(|o| o.name.clone()) {
                            ui.horizontal(|ui| {
                                plain(ui, RichText::new(format!("⚖  {name}")).size(z.text).strong().color(series(1)));
                                if ui.small_button("✖").on_hover_text(t("Stop comparing")).clicked() {
                                    self.other = None;
                                }
                            });
                            ui.add_space(4.0);
                        }
                        if let Some(found) = self.compare_find.show(ui, &self.http.clone(), &t("Compare with…")) {
                            let mut pick = None;
                            for (k, f) in found.iter().enumerate() {
                                if row(ui, k, z.row, true, false, |ui| {
                                    plain(ui, RichText::new(format!("⚖  {}", f.name)).size(z.text));
                                })
                                .clicked()
                                {
                                    pick = Some(f.clone());
                                }
                            }
                            if let Some(f) = pick {
                                self.compare_with(&ctx, f.id, f.name);
                                self.compare_find.text.clear();
                            }
                        }
                    });
                }

                // The top players, by any stat.
                ui.add_space(10.0);
                let mut pick = None;
                crate::section(ui, "side_top_players", &t("🏆  Top players"), Some(&t("The best ten players for each stat")), |ui| {
                    metric_picker(ui, "side_metric", &mut self.metric);
                    ui.add_space(4.0);
                    let metric = self.metric;
                    let http = self.http.clone();
                    let load = self.ranked[metric].get_or_insert_with(|| {
                        let m = METRICS[metric].0;
                        Load::start(&ctx, move || fetch_ranked(&http, m))
                    });
                    let current = self.window.as_ref().map(|w| w.id);
                    show_load(ui, load, |ui, rows: &Vec<Ranked>| {
                        for (k, r) in rows.iter().enumerate() {
                            let resp = row(ui, k, z.row, true, current == Some(r.id), |ui| {
                                place_badge(ui, k as u64 + 1);
                                plain(ui, RichText::new(&r.name).size(z.text).strong());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    plain(ui, RichText::new(thousands(r.value)).size(z.text - 1.0).weak());
                                });
                            });
                            if resp.on_hover_text(t("Open this player's profile")).clicked() {
                                pick = Some((r.id, r.name.clone()));
                            }
                        }
                    });
                });
                if let Some((id, name)) = pick {
                    self.open(&ctx, id, name);
                    self.opened = false;
                }
            });
        });

        let mut back_to_tab = false;
        egui::CentralPanel::default_margins().show(ui, |ui| {
            if self.window.is_none() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.label(RichText::new(t("👤 Clone Hero players")).heading());
                    ui.weak(t("Search for a player on the left, or click a name on any leaderboard."));
                });
                return;
            }
            // Back: the previous profile, or the tab this came from.
            if (!self.history.is_empty() || can_return)
                && ui.add(egui::Button::new(RichText::new(t("←  Back")).size(14.0)).min_size(vec2(0.0, 28.0))).clicked()
            {
                match self.history.pop() {
                    Some((id, name)) => self.show_profile(&ctx, id, name),
                    None => back_to_tab = true,
                }
            }
            let http = self.http.clone();
            let (window, other) = (&mut self.window, &mut self.other);
            if let Some(w) = window {
                crate::fancy_scroll(ui, "player_profile", |ui| profile_view(ui, &http, w, other.as_mut(), z));
            }
        });
        back_to_tab
    }

    /// The Leaderboards tab: site totals, top players, popular songs, new charts and the
    /// newest scores.
    pub fn leaderboards_tab(&mut self, ui: &mut egui::Ui, charts: &[crate::api::Chart], covers: &mut crate::covers::Covers) {
        let z = self.sizes;
        if self.newest.0 != charts.len() {
            // The 15 most recently added or updated charts on Encore.
            let mut newest: Vec<&crate::api::Chart> = charts.iter().filter(|c| c.modified.is_some()).collect();
            let n = newest.len().min(15);
            if n > 0 {
                newest.select_nth_unstable_by(n - 1, |a, b| b.modified.cmp(&a.modified));
                newest.truncate(n);
                newest.sort_by(|a, b| b.modified.cmp(&a.modified));
            }
            self.newest = (charts.len(), newest.into_iter().cloned().collect());
        }
        let ctx = ui.ctx().clone();
        let http = self.http.clone();
        let mut open = None;
        crate::fancy_scroll(ui, "leaderboards", |ui| {
            ui.add_space(8.0);
            let totals = self.totals.get_or_insert_with(|| {
                let http = http.clone();
                Load::start(&ctx, move || fetch_totals(&http))
            });
            if let Some(Totals(list)) = totals.ready() {
                let stats: Vec<Stat> = list.iter().enumerate().map(|(k, (label, value))| Stat { icon: TOTAL_ICONS[k], label, value: *value, shown: thousands(*value), rank: None }).collect();
                tile_rows(ui, z, &stats, |ui, s| stat_tile(ui, z, s, 0));
                ui.add_space(8.0);
            }

            let wide = ui.available_width() > 900.0;
            let body = |ui: &mut egui::Ui, col: usize, me: &mut Self, covers: &mut crate::covers::Covers, open: &mut Option<(u64, String)>| {
                if col == 0 {
                    crate::section(ui, "lb_players", &t("🏆  Top players"), Some(&t("The best ten players for each stat")), |ui| {
                        metric_picker(ui, "lb_metric", &mut me.metric);
                        ui.add_space(4.0);
                        let metric = me.metric;
                        let load = me.ranked[metric].get_or_insert_with(|| {
                            let http = http.clone();
                            let m = METRICS[metric].0;
                            Load::start(&ctx, move || fetch_ranked(&http, m))
                        });
                        show_load(ui, load, |ui, rows: &Vec<Ranked>| {
                            let max = rows.iter().map(|r| r.value).max().unwrap_or(1).max(1);
                            for (k, r) in rows.iter().enumerate() {
                                let resp = row(ui, k, z.row, true, false, |ui| {
                                    place_badge(ui, k as u64 + 1);
                                    plain(ui, RichText::new(&r.name).size(z.text).strong());
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        plain(ui, RichText::new(thousands(r.value)).size(z.text).monospace());
                                        mini_bar(ui, 110.0, r.value as f32 / max as f32, theme::accent());
                                    });
                                });
                                if resp.on_hover_text(t("Open this player's profile")).clicked() {
                                    *open = Some((r.id, r.name.clone()));
                                }
                            }
                        });
                    });
                    ui.add_space(8.0);
                    crate::section(ui, "lb_songs", &t("🔥  Popular songs"), Some(&t("The most played songs; click one to find it")), |ui| {
                        egui::ComboBox::from_id_salt("lb_period").selected_text(t(PERIODS[me.period].1)).width(160.0).show_ui(ui, |ui| {
                            for (k, (_, label)) in PERIODS.iter().enumerate() {
                                ui.selectable_value(&mut me.period, k, t(label));
                            }
                        });
                        ui.add_space(4.0);
                        let period = me.period;
                        let load = me.popular[period].get_or_insert_with(|| {
                            let http = http.clone();
                            let p = PERIODS[period].0;
                            Load::start(&ctx, move || fetch_popular(&http, p))
                        });
                        show_load(ui, load, |ui, rows: &Vec<Popular>| {
                            let max = rows.iter().map(|r| r.plays).max().unwrap_or(1).max(1);
                            for (k, r) in rows.iter().take(20).enumerate() {
                                let resp = row(ui, k, z.row, true, false, |ui| {
                                    plain(ui, RichText::new(format!("{:>2}", k + 1)).size(z.text).weak().monospace());
                                    plain(ui, RichText::new(&r.name).size(z.text).strong());
                                    plain(ui, RichText::new(&r.artist).size(z.text).weak());
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        plain(ui, RichText::new(thousands(r.plays)).size(z.text).monospace());
                                        mini_bar(ui, 90.0, r.plays as f32 / max as f32, theme::accent());
                                    });
                                });
                                if resp.on_hover_text(t("Search for this song")).clicked() {
                                    request_search(ui.ctx(), &format!("{} {}", r.artist, r.name));
                                }
                            }
                        });
                    });
                } else {
                    crate::section(ui, "lb_new_charts", &t("✨  New on Encore"), Some(&t("The charts most recently added to Chorus Encore; click one to see it in Charts")), |ui| {
                        for (k, c) in me.newest.1.iter().enumerate() {
                            let height = z.cover + 10.0;
                            let resp = row(ui, k, height, true, false, |ui| {
                                crate::cover(ui, covers, c.art.as_deref(), z.cover, crate::covers::THUMB);
                                ui.add_space(4.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    plain(ui, RichText::new(format!("{} · {}", c.name.as_deref().unwrap_or("?"), c.artist.as_deref().unwrap_or("?"))).size(z.text).strong());
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        crate::tags(ui, c, false);
                                        for (i, letter) in [(0usize, "G"), (3, "B"), (4, "D"), (5, "K")] {
                                            let d = c.diffs[i];
                                            if d >= 0 {
                                                plain(ui, RichText::new(format!("{letter}{d}")).size(z.text - 1.5).strong().color(theme::difficulty_color(d)));
                                            }
                                        }
                                        if let Some(g) = &c.genre {
                                            plain(ui, RichText::new(g).size(z.text - 1.5).weak());
                                        }
                                    });
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    plain(ui, RichText::new(c.uploaded()).size(z.text - 1.5).weak());
                                    if let Some(ch) = &c.charter {
                                        plain(ui, RichText::new(format!("✏ {ch}")).size(z.text - 1.5).weak());
                                    }
                                });
                            });
                            if resp.on_hover_text(t("Show this chart in Charts")).clicked() {
                                request_search(ui.ctx(), &c.md5);
                            }
                        }
                    });
                    ui.add_space(8.0);
                    crate::section(ui, "lb_latest", &t("🕒  Latest scores"), Some(&t("The newest scores submitted to Clone Hero; click one to see the player")), |ui| {
                        if ui.small_button(format!("↻  {}", t("Refresh"))).clicked() {
                            me.recent = None;
                        }
                        let load = me.recent.get_or_insert_with(|| {
                            let http = http.clone();
                            Load::start(&ctx, move || fetch_recent(&http))
                        });
                        show_load(ui, load, |ui, rows: &Vec<Recent>| {
                            for (k, r) in rows.iter().take(50).enumerate() {
                                let line = ScoreLine {
                                    title: format!("{} · {}", r.name, r.artist),
                                    who: Some(r.player.clone()),
                                    instrument: &r.instrument,
                                    difficulty: &r.difficulty,
                                    accuracy: &r.accuracy,
                                    stars: r.stars,
                                    fc: r.fc,
                                    pfc: r.pfc,
                                    when: &r.when,
                                    score: r.score,
                                    placement: None,
                                    detail: None,
                                };
                                if score_row(ui, k, z, &line, None).on_hover_text(t("Open this player's profile")).clicked() {
                                    *open = Some((r.id, r.player.clone()));
                                }
                            }
                        });
                    });
                }
            };
            if wide {
                ui.columns(2, |cols| {
                    body(&mut cols[0], 0, self, covers, &mut open);
                    body(&mut cols[1], 1, self, covers, &mut open);
                });
            } else {
                body(ui, 0, self, covers, &mut open);
                body(ui, 1, self, covers, &mut open);
            }
        });
        if let Some((id, name)) = open {
            self.open(&ctx, id, name);
        }
    }
}

// ─────────────────────────────── Profile ───────────────────────────────

fn profile_view(ui: &mut egui::Ui, http: &Client, w: &mut Window, other: Option<&mut Other>, z: Sizes) {
    let p = match w.profile.poll() {
        Load::Loading(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(&w.name).strong());
                ui.weak(t("Loading the profile…"));
            });
            return;
        }
        Load::Failed(e) => {
            ui.label(RichText::new(tr!("Couldn't load the profile: {error}", error = e)).weak());
            return;
        }
        Load::Ready(p) => p,
    };
    ui.add_space(8.0);
    let avatar = avatar_texture(ui.ctx(), &mut w.avatar, p, "avatar_main");
    header_card(ui, p, avatar.as_ref());
    ui.add_space(10.0);

    // Comparing comes first: it's what this screen is most often for.
    if let Some(o) = other {
        let name = o.name.clone();
        match o.profile.poll() {
            Load::Loading(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak(tr!("Loading {name}…", name = name));
                });
            }
            Load::Failed(e) => {
                ui.weak(tr!("Couldn't load the profile: {error}", error = e));
            }
            Load::Ready(op) => {
                let other_avatar = avatar_texture(ui.ctx(), &mut o.avatar, op, "avatar_other");
                crate::section(ui, "player_compare", &tr!("⚖  {a} vs {b}", a = p.name, b = op.name), Some(&t("Each stat's share of the two players' combined total")), |ui| {
                    compare_view(ui, p, op, avatar.as_ref(), other_avatar.as_ref(), z);
                });
                ui.add_space(8.0);
            }
        }
    }

    crate::section(ui, "player_stats", &t("📊  Stats"), Some(&t("Their totals, and where each one ranks among all Clone Hero players")), |ui| {
        tile_rows(ui, z, &p.stats, |ui, s| stat_tile(ui, z, s, p.players));
    });
    ui.add_space(8.0);

    if !p.latest.is_empty() {
        if w.placements.len() != p.latest.len() {
            w.placements = p
                .latest
                .iter()
                .map(|s| {
                    let (http, id, s) = (http.clone(), p.id, s.clone());
                    Some(Load::start(ui.ctx(), move || fetch_placement(&http, id, &s)))
                })
                .collect();
        }
        let placements = &mut w.placements;
        crate::section(ui, "player_latest", &t("🕒  Latest scores"), Some(&t("Their most recent best scores; hover a score for its breakdown")), |ui| {
            for (k, s) in p.latest.iter().enumerate() {
                let placement = placements.get_mut(k).and_then(|l| l.as_mut()).and_then(|l| l.ready().copied());
                let line = ScoreLine {
                    title: format!("{} · {}", s.name, s.artist),
                    who: None,
                    instrument: &s.instrument,
                    difficulty: &s.difficulty,
                    accuracy: &s.accuracy,
                    stars: s.stars,
                    fc: s.fc,
                    pfc: s.pfc,
                    when: &s.when,
                    score: s.total,
                    placement,
                    detail: Some(format!(
                        "{}  ·  {}  ·  {}",
                        tr!("Max combo {combo}", combo = thousands(s.max_combo)),
                        tr!("Star power {hit}/{count}", hit = s.sp_hit, count = s.sp_count),
                        tr!("{count} squeezes", count = s.squeezes)
                    )),
                };
                if score_row(ui, k, z, &line, Some(&s.parts)).on_hover_text(t("Search for this song")).clicked() {
                    request_search(ui.ctx(), &format!("{} {}", s.artist, s.name));
                }
            }
        });
        ui.add_space(8.0);
    }

    let list = |ui: &mut egui::Ui, id: &str, title: &str, tip: &str, rows: Vec<(String, String, u64)>, search: bool| {
        if rows.is_empty() {
            return;
        }
        crate::section(ui, id, title, Some(tip), |ui| {
            let max = rows.iter().map(|r| r.2).max().unwrap_or(1).max(1);
            for (k, (main, sub, plays)) in rows.iter().enumerate() {
                let resp = row(ui, k, z.row, search, false, |ui| {
                    plain(ui, RichText::new(format!("{:>2}", k + 1)).size(z.text).weak().monospace());
                    plain(ui, RichText::new(main).size(z.text).strong());
                    if !sub.is_empty() {
                        plain(ui, RichText::new(sub).size(z.text).weak());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        plain(ui, RichText::new(tr!("{count} plays", count = thousands(*plays))).size(z.text - 1.0).weak());
                        mini_bar(ui, 100.0, *plays as f32 / max as f32, theme::accent());
                    });
                });
                if search && resp.on_hover_text(t("Search for this")).clicked() {
                    request_search(ui.ctx(), &format!("{sub} {main}").trim().to_owned());
                }
            }
        });
        ui.add_space(8.0);
    };
    list(ui, "player_most_played", &t("🔁  Most played"), &t("Songs they've played most; click one to find it"), p.most_played.iter().map(|(n, a, c)| (n.clone(), a.clone(), *c)).collect(), true);
    list(ui, "player_artists", &t("🎤  Top artists"), &t("Artists they've played most; click one to find their songs"), p.artists.iter().map(|(a, c)| (a.clone(), String::new(), *c)).collect(), true);
    list(
        ui,
        "player_instruments",
        &t("🎸  Instruments"),
        &t("Scores submitted per instrument"),
        p.instruments.iter().map(|(i, c)| (format!("{}  {}", instrument_icon(i), crate::leaderboard::instrument_name(i)), String::new(), *c)).collect(),
        false,
    );
    ui.add_space(12.0);
}

/// The picture for a profile as a texture, made once.
fn avatar_texture(ctx: &egui::Context, slot: &mut Option<egui::TextureHandle>, p: &Profile, name: &str) -> Option<egui::TextureHandle> {
    if slot.is_none()
        && let Some(img) = &p.avatar
    {
        *slot = Some(ctx.load_texture(format!("{name}_{}", p.id), img.clone(), egui::TextureOptions::LINEAR));
    }
    slot.clone()
}

/// A round avatar: the player's picture, or their initial on their default colour.
fn avatar_circle(ui: &mut egui::Ui, p: &Profile, tex: Option<&egui::TextureHandle>, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    match tex {
        Some(tex) => {
            ui.put(rect, egui::Image::new(tex).fit_to_exact_size(rect.size()).corner_radius(CornerRadius::same((size / 2.0) as u8)));
        }
        None => {
            ui.painter().circle_filled(rect.center(), size / 2.0, p.avatar_color);
            let initial = p.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(size * 0.46), Color32::WHITE);
        }
    }
}

/// The profile's top card: picture, name, overall rank, when they joined and a link.
fn header_card(ui: &mut egui::Ui, p: &Profile, avatar: Option<&egui::TextureHandle>) {
    let accent = theme::accent();
    egui::Frame::new()
        .fill(accent.linear_multiply(0.10))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                avatar_circle(ui, p, avatar, 64.0);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&p.name).size(26.0).strong());
                    ui.horizontal_wrapped(|ui| {
                        if let Some(rank) = p.stats.first().and_then(|s| s.rank) {
                            theme::tag(ui, &format!("🏆 {}", top_percent(rank, p.players)), accent)
                                .on_hover_text(tr!("#{rank} of {players} players by total score", rank = thousands(rank), players = thousands(p.players)));
                        }
                        if !p.joined.is_empty() {
                            ui.weak(tr!("Joined {date}", date = p.joined));
                        }
                        let link = ui.add(egui::Button::new(RichText::new(t("View on Clone Hero ↗")).color(ui.visuals().hyperlink_color)).frame(false));
                        if link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(format!("{SITE_URL}/users/{}", p.id)));
                        }
                    });
                });
            });
        });
}

/// Tiles in rows of as many as fit, each row top-aligned.
fn tile_rows<T>(ui: &mut egui::Ui, z: Sizes, items: &[T], mut tile: impl FnMut(&mut egui::Ui, &T)) {
    let gap = 8.0;
    let columns = ((ui.available_width() + gap) / (z.tile + gap)).floor().max(1.0) as usize;
    for chunk in items.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for item in chunk {
                tile(ui, item);
            }
        });
        ui.add_space(gap);
    }
}

/// One stat as a tile: an icon, its name, its value and how it ranks.
fn stat_tile(ui: &mut egui::Ui, z: Sizes, s: &Stat, players: u64) {
    egui::Frame::new()
        .fill(ui.visuals().widgets.inactive.bg_fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(z.tile - 20.0);
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), 17.0, theme::accent().linear_multiply(0.18));
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, s.icon, egui::FontId::proportional(16.0), ui.visuals().strong_text_color());
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    plain(ui, RichText::new(t(s.label)).size(z.text - 1.0).weak());
                    // Big numbers shrink to fit rather than being cut short.
                    let width = ui.painter().layout_no_wrap(s.shown.clone(), egui::FontId::proportional(z.value), Color32::WHITE).size().x;
                    let size = z.value * (ui.available_width() / width.max(1.0)).min(1.0);
                    plain(ui, RichText::new(&s.shown).size(size).strong());
                    match s.rank.filter(|_| players > 0) {
                        Some(rank) => {
                            theme::tag(ui, &top_percent(rank, players), theme::accent())
                                .on_hover_text(tr!("#{rank} of {players} players", rank = thousands(rank), players = thousands(players)));
                        }
                        // Keep tiles the same height with or without a rank.
                        None if players > 0 => {
                            ui.add_space(z.text + 6.0);
                        }
                        None => {}
                    }
                });
            });
        });
}

/// The two players' stats as donuts: each stat's share of their combined total.
fn compare_view(ui: &mut egui::Ui, a: &Profile, b: &Profile, a_tex: Option<&egui::TextureHandle>, b_tex: Option<&egui::TextureHandle>, z: Sizes) {
    // Who's who.
    ui.horizontal(|ui| {
        for (k, (p, tex)) in [(a, a_tex), (b, b_tex)].into_iter().enumerate() {
            avatar_circle(ui, p, tex, 26.0);
            let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(3), series(k));
            plain(ui, RichText::new(&p.name).size(z.text + 1.0).strong());
            ui.add_space(16.0);
        }
    });
    ui.add_space(6.0);
    let card = (z.tile * 0.8).max(150.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (sa, sb) in a.stats.iter().zip(&b.stats) {
            egui::Frame::new()
                .fill(ui.visuals().widgets.inactive.bg_fill)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(card - 20.0);
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        plain(ui, RichText::new(format!("{}  {}", sa.icon, t(sa.label))).size(z.text - 0.5).weak());
                        let total = (sa.value + sb.value).max(1) as f32;
                        let share_a = sa.value as f32 / total;
                        let d = (card - 50.0).clamp(70.0, 120.0);
                        let (rect, resp) = ui.allocate_exact_size(vec2(d, d), Sense::hover());
                        donut(ui.painter(), rect.center(), d / 2.0, d / 2.0 * 0.62, &[(share_a, series(0)), (1.0 - share_a, series(1))]);
                        let lead = if sa.value >= sb.value { (share_a, 0) } else { (1.0 - share_a, 1) };
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, format!("{:.0}%", lead.0 * 100.0), egui::FontId::proportional(16.0), series(lead.1));
                        resp.on_hover_text(format!("{}: {:.1}%\n{}: {:.1}%", a.name, share_a * 100.0, b.name, (1.0 - share_a) * 100.0));
                        for (k, (p, s, o)) in [(a, sa, sb), (b, sb, sa)].into_iter().enumerate() {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                ui.painter().rect_filled(r, CornerRadius::same(2), series(k));
                                let ahead = if s.value > o.value { " ▲" } else { "" };
                                plain(ui, RichText::new(format!("{}{ahead}", s.shown)).size(z.text).strong()).on_hover_text(&p.name);
                            });
                        }
                    });
                });
        }
    });
}

/// A ring chart: `parts` are (fraction, colour), with a small gap between them.
fn donut(painter: &egui::Painter, centre: egui::Pos2, outer: f32, inner: f32, parts: &[(f32, Color32)]) {
    use std::f32::consts::TAU;
    let shown: Vec<&(f32, Color32)> = parts.iter().filter(|p| p.0 > 0.0005).collect();
    let gap = if shown.len() > 1 { 0.035 } else { 0.0 };
    let mut start = -TAU / 4.0;
    for &&(frac, color) in &shown {
        let sweep = (frac * TAU - gap).max(0.01);
        let steps = ((sweep / TAU) * 96.0).ceil().max(2.0) as usize;
        let mut mesh = egui::Mesh::default();
        for k in 0..=steps {
            let a = start + sweep * k as f32 / steps as f32;
            let dir = vec2(a.cos(), a.sin());
            mesh.colored_vertex(centre + dir * outer, color);
            mesh.colored_vertex(centre + dir * inner, color);
            if k > 0 {
                let i = (2 * k) as u32;
                mesh.add_triangle(i - 2, i - 1, i);
                mesh.add_triangle(i - 1, i + 1, i);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        start += frac * TAU;
    }
}

// ─────────────────────────────── Rows ───────────────────────────────

/// A full-width list row: striped odd/even, tinted when hovered (if clickable) or `selected`.
fn row(ui: &mut egui::Ui, k: usize, height: f32, clickable: bool, selected: bool, add: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    let sense = if clickable { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), sense);
    let fill = if selected {
        theme::accent().linear_multiply(0.28)
    } else if clickable && resp.hovered() {
        theme::accent().linear_multiply(0.14)
    } else if k % 2 == 0 {
        ui.visuals().widgets.inactive.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut child);
    if clickable {
        // Screen readers announce it as a button; its text is read from the labels inside.
        resp.widget_info(|| egui::WidgetInfo::new(egui::WidgetType::Button));
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Text that doesn't take the mouse (so the row around it stays clickable), cut short if long.
fn plain(ui: &mut egui::Ui, text: RichText) -> egui::Response {
    ui.add(egui::Label::new(text).selectable(false).truncate())
}

/// A small horizontal bar, `frac` full.
fn mini_bar(ui: &mut egui::Ui, width: f32, frac: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 8.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(4), ui.visuals().extreme_bg_color);
    if frac > 0.0 {
        let fill = egui::Rect::from_min_size(rect.min, vec2((rect.width() * frac.clamp(0.0, 1.0)).max(4.0), rect.height()));
        ui.painter().rect_filled(fill, CornerRadius::same(4), color);
    }
}

fn instrument_icon(id: &str) -> &'static str {
    match id {
        "drums" | "prodrums" => "🥁",
        "keys" | "keysghl" => "🎹",
        _ => "🎸",
    }
}

/// What a score row shows.
struct ScoreLine<'a> {
    title: String,
    who: Option<String>,
    instrument: &'a str,
    difficulty: &'a str,
    accuracy: &'a str,
    stars: u64,
    fc: bool,
    pfc: bool,
    when: &'a str,
    score: u64,
    placement: Option<(u64, u64)>,
    /// More about the run, for the breakdown tooltip.
    detail: Option<String>,
}

/// A score as a two-line row: the song (and player) with the score on the right, then an
/// accuracy bar, stars, FC and placement. `parts` adds a breakdown tooltip to the score.
fn score_row(ui: &mut egui::Ui, k: usize, z: Sizes, s: &ScoreLine, parts: Option<&[u64; 6]>) -> egui::Response {
    let height = z.row * 1.7 + 14.0;
    row(ui, k, height, true, false, |ui| {
        plain(ui, RichText::new(instrument_icon(s.instrument)).size(z.text + 6.0));
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.horizontal(|ui| {
                if let Some(who) = &s.who {
                    plain(ui, RichText::new(who).size(z.text).strong().color(theme::accent()));
                }
                plain(ui, RichText::new(&s.title).size(z.text).strong());
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let acc: f32 = s.accuracy.parse().unwrap_or(0.0);
                let color = if s.pfc { theme::warn() } else if s.fc { theme::good() } else { theme::accent() };
                mini_bar(ui, 90.0, acc / 100.0, color);
                plain(ui, RichText::new(format!("{}%", s.accuracy)).size(z.text - 1.0).strong());
                plain(ui, RichText::new(format!("★ {}", s.stars)).size(z.text - 1.0).color(Color32::from_rgb(240, 190, 40)));
                if s.pfc {
                    theme::tag(ui, "PFC", theme::warn());
                } else if s.fc {
                    theme::tag(ui, "FC", theme::good());
                }
                plain(ui, RichText::new(s.difficulty).size(z.text - 1.5).weak());
                plain(ui, RichText::new(s.when).size(z.text - 1.5).weak());
            });
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let score = plain(ui, RichText::new(thousands(s.score)).size(z.text + 1.0).monospace().strong());
            if let Some(parts) = parts {
                score.on_hover_ui(|ui| {
                    ui.set_min_width(280.0);
                    ui.label(RichText::new(t("Where the points came from")).strong());
                    stacked_bar(ui, parts);
                    if let Some(detail) = &s.detail {
                        ui.add_space(4.0);
                        ui.weak(detail);
                    }
                });
            }
            if let Some((place, of)) = s.placement {
                ui.add_space(8.0);
                let text = format!("{} / {}", thousands(place), thousands(of));
                plain(ui, RichText::new(text).size(z.text - 1.0).weak()).on_hover_text(top_percent(place, of));
                place_badge(ui, place);
            }
        });
    })
}

/// A stacked bar of a score's parts with a legend that gives each part's points.
fn stacked_bar(ui: &mut egui::Ui, parts: &[u64; 6]) {
    let total: u64 = parts.iter().sum();
    if total == 0 {
        return;
    }
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let gap = 2.0;
    let mut x = rect.left();
    let shown: Vec<(usize, u64)> = parts.iter().copied().enumerate().filter(|p| p.1 > 0).collect();
    let usable = rect.width() - gap * (shown.len().saturating_sub(1)) as f32;
    for (n, &(k, v)) in shown.iter().enumerate() {
        let w = (usable * v as f32 / total as f32).max(2.0);
        let seg = egui::Rect::from_min_size(pos2(x, rect.top()), vec2(w, rect.height()));
        let end = |last: bool| if last { 4 } else { 0 };
        let radius = CornerRadius { nw: end(n == 0), sw: end(n == 0), ne: end(n + 1 == shown.len()), se: end(n + 1 == shown.len()) };
        painter.rect_filled(seg, radius, series(k));
        x += w + gap;
    }
    ui.add_space(4.0);
    for &(k, v) in &shown {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(2), series(k));
            ui.label(RichText::new(t(PARTS[k])).size(12.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("{}  ({:.0}%)", thousands(v), v as f64 / total as f64 * 100.0)).size(12.5).monospace());
            });
        });
    }
}

/// A dropdown for which stat to rank players by.
fn metric_picker(ui: &mut egui::Ui, id: &str, metric: &mut usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(t("Ranked by")).weak());
        let (_, icon, label) = METRICS[*metric];
        egui::ComboBox::from_id_salt(id).selected_text(format!("{icon}  {}", t(label))).width(170.0).show_ui(ui, |ui| {
            for (k, (_, icon, label)) in METRICS.iter().enumerate() {
                ui.selectable_value(metric, k, format!("{icon}  {}", t(label)));
            }
        });
    });
}

/// Shows a loading spinner, an error, or the loaded rows.
fn show_load<T: Send + 'static>(ui: &mut egui::Ui, load: &mut Load<T>, add: impl FnOnce(&mut egui::Ui, &T)) {
    match load.poll() {
        Load::Loading(_) => {
            ui.spinner();
        }
        Load::Failed(e) => {
            ui.weak(tr!("Couldn't load this: {error}", error = e));
        }
        Load::Ready(v) => add(ui, v),
    }
}

/// A placement: a trophy in gold, silver or bronze for the top three, else "#n".
pub fn place_badge(ui: &mut egui::Ui, place: u64) {
    let color = match place {
        1 => Color32::from_rgb(240, 190, 40),
        2 => Color32::from_rgb(190, 195, 205),
        3 => Color32::from_rgb(205, 125, 60),
        _ => ui.visuals().weak_text_color(),
    };
    let (rect, _) = ui.allocate_exact_size(vec2(34.0, 20.0), Sense::hover());
    if place <= 3 {
        ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, "🏆", egui::FontId::proportional(15.0), color);
        ui.painter().text(rect.right_center() - vec2(4.0, 0.0), egui::Align2::RIGHT_CENTER, place.to_string(), egui::FontId::proportional(12.5), color);
    } else {
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, format!("#{place}"), egui::FontId::proportional(12.5), color);
    }
}

fn request_search(ctx: &egui::Context, text: &str) {
    ctx.data_mut(|d| d.insert_temp(search_request_id(), text.to_owned()));
}
