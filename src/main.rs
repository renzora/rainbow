//! Rainbow: a small desktop alternative to Bridge for Chorus Encore (enchor.us).

#![windows_subsystem = "windows"]

mod api;
mod catalogue;
mod covers;
mod filter;
mod games;
mod settings;
mod sng;
mod theme;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use egui::{Color32, CornerRadius, CursorIcon, RichText, Sense, vec2};
use egui_extras::{Column, TableBuilder};

use api::{Chart, Client, DIFFICULTIES, DRUM_TYPES, DRUMS, INSTRUMENTS, Manifest, flag};
use catalogue::Catalogue;
use covers::Covers;
use filter::{Filters, Index, Range, SortKey, TEXT_FIELDS, TOGGLES};
use settings::{Density, DownloadFormat, Settings};
use theme::{ACCENT, AMBER, GREEN, RED};

const MAX_LOG: usize = 500;
/// Downloads bigger than this ask for confirmation and use the polite delay.
const BULK: usize = 50;
/// While the catalogue is incomplete, ask Encore directly once typing pauses this long.
const LIVE_SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);
const LOGO_PNG: &[u8] = include_bytes!("../assets/icon-256.png");

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rainbow")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([900.0, 500.0])
            .with_icon(eframe::icon_data::from_png_bytes(LOGO_PNG).unwrap_or_default()),
        ..Default::default()
    };
    eframe::run_native("Rainbow", options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))))
}

/// Messages from background threads to the UI.
enum Msg {
    /// Results of a direct Encore search, used while the catalogue is still downloading.
    LiveResults(Vec<Chart>),
    LiveFailed(String),
    TrackHash(Result<HashSet<String>, String>),
    CataloguePage { page: u32, pages: u32, charts: usize, new: Vec<Chart> },
    CatalogueDone(Result<(Catalogue, usize), String>),
    FileProgress { title: String, done: u64, total: Option<u64> },
    Queue { checked: usize, downloaded: usize, failed: usize },
    Status(String),
    Log(String),
    JobDone,
}

enum CatStatus {
    Idle,
    Building { page: u32, pages: u32, charts: usize },
    Failed(String),
}

struct Job {
    title: String,
    total: usize,
    checked: usize,
    downloaded: usize,
    failed: usize,
    file: Option<(String, u64, Option<u64>)>,
}

struct Confirm {
    title: String,
    charts: Vec<Chart>,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Browse,
    Settings,
}

#[derive(Clone, Copy)]
enum Col {
    Check,
    Cover,
    Name,
    Artist,
    Album,
    Genre,
    Year,
    Charter,
    Length,
    Difficulty,
    Uploaded,
    Tags,
}

#[derive(Clone, Copy)]
enum Section {
    Text,
    Ranges,
    Chart,
    Features,
}

/// How a row was clicked, for file-manager style selection.
enum RowClick {
    Plain(usize),
    Toggle(usize),
    Range(usize, bool),
    Checkbox(usize),
}

struct App {
    client: Client,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,

    settings: Settings,
    saved_settings: Settings,
    tab: Tab,
    logo: Option<egui::TextureHandle>,
    covers: Covers,

    // Every chart we know about: the catalogue (complete or still downloading) plus any
    // charts found by live searches. Searching always runs locally over this.
    charts: Vec<Chart>,
    known: HashSet<String>,
    index: Index,
    results: Vec<usize>,
    /// The filters and chart count of the last search, to re-run it only when needed.
    searched: Option<(Filters, usize)>,
    filters: Filters,
    show_advanced: bool,
    show_log: bool,
    search_bottom: f32,

    cat_complete: bool,
    cat_updated: u64,
    cat_next_page: u32,
    cat_total: Option<u64>,
    cat_status: CatStatus,
    cat_stop: Arc<AtomicBool>,

    live_sent: String,
    live_edited: Instant,
    live_searching: bool,
    track_lookup: bool,

    selected: HashSet<String>,
    anchor: Option<usize>,
    detail: Option<Chart>,
    scroll_to_top: bool,
    manifest: Arc<Mutex<Manifest>>,

    job: Option<Job>,
    stop: Arc<AtomicBool>,
    confirm: Option<Confirm>,
    status: Option<(String, Instant)>,
    log: Vec<String>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let status_tx = tx.clone();
        let repaint = ctx.clone();
        let client = Client::new().expect("failed to create HTTP client").with_status(Arc::new(move |m| {
            let _ = status_tx.send(Msg::Status(m));
            repaint.request_repaint();
        }));
        let settings = Settings::load();
        theme::apply(ctx, settings.dark, settings.zoom);
        theme::install_fonts(ctx);
        let logo = image::load_from_memory(LOGO_PNG).ok().map(|img| {
            let img = img.resize(96, 96, image::imageops::FilterType::Triangle).to_rgba8();
            let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            ctx.load_texture("logo", color, egui::TextureOptions::LINEAR)
        });
        let mut app = Self {
            client,
            tx,
            rx,
            manifest: Arc::new(Mutex::new(Manifest::load(&settings.dir))),
            saved_settings: settings.clone(),
            settings,
            tab: Tab::Browse,
            logo,
            covers: Covers::new(ctx),
            charts: Vec::new(),
            known: HashSet::new(),
            index: Index::default(),
            results: Vec::new(),
            searched: None,
            filters: Filters::default(),
            show_advanced: false,
            show_log: false,
            search_bottom: 0.0,
            cat_complete: false,
            cat_updated: 0,
            cat_next_page: 0,
            cat_total: None,
            cat_status: CatStatus::Idle,
            cat_stop: Arc::new(AtomicBool::new(false)),
            live_sent: String::new(),
            live_edited: Instant::now(),
            live_searching: false,
            track_lookup: false,
            selected: HashSet::new(),
            anchor: None,
            detail: None,
            scroll_to_top: false,
            job: None,
            stop: Arc::new(AtomicBool::new(false)),
            confirm: None,
            status: None,
            log: Vec::new(),
        };

