//! Rainbow: a small desktop alternative to Bridge for Chorus Encore (enchor.us).

#![windows_subsystem = "windows"]

mod api;
mod catalogue;
mod filter;
mod settings;
mod sng;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};
use egui_extras::{Column, TableBuilder};

use api::{Chart, Client, DIFFICULTIES, DRUM_TYPES, DRUMS, INSTRUMENTS, Manifest};
use catalogue::Catalogue;
use filter::{Filters, Index, Range, SortKey, TEXT_FIELDS, TOGGLES};
use settings::{DownloadFormat, Settings};

/// Online mode: start fetching the next page when scrolled within this many rows of the end.
const PREFETCH_ROWS: usize = 30;
const MAX_LOG: usize = 500;
/// Downloads bigger than this ask for confirmation and use the polite delay.
const BULK: usize = 50;
const GREEN: Color32 = Color32::from_rgb(80, 180, 90);
const RED: Color32 = Color32::from_rgb(220, 90, 90);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rainbow")
            .with_inner_size([1250.0, 800.0])
            .with_min_inner_size([800.0, 450.0])
            .with_icon(eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")).unwrap_or_default()),
        ..Default::default()
    };
    eframe::run_native("Rainbow", options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))))
}

/// Messages from background threads to the UI.
enum Msg {
    Results { charts: Vec<Chart>, found: u64, page: u32 },
    SearchFailed(String),
    TrackHash(Result<HashSet<String>, String>),
    CatalogueProgress { page: u32, pages: u32, charts: usize },
    CatalogueDone(Result<(Catalogue, usize), String>),
    FileProgress { title: String, done: u64, total: Option<u64> },
    Queue { checked: usize, downloaded: usize, failed: usize },
    Status(String),
    Log(String),
    JobDone,
}

/// Search results fetched page by page from the API, used until the catalogue is complete.
#[derive(Default)]
struct Online {
    query: String,
    instrument: Option<u8>,
    difficulty: Option<u8>,
    results: Vec<Chart>,
    found: u64,
    page: u32,
    searching: bool,
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
    Name,
    Artist,
    Album,
    Genre,
    Year,
    Charter,
    Length,
    Difficulty,
    Uploaded,
    Status,
}

struct App {
    client: Client,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,

    settings: Settings,
    saved_settings: Settings,
    tab: Tab,

    filters: Filters,
    searched_filters: Option<Filters>,
    show_advanced: bool,
    track_lookup: bool,
    online: Online,
    catalogue: Option<Arc<Catalogue>>,
    index: Option<Index>,
    local_results: Vec<usize>,
    cat_status: CatStatus,
    cat_stop: Arc<AtomicBool>,

    selected: HashSet<String>,
    scroll_to_top: bool,
    manifest: Arc<Mutex<Manifest>>,

    job: Option<Job>,
    stop: Arc<AtomicBool>,
    confirm: Option<Confirm>,
    status: Option<(String, Instant)>,
    log: Vec<String>,
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
        apply_appearance(ctx, &settings);
        let mut app = Self {
            client,
            tx,
            rx,
            manifest: Arc::new(Mutex::new(Manifest::load(&settings.dir))),
            saved_settings: settings.clone(),
            settings,
            tab: Tab::Browse,
            filters: Filters::default(),
            searched_filters: None,
            show_advanced: false,
            track_lookup: false,
            online: Online::default(),
            catalogue: None,
            index: None,
            local_results: Vec::new(),
            cat_status: CatStatus::Idle,
            cat_stop: Arc::new(AtomicBool::new(false)),
            selected: HashSet::new(),
            scroll_to_top: false,
            job: None,
            stop: Arc::new(AtomicBool::new(false)),
            confirm: None,
            status: None,
            log: Vec::new(),
        };