        match Catalogue::load() {
            Some(cat) => {
                app.log(format!("Loaded catalogue: {} charts", cat.charts.len()));
                app.cat_complete = cat.complete;
                app.cat_updated = cat.updated;
                app.cat_next_page = cat.next_page;
                app.replace_charts(cat.charts);
                // Only ask Encore for new charts if the catalogue is old enough (or unfinished).
                let stale = match app.settings.refresh_after_hours {
                    Some(h) => now().saturating_sub(app.cat_updated) >= h as u64 * 3600,
                    None => false,
                };
                if !app.cat_complete || stale {
                    app.update_catalogue(false);
                }
            }
            None => {
                if catalogue::path().exists() {
                    app.log("Catalogue format changed; downloading it again with the new fields".to_owned());
                }
                app.update_catalogue(false);
            }
        }
        app
    }

    fn replace_charts(&mut self, charts: Vec<Chart>) {
        self.known = charts.iter().map(|c| c.md5.clone()).collect();
        self.index = Index::new(&charts);
        self.charts = charts;
        self.searched = None;
    }

    /// Adds charts we haven't seen yet; they become searchable immediately.
    fn add_charts(&mut self, charts: Vec<Chart>) {
        for c in charts {
            if self.known.insert(c.md5.clone()) {
                self.index.push(&c);
                self.charts.push(c);
            }
        }
    }

    /// Re-runs the search when the filters change or new charts arrive.
    fn refresh_results(&mut self) {
        let filters_changed = self.searched.as_ref().is_none_or(|(f, _)| *f != self.filters);
        let grew = self.searched.as_ref().is_some_and(|(_, n)| *n != self.charts.len());
        if !filters_changed && !grew {
            return;
        }
        self.results = self.index.search(&self.charts, &self.filters);
        if filters_changed {
            self.scroll_to_top = true;
            self.anchor = None;
        }
        self.searched = Some((self.filters.clone(), self.charts.len()));
    }

    /// While the catalogue is still downloading, also ask Encore directly for the typed query
    /// so results show up right away.
    fn live_search(&mut self) {
        if self.cat_complete || self.live_searching || self.live_edited.elapsed() < LIVE_SEARCH_DEBOUNCE {
            return;
        }
        let query = self.filters.query.trim().to_owned();
        let key = format!("{query}|{:?}|{:?}", self.filters.instrument, self.filters.difficulty);
        if query.is_empty() || key == self.live_sent {
            return;
        }
        self.live_sent = key;
        self.live_searching = true;
        let (client, tx) = (self.client.clone(), self.tx.clone());
        let (inst, diff) = (self.filters.instrument, self.filters.difficulty);
        thread::spawn(move || {
            let msg = match client.search(&query, 1, inst, diff) {
                Ok(r) => Msg::LiveResults(r.charts),
                Err(e) => Msg::LiveFailed(format!("{e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn lookup_track_hash(&mut self) {
        let hash = self.filters.track_hash.trim().to_owned();
        if hash.is_empty() {
            self.filters.track_md5s = None;
            return;
        }
        self.track_lookup = true;
        let (client, tx) = (self.client.clone(), self.tx.clone());
        thread::spawn(move || {
            let _ = tx.send(Msg::TrackHash(client.md5s_for_track_hash(&hash).map_err(|e| format!("{e:#}"))));
        });
    }

    /// Fetches new charts (or continues/starts a full download of the catalogue).
    fn update_catalogue(&mut self, from_scratch: bool) {
        let mut cat = if from_scratch {
            Catalogue::default()
        } else {
            Catalogue {
                version: 0,
                updated: self.cat_updated,
                complete: self.cat_complete,
                next_page: self.cat_next_page,
                charts: self.charts.clone(),
            }
        };
        self.cat_stop.store(false, Ordering::Relaxed);
        self.cat_status = CatStatus::Building { page: cat.next_page, pages: 0, charts: cat.charts.len() };
        let (client, tx, stop) = (self.client.clone(), self.tx.clone(), self.cat_stop.clone());
        thread::spawn(move || {
            let result = cat.update(&client, &stop, |p| {
                let catalogue::Progress::Page { page, pages, charts, new } = p;
                let _ = tx.send(Msg::CataloguePage { page, pages, charts, new });
            });
            let _ = tx.send(Msg::CatalogueDone(result.map(|added| (cat, added)).map_err(|e| format!("{e:#}"))));
        });
    }

    fn start_download(&mut self, title: String, charts: Vec<Chart>) {
        let delay =
            if charts.len() > BULK { Duration::from_secs_f32(self.settings.delay_secs) } else { Duration::ZERO };
        self.stop.store(false, Ordering::Relaxed);
        self.job = Some(Job { title, total: charts.len(), checked: 0, downloaded: 0, failed: 0, file: None });
        let mut w = Worker {
            client: self.client.clone(),
            tx: self.tx.clone(),
            settings: self.settings.clone(),
            manifest: self.manifest.clone(),
            stop: self.stop.clone(),
            checked: 0,
            downloaded: 0,
            failed: 0,
        };
        thread::spawn(move || {
            for chart in &charts {
                if w.stopped() {
                    break;
                }
                if w.download(chart) {
                    w.wait(delay);
                }
            }
            w.send_queue();
            let _ = w.tx.send(Msg::JobDone);
        });
    }

    /// Starts small downloads right away; asks first for big ones.
    fn request_download(&mut self, title: String, charts: Vec<Chart>) {
        if charts.len() > BULK {
            self.confirm = Some(Confirm { title, charts });
        } else if !charts.is_empty() {
            self.start_download(title, charts);
        }
    }

    fn selected_charts(&self) -> Vec<Chart> {
        self.results.iter().map(|&i| &self.charts[i]).filter(|c| self.selected.contains(&c.md5)).cloned().collect()
    }

    fn log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > MAX_LOG {
            self.log.drain(..self.log.len() - MAX_LOG);
        }
    }

    fn handle_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::LiveResults(charts) => {
                    self.live_searching = false;
                    self.add_charts(charts);
                }
                Msg::LiveFailed(e) => {
                    self.live_searching = false;
                    self.log(format!("Search failed: {e}"));
                }
                Msg::TrackHash(result) => {
                    self.track_lookup = false;
                    match result {
                        Ok(md5s) => {
                            self.log(format!("Track hash matched {} chart(s)", md5s.len()));
                            self.filters.track_md5s = Some(md5s);
                        }
                        Err(e) => self.log(format!("Track hash lookup failed: {e}")),
                    }
                }
                Msg::CataloguePage { page, pages, charts, new } => {
                    self.cat_status = CatStatus::Building { page, pages, charts };
                    self.cat_total = Some(pages as u64 * api::MAX_PER_PAGE as u64);
                    self.add_charts(new);
                }
                Msg::CatalogueDone(Ok((cat, added))) => {
                    self.cat_status = CatStatus::Idle;
                    self.cat_updated = cat.updated;
                    self.cat_next_page = cat.next_page;
                    if cat.complete {
                        self.log(format!("Catalogue up to date: {} charts ({added} new)", cat.charts.len()));
                        let was_complete = self.cat_complete;
                        self.cat_complete = true;
                        // Switch to the catalogue's own order (newest first) once it's whole.
                        if added > 0 || !was_complete {
                            let extra: Vec<Chart> = {
                                let in_cat: HashSet<&str> = cat.charts.iter().map(|c| c.md5.as_str()).collect();
                                self.charts.iter().filter(|c| !in_cat.contains(c.md5.as_str())).cloned().collect()
                            };
                            self.replace_charts(cat.charts);
                            self.add_charts(extra);
                        }
                    } else {
                        let how = if self.cat_stop.load(Ordering::Relaxed) { "paused" } else { "incomplete" };
                        self.log(format!("Catalogue {how} at {} charts", cat.charts.len()));
                    }
                }
                Msg::CatalogueDone(Err(e)) => {
                    self.log(format!("Catalogue update failed: {e}"));
                    self.cat_status = CatStatus::Failed(e);
                }
                Msg::FileProgress { title, done, total } => {
                    if let Some(job) = &mut self.job {
                        job.file = Some((title, done, total));
                    }
                }
                Msg::Queue { checked, downloaded, failed } => {
                    if let Some(job) = &mut self.job {
                        job.checked = checked;
                        job.downloaded = downloaded;
                        job.failed = failed;
                        job.file = None;
                    }
                }
                Msg::Status(s) => {
                    self.log(s.clone());
                    self.status = Some((s, Instant::now()));
                }
                Msg::Log(line) => self.log(line),
                Msg::JobDone => {
                    if let Some(job) = self.job.take() {
                        let how = if self.stop.load(Ordering::Relaxed) { "Stopped" } else { "Finished" };
                        self.log(format!(
                            "{how}: {}: {} downloaded, {} already had, {} failed",
                            job.title,
                            job.downloaded,
                            job.checked - job.downloaded - job.failed,
                            job.failed
                        ));
                    }
                }
            }
        }
    }

    // ─────────────────────────────── Header ───────────────────────────────

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(38.0, 38.0)));
            }
            ui.label(theme::title(24.0));
            ui.add_space(18.0);
            for (tab, label) in [(Tab::Browse, "🔍  Browse"), (Tab::Settings, "⚙  Settings")] {
                let on = self.tab == tab;
                let text = RichText::new(label).size(15.0);
                let button = if on {
                    egui::Button::new(text.color(Color32::WHITE).strong()).fill(ACCENT)
                } else {
                    egui::Button::new(text).frame(false)
                };
                if ui.add(button.corner_radius(CornerRadius::same(16)).min_size(vec2(110.0, 32.0))).clicked() {
                    self.tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(job) = &self.job {
                    ui.add(egui::ProgressBar::new(job.checked as f32 / job.total.max(1) as f32).desired_width(140.0).show_percentage());
                    ui.label(RichText::new(format!("⬇ {}", job.downloaded)).color(GREEN));
                }
            });
        });
        ui.add_space(6.0);
    }

    // ───────────────────────────── Browse tab ─────────────────────────────

    fn search_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.filters.query)
                    .hint_text("🔍  Search songs, artists, albums, charters…")
                    .font(egui::FontId::proportional(16.0))
                    .margin(egui::Margin::symmetric(10, 7))
                    .desired_width((ui.available_width() - 560.0).max(220.0)),
            );
            if resp.changed() {
                self.live_edited = Instant::now();
            }

            let inst_text = self.filters.instrument.map_or("🎵  Any Instrument".to_owned(), |i| {
                format!("{}  {}", instrument_icon(i), INSTRUMENTS[i as usize].1)
            });
            egui::ComboBox::from_id_salt("instrument").selected_text(inst_text).width(180.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.filters.instrument, None, "🎵  Any Instrument");
                for (k, (_, name, _)) in INSTRUMENTS.iter().enumerate() {
                    let label = format!("{}  {name}", instrument_icon(k as u8));
                    ui.selectable_value(&mut self.filters.instrument, Some(k as u8), label);
                }
            });
            let diff_text = self.filters.difficulty.map_or("Any Difficulty", |d| DIFFICULTIES[d as usize].1);
            egui::ComboBox::from_id_salt("difficulty").selected_text(diff_text).width(120.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.filters.difficulty, None, "Any Difficulty");
                for (k, (_, name)) in DIFFICULTIES.iter().enumerate() {
                    ui.selectable_value(&mut self.filters.difficulty, Some(k as u8), *name);
                }
            });
            if self.filters.instrument == Some(DRUMS) {
                let dt = self.filters.drum_type.map_or("Any Drum Type", |d| DRUM_TYPES[d as usize]);
                egui::ComboBox::from_id_salt("drumtype").selected_text(dt).width(110.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filters.drum_type, None, "Any Drum Type");
                    for (k, name) in DRUM_TYPES.iter().enumerate() {
                        ui.selectable_value(&mut self.filters.drum_type, Some(k as u8), *name);
                    }
                });
            }

            let n = self.filters.advanced_count();
            let label = format!(
                "🎛  Advanced search{} {}",
                if n > 0 { format!(" ({n})") } else { String::new() },
                if self.show_advanced { "⏶" } else { "⏷" }
            );
            if pill(ui, self.show_advanced || n > 0, &label).clicked() {
                self.show_advanced = !self.show_advanced;
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let f = &mut self.filters;
            let all = !f.no_video && !f.no_broken && !f.legacy && f.game.is_none() && f.advanced_count() == 0;
            if pill(ui, all, "🌈  All songs").on_hover_text("Clear all filters").clicked() {
                f.no_video = false;
                f.no_broken = false;
                f.legacy = false;
                f.game = None;
                f.clear_advanced();
            }
            if pill(ui, f.no_video, "🎬  No videos").on_hover_text("Hide charts that have a video background").clicked() {
                f.no_video = !f.no_video;
            }
            if pill(ui, f.no_broken, "🛠  No broken songs")
                .on_hover_text("Hide charts Encore flags as unplayable: missing or unreadable chart/audio, or no notes")
                .clicked()
            {
                f.no_broken = !f.no_broken;
            }
            ui.separator();
            if pill(ui, f.legacy || f.game.is_some(), "🏆  Legacy")
                .on_hover_text("Only the original charts from the official Guitar Hero and Rock Band games and their DLC")
                .clicked()
            {
                f.legacy = !(f.legacy || f.game.is_some());
                f.game = None;
            }
            let label = f.game.as_deref().map_or("🎮  Any game".to_owned(), |g| format!("🎮  {}", games::game_name(g)));
            let index = &self.index;
            egui::ComboBox::from_id_salt("game").selected_text(label).width(240.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut f.game, None, "🎮  Any game");
                for (g, n) in index.games() {
                    ui.selectable_value(&mut f.game, Some(g.clone()), format!("{}  ({n})", games::game_name(&g)));
                }
            });
        });
    }

    /// The advanced search panel, shown floating over the results. Its cards reflow into as
    /// many columns as fit, and it scrolls if the window is short.
    fn advanced_overlay(&mut self, ctx: &egui::Context) {
        if !self.show_advanced {
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_advanced = false;
            return;
        }
        let screen = ctx.content_rect();
        let width = (screen.width() - 36.0).max(300.0);
        let max_height = (screen.bottom() - self.search_bottom - 70.0).max(150.0);
        egui::Area::new(egui::Id::new("advanced_search"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(screen.left() + 10.0, self.search_bottom + 2.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("🎛  Advanced search").size(17.0).strong().color(theme::ACCENT_HOVER));
                        ui.weak(format!("{} results", self.results.len()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✖  Close").on_hover_text("Esc").clicked() {
                                self.show_advanced = false;
                            }
                            if ui.button("↺  Reset").on_hover_text("Clear all advanced filters").clicked() {
                                self.filters.clear_advanced();
                            }
                        });
                    });
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().max_height(max_height).auto_shrink([false, true]).show(ui, |ui| {
                        self.advanced_search(ui);
                    });
                });
            });
    }

    fn advanced_search(&mut self, ui: &mut egui::Ui) {
        const GAP: f32 = 10.0;
        const MIN_CARD: f32 = 280.0;
        let avail = ui.available_width();
        let columns = (((avail + GAP) / (MIN_CARD + GAP)).floor() as usize).clamp(1, 4);
        let card_w = (avail - GAP * (columns - 1) as f32) / columns as f32;
        let mut lookup = false;
        let sections = [Section::Text, Section::Ranges, Section::Chart, Section::Features];
        for row in sections.chunks(columns) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for section in row {
                    lookup |= self.advanced_card(ui, *section, card_w);
                }
            });
            ui.add_space(GAP);
        }
        if lookup && !self.track_lookup {
            self.lookup_track_hash();
        }
    }

    /// One advanced-search card. Returns true if a track hash lookup was requested.
    fn advanced_card(&mut self, ui: &mut egui::Ui, section: Section, width: f32) -> bool {
        let mut lookup = false;
        let track_lookup = self.track_lookup;
        let f = &mut self.filters;
        let inner = width - 26.0; // frame margin + stroke
        let title = match section {
            Section::Text => "🔤  Search by",
            Section::Ranges => "📏  Ranges",
            Section::Chart => "🔑  Chart",
            Section::Features => "✨  Features",
        };
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(inner);
                ui.vertical(|ui| {
                    ui.label(RichText::new(title).strong().color(theme::ACCENT_HOVER));
                    ui.add_space(6.0);
                    match section {
                        Section::Text => {
                            let field_w = inner - 2.0 * 52.0 - 16.0;
                            egui::Grid::new("adv_text").num_columns(3).spacing([8.0, 5.0]).show(ui, |ui| {
                                ui.label("");
                                ui.small("Exact").on_hover_text("Only include results that match perfectly. (not case sensitive)");
                                ui.small("Exclude").on_hover_text("Do not include results that match this.");
                                ui.end_row();
                                for (k, label) in TEXT_FIELDS.iter().enumerate() {
                                    ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut f.text[k].value).hint_text(*label));
                                    ui.vertical_centered(|ui| theme::checkbox(ui, &mut f.text[k].exact, ""));
                                    ui.vertical_centered(|ui| theme::checkbox(ui, &mut f.text[k].exclude, ""));
                                    ui.end_row();
                                }
                            });
                        }
                        Section::Ranges => {
                            let label_w = inner - 2.0 * 64.0 - 16.0;
                            egui::Grid::new("adv_ranges").num_columns(3).spacing([8.0, 5.0]).min_col_width(0.0).show(ui, |ui| {
                                for (label, range, tip) in [
                                    ("⏱  Length (min)", &mut f.length_min, ""),
                                    ("🔥  Intensity", &mut f.intensity, "Also known as chart difficulty. Typically a number between 0 and 6."),
                                    ("⚡  Average NPS", &mut f.avg_nps, "Average notes per second, for the chosen instrument/difficulty."),
                                    ("🚀  Max NPS", &mut f.max_nps, "Highest notes per second, for the chosen instrument/difficulty."),
                                    ("📅  Year", &mut f.year, ""),
                                ] {
                                    range_row(ui, label, range, tip, label_w);
                                }
                            });
                        }
                        Section::Chart => {
                            let field_w = inner - 110.0;
                            egui::Grid::new("adv_misc").num_columns(2).spacing([8.0, 5.0]).show(ui, |ui| {
                                ui.label("Modified after").on_hover_text("The date of the last time this chart was modified in Google Drive.");
                                ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut f.modified_after).hint_text("YYYY-MM-DD"));
                                ui.end_row();
                                ui.label("Hash").on_hover_text("The MD5 hash of the chart folder or .sng file. You can enter multiple values if they are separated by commas.");
                                ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut f.hash).hint_text("md5, md5, …"));
                                ui.end_row();
                                ui.label("Track hash").on_hover_text("The hash of only things that impact scoring on a specific track. Multiple values can be separated by commas. Press Enter to look it up on Encore's server.");
                                let resp = ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut f.track_hash).hint_text("press Enter to look up"));
                                ui.end_row();
                                if resp.changed() {
                                    f.track_md5s = None;
                                }
                                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    lookup = true;
                                }
                            });
                            if track_lookup {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.small("Looking up track hash…");
                                });
                            } else if let Some(md5s) = &f.track_md5s {
                                ui.small(format!("Track hash matches {} chart(s)", md5s.len()));
                            }
                        }
                        Section::Features => {
                            ui.small("Click to cycle: any → must have → must not have");
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                                for (k, (label, _, applies)) in TOGGLES.iter().enumerate() {
                                    let state = &mut f.toggles[k];
                                    let (icon, color, fill) = match state {
                                        None => ("○", ui.visuals().weak_text_color(), ui.visuals().widgets.inactive.bg_fill),
                                        Some(true) => ("✔", GREEN, GREEN.linear_multiply(0.18)),
                                        Some(false) => ("✖", RED, RED.linear_multiply(0.18)),
                                    };
                                    let button = egui::Button::new(RichText::new(format!("{icon} {label}")).color(color))
                                        .fill(fill)
                                        .corner_radius(CornerRadius::same(14));
                                    let resp = ui.add_enabled(applies.enabled(f.instrument), button);
                                    if resp.clicked() {
                                        *state = match state {
                                            None => Some(true),
                                            Some(true) => Some(false),
                                            Some(false) => None,
                                        };
                                    }
                                }
                            });
                        }
                    }
                });
            });
        lookup
    }

    /// The fixed status bar at the bottom: catalogue progress, rate-limit notices, the log
    /// and links to Encore.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match &self.cat_status {
                CatStatus::Building { page, pages, charts } => {
                    ui.spinner();
                    if self.cat_complete {
                        ui.label(format!("Checking for new charts… {charts} charts"));
                    } else {
                        if *pages > 0 {
                            ui.add(
                                egui::ProgressBar::new(*page as f32 / (*pages).max(1) as f32)
                                    .desired_width(140.0)
                                    .text(format!("{page}/{pages}")),
                            );
                        }
                        ui.label(format!("Downloading catalogue: {charts} charts"))
                            .on_hover_text("Already searchable: results fill in as it downloads.");
                    }
                    if ui.small_button("⏸").on_hover_text("Pause").clicked() {
                        self.cat_stop.store(true, Ordering::Relaxed);
                    }
                }
                CatStatus::Failed(e) => {
                    ui.colored_label(RED, "⚠ Catalogue update failed").on_hover_text(e.clone());
                    if ui.small_button("Retry").clicked() {
                        self.update_catalogue(false);
                    }
                }
                CatStatus::Idle if self.cat_complete => {
                    ui.label(format!("📚 {} charts", self.charts.len()));
                    ui.weak(format!("updated {}", ago(self.cat_updated)));
                    if ui.small_button("⟳").on_hover_text("Check Encore for new charts").clicked() {
                        self.update_catalogue(false);
                    }
                }
                CatStatus::Idle => {
                    ui.label(format!("Catalogue paused at {} charts", self.charts.len()));
                    if ui.small_button("▶").on_hover_text("Resume").clicked() {
                        self.update_catalogue(false);
                    }
                }
            }
            if let Some((s, at)) = &self.status
                && at.elapsed() < Duration::from_secs(60)
            {
                ui.separator();
                ui.colored_label(AMBER, format!("⏳ {s}"));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.hyperlink_to("🌈 GitHub", "https://github.com/RenzoraEngine/rainbow")
                    .on_hover_text("Rainbow's source code, releases and issue tracker");
                ui.hyperlink_to("❤ Patreon", "https://www.patreon.com/cw/ChorusEncore701?l=en-GB")
                    .on_hover_text("Support Chorus Encore: it's donation-funded and pays for the servers and file hosting Rainbow uses");
                ui.hyperlink_to("💬 Discord", "https://discord.gg/cqaUXGm")
                    .on_hover_text("Join the Chorus Encore Discord community");
                ui.hyperlink_to("🌐 Encore", "https://www.enchor.us")
                    .on_hover_text("Chorus Encore, the chart search engine Rainbow is built on");
                ui.separator();
                if ui
                    .selectable_label(self.show_log, format!("📜 Log ({})", self.log.len()))
                    .on_hover_text("Show activity and errors")
                    .clicked()
                {
                    self.show_log = !self.show_log;
                }
            });
        });
    }

    /// Download progress, shown above the status bar while a download runs.
    fn download_bar(&mut self, ui: &mut egui::Ui) {
        let Some(job) = &self.job else {
            return;
        };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong(format!("⬇ {}", job.title));
            ui.label(format!("{} / {} checked", job.checked, job.total));
            ui.colored_label(GREEN, format!("✔ {} downloaded", job.downloaded));
            ui.weak(format!("{} already had", job.checked - job.downloaded - job.failed));
            if job.failed > 0 {
                ui.colored_label(RED, format!("✖ {} failed", job.failed));
            }
            if ui.small_button("⏹ Stop").clicked() {
                self.stop.store(true, Ordering::Relaxed);
            }
        });
        ui.add(egui::ProgressBar::new(job.checked as f32 / job.total.max(1) as f32).show_percentage().fill(ACCENT));
        if let Some((title, done, total)) = &job.file {
            let frac = total.map(|t| *done as f32 / t.max(1) as f32).unwrap_or(0.0);
            ui.add(egui::ProgressBar::new(frac).text(format!("{title}  ({:.1} MB)", *done as f64 / 1_048_576.0)));
        }
        ui.add_space(4.0);
    }

    fn log_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_log;
        egui::Window::new("📜 Log").open(&mut open).default_size([640.0, 320.0]).show(ctx, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for line in &self.log {
                    ui.label(line);
                }
            });
        });
        self.show_log = open;
    }

    fn columns(&self) -> Vec<Col> {
        let c = &self.settings.columns;
        let mut cols = vec![Col::Check];
        if c.cover {
            cols.push(Col::Cover);
        }
        cols.push(Col::Name);
        for (on, col) in [
            (c.artist, Col::Artist),
            (c.album, Col::Album),
            (c.genre, Col::Genre),
            (c.year, Col::Year),
            (c.charter, Col::Charter),
            (c.length, Col::Length),
            (c.difficulty, Col::Difficulty),
            (c.uploaded, Col::Uploaded),
        ] {
            if on {
                cols.push(col);
            }
        }
        cols.push(Col::Tags);
        cols
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        let count = self.results.len();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{count} results")).strong());
            if !self.cat_complete {
                if self.live_searching {
                    ui.spinner();
                }
                let total = self.cat_total.map_or(String::new(), |t| format!(" of ~{t}"));
                ui.weak(format!("(searching {} charts{total} so far)", self.charts.len()));
            }
            ui.separator();
            if ui.button("☑ Select all").clicked() {
                self.selected = self.results.iter().map(|&i| self.charts[i].md5.clone()).collect();
            }
            if ui.button("☐ Clear").clicked() {
                self.selected.clear();
            }
            ui.weak("Shift/Ctrl+click to select several");
            ui.separator();
            for (d, icon, tip) in [(Density::Compact, "☰", "Compact rows"), (Density::Default, "▤", "Default rows"), (Density::Large, "▦", "Large rows with bigger covers")] {
                if ui.selectable_label(self.settings.density == d, icon).on_hover_text(tip).clicked() {
                    self.settings.density = d;
                    self.settings.cover_size = d.cover_size();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let resp = ui
                    .add_enabled(!busy && self.cat_complete, egui::Button::new("⬇ Entire library"))
                    .on_hover_text("Download every chart on Encore")
                    .on_disabled_hover_text("Available once the catalogue has finished downloading");
                if resp.clicked() {
                    let charts = self.charts.clone();
                    self.request_download("Entire Encore library".to_owned(), charts);
                }
                let resp = ui.add_enabled(!busy && count > 0, egui::Button::new(format!("⬇ All {count} results")));
                if resp.clicked() {
                    let charts: Vec<Chart> = self.results.iter().map(|&i| self.charts[i].clone()).collect();
                    self.request_download("All current results".to_owned(), charts);
                }
                let n = self.selected.len();
                if ui.add_enabled(!busy && n > 0, theme::primary(format!("⬇ Download selected ({n})"))).clicked() {
                    let charts = self.selected_charts();
                    self.selected.clear();
                    self.request_download(format!("{} selected chart(s)", charts.len()), charts);
                }
            });
        });
        ui.add_space(4.0);

        let cols = self.columns();
        let manifest = self.manifest.lock().unwrap();
        let mut click = None;
        let mut sort_click = None;
        let mut toggle_all = false;
        let any_selected = !self.selected.is_empty();
        let all_selected = count > 0 && self.results.iter().all(|&i| self.selected.contains(&self.charts[i].md5));
        let show_cover = self.settings.columns.cover;
        let cover_px = self.settings.cover_size;
        let row_h = if show_cover { cover_px + 8.0 } else { self.settings.density.row_height() };
        let mut cover_width = None;

        // Split borrows: the table body reads results while loading covers mutably.
        let (charts, results) = (&self.charts, &self.results);
        let covers = &mut self.covers;
        let selected = &self.selected;
        let detail_md5 = self.detail.as_ref().map(|c| c.md5.clone());
        let (sort, desc) = (self.filters.sort, self.filters.descending);
        let instrument = self.filters.instrument;

        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        if std::mem::take(&mut self.scroll_to_top) {
            table = table.scroll_to_row(0, None);
        }
        for col in &cols {
            table = table.column(match col {
                Col::Check => Column::exact(26.0),
                Col::Cover => Column::initial(self.settings.cover_size + 8.0).range(24.0..=240.0),
                Col::Name => Column::initial(240.0).at_least(80.0).clip(true),
                Col::Artist | Col::Album => Column::initial(170.0).at_least(40.0).clip(true),
                Col::Genre => Column::initial(110.0).at_least(40.0).clip(true),
                Col::Charter => Column::initial(120.0).at_least(40.0).clip(true),
                Col::Year => Column::initial(50.0).at_least(30.0).clip(true),
                Col::Length => Column::initial(55.0).at_least(30.0),
                Col::Difficulty => Column::initial(110.0).at_least(40.0).clip(true),
                Col::Uploaded => Column::initial(85.0).at_least(40.0),
                Col::Tags => Column::remainder().at_least(110.0).clip(true),
            });
        }
        let scroll = table
            .header(26.0, |mut h| {
                for col in &cols {
                    if let Col::Check = col {
                        h.col(|ui| {
                            let mut all = all_selected;
                            let tip = if all_selected { "Deselect all results" } else { "Select all results" };
                            if theme::checkbox_ex(ui, &mut all, "", any_selected && !all_selected).on_hover_text(tip).clicked() {
                                toggle_all = true;
                            }
                        });
                        continue;
                    }
                    let (title, key) = match col {
                        Col::Check | Col::Cover => ("", None),
                        Col::Tags => ("Tags", None),
                        Col::Name => ("Name", Some(SortKey::Name)),
                        Col::Artist => ("Artist", Some(SortKey::Artist)),
                        Col::Album => ("Album", Some(SortKey::Album)),
                        Col::Genre => ("Genre", Some(SortKey::Genre)),
                        Col::Year => ("Year", Some(SortKey::Year)),
                        Col::Charter => ("Charter", Some(SortKey::Charter)),
                        Col::Length => ("Length", Some(SortKey::Length)),
                        Col::Difficulty => ("Difficulty", Some(SortKey::Difficulty)),
                        Col::Uploaded => ("Uploaded", Some(SortKey::Uploaded)),
                    };
                    h.col(|ui| {
                        let Some(key) = key else {
                            ui.strong(title);
                            return;
                        };
                        let active = sort == key;
                        let arrow = if active { if desc { " ⏷" } else { " ⏶" } } else { "" };
                        let mut text = RichText::new(format!("{title}{arrow}")).strong();
                        if active {
                            text = text.color(theme::ACCENT_HOVER);
                        }
                        if ui.add(egui::Button::new(text).frame(false)).on_hover_text("Sort").clicked() {
                            sort_click = Some(key);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(row_h, count, |mut row| {
                    let i = row.index();
                    let c = &charts[results[i]];
                    let is_checked = selected.contains(&c.md5);
                    row.set_selected(is_checked || detail_md5.as_deref() == Some(c.md5.as_str()));
                    let text = |s: &Option<String>| s.clone().unwrap_or_default();
                    for col in &cols {
                        row.col(|ui| match col {
                            Col::Check => {
                                let mut checked = is_checked;
                                if theme::checkbox(ui, &mut checked, "").changed() {
                                    click = Some(RowClick::Checkbox(i));
                                }
                            }
                            Col::Cover => {
                                cover_width.get_or_insert(ui.available_width());
                                cover(ui, covers, c.art.as_deref(), cover_px, covers::THUMB);
                            }
                            Col::Name => {
                                ui.label(RichText::new(text(&c.name)).strong());
                            }
                            Col::Artist => {
                                ui.label(text(&c.artist));
                            }
                            Col::Album => {
                                ui.weak(text(&c.album));
                            }
                            Col::Genre => {
                                if let Some(g) = &c.genre {
                                    theme::chip(ui, g, tag_color(g));
                                }
                            }
                            Col::Year => {
                                ui.weak(text(&c.year));
                            }
                            Col::Charter => {
                                ui.label(text(&c.charter));
                            }
                            Col::Length => {
                                ui.weak(c.length_text());
                            }
                            Col::Difficulty => difficulty_cell(ui, c, instrument),
                            Col::Uploaded => {
                                ui.weak(c.uploaded());
                            }
                            Col::Tags => tags(ui, c, manifest.contains(&c.md5)),
                        });
                    }
                    let resp = row.response();
                    if resp.hovered() {
                        resp.ctx.set_cursor_icon(CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        let m = resp.ctx.input(|i| i.modifiers);
                        click = Some(if m.shift {
                            RowClick::Range(i, m.command)
                        } else if m.command {
                            RowClick::Toggle(i)
                        } else {
                            RowClick::Plain(i)
                        });
                    }
                });
            });
        drop(manifest);

        // Floating "back to top" button once the list is scrolled down.
        if scroll.state.offset.y > 300.0 {
            let pos = scroll.inner_rect.right_bottom() - vec2(64.0, 60.0);
            egui::Area::new(egui::Id::new("scroll_top")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
                let button = theme::primary("⬆").corner_radius(CornerRadius::same(22)).min_size(vec2(44.0, 44.0));
                if ui.add(button).on_hover_text("Back to top").clicked() {
                    self.scroll_to_top = true;
                }
            });
        }

        // The cover column was resized: scale the covers (and rows) to match.
        if let Some(w) = cover_width {
            let size = (w - 8.0).clamp(16.0, 232.0);
            if (size - self.settings.cover_size).abs() > 0.5 {
                self.settings.cover_size = size;
            }
        }
        if let Some(click) = click {
            self.handle_click(click);
        }
        if toggle_all {
            if all_selected {
                for &i in &self.results {
                    self.selected.remove(&self.charts[i].md5);
                }
            } else {
                self.selected.extend(self.results.iter().map(|&i| self.charts[i].md5.clone()));
            }
        }
        if let Some(key) = sort_click {
            let f = &mut self.filters;
            if f.sort == key {
                // asc → desc → back to default order
                if f.descending {
                    f.sort = SortKey::Newest;
                    f.descending = false;
                } else {
                    f.descending = true;
                }
            } else {
                f.sort = key;
                f.descending = false;
            }
        }
    }

    /// File-manager style selection: click selects one row (and shows its details),
    /// Ctrl/Cmd+click toggles a row, Shift+click selects a range (adding to the selection
    /// if Ctrl/Cmd is also held), and the checkbox toggles without touching other rows.
    fn handle_click(&mut self, click: RowClick) {
        let md5 = |app: &Self, i: usize| app.charts[app.results[i]].md5.clone();
        match click {
            RowClick::Plain(i) => {
                let c = self.charts[self.results[i]].clone();
                self.selected = HashSet::from([c.md5.clone()]);
                self.detail = Some(c);
                self.anchor = Some(i);
            }
            RowClick::Toggle(i) | RowClick::Checkbox(i) => {
                let m = md5(self, i);
                if !self.selected.remove(&m) {
                    self.selected.insert(m);
                }
                self.anchor = Some(i);
            }
            RowClick::Range(i, add) => {
                let a = self.anchor.unwrap_or(i);
                if !add {
                    self.selected.clear();
                }
                for k in a.min(i)..=a.max(i) {
                    self.selected.insert(md5(self, k));
                }
            }
        }
    }

    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.detail.clone() else {
            return;
        };
        let downloaded = self.manifest.lock().unwrap().contains(&c.md5);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if ui.small_button("✖").on_hover_text("Close").clicked() {
                        self.detail = None;
                    }
                });
            });
            let size = ui.available_width().min(320.0);
            ui.vertical_centered(|ui| cover(ui, &mut self.covers, c.art.as_deref(), size, covers::LARGE));
            ui.add_space(8.0);
            ui.label(RichText::new(c.name.as_deref().unwrap_or("Unknown")).heading().strong());
            ui.label(RichText::new(c.artist.as_deref().unwrap_or("Unknown Artist")).size(16.0).color(theme::ACCENT_HOVER));
            let sub: Vec<&str> = [c.album.as_deref(), c.year.as_deref()].into_iter().flatten().collect();
            if !sub.is_empty() {
                ui.weak(sub.join("  ·  "));
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(g) = games::game_key(c.icon.as_deref()) {
                    theme::chip(ui, &format!("🏆 {}", games::game_name(&g)), AMBER);
                }
                if let Some(g) = &c.genre {
                    theme::chip(ui, g, tag_color(g));
                }
                if let Some(ch) = &c.charter {
                    theme::chip(ui, &format!("✏ {ch}"), theme::RAINBOW[4]);
                }
                if !c.length_text().is_empty() {
                    theme::chip(ui, &format!("⏱ {}", c.length_text()), theme::RAINBOW[6]);
                }
                if !c.uploaded().is_empty() {
                    theme::chip(ui, &format!("📅 {}", c.uploaded()), Color32::GRAY);
                }
            });

            ui.add_space(10.0);
            if downloaded {
                ui.colored_label(GREEN, RichText::new("✔ Downloaded").strong());
            } else if ui
                .add_enabled(self.job.is_none(), theme::primary("⬇  Download").min_size(vec2(ui.available_width(), 34.0)))
                .clicked()
            {
                self.start_download(c.title(), vec![c.clone()]);
            }

            ui.add_space(12.0);
            ui.label(RichText::new("Instruments").strong());
            egui::Grid::new("detail_diffs").num_columns(3).spacing([10.0, 4.0]).show(ui, |ui| {
                for (k, (_, name, _)) in INSTRUMENTS.iter().enumerate() {
                    let d = c.diffs[k];
                    let notes = c.notes.iter().filter(|s| s.i == k as u8).min_by_key(|s| s.d); // hardest first
                    if d < 0 && notes.is_none() {
                        continue;
                    }
                    ui.label(format!("{}  {name}", instrument_icon(k as u8)));
                    if d >= 0 {
                        ui.label(theme::difficulty_dots(d)).on_hover_text(format!("Intensity {d}"));
                    } else {
                        ui.label("");
                    }
                    match notes {
                        Some(s) => ui.weak(format!("{} notes · {:.0} max NPS", s.n, s.nps)),
                        None => ui.label(""),
                    };
                    ui.end_row();
                }
            });

            ui.add_space(10.0);
            ui.label(RichText::new("Features").strong());
            ui.horizontal_wrapped(|ui| {
                let any = feature_tags(ui, &c, true);
                if !any {
                    ui.weak("None");
                }
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.weak("MD5");
                ui.monospace(RichText::new(&c.md5).small());
                if ui.small_button("📋").on_hover_text("Copy").clicked() {
                    ui.ctx().copy_text(c.md5.clone());
                }
            });
            ui.add_space(8.0);
        });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else {
            return;
        };
        let mut open = true;
        let mut decision = None;
        egui::Window::new("⬇ Start bulk download?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let have = {
                    let m = self.manifest.lock().unwrap();
                    confirm.charts.iter().filter(|c| m.contains(&c.md5)).count()
                };
                let todo = confirm.charts.len() - have;
                ui.label(RichText::new(&confirm.title).heading());
                ui.label(format!("{} charts: {have} already downloaded, {todo} to go.", confirm.charts.len()));
                ui.weak("You can stop at any time; next time it picks up where it left off.");
                ui.add_space(8.0);
                theme::checkbox(ui, &mut self.settings.video, "🎬 Download video backgrounds (much larger)");
                ui.horizontal(|ui| {
                    ui.label("Format:");
                    ui.radio_value(&mut self.settings.format, DownloadFormat::Sng, ".sng");
                    ui.radio_value(&mut self.settings.format, DownloadFormat::Folder, "Chart Folder");
                });
                ui.horizontal(|ui| {
                    ui.label("Wait between downloads:");
                    ui.add(egui::Slider::new(&mut self.settings.delay_secs, 0.5..=10.0).suffix(" s"));
                });
                let hours = todo as f32 * (self.settings.delay_secs + 1.0) / 3600.0;
                ui.weak(format!("Roughly {hours:.0}+ hours at that pace (plus download time)."));
                ui.weak("Encore is donation-funded; please keep the delay reasonable.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(theme::primary("Start")).clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(false);
                    }
                });
            });
        if decision == Some(true) {
            let Confirm { title, charts } = self.confirm.take().unwrap();
            self.start_download(title, charts);
        } else if decision == Some(false) || !open {
            self.confirm = None;
        }
    }

    // ──────────────────────────── Settings tab ────────────────────────────

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        let example = self.charts.first().cloned();
        let building = matches!(self.cat_status, CatStatus::Building { .. });
        let mut rebuild = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_max_width(820.0);
            let s = &mut self.settings;
            ui.add_space(10.0);

            card(ui, "📁 Chart library", |ui| {
                ui.horizontal(|ui| {
                    ui.monospace(s.dir.display().to_string());
                });
                ui.horizontal(|ui| {
                    if ui.button("📂 Open Folder").clicked() {
                        open_folder(&s.dir);
                    }
                    if ui.add_enabled(!busy, theme::primary("Choose…")).clicked()
                        && let Some(dir) = rfd::FileDialog::new().set_directory(&s.dir).pick_folder()
                    {
                        s.dir = dir;
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Chart folder name");
                    ui.label("ⓘ").on_hover_text(
                        "Name for each downloaded chart. Use {name} {artist} {album} {genre} {year} {charter}.\n\
                         Use / to make subfolders, e.g. {artist}/{album}/{name} ({charter})",
                    );
                });
                ui.add(egui::TextEdit::singleline(&mut s.folder_name).desired_width(f32::INFINITY));
                if let Some(example) = &example {
                    ui.weak(format!("e.g. {}", api::resolve_folder_name(&s.folder_name, example)));
                }
            });

            card(ui, "🎨 Appearance", |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut s.dark, true, "🌙 Dark");
                    ui.selectable_value(&mut s.dark, false, "☀ Light");
                    ui.separator();
                    if ui.button("➖").on_hover_text("Zoom out").clicked() {
                        s.zoom = (s.zoom - 0.1).max(0.5);
                    }
                    ui.label(format!("{:.0}%", s.zoom * 100.0));
                    if ui.button("➕").on_hover_text("Zoom in").clicked() {
                        s.zoom = (s.zoom + 0.1).min(2.5);
                    }
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Table layout:");
                    for (d, label) in [(Density::Compact, "Compact"), (Density::Default, "Default"), (Density::Large, "Large")] {
                        if ui.radio_value(&mut s.density, d, label).changed() {
                            s.cover_size = d.cover_size();
                        }
                    }
                });
                ui.weak("Tip: drag the edge of the cover column to make covers bigger or smaller.");
                ui.add_space(4.0);
                ui.label("Table columns:");
                ui.horizontal_wrapped(|ui| {
                    let c = &mut s.columns;
                    theme::checkbox(ui, &mut c.cover, "Cover");
                    theme::checkbox(ui, &mut c.artist, "Artist");
                    theme::checkbox(ui, &mut c.album, "Album");
                    theme::checkbox(ui, &mut c.genre, "Genre");
                    theme::checkbox(ui, &mut c.year, "Year");
                    theme::checkbox(ui, &mut c.charter, "Charter");
                    theme::checkbox(ui, &mut c.length, "Length");
                    theme::checkbox(ui, &mut c.difficulty, "Difficulty");
                    theme::checkbox(ui, &mut c.uploaded, "Upload Date");
                });
            });

            card(ui, "⬇ Downloads", |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Format:");
                        ui.radio_value(&mut s.format, DownloadFormat::Sng, ".sng");
                        ui.radio_value(&mut s.format, DownloadFormat::Folder, "Chart Folder");
                        ui.label("ⓘ").on_hover_text(
                            ".sng: one file per chart; Clone Hero and YARG read these directly.\n\
                             Chart Folder: unpacked into a normal folder with song.ini, notes and audio files.",
                        );
                    });
                    theme::checkbox(ui, &mut s.video, "🎬 Download video backgrounds");
                });
                ui.horizontal(|ui| {
                    ui.label("Wait between bulk downloads:");
                    ui.add(egui::Slider::new(&mut s.delay_secs, 0.5..=10.0).suffix(" s"));
                });
            });

            card(ui, "📚 Catalogue", |ui| {
                let size = std::fs::metadata(catalogue::path()).map(|m| m.len()).unwrap_or(0);
                ui.weak(format!("{}  ({:.1} MB)", catalogue::path().display(), size as f64 / 1_048_576.0));
                ui.horizontal(|ui| {
                    ui.label("Check for new charts on startup:");
                    let text = match s.refresh_after_hours {
                        None => "Never (use the Refresh button)",
                        Some(0) => "Every launch",
                        Some(12) => "If older than 12 hours",
                        Some(24) => "If older than 1 day",
                        Some(168) => "If older than 1 week",
                        Some(_) => "Custom",
                    };
                    egui::ComboBox::from_id_salt("refresh").selected_text(text).width(220.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.refresh_after_hours, Some(0), "Every launch");
                        ui.selectable_value(&mut s.refresh_after_hours, Some(12), "If older than 12 hours");
                        ui.selectable_value(&mut s.refresh_after_hours, Some(24), "If older than 1 day");
                        ui.selectable_value(&mut s.refresh_after_hours, Some(168), "If older than 1 week");
                        ui.selectable_value(&mut s.refresh_after_hours, None, "Never (use the Refresh button)");
                    });
                });
                if ui
                    .add_enabled(!building, egui::Button::new("⟳ Rebuild from scratch"))
                    .on_hover_text("Re-download the whole chart list (about 5 minutes). Normally a refresh is enough.")
                    .clicked()
                {
                    rebuild = true;
                }
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.hyperlink_to("🌐 Chorus Encore", "https://www.enchor.us");
                ui.hyperlink_to("💬 Encore Discord", "https://discord.gg/cqaUXGm");
                ui.hyperlink_to("❤ Encore Patreon", "https://www.patreon.com/cw/ChorusEncore701?l=en-GB");
                ui.hyperlink_to("🌈 Rainbow on GitHub", "https://github.com/RenzoraEngine/rainbow");
            });
            ui.weak(concat!("Rainbow v", env!("CARGO_PKG_VERSION")));
        });
        if rebuild {
            self.update_catalogue(true);
        }
    }

    /// Saves settings and applies side effects when they change.
    fn sync_settings(&mut self, ctx: &egui::Context) {
        if self.settings == self.saved_settings {
            return;
        }
        if self.settings.dir != self.saved_settings.dir {
            self.manifest = Arc::new(Mutex::new(Manifest::load(&self.settings.dir)));
        }
        if self.settings.dark != self.saved_settings.dark || self.settings.zoom != self.saved_settings.zoom {
            theme::apply(ctx, self.settings.dark, self.settings.zoom);
        }
        self.settings.save();
        self.saved_settings = self.settings.clone();
    }
}

/// A titled, rounded section.
fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(10))
        .outer_margin(egui::Margin::symmetric(0, 4))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(title).strong().color(theme::ACCENT_HOVER));
                ui.add_space(2.0);
                add(ui);
            });
        });
}

/// A toggle-style rounded button.
fn pill(ui: &mut egui::Ui, on: bool, text: &str) -> egui::Response {
    let text = if on { RichText::new(text).color(Color32::WHITE).strong() } else { RichText::new(text) };
    let fill = if on { ACCENT } else { ui.visuals().widgets.inactive.bg_fill };
    ui.add(egui::Button::new(text).fill(fill).corner_radius(CornerRadius::same(14)))
}

/// Album art, or a placeholder while it loads (or if the chart has none).
fn cover(ui: &mut egui::Ui, covers: &mut Covers, art: Option<&str>, px: f32, size: u32) {
    let radius = CornerRadius::same(if px > 100.0 { 10 } else { 5 });
    if let Some(tex) = art.and_then(|a| covers.get(a, size)) {
        ui.add(egui::Image::new(tex).fit_to_exact_size(vec2(px, px)).corner_radius(radius));
        return;
    }
    let (rect, _) = ui.allocate_exact_size(vec2(px, px), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, radius, ui.visuals().widgets.inactive.bg_fill);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "♪",
        egui::FontId::proportional(px * 0.45),
        ui.visuals().weak_text_color(),
    );
}