        let cached = Catalogue::load();
        match &cached {
            Some(cat) => app.log(format!("Loaded catalogue: {} charts", cat.charts.len())),
            None if catalogue::path().exists() => {
                app.log("Catalogue format changed; rebuilding it with the new search fields".to_owned())
            }
            None => {}
        }
        if let Some(cat) = cached.clone().filter(|c| c.complete) {
            app.set_catalogue(cat);
        } else {
            app.search_online();
        }
        app.update_catalogue(cached.unwrap_or_default());
        app
    }

    /// True once searches run against the local catalogue.
    fn local(&self) -> bool {
        self.index.is_some()
    }

    fn set_catalogue(&mut self, cat: Catalogue) {
        self.index = Some(Index::new(&cat.charts));
        self.catalogue = Some(Arc::new(cat));
        self.searched_filters = None; // re-run the search
    }

    /// Re-runs the local search whenever the filters change.
    fn refresh_local(&mut self) {
        if self.searched_filters.as_ref() == Some(&self.filters) {
            return;
        }
        if let (Some(index), Some(cat)) = (&self.index, &self.catalogue) {
            self.local_results = index.search(&cat.charts, &self.filters);
            self.scroll_to_top = true;
            self.searched_filters = Some(self.filters.clone());
        }
    }

    fn search_online(&mut self) {
        self.online = Online {
            query: self.filters.query.clone(),
            instrument: self.filters.instrument,
            difficulty: self.filters.difficulty,
            ..Default::default()
        };
        self.selected.clear();
        self.fetch_online_page(1);
    }

    fn fetch_online_page(&mut self, page: u32) {
        self.online.searching = true;
        let client = self.client.clone();
        let tx = self.tx.clone();
        let q = if self.online.query.trim().is_empty() { "*".to_owned() } else { self.online.query.clone() };
        let (inst, diff) = (self.online.instrument, self.online.difficulty);
        thread::spawn(move || {
            let msg = match client.search(&q, page, inst, diff) {
                Ok(r) => Msg::Results { charts: r.charts, found: r.found, page },
                Err(e) => Msg::SearchFailed(format!("{e:#}")),
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
        let client = self.client.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::TrackHash(client.md5s_for_track_hash(&hash).map_err(|e| format!("{e:#}"))));
        });
    }

    fn update_catalogue(&mut self, mut cat: Catalogue) {
        self.cat_stop.store(false, Ordering::Relaxed);
        self.cat_status = CatStatus::Building { page: cat.next_page, pages: 0, charts: cat.charts.len() };
        let client = self.client.clone();
        let tx = self.tx.clone();
        let stop = self.cat_stop.clone();
        thread::spawn(move || {
            let result = cat.update(&client, &stop, |p| {
                let catalogue::Progress::Page { page, pages, charts } = p;
                let _ = tx.send(Msg::CatalogueProgress { page, pages, charts });
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

    fn chart_at(&self, row: usize) -> Option<&Chart> {
        match (&self.catalogue, self.local()) {
            (Some(cat), true) => self.local_results.get(row).map(|&i| &cat.charts[i]),
            _ => self.online.results.get(row),
        }
    }

    fn result_count(&self) -> usize {
        if self.local() { self.local_results.len() } else { self.online.results.len() }
    }

    fn selected_charts(&self) -> Vec<Chart> {
        (0..self.result_count())
            .filter_map(|i| self.chart_at(i))
            .filter(|c| self.selected.contains(&c.md5))
            .cloned()
            .collect()
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
                Msg::Results { charts, found, page } => {
                    let o = &mut self.online;
                    o.searching = false;
                    o.found = found;
                    o.page = page;
                    self.scroll_to_top |= page == 1;
                    // The API sometimes repeats a chart; keep the first copy.
                    let mut seen: HashSet<String> = o.results.iter().map(|c| c.md5.clone()).collect();
                    o.results.extend(charts.into_iter().filter(|c| seen.insert(c.md5.clone())));
                }
                Msg::SearchFailed(e) => {
                    self.online.searching = false;
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
                Msg::CatalogueProgress { page, pages, charts } => {
                    self.cat_status = CatStatus::Building { page, pages, charts };
                }
                Msg::CatalogueDone(Ok((cat, added))) => {
                    self.cat_status = CatStatus::Idle;
                    let stopped = self.cat_stop.load(Ordering::Relaxed);
                    if cat.complete {
                        self.log(format!("Catalogue up to date: {} charts ({added} new)", cat.charts.len()));
                        if added > 0 || !self.local() {
                            self.set_catalogue(cat);
                        }
                    } else {
                        let how = if stopped { "paused" } else { "incomplete" };
                        self.log(format!("Catalogue {how} at {} charts", cat.charts.len()));
                        self.catalogue = Some(Arc::new(cat));
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

    // ───────────────────────────── Browse tab ─────────────────────────────

    fn search_bar(&mut self, ui: &mut egui::Ui) {
        let local = self.local();
        ui.horizontal(|ui| {
            let hint = if local { "Search…  (instant)" } else { "Search…  (press Enter)" };
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.filters.query)
                    .hint_text(hint)
                    .desired_width((ui.available_width() - 560.0).max(200.0)),
            );
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            let inst_text = self.filters.instrument.map_or("Any Instrument", |i| INSTRUMENTS[i as usize].1);
            let mut changed = false;
            egui::ComboBox::from_id_salt("instrument").selected_text(inst_text).width(150.0).show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut self.filters.instrument, None, "Any Instrument").changed();
                for (k, (_, name, _)) in INSTRUMENTS.iter().enumerate() {
                    changed |= ui.selectable_value(&mut self.filters.instrument, Some(k as u8), *name).changed();
                }
            });
            let diff_text = self.filters.difficulty.map_or("Any Difficulty", |d| DIFFICULTIES[d as usize].1);
            egui::ComboBox::from_id_salt("difficulty").selected_text(diff_text).width(110.0).show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut self.filters.difficulty, None, "Any Difficulty").changed();
                for (k, (_, name)) in DIFFICULTIES.iter().enumerate() {
                    changed |= ui.selectable_value(&mut self.filters.difficulty, Some(k as u8), *name).changed();
                }
            });
            if self.filters.instrument == Some(DRUMS) && local {
                let dt = self.filters.drum_type.map_or("Any Drum Type", |d| DRUM_TYPES[d as usize]);
                egui::ComboBox::from_id_salt("drumtype").selected_text(dt).width(110.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filters.drum_type, None, "Any Drum Type");
                    for (k, name) in DRUM_TYPES.iter().enumerate() {
                        ui.selectable_value(&mut self.filters.drum_type, Some(k as u8), *name);
                    }
                });
            }
            let clicked = ui.button("🔍 Search").clicked();
            if !local && (enter || clicked || changed) && !self.online.searching {
                self.search_online();
                resp.request_focus();
            }

            let n = self.filters.advanced_count();
            let label = format!("Advanced search{} {}", if n > 0 { format!(" ({n})") } else { String::new() }, if self.show_advanced { "⏶" } else { "⏷" });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.show_advanced, label);
            });
        });

        // Quick options.
        ui.add_enabled_ui(local, |ui| {
            ui.horizontal(|ui| {
                let f = &mut self.filters;
                let all = !f.no_video && !f.no_broken && f.advanced_count() == 0;
                if ui.selectable_label(all, "All songs").on_hover_text("Clear all filters").clicked() {
                    f.no_video = false;
                    f.no_broken = false;
                    f.clear_advanced();
                }
                ui.toggle_value(&mut f.no_video, "No videos").on_hover_text("Hide charts that have a video background");
                ui.toggle_value(&mut f.no_broken, "No broken songs").on_hover_text(
                    "Hide charts Encore flags as unplayable: missing or unreadable chart/audio, or no notes",
                );
                if !local {
                    ui.weak("Filters and advanced search are available once the catalogue has finished building.");
                }
            });
        });

        if self.show_advanced {
            ui.add_space(4.0);
            ui.add_enabled_ui(local, |ui| self.advanced_search(ui));
        }
    }

    fn advanced_search(&mut self, ui: &mut egui::Ui) {
        let mut lookup = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let f = &mut self.filters;
            ui.horizontal_top(|ui| {
                egui::Grid::new("adv_text").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                    ui.strong("Search by");
                    ui.strong("Exact").on_hover_text("Only include results that match perfectly. (not case sensitive)");
                    ui.strong("Exclude").on_hover_text("Do not include results that match this.");
                    ui.end_row();
                    for (k, label) in TEXT_FIELDS.iter().enumerate() {
                        ui.add(egui::TextEdit::singleline(&mut f.text[k].value).hint_text(*label).desired_width(170.0));
                        ui.checkbox(&mut f.text[k].exact, "");
                        ui.checkbox(&mut f.text[k].exclude, "");
                        ui.end_row();
                    }
                });
                ui.separator();
                egui::Grid::new("adv_ranges").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    range_row(ui, "Length (minutes)", &mut f.length_min, None);
                    range_row(ui, "Intensity", &mut f.intensity, Some("Also known as chart difficulty. Typically a number between 0 and 6."));
                    range_row(ui, "Average NPS", &mut f.avg_nps, Some("Average notes per second, for the chosen instrument/difficulty."));
                    range_row(ui, "Max NPS", &mut f.max_nps, Some("Highest notes per second, for the chosen instrument/difficulty."));
                    range_row(ui, "Year", &mut f.year, None);
                });
                ui.separator();
                egui::Grid::new("adv_misc").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Modified After").on_hover_text("The date of the last time this chart was modified in Google Drive.");
                    ui.add(egui::TextEdit::singleline(&mut f.modified_after).hint_text("YYYY-MM-DD").desired_width(150.0));
                    ui.end_row();
                    ui.label("Hash").on_hover_text("The MD5 hash of the chart folder or .sng file. You can enter multiple values if they are separated by commas.");
                    ui.add(egui::TextEdit::singleline(&mut f.hash).desired_width(150.0));
                    ui.end_row();
                    ui.label("Track Hash").on_hover_text("The hash of only things that impact scoring on a specific track. Multiple values can be separated by commas. Looked up on Encore's server.");
                    let resp = ui.add(egui::TextEdit::singleline(&mut f.track_hash).desired_width(150.0));
                    ui.end_row();
                    if resp.changed() {
                        f.track_md5s = None;
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        lookup = true;
                    }
                });
            });
            ui.add_space(6.0);
            let f = &mut self.filters;
            egui::Grid::new("adv_toggles").num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
                for (k, (label, _, applies)) in TOGGLES.iter().enumerate() {
                    let enabled = applies.enabled(f.instrument);
                    let state = &mut f.toggles[k];
                    let (icon, color) = match state {
                        None => ("◻", ui.visuals().weak_text_color()),
                        Some(true) => ("✔", GREEN),
                        Some(false) => ("✖", RED),
                    };
                    let text = RichText::new(format!("{icon} {label}")).color(color);
                    let resp = ui
                        .add_enabled(enabled, egui::Button::new(text).frame(false))
                        .on_hover_text("Click to cycle: any → must have → must not have");
                    if resp.clicked() {
                        *state = match state {
                            None => Some(true),
                            Some(true) => Some(false),
                            Some(false) => None,
                        };
                    }
                    if k % 4 == 3 {
                        ui.end_row();
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Reset advanced search").clicked() {
                    self.filters.clear_advanced();
                }
                if self.track_lookup {
                    ui.spinner();
                    ui.label("Looking up track hash…");
                } else if self.filters.track_md5s.is_none()
                    && !self.filters.track_hash.trim().is_empty()
                    && ui.button("Look up track hash").clicked()
                {
                    lookup = true;
                }
            });
        });
        if lookup && !self.track_lookup {
            self.lookup_track_hash();
        }
    }

    fn catalogue_bar(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        ui.horizontal(|ui| {
            ui.label("Catalogue:");
            match &self.cat_status {
                CatStatus::Building { page, pages, charts } => {
                    ui.spinner();
                    let complete = self.catalogue.as_ref().is_some_and(|c| c.complete);
                    let what = if complete { "Checking for new charts" } else { "Building" };
                    if *pages > 0 {
                        ui.label(format!("{what}… page {page}/{pages}, {charts} charts"));
                    } else {
                        ui.label(format!("{what}…"));
                    }
                    if !complete {
                        ui.weak("(searching Encore online until this finishes)");
                    }
                    if ui.button("⏸ Pause").clicked() {
                        self.cat_stop.store(true, Ordering::Relaxed);
                    }
                }
                CatStatus::Failed(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, "update failed").on_hover_text(e.clone());
                    if ui.button("Retry").clicked() {
                        self.update_catalogue(self.catalogue.as_deref().cloned().unwrap_or_default());
                    }
                }
                CatStatus::Idle => match self.catalogue.as_deref() {
                    Some(cat) if cat.complete => {
                        ui.label(format!("{} charts, updated {}", cat.charts.len(), ago(cat.updated)));
                        if ui.button("⟳ Refresh").clicked() {
                            self.update_catalogue(cat.clone());
                        }
                    }
                    Some(cat) => {
                        ui.label(format!("incomplete ({} charts so far)", cat.charts.len()));
                        if ui.button("▶ Resume").clicked() {
                            self.update_catalogue(cat.clone());
                        }
                    }
                    None => {
                        if ui.button("Build").clicked() {
                            self.update_catalogue(Catalogue::default());
                        }
                    }
                },
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let all = self.catalogue.as_ref().filter(|c| c.complete && self.local());
                let label = match all {
                    Some(c) => format!("⬇ Download entire library ({})…", c.charts.len()),
                    None => "⬇ Download entire library…".to_owned(),
                };
                let resp = ui
                    .add_enabled(!busy && all.is_some(), egui::Button::new(label))
                    .on_disabled_hover_text("Available once the catalogue has finished building");
                if resp.clicked()
                    && let Some(c) = all
                {
                    let charts = c.charts.clone();
                    self.request_download("Entire Encore library".to_owned(), charts);
                }
            });
        });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if let Some(job) = &self.job {
            ui.horizontal(|ui| {
                ui.strong(&job.title);
                ui.label(format!(
                    "{} / {} checked · {} downloaded · {} already had · {} failed",
                    job.checked,
                    job.total,
                    job.downloaded,
                    job.checked - job.downloaded - job.failed,
                    job.failed
                ));
                if ui.button("⏹ Stop").clicked() {
                    self.stop.store(true, Ordering::Relaxed);
                }
            });
            ui.add(egui::ProgressBar::new(job.checked as f32 / job.total.max(1) as f32).show_percentage());
            if let Some((title, done, total)) = &job.file {
                let frac = total.map(|t| *done as f32 / t.max(1) as f32).unwrap_or(0.0);
                ui.add(egui::ProgressBar::new(frac).text(format!("{title}  ({:.1} MB)", *done as f64 / 1_048_576.0)));
            }
        }
        if let Some((s, at)) = &self.status
            && at.elapsed() < Duration::from_secs(60)
        {
            ui.colored_label(ui.visuals().warn_fg_color, s);
        }
        egui::CollapsingHeader::new(format!("Log ({})", self.log.len())).default_open(false).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(150.0).stick_to_bottom(true).show(ui, |ui| {
                for line in &self.log {
                    ui.label(line);
                }
            });
        });
        ui.add_space(4.0);
    }

    fn columns(&self) -> Vec<Col> {
        let c = &self.settings.columns;
        let mut cols = vec![Col::Check, Col::Name];
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
        cols.push(Col::Status);
        cols
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        let local = self.local();
        let count = self.result_count();
        ui.horizontal(|ui| {
            if self.online.searching {
                ui.spinner();
            }
            if local {
                ui.label(format!("{count} results"));
            } else {
                ui.label(format!("{} found online, showing {count}", self.online.found));
            }
            ui.separator();
            if ui.button("Select all").clicked() {
                self.selected = (0..count).filter_map(|i| self.chart_at(i)).map(|c| c.md5.clone()).collect();
            }
            if ui.button("Clear").clicked() {
                self.selected.clear();
            }
            let n = self.selected.len();
            if ui.add_enabled(!busy && n > 0, egui::Button::new(format!("⬇ Download selected ({n})"))).clicked() {
                let charts = self.selected_charts();
                self.selected.clear();
                self.request_download(format!("{} selected chart(s)", charts.len()), charts);
            }
            let resp = ui
                .add_enabled(!busy && local && count > 0, egui::Button::new(format!("⬇ Download all {count} results…")))
                .on_disabled_hover_text("Available once the catalogue has finished building");
            if resp.clicked() {
                let charts: Vec<Chart> = (0..count).filter_map(|i| self.chart_at(i)).cloned().collect();
                self.request_download("All current results".to_owned(), charts);
            }
        });
        ui.separator();

        let cols = self.columns();
        let manifest = self.manifest.lock().unwrap();
        let mut toggled = Vec::new();
        let mut load_more = false;
        let mut sort_click = None;
        let has_more = !local && (count as u64) < self.online.found;
        let row_h = if self.settings.compact { 18.0 } else { 26.0 };
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        if std::mem::take(&mut self.scroll_to_top) {
            table = table.scroll_to_row(0, None);
        }
        for col in &cols {
            table = table.column(match col {
                Col::Check => Column::exact(24.0),
                Col::Name => Column::initial(240.0).clip(true),
                Col::Artist | Col::Album => Column::initial(180.0).clip(true),
                Col::Genre | Col::Charter => Column::initial(120.0).clip(true),
                Col::Year => Column::initial(50.0).clip(true),
                Col::Length => Column::initial(55.0),
                Col::Difficulty => Column::initial(95.0).clip(true),
                Col::Uploaded => Column::initial(85.0),
                Col::Status => Column::remainder().at_least(90.0),
            });
        }
        let (sort, desc) = (self.filters.sort, self.filters.descending);
        let instrument = self.filters.instrument;
        table
            .header(22.0, |mut h| {
                for col in &cols {
                    let (title, key) = match col {
                        Col::Check | Col::Status => ("", None),
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
                        let Some(key) = key.filter(|_| local) else {
                            ui.strong(title);
                            return;
                        };
                        let arrow = if sort == key { if desc { " ⏷" } else { " ⏶" } } else { "" };
                        if ui.add(egui::Button::new(RichText::new(format!("{title}{arrow}")).strong()).frame(false)).clicked() {
                            sort_click = Some(key);
                        }
                    });
                }
            })
            .body(|body| {
                // Only visible rows are laid out, so seeing one near the end means the user
                // has scrolled there: fetch the next page (online mode only).
                body.rows(row_h, count + usize::from(has_more), |mut row| {
                    let i = row.index();
                    if has_more && i + PREFETCH_ROWS >= count {
                        load_more = true;
                    }
                    let Some(c) = self.chart_at(i) else {
                        row.col(|_| {});
                        row.col(|ui| {
                            ui.spinner();
                            ui.label("Loading more…");
                        });
                        return;
                    };
                    let text = |s: &Option<String>| s.clone().unwrap_or_default();
                    for col in &cols {
                        row.col(|ui| match col {
                            Col::Check => {
                                let mut checked = self.selected.contains(&c.md5);
                                if ui.checkbox(&mut checked, "").changed() {
                                    toggled.push(c.md5.clone());
                                }
                            }
                            Col::Name => {
                                ui.label(text(&c.name));
                            }
                            Col::Artist => {
                                ui.label(text(&c.artist));
                            }
                            Col::Album => {
                                ui.label(text(&c.album));
                            }
                            Col::Genre => {
                                ui.label(text(&c.genre));
                            }
                            Col::Year => {
                                ui.label(text(&c.year));
                            }
                            Col::Charter => {
                                ui.label(text(&c.charter));
                            }
                            Col::Length => {
                                ui.label(c.length_text());
                            }
                            Col::Difficulty => {
                                ui.label(difficulty_text(c, instrument));
                            }
                            Col::Uploaded => {
                                ui.label(c.uploaded());
                            }
                            Col::Status => {
                                if manifest.contains(&c.md5) {
                                    ui.colored_label(GREEN, "✔ downloaded");
                                } else if c.has(api::flag::BROKEN) {
                                    ui.colored_label(RED, "⚠ broken").on_hover_text("Encore reports this chart as unplayable");
                                }
                            }
                        });
                    }
                });
            });
        drop(manifest);

        for md5 in toggled {
            if !self.selected.remove(&md5) {
                self.selected.insert(md5);
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
        if load_more && !self.online.searching {
            self.fetch_online_page(self.online.page + 1);
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else {
            return;
        };
        let mut open = true;
        let mut decision = None;
        egui::Window::new("Start bulk download?")
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
                ui.strong(&confirm.title);
                ui.label(format!("{} charts: {have} already downloaded, {todo} to go.", confirm.charts.len()));
                ui.label("You can stop at any time; next time it picks up where it left off.");
                ui.add_space(6.0);
                ui.checkbox(&mut self.settings.video, "Download video backgrounds (much larger)");
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
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Start").clicked() {
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
        egui::ScrollArea::vertical().show(ui, |ui| {
            let s = &mut self.settings;
            ui.add_space(8.0);
            ui.strong("Chart library directory");
            ui.horizontal(|ui| {
                ui.monospace(s.dir.display().to_string());
                if ui.button("Open Folder").clicked() {
                    open_folder(&s.dir);
                }
                if ui.add_enabled(!busy, egui::Button::new("Choose…")).clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_directory(&s.dir).pick_folder()
                {
                    s.dir = dir;
                }
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.strong("Chart folder name");
                ui.label("ⓘ").on_hover_text(
                    "Name for each downloaded chart. Use {name} {artist} {album} {genre} {year} {charter}.\n\
                     Use / to make subfolders, e.g. {artist}/{album}/{name} ({charter})",
                );
            });
            ui.add(egui::TextEdit::singleline(&mut s.folder_name).desired_width(f32::INFINITY));
            if let Some(example) = self.catalogue.as_ref().and_then(|c| c.charts.first()) {
                ui.weak(format!("e.g. {}", api::resolve_folder_name(&s.folder_name, example)));
            }

            ui.add_space(12.0);
            ui.strong("Appearance");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut s.dark, true, "Dark");
                ui.selectable_value(&mut s.dark, false, "Light");
                ui.separator();
                if ui.button("➖").on_hover_text("Zoom out").clicked() {
                    s.zoom = (s.zoom - 0.1).max(0.5);
                }
                ui.label(format!("{:.0}%", s.zoom * 100.0));
                if ui.button("➕").on_hover_text("Zoom in").clicked() {
                    s.zoom = (s.zoom + 0.1).min(2.5);
                }
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.strong("Download Format");
                ui.label("ⓘ").on_hover_text(
                    ".sng: one file per chart; Clone Hero and YARG read these directly.\n\
                     Chart Folder: unpacked into a normal folder with song.ini, notes and audio files.",
                );
            });
            ui.add_enabled_ui(!busy, |ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut s.format, DownloadFormat::Sng, ".sng");
                    ui.radio_value(&mut s.format, DownloadFormat::Folder, "Chart Folder");
                });
                ui.checkbox(&mut s.video, "Download Video Backgrounds");
            });
            ui.horizontal(|ui| {
                ui.label("Wait between bulk downloads:");
                ui.add(egui::Slider::new(&mut s.delay_secs, 0.5..=10.0).suffix(" s"));
            });

            ui.add_space(12.0);
            ui.strong("Table Layout");
            ui.horizontal(|ui| {
                ui.radio_value(&mut s.compact, false, "Default");
                ui.radio_value(&mut s.compact, true, "Compact");
            });

            ui.add_space(12.0);
            ui.strong("Table Columns");
            ui.horizontal(|ui| {
                let c = &mut s.columns;
                ui.checkbox(&mut c.artist, "Artist");
                ui.checkbox(&mut c.album, "Album");
                ui.checkbox(&mut c.genre, "Genre");
                ui.checkbox(&mut c.year, "Year");
            });
            ui.horizontal(|ui| {
                let c = &mut s.columns;
                ui.checkbox(&mut c.charter, "Charter");
                ui.checkbox(&mut c.length, "Length");
                ui.checkbox(&mut c.difficulty, "Difficulty");
                ui.checkbox(&mut c.uploaded, "Upload Date");
            });

            ui.add_space(12.0);
            ui.strong("Catalogue");
            let size = std::fs::metadata(catalogue::path()).map(|m| m.len()).unwrap_or(0);
            ui.label(format!("{}  ({:.1} MB)", catalogue::path().display(), size as f64 / 1_048_576.0));
            let building = matches!(self.cat_status, CatStatus::Building { .. });
            if ui
                .add_enabled(!building, egui::Button::new("Rebuild from scratch"))
                .on_hover_text("Re-download the whole chart list (about 4 minutes). Normally a refresh is enough.")
                .clicked()
            {
                self.update_catalogue(Catalogue::default());
            }

            ui.add_space(20.0);
            ui.horizontal(|ui| {
                ui.hyperlink_to("Chorus Encore", "https://www.enchor.us");
                ui.hyperlink_to("Encore Patreon", "https://www.patreon.com/ChorusEncore701");
                ui.hyperlink_to("Bridge (official app)", "https://github.com/Geomitron/Bridge");
            });
            ui.weak(concat!("Rainbow v", env!("CARGO_PKG_VERSION")));
        });
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
            apply_appearance(ctx, &self.settings);
        }
        self.settings.save();
        self.saved_settings = self.settings.clone();
    }
}