fn instrument_icon(i: u8) -> &'static str {
    match i {
        4 => "🥁",
        5 => "🎹",
        _ => "🎸",
    }
}

/// A stable colour per tag text (e.g. genre), picked from the logo's palette.
fn tag_color(s: &str) -> Color32 {
    let h = s.to_lowercase().bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    theme::RAINBOW[(h % theme::RAINBOW.len() as u32) as usize]
}

/// Difficulty for the chosen instrument as dots, or a compact per-instrument summary.
fn difficulty_cell(ui: &mut egui::Ui, c: &Chart, instrument: Option<u8>) {
    if let Some(i) = instrument {
        let d = c.diffs[i as usize];
        if d >= 0 {
            ui.label(theme::difficulty_dots(d));
        }
        return;
    }
    for (k, label) in [(0, "G"), (3, "B"), (4, "D"), (5, "K")] {
        let d = c.diffs[k];
        if d >= 0 {
            let color = theme::RAINBOW[[3, 3, 2, 2, 1, 0, 0][d.clamp(0, 6) as usize]];
            ui.label(RichText::new(format!("{label}{d}")).color(color).small().strong())
                .on_hover_text(format!("{}: {d}", INSTRUMENTS[k].1));
        }
    }
}

/// Icon tags for a table row: downloaded/broken status, source game, instruments and features.
fn tags(ui: &mut egui::Ui, c: &Chart, downloaded: bool) {
    ui.spacing_mut().item_spacing.x = 4.0;
    if downloaded {
        ui.colored_label(GREEN, "✔").on_hover_text("Downloaded");
    }
    if c.has(flag::BROKEN) {
        ui.colored_label(RED, "⚠").on_hover_text("Encore reports this chart as unplayable");
    }
    if let Some(g) = games::game_key(c.icon.as_deref()) {
        ui.colored_label(AMBER, "🏆").on_hover_text(games::game_name(&g));
    }
    let mut icons: Vec<&str> = Vec::new();
    for (k, _) in INSTRUMENTS.iter().enumerate() {
        if c.diffs[k] >= 0 || c.notes.iter().any(|s| s.i == k as u8) {
            let icon = instrument_icon(k as u8);
            if !icons.contains(&icon) {
                icons.push(icon);
            }
        }
    }
    for icon in icons {
        ui.label(icon);
    }
    feature_tags(ui, c, false);
}