fn range_row(ui: &mut egui::Ui, label: &str, r: &mut Range, tip: Option<&str>) {
    let resp = ui.label(label);
    if let Some(tip) = tip {
        resp.on_hover_text(tip);
    }
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut r.min).hint_text("Min").desired_width(55.0));
        ui.add(egui::TextEdit::singleline(&mut r.max).hint_text("Max").desired_width(55.0));
    });
    ui.end_row();
}

/// Difficulty for the chosen instrument, or a short summary like "G4 B2 D5".
fn difficulty_text(c: &Chart, instrument: Option<u8>) -> String {
    if let Some(i) = instrument {
        let d = c.diffs[i as usize];
        return if d >= 0 { d.to_string() } else { String::new() };
    }
    let short: HashMap<usize, &str> = [(0, "G"), (2, "R"), (3, "B"), (4, "D"), (5, "K"), (6, "GHL")].into();
    let mut parts = Vec::new();
    for k in [0, 3, 4, 5, 2, 6] {
        if c.diffs[k] >= 0 {
            parts.push(format!("{}{}", short[&k], c.diffs[k]));
        }
    }
    parts.join(" ")
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

fn apply_appearance(ctx: &egui::Context, s: &Settings) {
    ctx.set_theme(if s.dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.set_zoom_factor(s.zoom);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_messages();
        let ctx = ui.ctx().clone();

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Browse, RichText::new("Browse").size(15.0));
                ui.selectable_value(&mut self.tab, Tab::Settings, RichText::new("Settings").size(15.0));
            });
        });
        egui::Panel::bottom("bottom").show(ui, |ui| self.bottom_bar(ui));
        match self.tab {
            Tab::Browse => {
                egui::Panel::top("search").show(ui, |ui| {
                    ui.add_space(6.0);
                    self.search_bar(ui);
                    ui.add_space(2.0);
                    self.catalogue_bar(ui);
                    ui.add_space(4.0);
                });
                self.refresh_local();
                egui::CentralPanel::default_margins().show(ui, |ui| self.results(ui));
            }
            Tab::Settings => {
                egui::CentralPanel::default_margins().show(ui, |ui| self.settings_tab(ui));
            }
        }
        self.confirm_dialog(&ctx);
        self.sync_settings(&ctx);

        let building = matches!(self.cat_status, CatStatus::Building { .. });
        if self.online.searching || self.job.is_some() || building || self.track_lookup {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

fn ago(unix: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    match now.saturating_sub(unix) {
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