/// Feature tags; `full` shows labelled chips (details panel), otherwise just icons.
fn feature_tags(ui: &mut egui::Ui, c: &Chart, full: bool) -> bool {
    let features = [
        (flag::VIDEO, "🎬", "Video", theme::RAINBOW[4]),
        (flag::LYRICS, "📝", "Lyrics", theme::RAINBOW[2]),
        (flag::VOCALS, "🎤", "Vocals", theme::RAINBOW[6]),
        (flag::SOLO, "🌟", "Solos", theme::RAINBOW[1]),
        (flag::TAP, "👆", "Tap notes", theme::RAINBOW[3]),
        (flag::OPEN, "⭕", "Open notes", theme::RAINBOW[5]),
        (flag::FORCED, "⚡", "Forced notes", theme::RAINBOW[2]),
        (flag::ROLL, "🌀", "Roll lanes", theme::RAINBOW[4]),
        (flag::KICK2X, "👣", "2x kick", theme::RAINBOW[0]),
        (flag::MODCHART, "🧪", "Modchart", theme::RAINBOW[6]),
        (flag::ISSUES, "⚠", "Chart issues", AMBER),
        (flag::BROKEN, "⛔", "Broken", RED),
    ];
    let mut any = false;
    for (bit, icon, label, color) in features {
        if !c.has(bit) {
            continue;
        }
        any = true;
        if full {
            theme::chip(ui, &format!("{icon} {label}"), color);
        } else if !matches!(bit, flag::ISSUES | flag::BROKEN | flag::FORCED | flag::OPEN | flag::TAP) {
            ui.label(RichText::new(icon).color(color)).on_hover_text(label);
        }
    }
    any
}

fn range_row(ui: &mut egui::Ui, label: &str, r: &mut Range, tip: &str, label_w: f32) {
    let resp = ui.add_sized([label_w, 24.0], egui::Label::new(label).truncate());
    if !tip.is_empty() {
        resp.on_hover_text(tip);
    }
    ui.add_sized([64.0, 24.0], egui::TextEdit::singleline(&mut r.min).hint_text("Min"));
    ui.add_sized([64.0, 24.0], egui::TextEdit::singleline(&mut r.max).hint_text("Max"));
    ui.end_row();
}

/// Opens a folder in the system file manager.
fn open_folder(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(dir).spawn();
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_messages();
        self.covers.poll();
        let ctx = ui.ctx().clone();

        egui::Panel::top("header").show(ui, |ui| self.header(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(3.0);
            self.status_bar(ui);
            ui.add_space(3.0);
        });
        if self.job.is_some() {
            egui::Panel::bottom("download").show(ui, |ui| self.download_bar(ui));
        }
        match self.tab {
            Tab::Browse => {
                let search = egui::Panel::top("search").show(ui, |ui| {
                    ui.add_space(8.0);
                    self.search_bar(ui);
                    ui.add_space(8.0);
                });
                self.search_bottom = search.response.rect.bottom();
                if self.detail.is_some() {
                    egui::Panel::right("detail").resizable(true).default_size(340.0).show(ui, |ui| self.detail_panel(ui));
                }
                self.live_search();
                self.refresh_results();
                egui::CentralPanel::default_margins().show(ui, |ui| self.results(ui));
                self.advanced_overlay(&ctx);
            }
            Tab::Settings => {
                egui::CentralPanel::default_margins().show(ui, |ui| self.settings_tab(ui));
            }
        }
        self.confirm_dialog(&ctx);
        self.log_window(&ctx);
        self.sync_settings(&ctx);

        let building = matches!(self.cat_status, CatStatus::Building { .. });
        if self.live_searching || self.job.is_some() || building || self.track_lookup {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if !self.cat_complete && !self.filters.query.is_empty() {
            // Wake up after the typing pause so the live search fires.
            ctx.request_repaint_after(LIVE_SEARCH_DEBOUNCE);
        }
    }
}

fn ago(unix: u64) -> String {
    match now().saturating_sub(unix) {
        s if s < 90 => "just now".to_owned(),
        s if s < 90 * 60 => format!("{} min ago", s / 60),
        s if s < 36 * 3600 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}

/// State owned by a background download thread.
struct Worker {
    client: Client,
    tx: Sender<Msg>,
    settings: Settings,
    manifest: Arc<Mutex<Manifest>>,
    stop: Arc<AtomicBool>,
    checked: usize,
    downloaded: usize,
    failed: usize,
}

impl Worker {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Sleeps, but wakes early if Stop is pressed.
    fn wait(&self, d: Duration) {
        let end = Instant::now() + d;
        while !self.stopped() && Instant::now() < end {
            thread::sleep(Duration::from_millis(100).min(end - Instant::now()));
        }
    }

    fn send_queue(&self) {
        let _ = self.tx.send(Msg::Queue { checked: self.checked, downloaded: self.downloaded, failed: self.failed });
    }

    /// Downloads one chart unless it's already in the manifest. Returns true if it hit the network.
    fn download(&mut self, chart: &Chart) -> bool {
        self.checked += 1;
        if self.manifest.lock().unwrap().contains(&chart.md5) {
            // Skips are fast; don't flood the UI with a message for each one.
            if self.checked % 500 == 0 {
                self.send_queue();
            }
            return false;
        }
        let title = chart.title();
        let tx = self.tx.clone();
        let result = self.client.download(chart, &self.settings, |done, total| {
            let _ = tx.send(Msg::FileProgress { title: title.clone(), done, total });
        });
        let line = match result {
            Ok(path) => {
                self.downloaded += 1;
                match self.manifest.lock().unwrap().add(chart, &path) {
                    Ok(()) => format!("Downloaded {title}"),
                    Err(e) => format!("Downloaded {title}, but couldn't update download list: {e:#}"),
                }
            }
            Err(e) => {
                self.failed += 1;
                format!("FAILED {title}: {e:#}")
            }
        };
        let _ = self.tx.send(Msg::Log(line));
        self.send_queue();
        true
    }
}
