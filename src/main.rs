//! Rainbow: a small desktop alternative to Bridge for Chorus Encore (enchor.us).

#![windows_subsystem = "windows"]

mod api;
mod catalogue;
mod chart;
mod rockband;
mod covers;
mod dta;
mod filter;
mod games;
mod i18n;
mod import;
mod preview;
mod settings;
mod sng;
mod stfs;
mod theme;
mod unzip;

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
use i18n::t;

/// Marks a string for translation where it's defined; it's translated where it's shown.
const fn n(s: &'static str) -> &'static str {
    s
}
use catalogue::Catalogue;
use covers::Covers;
use filter::{Filters, Index, Range, SortKey, TEXT_FIELDS, TOGGLES};
use settings::{Density, DownloadFormat, Settings};

const MAX_LOG: usize = 500;
/// Downloads bigger than this ask for confirmation and use the polite delay.
const BULK: usize = 50;
/// While the catalogue is incomplete, ask Encore directly once typing pauses this long.
const LIVE_SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);
const LOGO_PNG: &[u8] = include_bytes!("../assets/icon-256.png");
const RENZORA_PNG: &[u8] = include_bytes!("../assets/renzora.png");
const RENZORA_URL: &str = "https://github.com/renzora";

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

#[derive(Clone, Copy, PartialEq, Debug)]
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

/// Clicking a value (artist, charter, game, ...) shows every chart that shares it.
#[derive(Clone)]
enum Nav {
    /// An advanced-search text field (index into TEXT_FIELDS) and whether to match exactly.
    Text(usize, String, bool),
    Album { artist: Option<String>, album: String },
    Game(String),
    Instrument(u8),
    Difficulty(u8, i8),
    Feature(u16),
}

/// What was picked from a result row's right-click menu.
enum RowAction {
    Details(usize),
    Toggle(usize),
    Download(usize),
    Preview(usize),
    Reveal(usize),
    Nav(Nav),
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
    /// Interface languages, built in and from the user's lang folder.
    languages: Vec<i18n::Language>,
    renzora: Option<egui::TextureHandle>,
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
    show_shortcuts: bool,
    /// Blurred screenshot shown behind the chart preview, and whether one was requested.
    backdrop: Option<egui::TextureHandle>,
    backdrop_requested: bool,
    /// Measured widths of the results toolbar's two halves.
    toolbar_w: (f32, f32),
    /// Measured width of the controls beside the search box.
    search_controls_w: f32,
    /// Row to scroll into view (keyboard navigation).
    scroll_to: Option<usize>,
    /// Bumped to reset the results table's column widths.
    table_gen: u32,
    table_density: Option<Density>,
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
    /// Filters before each click-through, for the Back button.
    history: Vec<Filters>,
    detail: Option<Chart>,
    preview: Option<preview::Preview>,
    scroll_to_top: bool,
    manifest: Arc<Mutex<Manifest>>,

    job: Option<Job>,
    stop: Arc<AtomicBool>,
    unzip_running: Arc<AtomicBool>,
    unzip_last: Option<Instant>,
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
        i18n::set_language(&settings.language);
        theme::apply(ctx, settings.theme, settings.zoom, settings.access());
        theme::install_fonts(ctx, settings.font, &i18n::current());
        egui_extras::install_image_loaders(ctx);
        let logo = image::load_from_memory(LOGO_PNG).ok().map(|img| {
            let img = img.resize(96, 96, image::imageops::FilterType::Triangle).to_rgba8();
            let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            ctx.load_texture("logo", color, egui::TextureOptions::LINEAR)
        });
        let renzora = image::load_from_memory(RENZORA_PNG).ok().map(|img| {
            let img = img.to_rgba8();
            let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            ctx.load_texture("renzora", color, egui::TextureOptions::LINEAR)
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
            languages: i18n::languages(),
            renzora,
            covers: Covers::new(ctx),
            charts: Vec::new(),
            known: HashSet::new(),
            index: Index::default(),
            results: Vec::new(),
            searched: None,
            filters: Filters::default(),
            show_advanced: false,
            show_log: false,
            show_shortcuts: false,
            backdrop: None,
            backdrop_requested: false,
            toolbar_w: (760.0, 520.0),
            search_controls_w: 560.0,
            scroll_to: None,
            table_gen: 0,
            table_density: None,
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
            history: Vec::new(),
            detail: None,
            preview: None,
            scroll_to_top: false,
            job: None,
            stop: Arc::new(AtomicBool::new(false)),
            unzip_running: Arc::new(AtomicBool::new(false)),
            unzip_last: None,
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

    fn import_options(&self) -> import::Options {
        import::Options {
            unzip: self.settings.auto_extract,
            convert: self.settings.auto_convert,
            delete: self.settings.delete_zips,
            folder_template: self.settings.folder_name.clone(),
        }
    }

    /// Keyboard shortcuts. Shortcuts with Ctrl work everywhere; plain keys only when no text
    /// field has focus.
    fn keyboard(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let press = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if press(Modifiers::NONE, Key::F1) {
            self.show_shortcuts = !self.show_shortcuts;
        }
        if press(Modifiers::COMMAND, Key::F) {
            self.tab = Tab::Browse;
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("search")));
        }
        if press(Modifiers::COMMAND, Key::Num1) {
            self.tab = Tab::Browse;
        }
        if press(Modifiers::COMMAND, Key::Num2) || press(Modifiers::COMMAND, Key::Comma) {
            self.tab = Tab::Settings;
        }
        if press(Modifiers::COMMAND, Key::K) {
            self.tab = Tab::Browse;
            self.show_advanced = !self.show_advanced;
        }
        if press(Modifiers::COMMAND, Key::L) {
            self.show_log = !self.show_log;
        }
        if press(Modifiers::NONE, Key::F5) && !matches!(self.cat_status, CatStatus::Building { .. }) {
            self.update_catalogue(false);
        }
        if press(Modifiers::COMMAND, Key::D) && self.job.is_none() && !self.selected.is_empty() {
            let charts = self.selected_charts();
            self.selected.clear();
            self.request_download(tr!("{count} selected chart(s)", count = charts.len()), charts);
        }
        if press(Modifiers::COMMAND, Key::P)
            && let Some(c) = self.detail.clone()
        {
            self.preview = Some(preview::Preview::open(&self.client, &c, ctx, self.filters.instrument, self.filters.difficulty));
        }

        let typing = ctx.memory(|m| m.focused().is_some()) && ctx.egui_wants_keyboard_input();
        if typing || self.tab != Tab::Browse {
            return;
        }
        if press(Modifiers::COMMAND, Key::A) {
            self.selected = self.results.iter().map(|&i| self.charts[i].md5.clone()).collect();
        }
        if !self.show_advanced && !self.show_shortcuts && press(Modifiers::NONE, Key::Escape) {
            if self.detail.is_some() {
                self.detail = None;
            } else {
                self.selected.clear();
            }
        }
        let n = self.results.len();
        if n == 0 {
            return;
        }
        let cur = self.anchor.map(|a| a.min(n - 1));
        let page = 10;
        let mut target = None;
        let mut extend = false;
        for (shift, m) in [(false, Modifiers::NONE), (true, Modifiers::SHIFT)] {
            let moves = [
                (Key::ArrowDown, cur.map_or(0, |c| (c + 1).min(n - 1))),
                (Key::ArrowUp, cur.map_or(0, |c| c.saturating_sub(1))),
                (Key::PageDown, cur.map_or(0, |c| (c + page).min(n - 1))),
                (Key::PageUp, cur.map_or(0, |c| c.saturating_sub(page))),
                (Key::Home, 0),
                (Key::End, n - 1),
            ];
            for (key, to) in moves {
                if press(m, key) {
                    target = Some(to);
                    extend = shift;
                }
            }
        }
        if let Some(to) = target {
            if extend {
                // Shift+arrows grow the selection; the details follow the cursor.
                self.selected.insert(self.charts[self.results[to]].md5.clone());
                self.detail = Some(self.charts[self.results[to]].clone());
                self.anchor = Some(to);
            } else {
                self.handle_click(RowClick::Plain(to));
            }
            self.scroll_to = Some(to);
        }
        if let Some(c) = cur {
            if press(Modifiers::NONE, Key::Space) {
                self.handle_click(RowClick::Checkbox(c));
            }
            if press(Modifiers::NONE, Key::Enter) {
                self.detail = Some(self.charts[self.results[c]].clone());
            }
        }
    }

    /// While a pop-up (chart preview, download confirmation, shortcuts) is open, covers the app
    /// with a blurred, dimmed copy of itself. Clicking it closes the pop-up. Returns false on the frame the screenshot for the blur
    /// is taken, so the preview isn't in it.
    fn modal_backdrop(&mut self, ctx: &egui::Context) -> bool {
        let open = self.preview.is_some() || self.confirm.is_some() || self.show_shortcuts;
        if !open {
            self.backdrop = None;
            self.backdrop_requested = false;
            return false;
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = shot {
            self.backdrop = Some(ctx.load_texture("preview_backdrop", theme::blurred(&image), egui::TextureOptions::LINEAR));
        }
        if !self.backdrop_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.backdrop_requested = true;
            ctx.request_repaint();
            return false;
        }

        let screen = ctx.content_rect();
        let mut close = false;
        egui::Area::new(egui::Id::new("preview_backdrop"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
                let painter = ui.painter();
                match &self.backdrop {
                    Some(tex) => {
                        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                        painter.image(tex.id(), rect, uv, Color32::WHITE);
                        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(110));
                    }
                    None => {
                        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(170));
                    }
                }
                close = resp.on_hover_cursor(CursorIcon::Default).clicked();
            });
        if close {
            // Close the top-most pop-up.
            if self.show_shortcuts {
                self.show_shortcuts = false;
            } else if self.confirm.is_some() {
                self.confirm = None;
            } else {
                self.preview = None;
            }
            return false;
        }
        true
    }

    /// Shows a downloaded chart in the file manager (or the songs folder if it's moved).
    fn reveal_chart(&mut self, md5: &str) {
        let location = self.manifest.lock().unwrap().location(md5);
        match location {
            Some(path) => reveal(&path),
            None => {
                self.log("That chart isn't where it was saved any more; opening the songs folder".to_owned());
                open_folder(&self.settings.dir);
            }
        }
    }

    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_shortcuts;
        egui::Window::new(t("⌨  Keyboard shortcuts"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
            let ctrl = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
            let rows: Vec<(String, &str)> = vec![
                (format!("{ctrl}+F"), n("Search")),
                (format!("{ctrl}+K"), n("Advanced search")),
                ("↑ ↓  PgUp PgDn  Home End".into(), n("Move through results")),
                ("Shift + ↑ ↓".into(), n("Add rows to the selection")),
                ("Space".into(), n("Tick / untick the current row")),
                ("Enter".into(), n("Show the current row's details")),
                (format!("{ctrl}+A"), n("Select all results")),
                ("Esc".into(), n("Close details, then clear selection")),
                (format!("{ctrl}+D"), n("Download selected")),
                (format!("{ctrl}+P"), n("Preview the chart in details")),
                (format!("{ctrl}+1 / {ctrl}+2"), n("Browse / Settings")),
                (format!("{ctrl}+L"), n("Show or hide the log")),
                ("F5".into(), n("Check for new charts")),
                ("F1".into(), n("This list")),
            ];
            egui::Grid::new("shortcuts").num_columns(2).spacing([24.0, 8.0]).show(ui, |ui| {
                for (keys, what) in rows {
                    egui::Frame::new()
                        .fill(ui.visuals().widgets.inactive.bg_fill)
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(8, 3))
                        .show(ui, |ui| ui.label(RichText::new(keys).monospace().strong()));
                    ui.label(RichText::new(t(what)).size(14.5));
                    ui.end_row();
                }
            });
        });
        self.show_shortcuts = open;
    }

    /// Something to pass to background imports for progress messages.
    fn reporter(&self) -> impl Fn(String) + Send + 'static {
        let tx = self.tx.clone();
        move |line: String| {
            let _ = tx.send(Msg::Status(line));
        }
    }

    /// Checks the songs folder for .zip files and Rock Band CON packages every few seconds
    /// (if enabled), or right away when forced.
    fn auto_unzip(&mut self, force: bool) {
        let enabled = self.settings.auto_extract || self.settings.auto_convert;
        let due = self.unzip_last.is_none_or(|t| t.elapsed() >= Duration::from_secs(10));
        if !(force || enabled && due) || self.unzip_running.swap(true, Ordering::SeqCst) {
            return;
        }
        self.unzip_last = Some(Instant::now());
        let mut opts = self.import_options();
        if force {
            opts.unzip = true;
            opts.convert = true;
        }
        let dir = self.settings.dir.clone();
        let (report, running) = (self.reporter(), self.unzip_running.clone());
        thread::spawn(move || {
            import::process_folder(&dir, &opts, &report);
            running.store(false, Ordering::SeqCst);
        });
    }

    /// Files dropped onto the window: convert CON files, import zips.
    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if dropped.is_empty() {
            return;
        }
        let (opts, dir, report) = (self.import_options(), self.settings.dir.clone(), self.reporter());
        thread::spawn(move || import::import_dropped(dropped, &dir, &opts, &report));
    }

    /// A hint over the whole window while files are dragged onto it.
    fn drop_overlay(&self, ctx: &egui::Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop_overlay")));
        let screen = ctx.content_rect();
        painter.rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(190));
        let inner = screen.shrink(24.0);
        painter.rect_stroke(inner, CornerRadius::same(18), egui::Stroke::new(3.0, theme::accent_hover()), egui::StrokeKind::Inside);
        painter.text(inner.center() - vec2(0.0, 18.0), egui::Align2::CENTER_CENTER, t("Drop to import"), egui::FontId::proportional(30.0), Color32::WHITE);
        painter.text(
            inner.center() + vec2(0.0, 20.0),
            egui::Align2::CENTER_CENTER,
            t("Rock Band CON files are converted for Clone Hero · .zip files are extracted"),
            egui::FontId::proportional(16.0),
            Color32::from_gray(210),
        );
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
        ui.horizontal(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(36.0, 36.0)).corner_radius(CornerRadius::same(9)));
            }
            ui.label(theme::title(24.0));
            ui.add_space(18.0);
            for (tab, label) in [(Tab::Browse, n("🔍  Browse")), (Tab::Settings, n("⚙  Settings"))] {
                let on = self.tab == tab;
                let text = RichText::new(t(label)).size(15.0);
                let button = if on {
                    egui::Button::new(text.color(theme::on_accent()).strong()).fill(theme::accent())
                } else {
                    egui::Button::new(text).frame(false)
                };
                if ui.add(button.corner_radius(CornerRadius::same(16)).min_size(vec2(110.0, 32.0))).clicked() {
                    self.tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                credit(ui, self.renzora.as_ref(), 26.0);
                ui.add_space(12.0);
                if let Some(job) = &self.job {
                    ui.add(egui::ProgressBar::new(job.checked as f32 / job.total.max(1) as f32).desired_width(140.0).show_percentage());
                    ui.label(RichText::new(format!("⬇ {}", job.downloaded)).color(theme::good()));
                }
            });
        });
        
    }

    // ───────────────────────────── Browse tab ─────────────────────────────

    fn search_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.filters.query)
                    .id(egui::Id::new("search"))
                    .hint_text(t("🔍  Search songs, artists, albums, charters…  (Ctrl+F)"))
                    .font(egui::FontId::proportional(16.0))
                    .margin(egui::Margin::symmetric(10, 7))
                    .desired_width((ui.available_width() - self.search_controls_w - 16.0).max(200.0)),
            );
            // Measure the controls after the search box, so the box leaves exactly enough room.
            let controls_start = ui.cursor().left();
            if text_menu(&resp, &mut self.filters.query) || resp.changed() {
                self.live_edited = Instant::now();
            }

            let inst_text = self.filters.instrument.map_or(t("🎵  Any Instrument"), |i| {
                format!("{}  {}", instrument_icon(i), t(INSTRUMENTS[i as usize].1))
            });
            egui::ComboBox::from_id_salt("instrument").selected_text(inst_text).width(180.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.filters.instrument, None, t("🎵  Any Instrument"));
                for (k, (_, name, _)) in INSTRUMENTS.iter().enumerate() {
                    let label = format!("{}  {}", instrument_icon(k as u8), t(name));
                    ui.selectable_value(&mut self.filters.instrument, Some(k as u8), label);
                }
            });
            let diff_text = self.filters.difficulty.map_or(t("Any Difficulty"), |d| t(DIFFICULTIES[d as usize].1));
            egui::ComboBox::from_id_salt("difficulty").selected_text(diff_text).width(120.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.filters.difficulty, None, t("Any Difficulty"));
                for (k, (_, name)) in DIFFICULTIES.iter().enumerate() {
                    ui.selectable_value(&mut self.filters.difficulty, Some(k as u8), t(name));
                }
            });
            if self.filters.instrument == Some(DRUMS) {
                let dt = self.filters.drum_type.map_or(t("Any Drum Type"), |d| t(DRUM_TYPES[d as usize]));
                egui::ComboBox::from_id_salt("drumtype").selected_text(dt).width(110.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filters.drum_type, None, t("Any Drum Type"));
                    for (k, name) in DRUM_TYPES.iter().enumerate() {
                        ui.selectable_value(&mut self.filters.drum_type, Some(k as u8), t(name));
                    }
                });
            }

            let n = self.filters.advanced_count();
            let label = format!(
                "{}{} {}",
                t("🎛  Advanced search"),
                if n > 0 { format!(" ({n})") } else { String::new() },
                if self.show_advanced { "⏶" } else { "⏷" }
            );
            if pill(ui, self.show_advanced || n > 0, &label).clicked() {
                self.show_advanced = !self.show_advanced;
            }
            self.search_controls_w = ui.min_rect().right() - controls_start;
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let f = &mut self.filters;
            let all = !f.no_video && !f.no_broken && !f.legacy && f.game.is_none() && f.advanced_count() == 0;
            if pill(ui, all, &t("🌈  All songs")).on_hover_text(t("Clear all filters")).clicked() {
                f.no_video = false;
                f.no_broken = false;
                f.legacy = false;
                f.game = None;
                f.clear_advanced();
            }
            if pill(ui, f.no_video, &t("🎬  No videos")).on_hover_text(t("Hide charts that have a video background")).clicked() {
                f.no_video = !f.no_video;
            }
            if pill(ui, f.no_broken, &t("🛠  No broken songs"))
                .on_hover_text(t("Hide charts Encore flags as unplayable: missing or unreadable chart/audio, or no notes"))
                .clicked()
            {
                f.no_broken = !f.no_broken;
            }
            ui.separator();
            if pill(ui, f.legacy || f.game.is_some(), &t("🏆  Legacy"))
                .on_hover_text(t("Only the original charts from the official Guitar Hero and Rock Band games and their DLC"))
                .clicked()
            {
                f.legacy = !(f.legacy || f.game.is_some());
                f.game = None;
            }
            let label = f.game.as_deref().map_or(t("🎮  Any game"), |g| format!("🎮  {}", games::game_name(g)));
            let index = &self.index;
            egui::ComboBox::from_id_salt("game").selected_text(label).width(240.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut f.game, None, t("🎮  Any game"));
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
        let width = screen.width();
        let max_height = (screen.bottom() - self.search_bottom - 70.0).max(150.0);
        egui::Area::new(egui::Id::new("advanced_search"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(screen.left(), self.search_bottom))
            .show(ctx, |ui| {
                // Full width, flush under the search bar, like a drop-down sheet.
                let frame = egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .shadow(ui.visuals().popup_shadow)
                    .corner_radius(CornerRadius { nw: 0, ne: 0, sw: 12, se: 12 })
                    .inner_margin(egui::Margin::symmetric(16, 12));
                frame.show(ui, |ui| {
                    ui.set_width(width - 32.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(t("🎛  Advanced search")).size(17.0).strong().color(theme::accent_hover()));
                        ui.weak(tr!("{count} results", count = self.results.len()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(t("✖  Close")).on_hover_text(t("Esc")).clicked() {
                                self.show_advanced = false;
                            }
                            if ui.button(t("↺  Reset")).on_hover_text(t("Clear all advanced filters")).clicked() {
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
            Section::Text => n("🔤  Search by"),
            Section::Ranges => n("📏  Ranges"),
            Section::Chart => n("🔑  Chart"),
            Section::Features => n("✨  Features"),
        };
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(inner);
                ui.vertical(|ui| {
                    ui.label(RichText::new(t(title)).strong().color(theme::accent_hover()));
                    ui.add_space(6.0);
                    match section {
                        Section::Text => {
                            let field_w = inner - 2.0 * 52.0 - 16.0;
                            egui::Grid::new("adv_text").num_columns(3).spacing([8.0, 5.0]).show(ui, |ui| {
                                ui.label("");
                                ui.small(t("Exact")).on_hover_text(t("Only include results that match perfectly. (not case sensitive)"));
                                ui.small(t("Exclude")).on_hover_text(t("Do not include results that match this."));
                                ui.end_row();
                                for (k, label) in TEXT_FIELDS.iter().enumerate() {
                                    let resp = ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut f.text[k].value).hint_text(t(label)));
                                    text_menu(&resp, &mut f.text[k].value);
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
                                    (n("⏱  Length (min)"), &mut f.length_min, ""),
                                    (n("🔥  Intensity"), &mut f.intensity, n("Also known as chart difficulty. Typically a number between 0 and 6.")),
                                    (n("⚡  Average NPS"), &mut f.avg_nps, n("Average notes per second, for the chosen instrument/difficulty.")),
                                    (n("🚀  Max NPS"), &mut f.max_nps, n("Highest notes per second, for the chosen instrument/difficulty.")),
                                    (n("📅  Year"), &mut f.year, ""),
                                ] {
                                    range_row(ui, label, range, tip, label_w);
                                }
                            });
                        }
                        Section::Chart => {
                            let field_w = inner - 110.0;
                            egui::Grid::new("adv_misc").num_columns(2).spacing([8.0, 5.0]).show(ui, |ui| {
                                ui.label(t("Modified after")).on_hover_text(t("The date of the last time this chart was modified in Google Drive."));
                                ui.horizontal(|ui| {
                                    // Type a date, or pick one from the calendar popup.
                                    let clear_w = if f.modified_after.is_empty() { 0.0 } else { 30.0 };
                                    let text_w = (field_w - 48.0 - clear_w).max(80.0);
                                    let resp = ui.add_sized([text_w, 24.0], egui::TextEdit::singleline(&mut f.modified_after).hint_text(t("YYYY-MM-DD")));
                                    text_menu(&resp, &mut f.modified_after);
                                    let mut date = f.modified_after.trim().parse::<jiff::civil::Date>().unwrap_or_else(|_| jiff::Zoned::now().date());
                                    let picker = egui_extras::DatePickerButton::new(&mut date)
                                        .id_salt("modified_after")
                                        .format("")
                                        .show_icon(true)
                                        .calendar_week(false);
                                    if ui.add(picker).on_hover_text(t("Pick a date")).changed() {
                                        f.modified_after = date.to_string();
                                    }
                                    if !f.modified_after.is_empty() && ui.small_button("✖").on_hover_text(t("Clear the date")).clicked() {
                                        f.modified_after.clear();
                                    }
                                });
                                ui.end_row();
                                ui.label(t("Hash")).on_hover_text(t("The MD5 hash of the chart folder or .sng file. You can enter multiple values if they are separated by commas."));
                                let resp = ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut f.hash).hint_text(t("md5, md5, …")));
                                text_menu(&resp, &mut f.hash);
                                ui.end_row();
                                ui.label(t("Track hash")).on_hover_text(t("The hash of only things that impact scoring on a specific track. Multiple values can be separated by commas. Press Enter to look it up on Encore's server."));
                                let resp = ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut f.track_hash).hint_text(t("press Enter to look up")));
                                if text_menu(&resp, &mut f.track_hash) {
                                    f.track_md5s = None;
                                }
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
                                    ui.small(t("Looking up track hash…"));
                                });
                            } else if let Some(md5s) = &f.track_md5s {
                                ui.small(tr!("Track hash matches {count} chart(s)", count = md5s.len()));
                            }
                        }
                        Section::Features => {
                            ui.small(t("Click to cycle: any → must have → must not have"));
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                                for (k, (label, _, applies)) in TOGGLES.iter().enumerate() {
                                    let state = &mut f.toggles[k];
                                    let (icon, color, fill) = match state {
                                        None => ("○", ui.visuals().weak_text_color(), ui.visuals().widgets.inactive.bg_fill),
                                        Some(true) => ("✔", theme::good(), theme::good().linear_multiply(0.18)),
                                        Some(false) => ("✖", theme::bad(), theme::bad().linear_multiply(0.18)),
                                    };
                                    let button = egui::Button::new(RichText::new(format!("{icon} {}", t(label))).color(color))
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
                        ui.label(tr!("Checking for new charts… {count} charts", count = charts));
                    } else {
                        if *pages > 0 {
                            ui.add(
                                egui::ProgressBar::new(*page as f32 / (*pages).max(1) as f32)
                                    .desired_width(140.0)
                                    .text(format!("{page}/{pages}")),
                            );
                        }
                        ui.label(tr!("Downloading catalogue: {count} charts", count = charts))
                            .on_hover_text(t("Already searchable: results fill in as it downloads."));
                    }
                    if ui.small_button("⏸").on_hover_text(t("Pause")).clicked() {
                        self.cat_stop.store(true, Ordering::Relaxed);
                    }
                }
                CatStatus::Failed(e) => {
                    ui.colored_label(theme::bad(), t("⚠ Catalogue update failed")).on_hover_text(e.clone());
                    if ui.small_button(t("Retry")).clicked() {
                        self.update_catalogue(false);
                    }
                }
                CatStatus::Idle if self.cat_complete => {
                    ui.label(tr!("📚 {count} charts", count = self.charts.len()));
                    ui.weak(tr!("updated {when}", when = ago(self.cat_updated)));
                    if ui.small_button("⟳").on_hover_text(t("Check Encore for new charts")).clicked() {
                        self.update_catalogue(false);
                    }
                }
                CatStatus::Idle => {
                    ui.label(tr!("Catalogue paused at {count} charts", count = self.charts.len()));
                    if ui.small_button("▶").on_hover_text(t("Resume")).clicked() {
                        self.update_catalogue(false);
                    }
                }
            }
            if let Some((s, at)) = &self.status
                && at.elapsed() < Duration::from_secs(60)
            {
                ui.separator();
                ui.colored_label(theme::warn(), format!("⏳ {s}"));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).monospace().weak())
                    .on_hover_text(t("Rainbow version"));
                ui.separator();
                web_link(
                    ui,
                    Some(egui::include_image!("../assets/icons/github.svg")),
                    "GitHub",
                    "https://github.com/renzora/rainbow",
                    n("Rainbow's source code, releases and issue tracker"),
                );
                web_link(
                    ui,
                    Some(egui::include_image!("../assets/icons/patreon.svg")),
                    "Patreon",
                    "https://www.patreon.com/cw/ChorusEncore701?l=en-GB",
                    n("Support Chorus Encore: it's donation-funded and pays for the servers and file hosting Rainbow uses"),
                );
                web_link(
                    ui,
                    Some(egui::include_image!("../assets/icons/discord.svg")),
                    "Discord",
                    "https://discord.gg/cqaUXGm",
                    n("Join the Chorus Encore Discord community"),
                );
                web_link(ui, None, "🌐 Encore", "https://www.enchor.us", n("Chorus Encore, the chart search engine Rainbow is built on"));
                ui.separator();
                self.language_picker(ui);
                ui.separator();
                if ui
                    .selectable_label(self.show_log, tr!("📜 Log ({count})", count = self.log.len()))
                    .on_hover_text(t("Show activity and errors"))
                    .clicked()
                {
                    self.show_log = !self.show_log;
                }
            });
        });
    }

    /// The interface language picker in the status bar. Each language is shown in itself.
    fn language_picker(&mut self, ui: &mut egui::Ui) {
        let languages = &self.languages;
        let current = if self.settings.language == "auto" {
            tr!("🌐 Auto ({language})", language = languages.iter().find(|l| l.code == i18n::system_language()).map_or("English", |l| l.name.as_str()))
        } else {
            format!("🌐 {}", languages.iter().find(|l| l.code == self.settings.language).map_or("English", |l| l.name.as_str()))
        };
        let mut picked = None;
        egui::ComboBox::from_id_salt("language").selected_text(current).height(420.0).show_ui(ui, |ui| {
            ui.set_min_width(220.0);
            if ui.selectable_label(self.settings.language == "auto", t("Auto (system language)")).clicked() {
                picked = Some("auto".to_owned());
            }
            ui.separator();
            for lang in languages {
                let label = if lang.name == lang.english_name { lang.name.clone() } else { format!("{}  ·  {}", lang.name, lang.english_name) };
                if ui.selectable_label(self.settings.language == lang.code, label).clicked() {
                    picked = Some(lang.code.clone());
                }
            }
        })
        .response
        .on_hover_text(t("Language"));
        if let Some(code) = picked {
            self.settings.language = code;
        }
    }

    /// Download progress, shown above the status bar while a download runs.
    fn download_bar(&mut self, ui: &mut egui::Ui) {
        let Some(job) = &self.job else {
            return;
        };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong(format!("⬇ {}", job.title));
            ui.label(tr!("{done} / {total} checked", done = job.checked, total = job.total));
            ui.colored_label(theme::good(), tr!("✔ {count} downloaded", count = job.downloaded));
            ui.weak(tr!("{count} already had", count = job.checked - job.downloaded - job.failed));
            if job.failed > 0 {
                ui.colored_label(theme::bad(), tr!("✖ {count} failed", count = job.failed));
            }
            if ui.small_button(t("⏹ Stop")).clicked() {
                self.stop.store(true, Ordering::Relaxed);
            }
        });
        ui.add(egui::ProgressBar::new(job.checked as f32 / job.total.max(1) as f32).show_percentage().fill(theme::accent()));
        if let Some((title, done, total)) = &job.file {
            let frac = total.map(|t| *done as f32 / t.max(1) as f32).unwrap_or(0.0);
            ui.add(egui::ProgressBar::new(frac).text(format!("{title}  ({:.1} MB)", *done as f64 / 1_048_576.0)));
        }
        ui.add_space(4.0);
    }

    /// The log slides up above the status bar; drag its top edge to resize it.
    fn log_panel(&mut self, ui: &mut egui::Ui) {
        let log = &self.log;
        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(220.0)
            .min_size(80.0)
            .max_size(600.0)
            .drag_to_open(false)
            .show_collapsible(ui, &mut self.show_log, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr!("📜 Log ({count})", count = log.len())).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(t("Copy all")).on_hover_text(t("Copy the whole log")).clicked() {
                            ui.ctx().copy_text(log.join("\n"));
                        }
                    });
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    for line in log {
                        ui.label(RichText::new(line).monospace());
                    }
                });
            });
    }

    /// The columns to show at this table width: the ones enabled in Settings, minus the least
    /// important ones while they don't fit. Name then stretches to fill the rest.
    fn columns(&self, avail: f32) -> Vec<Col> {
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

        let width = |cols: &[Col]| cols.iter().map(|&col| self.column_width(col) + 8.0).sum::<f32>();
        for drop in [Col::Uploaded, Col::Year, Col::Genre, Col::Tags, Col::Length, Col::Album, Col::Charter, Col::Difficulty] {
            if width(&cols) <= avail {
                break;
            }
            cols.retain(|&c| c != drop);
        }
        cols
    }

    /// Starting width of each column (Name's is its minimum; it grows to fill).
    fn column_width(&self, col: Col) -> f32 {
        match col {
            Col::Check => 36.0,
            Col::Cover => self.settings.cover_size + 16.0,
            Col::Name => 200.0,
            Col::Artist => 160.0,
            Col::Album => 160.0,
            Col::Genre => 115.0,
            Col::Year => 52.0,
            Col::Charter => 120.0,
            Col::Length => 60.0,
            Col::Difficulty => 105.0,
            Col::Uploaded => 90.0,
            Col::Tags => 120.0,
        }
    }

    /// Shows every chart sharing the clicked value. The previous search can be restored with Back.
    fn navigate(&mut self, nav: Nav) {
        self.history.push(self.filters.clone());
        let mut f = Filters { sort: self.filters.sort, descending: self.filters.descending, ..Default::default() };
        match nav {
            Nav::Text(k, value, exact) => f.text[k] = filter::TextFilter { value, exact, exclude: false },
            Nav::Album { artist, album } => {
                f.text[2] = filter::TextFilter { value: album, exact: true, exclude: false };
                if let Some(artist) = artist {
                    f.text[1] = filter::TextFilter { value: artist, exact: true, exclude: false };
                }
            }
            Nav::Game(g) => f.game = Some(g),
            Nav::Instrument(i) => f.instrument = Some(i),
            Nav::Difficulty(i, d) => {
                f.instrument = Some(i);
                f.intensity = Range { min: d.to_string(), max: d.to_string() };
            }
            Nav::Feature(bit) => {
                if let Some(k) = TOGGLES.iter().position(|(_, b, _)| *b == bit) {
                    f.toggles[k] = Some(true);
                }
            }
        }
        self.filters = f;
        self.show_advanced = false;
    }

    /// Removable chips for the active filters, plus Back after a click-through.
    fn active_filters(&mut self, ui: &mut egui::Ui) {
        let f = &self.filters;
        let mut chips: Vec<(String, Box<dyn Fn(&mut Filters)>)> = Vec::new();
        for (k, field) in f.text.iter().enumerate() {
            if !field.value.trim().is_empty() {
                let (name, value) = (t(TEXT_FIELDS[k]), field.value.trim());
                let text = if field.exclude {
                    tr!("{field} is not \"{value}\"", field = name, value = value)
                } else if field.exact {
                    tr!("{field} is \"{value}\"", field = name, value = value)
                } else {
                    tr!("{field} contains \"{value}\"", field = name, value = value)
                };
                chips.push((text, Box::new(move |f| f.text[k] = Default::default())));
            }
        }
        if let Some(i) = f.instrument {
            chips.push((format!("{} {}", instrument_icon(i), t(INSTRUMENTS[i as usize].1)), Box::new(|f| f.instrument = None)));
        }
        if let Some(d) = f.difficulty {
            chips.push((t(DIFFICULTIES[d as usize].1), Box::new(|f| f.difficulty = None)));
        }
        if let Some(g) = &f.game {
            chips.push((format!("🏆 {}", games::game_name(g)), Box::new(|f| f.game = None)));
        } else if f.legacy {
            chips.push((t("🏆  Legacy"), Box::new(|f| f.legacy = false)));
        }
        for (label, range, clear) in [
            (n("Length"), &f.length_min, (|f: &mut Filters| f.length_min = Range::default()) as fn(&mut Filters)),
            (n("Intensity"), &f.intensity, |f: &mut Filters| f.intensity = Range::default()),
            (n("Avg NPS"), &f.avg_nps, |f: &mut Filters| f.avg_nps = Range::default()),
            (n("Max NPS"), &f.max_nps, |f: &mut Filters| f.max_nps = Range::default()),
            (n("Year"), &f.year, |f: &mut Filters| f.year = Range::default()),
        ] {
            let (min, max) = (range.min.trim(), range.max.trim());
            let label = t(label);
            let text = match (min.is_empty(), max.is_empty()) {
                (true, true) => continue,
                _ if min == max => format!("{label} {min}"),
                (false, true) => format!("{label} ≥ {min}"),
                (true, false) => format!("{label} ≤ {max}"),
                (false, false) => format!("{label} {min}–{max}"),
            };
            chips.push((text, Box::new(clear)));
        }
        for (k, (label, _, _)) in TOGGLES.iter().enumerate() {
            if let Some(want) = f.toggles[k] {
                let text = if want { format!("✔ {}", t(label)) } else { tr!("✖ Not {feature}", feature = t(label)) };
                chips.push((text, Box::new(move |f| f.toggles[k] = None)));
            }
        }
        if !f.modified_after.trim().is_empty() {
            chips.push((tr!("Modified after {date}", date = f.modified_after.trim()), Box::new(|f| f.modified_after.clear())));
        }
        if !f.hash.trim().is_empty() || !f.track_hash.trim().is_empty() {
            chips.push((t("Hash"), Box::new(|f| {
                f.hash.clear();
                f.track_hash.clear();
                f.track_md5s = None;
            })));
        }
        // Back only makes sense while it leads somewhere different: once every filter is gone
        // (the full list again), there's nothing to go back to.
        if chips.is_empty() && self.filters.query.trim().is_empty() {
            self.history.clear();
        }
        while self.history.last() == Some(&self.filters) {
            self.history.pop();
        }
        if chips.is_empty() && self.history.is_empty() {
            return;
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if !self.history.is_empty() && ui.button(t("← Back")).on_hover_text(t("Go back to the previous search")).clicked() {
                self.filters = self.history.pop().unwrap_or_default();
                return;
            }
            let mut remove = None;
            for (k, (text, _)) in chips.iter().enumerate() {
                if theme::filter_chip(ui, text, theme::accent_hover(), &t("Remove this filter")) {
                    remove = Some(k);
                }
            }
            if let Some(k) = remove {
                (chips[k].1)(&mut self.filters);
            }
            if chips.len() > 1 && ui.small_button(t("Clear all")).clicked() {
                let keep = (self.filters.sort, self.filters.descending);
                self.filters = Filters { sort: keep.0, descending: keep.1, ..Default::default() };
            }
        });
    }

    fn toolbar_left(&mut self, ui: &mut egui::Ui, wide: bool) {
        let count = self.results.len();
        ui.label(RichText::new(tr!("{count} results", count = count)).strong());
        if !self.cat_complete {
            if self.live_searching {
                ui.spinner();
            }
            let searching = match self.cat_total {
                Some(total) => tr!("(searching {count} of ~{total} charts so far)", count = self.charts.len(), total = total),
                None => tr!("(searching {count} charts so far)", count = self.charts.len()),
            };
            ui.weak(searching);
        }
        ui.separator();
        let tip = t("Tip: Shift+click selects a range, Ctrl+click adds or removes one");
        if ui.button(t("☑ Select all")).on_hover_text(tip).clicked() {
            self.selected = self.results.iter().map(|&i| self.charts[i].md5.clone()).collect();
        }
        if ui.button(t("☐ Clear")).clicked() {
            self.selected.clear();
        }
        if wide {
            ui.weak(t("Shift/Ctrl+click to select several"));
        }
        ui.separator();
        for (d, icon, tip) in [(Density::Compact, "☰", n("Compact rows")), (Density::Default, "▤", n("Default rows")), (Density::Large, "▦", n("Large rows with bigger covers"))] {
            if ui.selectable_label(self.settings.density == d, icon).on_hover_text(t(tip)).clicked() {
                self.settings.density = d;
                self.settings.cover_size = d.cover_size();
            }
        }
    }

    /// Download buttons, laid out right to left.
    fn toolbar_right(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        let count = self.results.len();
        let resp = ui
            .add_enabled(!busy && self.cat_complete, egui::Button::new(t("⬇ Entire library")))
            .on_hover_text(t("Download every chart on Encore"))
            .on_disabled_hover_text(t("Available once the catalogue has finished downloading"));
        if resp.clicked() {
            let charts = self.charts.clone();
            self.request_download(t("Entire Encore library"), charts);
        }
        let resp = ui.add_enabled(!busy && count > 0, egui::Button::new(tr!("⬇ All {count} results", count = count)));
        if resp.clicked() {
            let charts: Vec<Chart> = self.results.iter().map(|&i| self.charts[i].clone()).collect();
            self.request_download(t("All current results"), charts);
        }
        let n = self.selected.len();
        if ui.add_enabled(!busy && n > 0, theme::download_button(tr!("⬇ Download selected ({count})", count = n))).clicked() {
            let charts = self.selected_charts();
            self.selected.clear();
            self.request_download(tr!("{count} selected chart(s)", count = charts.len()), charts);
        }
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let count = self.results.len();
        // The visible results area, for centring the floating "Back to top" button.
        let visible = ui.clip_rect().intersect(ui.max_rect());
        // One row if both halves fit (measured last frame, so it adapts to font and zoom),
        // otherwise the download buttons move to a second row.
        let width = ui.available_width();
        let (left_w, right_w) = self.toolbar_w;
        if width >= left_w + right_w + 24.0 {
            ui.horizontal(|ui| {
                let left = ui.scope(|ui| self.toolbar_left(ui, true)).response.rect.width();
                let right = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.toolbar_right(ui)).response.rect.width();
                self.toolbar_w = (left, right.min(width));
            });
        } else {
            ui.horizontal(|ui| self.toolbar_left(ui, false));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let right = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let start = ui.cursor().right();
                    self.toolbar_right(ui);
                    start - ui.min_rect().left()
                });
                self.toolbar_w.1 = right.inner;
            });
        }
        ui.add_space(4.0);

        // The table, with a strip on the right for the custom scroll bar.
        let full = ui.available_rect_before_wrap();
        let table_rect = full.with_max_x(full.right() - theme::SCROLLBAR_W - 6.0);
        let mut table_ui = ui.new_child(egui::UiBuilder::new().max_rect(table_rect));
        // Keep rows from spilling under the scroll bar.
        table_ui.set_clip_rect(table_rect.intersect(ui.clip_rect()));
        table_ui.style_mut().interaction.selectable_labels = false;
        // egui_extras draws the column resize bars with these strokes; make them easier to see and grab.
        let w = &mut table_ui.visuals_mut().widgets;
        w.noninteractive.bg_stroke.width = 3.0;
        w.hovered.bg_stroke.width = 4.0;
        w.active.bg_stroke.width = 4.0;
        table_ui.style_mut().interaction.resize_grab_radius_side = 7.0;
        // A background strip behind the column titles, setting them apart from the toolbar above.
        let header_bg = egui::Rect::from_min_size(table_rect.min - vec2(0.0, 2.0), vec2(table_rect.width(), 30.0));
        table_ui.painter().rect_filled(header_bg, CornerRadius::same(6), theme::palette().raised);
        let table_width = table_rect.width();
        let cols = self.columns(table_width);
        let widths: Vec<f32> = cols.iter().map(|&c| self.column_width(c)).collect();
        let manifest = self.manifest.lock().unwrap();
        let mut click = None;
        let mut nav: Option<Nav> = None;
        let mut row_menu: Option<RowAction> = None;
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

        if self.table_density != Some(self.settings.density) {
            self.table_density = Some(self.settings.density);
            self.table_gen += 1;
        }
        let name_size = match self.settings.density {
            Density::Compact => 13.5,
            Density::Default => 14.5,
            Density::Large => 17.0,
        };
        let mut table = TableBuilder::new(&mut table_ui)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            // A new id when the layout preset changes, so remembered column widths reset.
            // Separate remembered widths per layout and per set of visible columns.
            .id_salt(format!("results-{}-{:?}", self.table_gen, cols))
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        if let Some(row) = self.scroll_to.take() {
            table = table.scroll_to_row(row, None);
        }
        if std::mem::take(&mut self.scroll_to_top) {
            table = table.scroll_to_row(0, None);
        }
        // Name starts wide enough to fill the window. The last column takes up whatever is left
        // and has no resize handle: a filler column further left would move the columns being
        // dragged, and a handle at the window edge would fight with resizing the window.
        let others: f32 = cols.iter().zip(&widths).filter(|(c, _)| **c != Col::Name).map(|(_, w)| w + 8.0).sum();
        let name_w = (table_width - others - 8.0).max(widths[cols.iter().position(|c| *c == Col::Name).unwrap_or(0)]);
        let last = cols.len() - 1;
        for (k, (&col, &w)) in cols.iter().zip(&widths).enumerate() {
            table = table.column(match col {
                Col::Check => Column::exact(w),
                _ if k == last => Column::remainder().at_least(w.min(120.0)).clip(true),
                Col::Cover => Column::initial(w).range(32.0..=250.0),
                Col::Name => Column::initial(name_w).at_least(140.0).clip(true),
                _ => Column::initial(w).at_least(40.0).clip(true),
            });
        }
        let scroll = table
            .header(26.0, |mut h| {
                for col in &cols {
                    if let Col::Check = col {
                        h.col(|ui| {
                            ui.add_space(9.0);
                            let mut all = all_selected;
                            let tip = if all_selected { t("Deselect all results") } else { t("Select all results") };
                            if theme::checkbox_ex(ui, &mut all, "", any_selected && !all_selected).on_hover_text(tip).clicked() {
                                toggle_all = true;
                            }
                        });
                        continue;
                    }
                    let (title, key) = match col {
                        Col::Check | Col::Cover => ("", None),
                        Col::Tags => (n("Tags"), None),
                        Col::Name => (n("Name"), Some(SortKey::Name)),
                        Col::Artist => (n("Artist"), Some(SortKey::Artist)),
                        Col::Album => (n("Album"), Some(SortKey::Album)),
                        Col::Genre => (n("Genre"), Some(SortKey::Genre)),
                        Col::Year => (n("Year"), Some(SortKey::Year)),
                        Col::Charter => (n("Charter"), Some(SortKey::Charter)),
                        Col::Length => (n("Length"), Some(SortKey::Length)),
                        Col::Difficulty => (n("Difficulty"), Some(SortKey::Difficulty)),
                        Col::Uploaded => (n("Uploaded"), Some(SortKey::Uploaded)),
                    };
                    h.col(|ui| {
                        ui.add_space(8.0);
                        let Some(key) = key else {
                            ui.strong(t(title));
                            return;
                        };
                        let active = sort == key;
                        let arrow = if active { if desc { " ⏷" } else { " ⏶" } } else { "" };
                        let mut text = RichText::new(format!("{}{arrow}", t(title))).strong();
                        if active {
                            text = text.color(theme::accent_hover());
                        }
                        if ui.add(egui::Button::new(text).frame(false)).on_hover_text(t("Sort")).clicked() {
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
                        row.col(|ui| {
                            // Breathing room inside each cell; the checkbox is centred.
                            ui.add_space(if let Col::Check = col { 9.0 } else { 8.0 });
                            match col {
                            Col::Check => {
                                let mut checked = is_checked;
                                if theme::checkbox(ui, &mut checked, "").changed() {
                                    click = Some(RowClick::Checkbox(i));
                                }
                            }
                            Col::Cover => {
                                cover_width.get_or_insert(ui.max_rect().width());
                                cover(ui, covers, c.art.as_deref(), cover_px, covers::THUMB);
                            }
                            Col::Name => {
                                ui.label(RichText::new(text(&c.name)).strong().size(name_size));
                            }
                            Col::Artist => {
                                if let Some(a) = &c.artist
                                    && link(ui, RichText::new(a), n("Show all songs by this artist"))
                                {
                                    nav = Some(Nav::Text(1, a.clone(), true));
                                }
                            }
                            Col::Album => {
                                if let Some(a) = &c.album
                                    && link(ui, RichText::new(a).weak(), n("Show this album"))
                                {
                                    nav = Some(Nav::Album { artist: c.artist.clone(), album: a.clone() });
                                }
                            }
                            Col::Genre => {
                                if let Some(g) = &c.genre
                                    && theme::chip(ui, g, tag_color(g)).on_hover_text(t("Show this genre")).clicked()
                                {
                                    nav = Some(Nav::Text(3, g.clone(), true));
                                }
                            }
                            Col::Year => {
                                if let Some(y) = &c.year
                                    && link(ui, RichText::new(y).weak(), n("Show songs from this year"))
                                {
                                    nav = Some(Nav::Text(4, y.clone(), true));
                                }
                            }
                            Col::Charter => {
                                if let Some(ch) = &c.charter
                                    && link(ui, RichText::new(ch), n("Show everything charted by this charter"))
                                {
                                    nav = Some(Nav::Text(5, ch.clone(), false));
                                }
                            }
                            Col::Length => {
                                ui.weak(c.length_text());
                            }
                            Col::Difficulty => difficulty_cell(ui, c, instrument, &mut nav),
                            Col::Uploaded => {
                                ui.weak(c.uploaded());
                            }
                            Col::Tags => tags(ui, c, manifest.contains(&c.md5)),
                            }
                        });
                    }
                    let resp = row.response();
                    let have = manifest.contains(&c.md5);
                    resp.context_menu(|ui| row_context_menu(ui, c, i, is_checked, have, &mut row_menu));
                    // Screen readers announce the row as the song.
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, is_checked, c.title()));
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

        let bar = egui::Rect::from_min_max(
            egui::pos2(full.right() - theme::SCROLLBAR_W, scroll.inner_rect.top()),
            egui::pos2(full.right(), scroll.inner_rect.bottom()),
        );
        // A solid strip behind the scroll bar, from the table's edge to the panel's.
        let strip = egui::Rect::from_min_max(egui::pos2(table_rect.right(), bar.top()), egui::pos2(full.right(), bar.bottom()));
        ui.painter().rect_filled(strip, CornerRadius::ZERO, ui.visuals().panel_fill);
        let view = scroll.inner_rect.height();
        if let Some(offset) = theme::scrollbar(ui, bar, scroll.id.with("bar"), scroll.state.offset.y, scroll.content_size.y, view) {
            let mut state = scroll.state;
            state.offset.y = offset;
            state.store(ui.ctx(), scroll.id);
            ui.ctx().request_repaint();
        }
        ui.advance_cursor_after_rect(full);

        // Floating "back to top" button once the list is scrolled down.
        if scroll.state.offset.y > 300.0 {
            let pos = egui::pos2(visible.center().x, visible.bottom().min(scroll.inner_rect.bottom()) - 18.0);
            egui::Area::new(egui::Id::new("scroll_top"))
                .order(egui::Order::Foreground)
                .pivot(egui::Align2::CENTER_BOTTOM)
                .fixed_pos(pos)
                .show(ui.ctx(), |ui| {
                    if theme::back_to_top(ui).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        self.scroll_to_top = true;
                    }
                });
        }

        // The cover column was resized: scale the covers (and rows) to match.
        if let Some(w) = cover_width {
            let size = (w - 16.0).clamp(16.0, 234.0);
            if (size - self.settings.cover_size).abs() > 0.5 {
                self.settings.cover_size = size;
            }
        }
        if let Some(click) = click {
            self.handle_click(click);
        }
        if let Some(nav) = nav {
            self.navigate(nav);
        }
        match row_menu {
            Some(RowAction::Details(i)) => self.handle_click(RowClick::Plain(i)),
            Some(RowAction::Toggle(i)) => self.handle_click(RowClick::Checkbox(i)),
            Some(RowAction::Download(i)) => {
                let c = self.charts[self.results[i]].clone();
                if self.job.is_none() {
                    self.start_download(c.title(), vec![c]);
                }
            }
            Some(RowAction::Preview(i)) => {
                let c = self.charts[self.results[i]].clone();
                self.preview = Some(preview::Preview::open(&self.client, &c, ui.ctx(), self.filters.instrument, self.filters.difficulty));
            }
            Some(RowAction::Reveal(i)) => {
                let md5 = self.charts[self.results[i]].md5.clone();
                self.reveal_chart(&md5);
            }
            Some(RowAction::Nav(n)) => self.navigate(n),
            None => {}
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
        fancy_scroll(ui, "detail", |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if ui.small_button("✖").on_hover_text(t("Close")).clicked() {
                        self.detail = None;
                    }
                });
            });
            let size = ui.available_width().min(320.0);
            ui.vertical_centered(|ui| cover(ui, &mut self.covers, c.art.as_deref(), size, covers::LARGE));
            ui.add_space(8.0);
            let title = ui.label(RichText::new(c.name.clone().unwrap_or_else(|| t("Unknown"))).heading().strong());
            copy_menu(&title, &[(n("Copy song name"), c.name.clone().unwrap_or_default()), (n("Copy \"Artist - Song\""), c.title())]);
            let mut nav = None;
            if let Some(a) = &c.artist
                && link(ui, RichText::new(a).size(16.0).color(theme::accent()), n("Show all songs by this artist"))
            {
                nav = Some(Nav::Text(1, a.clone(), true));
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if let Some(a) = &c.album
                    && link(ui, RichText::new(a).weak(), n("Show this album"))
                {
                    nav = Some(Nav::Album { artist: c.artist.clone(), album: a.clone() });
                }
                if let Some(y) = &c.year {
                    if c.album.is_some() {
                        ui.weak("·");
                    }
                    if link(ui, RichText::new(y).weak(), n("Show songs from this year")) {
                        nav = Some(Nav::Text(4, y.clone(), true));
                    }
                }
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(g) = games::game_key(c.icon.as_deref())
                    && theme::chip(ui, &format!("🏆 {}", games::game_name(&g)), theme::warn()).on_hover_text(t("Show this game's charts")).clicked()
                {
                    nav = Some(Nav::Game(g));
                }
                if let Some(g) = &c.genre
                    && theme::chip(ui, g, tag_color(g)).on_hover_text(t("Show this genre")).clicked()
                {
                    nav = Some(Nav::Text(3, g.clone(), true));
                }
                if let Some(ch) = &c.charter
                    && theme::chip(ui, &format!("✏ {ch}"), theme::RAINBOW[4]).on_hover_text(t("Show everything charted by this charter")).clicked()
                {
                    nav = Some(Nav::Text(5, ch.clone(), false));
                }
                if !c.length_text().is_empty() {
                    theme::tag(ui, &format!("⏱ {}", c.length_text()), theme::RAINBOW[6]);
                }
                if !c.uploaded().is_empty() {
                    theme::tag(ui, &format!("📅 {}", c.uploaded()), Color32::GRAY);
                }
            });

            ui.add_space(10.0);
            // Download and Preview side by side, half the panel each.
            let (mut download, mut open_preview, mut reveal_it) = (false, false, false);
            let half = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).max(60.0);
            let busy = self.job.is_some();
            ui.horizontal(|ui| {
                if downloaded {
                    let good = theme::good();
                    let button = egui::Button::new(RichText::new(t("📂  Show in folder")).strong().color(good))
                        .fill(good.linear_multiply(0.15))
                        .stroke(egui::Stroke::new(1.0, good.linear_multiply(0.7)))
                        .min_size(vec2(half, 34.0));
                    reveal_it = ui.add(button).on_hover_text(t("Downloaded ✔  Open it in your file manager")).clicked();
                } else {
                    download = ui.add_enabled(!busy, theme::download_button(t("⬇  Download")).min_size(vec2(half, 34.0))).clicked();
                }
                open_preview = ui
                    .add(egui::Button::new(RichText::new(t("▶  Preview")).size(14.5).strong()).min_size(vec2(half, 34.0)))
                    .on_hover_text(t("Watch the notes fall, like in game (no audio)"))
                    .clicked();
            });
            if download {
                self.start_download(c.title(), vec![c.clone()]);
            }
            if reveal_it {
                self.reveal_chart(&c.md5);
            }
            if open_preview {
                self.preview = Some(preview::Preview::open(&self.client, &c, ui.ctx(), self.filters.instrument, self.filters.difficulty));
            }

            ui.add_space(12.0);
            ui.label(RichText::new(t("Instruments")).strong());
            // One full-width band per instrument, alternating shades. Two lines, so the note
            // count and NPS never get cut off however narrow the panel is.
            let mut row = 0;
            ui.spacing_mut().item_spacing.y = 2.0;
            for (k, (_, name, _)) in INSTRUMENTS.iter().enumerate() {
                let d = c.diffs[k];
                let notes = c.notes.iter().filter(|s| s.i == k as u8).min_by_key(|s| s.d); // hardest first
                if d < 0 && notes.is_none() {
                    continue;
                }
                let fill = if row % 2 == 0 { ui.visuals().widgets.inactive.bg_fill } else { Color32::TRANSPARENT };
                row += 1;
                egui::Frame::new()
                    .fill(fill)
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if link_ex(ui, RichText::new(format!("{}  {}", instrument_icon(k as u8), t(name))).strong(), n("Show charts for this instrument"), false) {
                                nav = Some(Nav::Instrument(k as u8));
                            }
                            if d >= 0 {
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let resp = ui.add(egui::Label::new(theme::difficulty_dots(d)).sense(Sense::click()));
                                    if resp.on_hover_text(tr!("Intensity {level}: show {instrument} charts at this intensity", level = d, instrument = t(name))).clicked() {
                                        nav = Some(Nav::Difficulty(k as u8, d));
                                    }
                                });
                            }
                        });
                        if let Some(s) = notes {
                            ui.label(RichText::new(tr!("{notes} notes  ·  {nps} max NPS", notes = s.n, nps = format!("{:.0}", s.nps))).size(12.5).weak());
                        }
                    });
            }

            ui.spacing_mut().item_spacing.y = 6.0;
            ui.add_space(10.0);
            ui.label(RichText::new(t("Features")).strong());
            ui.horizontal_wrapped(|ui| {
                let any = feature_tags(ui, &c, true, &mut nav);
                if !any {
                    ui.weak(t("None"));
                }
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.weak(t("MD5"));
                // The copy button sits at the right edge; the hash takes what's left and
                // shortens with "…" if needed (right-click or hover shows it in full).
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("📋").on_hover_text(t("Copy MD5")).clicked() {
                        ui.ctx().copy_text(c.md5.clone());
                    }
                    let hash = ui.add(egui::Label::new(RichText::new(&c.md5).monospace().small()).truncate());
                    copy_menu(&hash.on_hover_text(&c.md5), &[(n("Copy MD5"), c.md5.clone())]);
                });
            });
            ui.add_space(8.0);
            if let Some(nav) = nav {
                self.navigate(nav);
            }
        });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else {
            return;
        };
        let mut open = true;
        let mut decision = None;
        egui::Window::new(t("⬇ Start bulk download?"))
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(ctx.content_rect().center())
            .open(&mut open)
            .show(ctx, |ui| {
                let have = {
                    let m = self.manifest.lock().unwrap();
                    confirm.charts.iter().filter(|c| m.contains(&c.md5)).count()
                };
                let todo = confirm.charts.len() - have;
                ui.label(RichText::new(&confirm.title).heading());
                ui.label(tr!("{count} charts: {have} already downloaded, {todo} to go.", count = confirm.charts.len(), have = have, todo = todo));
                ui.weak(t("You can stop at any time; next time it picks up where it left off."));
                ui.add_space(8.0);
                theme::checkbox(ui, &mut self.settings.video, &t("🎬 Download video backgrounds (much larger)"));
                ui.horizontal(|ui| {
                    ui.label(t("Format:"));
                    ui.radio_value(&mut self.settings.format, DownloadFormat::Sng, ".sng");
                    ui.radio_value(&mut self.settings.format, DownloadFormat::Folder, t("Chart Folder"));
                });
                ui.horizontal(|ui| {
                    ui.label(t("Wait between downloads:"));
                    ui.add(egui::Slider::new(&mut self.settings.delay_secs, 0.5..=10.0).suffix(" s"));
                });
                let hours = todo as f32 * (self.settings.delay_secs + 1.0) / 3600.0;
                ui.weak(tr!("Roughly {hours}+ hours at that pace (plus download time).", hours = format!("{hours:.0}")));
                ui.weak(t("Encore is donation-funded; please keep the delay reasonable."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(theme::primary(t("Start"))).clicked() {
                        decision = Some(true);
                    }
                    if ui.button(t("Cancel")).clicked() {
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
        let mut actions = SettingsActions::default();
        let example = self.charts.first().cloned();
        let busy = self.job.is_some();
        let building = matches!(self.cat_status, CatStatus::Building { .. });
        fancy_scroll(ui, "settings", |ui| {
            // Centred content; two columns of cards on wide windows, one otherwise.
            let avail = ui.available_width();
            let width = (avail - 32.0).min(1240.0);
            let two = width >= 1000.0;
            let card_w = if two { (width - 16.0) / 2.0 } else { width };
            ui.add_space(12.0);
            ui.horizontal_top(|ui| {
                ui.add_space(((avail - width) / 2.0).max(0.0));
                let s = &mut self.settings;
                if two {
                    ui.vertical(|ui| {
                        library_card(ui, s, card_w, busy, example.as_ref(), &mut actions);
                        downloads_card(ui, s, card_w, busy, &mut actions);
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        appearance_card(ui, s, card_w);
                        accessibility_card(ui, s, card_w, &mut actions);
                        catalogue_card(ui, s, card_w, building, &mut actions);
                    });
                } else {
                    ui.vertical(|ui| {
                        library_card(ui, s, card_w, busy, example.as_ref(), &mut actions);
                        downloads_card(ui, s, card_w, busy, &mut actions);
                        appearance_card(ui, s, card_w);
                        accessibility_card(ui, s, card_w, &mut actions);
                        catalogue_card(ui, s, card_w, building, &mut actions);
                    });
                }
            });
            ui.add_space(8.0);
            let renzora = self.renzora.clone();
            ui.vertical_centered(|ui| {
                if let Some(tex) = &renzora {
                    let logo = ui.add(egui::Image::new(tex).fit_to_exact_size(vec2(56.0 * 110.0 / 128.0, 56.0)).sense(Sense::click()));
                    if logo.on_hover_text(RENZORA_URL).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(RENZORA_URL));
                    }
                }
                ui.label(RichText::new("Renzora Studios").strong().size(15.0));
                ui.weak(concat!("Rainbow v", env!("CARGO_PKG_VERSION")));
            });
            ui.add_space(12.0);
        });
        if actions.rebuild {
            self.update_catalogue(true);
        }
        if actions.show_shortcuts {
            self.show_shortcuts = true;
        }
        if actions.extract_now {
            self.auto_unzip(true);
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
        if self.settings.language != self.saved_settings.language {
            i18n::set_language(&self.settings.language);
        }
        if self.settings.font != self.saved_settings.font || self.settings.language != self.saved_settings.language {
            theme::install_fonts(ctx, self.settings.font, &i18n::current());
        }
        if self.settings.theme != self.saved_settings.theme
            || self.settings.zoom != self.saved_settings.zoom
            || self.settings.access() .color_blind != self.saved_settings.color_blind
            || self.settings.reduce_motion != self.saved_settings.reduce_motion
        {
            theme::apply(ctx, self.settings.theme, self.settings.zoom, self.settings.access());
        }
        self.settings.save();
        self.saved_settings = self.settings.clone();
    }
}

/// "Renzora Studios" with the studio's logo in a pill like the header's tabs; links to its
/// GitHub.
fn credit(ui: &mut egui::Ui, logo: Option<&egui::TextureHandle>, height: f32) {
    // Drawn by hand at its exact size, so it works in the header's right-to-left layout.
    let text = ui.painter().layout_no_wrap(
        "Renzora Studios".to_owned(),
        egui::FontId::new(13.5, egui::FontFamily::Name("bold".into())),
        ui.visuals().text_color(),
    );
    let logo_size = vec2(height * 110.0 / 128.0, height);
    let (pad_l, gap, pad_r, pad_y) = (6.0, 8.0, 12.0, 3.0);
    let logo_w = if logo.is_some() { logo_size.x + gap } else { 0.0 };
    let size = vec2(pad_l + logo_w + text.size().x + pad_r, height.max(text.size().y) + pad_y * 2.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let visuals = ui.visuals();
    let fill = if resp.hovered() { visuals.widgets.hovered.bg_fill } else { visuals.widgets.inactive.bg_fill };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(16), fill);
    if let Some(tex) = logo {
        let logo_rect = egui::Rect::from_min_size(egui::pos2(rect.left() + pad_l, rect.center().y - logo_size.y / 2.0), logo_size);
        painter.image(tex.id(), logo_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    }
    let text_pos = egui::pos2(rect.left() + pad_l + logo_w, rect.center().y - text.size().y / 2.0);
    painter.galley(text_pos, text, ui.visuals().text_color());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, "Renzora Studios"));
    if resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(RENZORA_URL).clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(RENZORA_URL));
    }
}

/// A status-bar link with an optional brand icon, tinted like a hyperlink.
fn web_link(ui: &mut egui::Ui, icon: Option<egui::ImageSource<'static>>, text: &str, url: &str, tip: &str) {
    let color = ui.visuals().hyperlink_color;
    let label = RichText::new(text).color(color);
    let button = match icon {
        Some(icon) => egui::Button::image_and_text(egui::Image::new(icon).fit_to_exact_size(vec2(15.0, 15.0)).tint(color), label),
        None => egui::Button::new(label),
    };
    let resp = ui.add(button.frame(false)).on_hover_text(format!("{}\n{url}", t(tip)));
    if resp.clicked() {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

#[derive(Default)]
struct SettingsActions {
    rebuild: bool,
    extract_now: bool,
    show_shortcuts: bool,
}

/// A full-width settings card with a large title and a short description.
fn settings_card(ui: &mut egui::Ui, width: f32, title: &str, about: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::same(18))
        .outer_margin(egui::Margin { left: 0, right: 0, top: 0, bottom: 16 })
        .show(ui, |ui| {
            ui.set_width(width - 38.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).size(19.0).strong().color(theme::accent_hover()));
                ui.label(RichText::new(about).size(13.5).weak());
                ui.add_space(10.0);
                ui.spacing_mut().item_spacing.y = 10.0;
                add(ui);
            });
        });
}

/// One setting: a label and description on the left, its control on the right.
fn setting_row(ui: &mut egui::Ui, label: &str, about: &str, control: impl FnOnce(&mut egui::Ui)) {
    ui.separator();
    // The label and description wrap in the width left beside the control (measured last
    // frame), so long descriptions never run under the buttons.
    let id = ui.id().with(("setting_row", label));
    let control_w = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
    ui.horizontal(|ui| {
        let text_w = (ui.available_width() - control_w - 16.0).max(140.0);
        ui.vertical(|ui| {
            ui.set_max_width(text_w);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(RichText::new(label).size(15.5).strong()).wrap());
            if !about.is_empty() {
                ui.add(egui::Label::new(RichText::new(about).size(13.0).weak()).wrap());
            }
        });
        let measured = ui
            .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                control(ui);
                ui.min_rect().width()
            })
            .inner;
        if (measured - control_w).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(id, measured));
            ui.ctx().request_repaint();
        }
    });
}

/// A row of pill buttons for picking one option.
fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    // Laid out right to left by the caller, so add the options in reverse.
    for (option, label) in options.iter().rev() {
        let on = *value == *option;
        let label = t(label);
        let text = if on { RichText::new(label).size(14.5).color(theme::on_accent()).strong() } else { RichText::new(label).size(14.5) };
        let fill = if on { theme::accent() } else { ui.visuals().widgets.inactive.bg_fill };
        let button = egui::Button::new(text).fill(fill).corner_radius(CornerRadius::same(14)).min_size(vec2(0.0, 32.0));
        if ui.add(button).clicked() && !on {
            *value = *option;
            changed = true;
        }
    }
    changed
}

fn library_card(ui: &mut egui::Ui, s: &mut Settings, width: f32, busy: bool, example: Option<&Chart>, actions: &mut SettingsActions) {
    let _ = actions;
    settings_card(ui, width, &t("📁  Chart library"), &t("Where Rainbow puts the charts it downloads."), |ui| {
        setting_row(ui, &t("Songs folder"), &t("Point this at your Clone Hero or YARG songs folder."), |ui| {
            if ui.add_enabled(!busy, theme::primary(t("Choose…")).min_size(vec2(0.0, 32.0))).clicked()
                && let Some(dir) = rfd::FileDialog::new().set_directory(&s.dir).pick_folder()
            {
                s.dir = dir;
            }
            if ui.add(egui::Button::new(RichText::new(t("📂 Open")).size(14.5)).min_size(vec2(0.0, 32.0))).clicked() {
                open_folder(&s.dir);
            }
        });
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(RichText::new(s.dir.display().to_string()).monospace().size(13.5)).truncate());
            });
        setting_row(ui, &t("Chart folder name"), &t("Use {name} {artist} {album} {genre} {year} {charter}, and / for subfolders."),
            |_| {},
        );
        let resp = ui.add(
            egui::TextEdit::singleline(&mut s.folder_name)
                .font(egui::FontId::proportional(15.0))
                .margin(egui::Margin::symmetric(10, 7))
                .desired_width(f32::INFINITY),
        );
        text_menu(&resp, &mut s.folder_name);
        if let Some(example) = example {
            ui.label(RichText::new(format!("e.g.  {}", api::resolve_folder_name(&s.folder_name, example))).size(13.0).weak());
        }
    });
}

fn downloads_card(ui: &mut egui::Ui, s: &mut Settings, width: f32, busy: bool, actions: &mut SettingsActions) {
    settings_card(ui, width, &t("⬇  Downloads"), &t("How charts are saved, and how politely bulk downloads run."), |ui| {
        ui.add_enabled_ui(!busy, |ui| {
            setting_row(ui, &t("Format"), &t(".sng is one file per chart. Chart Folder unpacks it with song.ini."), |ui| {
                segmented(ui, &mut s.format, &[(DownloadFormat::Sng, ".sng"), (DownloadFormat::Folder, n("Chart Folder"))]);
            });
            setting_row(ui, &t("Video backgrounds"), &t("Much larger downloads. Off gets the no-video version."), |ui| {
                theme::checkbox(ui, &mut s.video, "");
            });
        });
        setting_row(ui, &t("Wait between bulk downloads"), &t("Encore is donation-funded, so please keep this reasonable."), |ui| {
            ui.add(egui::Slider::new(&mut s.delay_secs, 0.5..=10.0).suffix(" s"));
        });
        setting_row(ui, &t("Convert Rock Band songs automatically"), &t("CON files (e.g. from RhythmVerse) in your songs folder become Clone Hero charts. You can also drop them onto the window."), |ui| {
            theme::checkbox(ui, &mut s.auto_convert, "");
        });
        setting_row(ui, &t("Extract .zip files automatically"), &t("Unpack zips that appear in your songs folder (e.g. from other sites)."), |ui| {
            theme::checkbox(ui, &mut s.auto_extract, "");
        });
        ui.add_enabled_ui(s.auto_extract || s.auto_convert, |ui| {
            let about = tr!("Otherwise they're moved to {zips} or {cons} in your songs folder.", zips = unzip::DONE_DIR, cons = import::CONVERTED_DIR);
            setting_row(ui, &t("Delete originals after processing"), &about, |ui| {
                theme::checkbox(ui, &mut s.delete_zips, "");
            });
        });
        if ui.add(egui::Button::new(RichText::new(t("📦  Process songs folder now")).size(14.5)).min_size(vec2(0.0, 32.0))).clicked() {
            actions.extract_now = true;
        }
    });
}

fn appearance_card(ui: &mut egui::Ui, s: &mut Settings, width: f32) {
    settings_card(ui, width, &t("🎨  Appearance"), &t("Theme, size and what the results table shows."), |ui| {
        setting_row(ui, &t("Theme"), &t("Colours for the whole app."), |_| {});
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            for kind in theme::ThemeKind::ALL {
                if theme_swatch(ui, kind, s.theme == kind).clicked() {
                    s.theme = kind;
                }
            }
        });
        setting_row(ui, &t("Zoom"), &t("Makes everything bigger or smaller."), |ui| {
            if ui.add(egui::Button::new(RichText::new("➕").size(15.0)).min_size(vec2(34.0, 32.0))).on_hover_text(t("Zoom in")).clicked() {
                s.zoom = (s.zoom + 0.1).min(2.5);
            }
            ui.label(RichText::new(format!("{:.0}%", s.zoom * 100.0)).size(15.0).strong());
            if ui.add(egui::Button::new(RichText::new("➖").size(15.0)).min_size(vec2(34.0, 32.0))).on_hover_text(t("Zoom out")).clicked() {
                s.zoom = (s.zoom - 0.1).max(0.5);
            }
        });
        setting_row(ui, &t("Row size"), &t("Drag the cover column's edge to fine-tune."), |ui| {
            if segmented(ui, &mut s.density, &[(Density::Compact, n("Compact")), (Density::Default, n("Default")), (Density::Large, n("Large"))]) {
                s.cover_size = s.density.cover_size();
            }
        });
        setting_row(ui, &t("Table columns"), &t("Columns also hide automatically when the window is narrow."), |_| {});
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(18.0, 10.0);
            let c = &mut s.columns;
            for (on, label) in [
                (&mut c.cover, n("Cover")),
                (&mut c.artist, n("Artist")),
                (&mut c.album, n("Album")),
                (&mut c.genre, n("Genre")),
                (&mut c.year, n("Year")),
                (&mut c.charter, n("Charter")),
                (&mut c.length, n("Length")),
                (&mut c.difficulty, n("Difficulty")),
                (&mut c.uploaded, n("Upload Date")),
            ] {
                theme::checkbox(ui, on, label);
            }
        });
    });
}

fn accessibility_card(ui: &mut egui::Ui, s: &mut Settings, width: f32, actions: &mut SettingsActions) {
    settings_card(ui, width, &t("♿  Accessibility & text"), &t("Fonts, colour vision, motion and keyboard control."), |ui| {
        setting_row(ui, &t("Font"), &t("Each option is shown in its own typeface."), |_| {});
        for font in theme::UiFont::ALL {
            let selected = s.font == font;
            let family = egui::FontFamily::Name(font.key().into());
            let resp = ui.horizontal(|ui| {
                let mut on = selected;
                let clicked = theme::checkbox(ui, &mut on, "").clicked();
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(t(font.name())).family(family.clone()).size(16.5));
                    ui.label(RichText::new(t(font.about())).size(12.5).weak());
                });
                clicked
            });
            if resp.inner || resp.response.interact(Sense::click()).clicked() {
                s.font = font;
            }
        }
        setting_row(ui, &t("Colour vision"), &t("Swaps status and difficulty colours for ones that stay distinct. Icons and numbers always show the meaning too."), |ui| {
            let options: Vec<(theme::ColorBlind, &str)> = theme::ColorBlind::ALL.iter().map(|c| (*c, c.name())).collect();
            segmented(ui, &mut s.color_blind, &options);
        });
        setting_row(ui, &t("Reduce motion"), &t("Turns off animations and smooth scrolling."), |ui| {
            theme::checkbox(ui, &mut s.reduce_motion, "");
        });
        setting_row(ui, &t("High contrast"), &t("Pure black and white with a bright accent. Pick it under Theme."), |ui| {
            let mut on = s.theme == theme::ThemeKind::HighContrast;
            if theme::checkbox(ui, &mut on, "").changed() {
                s.theme = if on { theme::ThemeKind::HighContrast } else { theme::ThemeKind::Midnight };
            }
        });
        setting_row(ui, &t("Text size"), &t("Zoom scales the whole app, text included."), |ui| {
            if ui.add(egui::Button::new(RichText::new("A+").size(15.0)).min_size(vec2(38.0, 32.0))).on_hover_text(t("Bigger")).clicked() {
                s.zoom = (s.zoom + 0.1).min(2.5);
            }
            ui.label(RichText::new(format!("{:.0}%", s.zoom * 100.0)).size(15.0).strong());
            if ui.add(egui::Button::new(RichText::new("A−").size(15.0)).min_size(vec2(38.0, 32.0))).on_hover_text(t("Smaller")).clicked() {
                s.zoom = (s.zoom - 0.1).max(0.5);
            }
        });
        setting_row(ui, &t("Keyboard shortcuts"), &t("Everything in Browse can be used from the keyboard."), |ui| {
            if ui.add(egui::Button::new(RichText::new(t("⌨  Show (F1)")).size(14.5)).min_size(vec2(0.0, 32.0))).clicked() {
                actions.show_shortcuts = true;
            }
        });
        ui.label(RichText::new(t("Screen readers (Narrator, NVDA, JAWS, VoiceOver, Orca) can read Rainbow's buttons, fields, checkboxes and results through AccessKit.")).size(12.5).weak());
    });
}

fn catalogue_card(ui: &mut egui::Ui, s: &mut Settings, width: f32, building: bool, actions: &mut SettingsActions) {
    settings_card(ui, width, &t("📚  Catalogue"), &t("Rainbow's local copy of Encore's chart list, for instant search."), |ui| {
        let size = std::fs::metadata(catalogue::path()).map(|m| m.len()).unwrap_or(0);
        ui.label(RichText::new(format!("{}  ·  {:.1} MB", catalogue::path().display(), size as f64 / 1_048_576.0)).size(13.0).weak());
        setting_row(ui, &t("Check for new charts on startup"), &t("The ⟳ button in the status bar works any time."), |ui| {
            let text = match s.refresh_after_hours {
                None => n("Never"),
                Some(0) => n("Every launch"),
                Some(12) => n("After 12 hours"),
                Some(24) => n("After 1 day"),
                Some(168) => n("After 1 week"),
                Some(_) => n("Custom"),
            };
            egui::ComboBox::from_id_salt("refresh").selected_text(RichText::new(t(text)).size(14.5)).width(170.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.refresh_after_hours, Some(0), t("Every launch"));
                ui.selectable_value(&mut s.refresh_after_hours, Some(12), t("After 12 hours"));
                ui.selectable_value(&mut s.refresh_after_hours, Some(24), t("After 1 day"));
                ui.selectable_value(&mut s.refresh_after_hours, Some(168), t("After 1 week"));
                ui.selectable_value(&mut s.refresh_after_hours, None, t("Never"));
            });
        });
        setting_row(ui, &t("Rebuild from scratch"), &t("Re-downloads the whole chart list (about 5 minutes)."), |ui| {
            if ui.add_enabled(!building, egui::Button::new(RichText::new(t("⟳ Rebuild")).size(14.5)).min_size(vec2(0.0, 32.0))).clicked() {
                actions.rebuild = true;
            }
        });
    });
}

/// A clickable preview of a theme: its background, a few surfaces and its accent.
fn theme_swatch(ui: &mut egui::Ui, kind: theme::ThemeKind, selected: bool) -> egui::Response {
    let pal = kind.palette();
    let (rect, resp) = ui.allocate_exact_size(vec2(112.0, 70.0), Sense::click());
    let painter = ui.painter();
    let r = CornerRadius::same(10);
    painter.rect_filled(rect, r, pal.panel);
    let bar = egui::Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(rect.width() - 20.0, 10.0));
    painter.rect_filled(bar, CornerRadius::same(5), pal.raised);
    let pill = egui::Rect::from_min_size(rect.min + vec2(10.0, 26.0), vec2(40.0, 12.0));
    painter.rect_filled(pill, CornerRadius::same(6), pal.accent);
    let pill2 = egui::Rect::from_min_size(rect.min + vec2(56.0, 26.0), vec2(28.0, 12.0));
    painter.rect_filled(pill2, CornerRadius::same(6), pal.hover);
    painter.text(rect.left_bottom() + vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, t(kind.name()), egui::FontId::proportional(13.0), pal.text);
    let stroke = if selected {
        egui::Stroke::new(2.5, theme::accent())
    } else if resp.hovered() {
        egui::Stroke::new(1.5, theme::accent_hover())
    } else {
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };
    painter.rect_stroke(rect, r, stroke, egui::StrokeKind::Inside);
    resp.on_hover_text(t(kind.name()))
}

/// A vertical scroll area that uses Rainbow's chunky scroll bar instead of egui's.
fn fancy_scroll<R>(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let full = ui.available_rect_before_wrap();
    let area = full.with_max_x(full.right() - theme::SCROLLBAR_W - 6.0);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    let out = egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(&mut child, add);
    let bar = egui::Rect::from_min_max(
        egui::pos2(full.right() - theme::SCROLLBAR_W, out.inner_rect.top()),
        egui::pos2(full.right(), out.inner_rect.bottom()),
    );
    let view = out.inner_rect.height();
    if let Some(offset) = theme::scrollbar(ui, bar, out.id.with("bar"), out.state.offset.y, out.content_size.y, view) {
        let mut state = out.state;
        state.offset.y = offset;
        state.store(ui.ctx(), out.id);
        ui.ctx().request_repaint();
    }
    ui.advance_cursor_after_rect(full);
    out.inner
}

/// A toggle-style rounded button.
fn pill(ui: &mut egui::Ui, on: bool, text: &str) -> egui::Response {
    let text = if on { RichText::new(text).color(theme::on_accent()).strong() } else { RichText::new(text) };
    let fill = if on { theme::accent() } else { ui.visuals().widgets.inactive.bg_fill };
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

/// Right-click menu for a result row.
fn row_context_menu(ui: &mut egui::Ui, c: &Chart, i: usize, checked: bool, downloaded: bool, action: &mut Option<RowAction>) {
    ui.set_min_width(210.0);
    let mut pick = |ui: &mut egui::Ui, label: &str, a: RowAction| {
        if ui.button(label).clicked() {
            *action = Some(a);
            ui.close();
        }
    };
    pick(ui, &t("ℹ  Show details"), RowAction::Details(i));
    if downloaded {
        pick(ui, &t("📂  Show in folder"), RowAction::Reveal(i));
    } else {
        pick(ui, &t("⬇  Download"), RowAction::Download(i));
    }
    pick(ui, &t("▶  Preview"), RowAction::Preview(i));
    pick(ui, &if checked { t("☐  Deselect") } else { t("☑  Select") }, RowAction::Toggle(i));
    ui.separator();
    ui.menu_button(t("📋  Copy"), |ui| {
        let mut items = vec![(n("\"Artist - Song (Charter)\""), c.title())];
        for (label, value) in [(n("Song name"), &c.name), (n("Artist"), &c.artist), (n("Album"), &c.album), (n("Charter"), &c.charter)] {
            if let Some(v) = value {
                items.push((label, v.clone()));
            }
        }
        items.push(("MD5", c.md5.clone()));
        for (label, value) in items {
            if ui.button(t(label)).clicked() {
                ui.ctx().copy_text(value);
                ui.close();
            }
        }
    });
    if let Some(a) = &c.artist {
        pick(ui, &t("🔍  More by this artist"), RowAction::Nav(Nav::Text(1, a.clone(), true)));
    }
    if let Some(ch) = &c.charter {
        pick(ui, &t("🔍  More by this charter"), RowAction::Nav(Nav::Text(5, ch.clone(), false)));
    }
}

/// A right-click menu that copies one or more values.
fn copy_menu(resp: &egui::Response, items: &[(&str, String)]) {
    resp.context_menu(|ui| {
        for (label, value) in items {
            if ui.button(format!("📋  {}", t(label))).clicked() {
                ui.ctx().copy_text(value.clone());
                ui.close();
            }
        }
    });
}

/// Right-click menu for a text field: cut, copy, paste, select all and clear.
/// Returns true if the text changed.
fn text_menu(resp: &egui::Response, text: &mut String) -> bool {
    use egui::text::{CCursor, CCursorRange};
    let id = resp.id;
    let mut changed = false;
    resp.context_menu(|ui| {
        let ctx = ui.ctx().clone();
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
        let len = text.chars().count();
        let (start, end) = state.cursor.char_range().map_or((len, len), |r| {
            let [a, b] = r.sorted_cursors();
            (a.index.0.min(len), b.index.0.min(len))
        });
        let byte = |ci: usize| text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b);
        let (bs, be) = (byte(start), byte(end));
        let has_selection = start < end;
        let place_cursor = |state: &mut egui::text_edit::TextEditState, at: usize| {
            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(at))));
        };
        if ui.add_enabled(has_selection, egui::Button::new(t("✂  Cut"))).clicked() {
            ctx.copy_text(text[bs..be].to_owned());
            text.replace_range(bs..be, "");
            place_cursor(&mut state, start);
            changed = true;
            ui.close();
        }
        if ui.add_enabled(has_selection, egui::Button::new(t("📋  Copy"))).clicked() {
            ctx.copy_text(text[bs..be].to_owned());
            ui.close();
        }
        if ui.button(t("📥  Paste")).clicked() {
            if let Ok(clip) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                let clip = clip.replace(['\r', '\n'], " ");
                text.replace_range(bs..be, &clip);
                place_cursor(&mut state, start + clip.chars().count());
                changed = true;
            }
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(len > 0, egui::Button::new(t("Select all"))).clicked() {
            state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(len))));
            ctx.memory_mut(|m| m.request_focus(id));
            ui.close();
        }
        if ui.add_enabled(len > 0, egui::Button::new(t("Clear"))).clicked() {
            text.clear();
            place_cursor(&mut state, 0);
            changed = true;
            ui.close();
        }
        state.store(&ctx, id);
    });
    changed
}

/// Text that acts as a link: turns the hover colour on hover, with a pointer cursor, and is
/// cut short with "…" if it doesn't fit. Returns true if clicked.
fn link(ui: &mut egui::Ui, text: RichText, tip: &str) -> bool {
    link_ex(ui, text, tip, true)
}

/// Like [`link`]; `truncate: false` always shows the whole text (e.g. in grids, which would
/// otherwise squeeze it down to "…").
fn link_ex(ui: &mut egui::Ui, text: RichText, tip: &str, truncate: bool) -> bool {
    let tip = t(tip);
    let mut label = egui::Label::new(text).sense(Sense::click()).selectable(false);
    if truncate {
        label = label.truncate();
    }
    let (pos, galley, resp) = label.layout_in_ui(ui);
    if ui.is_rect_visible(resp.rect) {
        if resp.hovered() {
            ui.painter().galley_with_override_text_color(pos, galley, theme::accent_hover());
        } else {
            ui.painter().galley(pos, galley, ui.visuals().text_color());
        }
    }
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip).clicked()
}

/// Difficulty for the chosen instrument as dots, or a compact per-instrument summary.
/// Clicking one shows that instrument's charts at that intensity.
fn difficulty_cell(ui: &mut egui::Ui, c: &Chart, instrument: Option<u8>, nav: &mut Option<Nav>) {
    if let Some(i) = instrument {
        let d = c.diffs[i as usize];
        if d >= 0 {
            let resp = ui.add(egui::Label::new(theme::difficulty_dots(d)).sense(Sense::click()));
            if resp.on_hover_text(tr!("Intensity {level}: show more like this", level = d)).clicked() {
                *nav = Some(Nav::Difficulty(i, d));
            }
        }
        return;
    }
    for (k, label) in [(0, "G"), (3, "B"), (4, "D"), (5, "K")] {
        let d = c.diffs[k];
        if d >= 0 {
            let color = theme::difficulty_color(d);
            let tip = tr!("{instrument} intensity {level}: show more like this", instrument = t(INSTRUMENTS[k].1), level = d);
            if link(ui, RichText::new(format!("{label}{d}")).color(color).small().strong(), &tip) {
                *nav = Some(Nav::Difficulty(k as u8, d));
            }
        }
    }
}

/// An icon with a tooltip. It only senses hover, so the row stays highlighted and clickable.
fn tag_icon(ui: &mut egui::Ui, text: RichText, tip: &str) {
    ui.add(egui::Label::new(text).selectable(false).sense(Sense::hover())).on_hover_text(t(tip));
}

/// Icon tags for a table row: downloaded/broken status, source game, instruments and features.
fn tags(ui: &mut egui::Ui, c: &Chart, downloaded: bool) {
    ui.spacing_mut().item_spacing.x = 4.0;
    if downloaded {
        tag_icon(ui, RichText::new("✔").color(theme::good()), n("Downloaded"));
    }
    if c.has(flag::BROKEN) {
        tag_icon(ui, RichText::new("⚠").color(theme::bad()), n("Encore reports this chart as unplayable"));
    }
    if let Some(g) = games::game_key(c.icon.as_deref()) {
        tag_icon(ui, RichText::new("🏆").color(theme::warn()), &games::game_name(&g));
    }
    // One icon per kind of instrument; its tooltip lists the parts it stands for.
    let mut icons: Vec<(&str, Vec<&str>)> = Vec::new();
    for (k, (_, name, _)) in INSTRUMENTS.iter().enumerate() {
        if c.diffs[k] >= 0 || c.notes.iter().any(|s| s.i == k as u8) {
            let icon = instrument_icon(k as u8);
            match icons.iter_mut().find(|(i, _)| *i == icon) {
                Some((_, names)) => names.push(name),
                None => icons.push((icon, vec![name])),
            }
        }
    }
    for (icon, names) in icons {
        tag_icon(ui, RichText::new(icon), &names.join("\n"));
    }
    feature_tags(ui, c, false, &mut None);
}

/// Feature tags; `full` shows labelled chips (details panel), otherwise just icons.
/// Clicking one shows charts with that feature.
fn feature_tags(ui: &mut egui::Ui, c: &Chart, full: bool, nav: &mut Option<Nav>) -> bool {
    let features = [
        (flag::VIDEO, "🎬", n("Video"), theme::RAINBOW[4]),
        (flag::LYRICS, "📝", n("Lyrics"), theme::RAINBOW[2]),
        (flag::VOCALS, "🎤", n("Vocals"), theme::RAINBOW[6]),
        (flag::SOLO, "🌟", n("Solos"), theme::RAINBOW[1]),
        (flag::TAP, "👆", n("Tap notes"), theme::RAINBOW[3]),
        (flag::OPEN, "⭕", n("Open notes"), theme::RAINBOW[5]),
        (flag::FORCED, "⚡", n("Forced notes"), theme::RAINBOW[2]),
        (flag::ROLL, "🌀", n("Roll lanes"), theme::RAINBOW[4]),
        (flag::KICK2X, "👣", n("2x kick"), theme::RAINBOW[0]),
        (flag::MODCHART, "🧪", n("Modchart"), theme::RAINBOW[6]),
        (flag::ISSUES, "⚠", n("Chart issues"), theme::warn()),
        (flag::BROKEN, "⛔", n("Broken"), theme::bad()),
    ];
    let mut any = false;
    for (bit, icon, label, color) in features {
        if !c.has(bit) {
            continue;
        }
        any = true;
        let clickable = bit != flag::BROKEN;
        let label = t(label);
        let tip = tr!("{feature}: show charts with this", feature = label);
        let clicked = if full {
            let text = format!("{icon} {label}");
            let resp = if clickable { theme::chip(ui, &text, color) } else { theme::tag(ui, &text, color) };
            if clickable { resp.on_hover_text(&tip).clicked() } else { false }
        } else if !matches!(bit, flag::ISSUES | flag::BROKEN | flag::FORCED | flag::OPEN | flag::TAP) {
            tag_icon(ui, RichText::new(icon).color(color), &label);
            false
        } else {
            false
        };
        if clicked && clickable {
            *nav = Some(Nav::Feature(bit));
        }
    }
    any
}

fn range_row(ui: &mut egui::Ui, label: &str, r: &mut Range, tip: &str, label_w: f32) {
    let resp = ui.add_sized([label_w, 24.0], egui::Label::new(t(label)).truncate());
    if !tip.is_empty() {
        resp.on_hover_text(t(tip));
    }
    let min = ui.add_sized([64.0, 24.0], egui::TextEdit::singleline(&mut r.min).hint_text(t("Min")));
    text_menu(&min, &mut r.min);
    let max = ui.add_sized([64.0, 24.0], egui::TextEdit::singleline(&mut r.max).hint_text(t("Max")));
    text_menu(&max, &mut r.max);
    ui.end_row();
}

/// Opens the file manager with `path` selected (or its folder, where selecting isn't supported).
fn reveal(path: &std::path::Path) {
    if cfg!(target_os = "windows") {
        let native = path.to_string_lossy().replace('/', "\\");
        let _ = std::process::Command::new("explorer").arg(format!("/select,{native}")).spawn();
    } else if cfg!(target_os = "macos") {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    } else if let Some(parent) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
    }
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
        self.auto_unzip(false);
        let ctx = ui.ctx().clone();
        self.handle_drops(&ctx);
        self.keyboard(&ctx);

        // Even padding above and below the header row.
        let header_frame = egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .inner_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 8 });
        egui::Panel::top("header").frame(header_frame).show(ui, |ui| self.header(ui));
        let status_frame = egui::Frame::new()
            .fill(ui.visuals().panel_fill)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .inner_margin(egui::Margin::symmetric(14, 8));
        egui::Panel::bottom("status").frame(status_frame).show(ui, |ui| {
            ui.set_min_height(24.0);
            self.status_bar(ui);
        });
        if self.job.is_some() {
            egui::Panel::bottom("download").show(ui, |ui| self.download_bar(ui));
        }
        self.log_panel(ui);
        match self.tab {
            Tab::Browse => {
                let search = egui::Panel::top("search").show(ui, |ui| {
                    ui.add_space(8.0);
                    self.search_bar(ui);
                    self.active_filters(ui);
                    ui.add_space(8.0);
                });
                self.search_bottom = search.response.rect.bottom();
                if self.detail.is_some() {
                    // Give the panel's resize line the same look as the table's column bars.
                    let normal = ui.style().clone();
                    let w = &mut ui.visuals_mut().widgets;
                    w.noninteractive.bg_stroke.width = 3.0;
                    w.hovered.fg_stroke = w.hovered.bg_stroke;
                    w.hovered.fg_stroke.width = 4.0;
                    w.active.fg_stroke = w.active.bg_stroke;
                    w.active.fg_stroke.width = 4.0;
                    ui.style_mut().interaction.resize_grab_radius_side = 7.0;
                    egui::Panel::right("detail").resizable(true).default_size(340.0).size_range(260.0..=640.0).show(ui, |ui| {
                        ui.set_style(normal.clone());
                        self.detail_panel(ui);
                    });
                    ui.set_style(normal);
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
        // Pop-ups sit over a blurred copy of the app; they're held back for the one frame the
        // screenshot for the blur is taken.
        if self.modal_backdrop(&ctx) {
            if let Some(p) = &mut self.preview
                && !p.show(&ctx)
            {
                self.preview = None;
            }
            self.confirm_dialog(&ctx);
            self.shortcuts_window(&ctx);
        }
        self.drop_overlay(&ctx);
        self.sync_settings(&ctx);

        let building = matches!(self.cat_status, CatStatus::Building { .. });
        if self.live_searching || self.job.is_some() || building || self.track_lookup {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if self.settings.auto_extract || self.settings.auto_convert {
            ctx.request_repaint_after(Duration::from_secs(10));
        } else if !self.cat_complete && !self.filters.query.is_empty() {
            // Wake up after the typing pause so the live search fires.
            ctx.request_repaint_after(LIVE_SEARCH_DEBOUNCE);
        }
    }
}

fn ago(unix: u64) -> String {
    match now().saturating_sub(unix) {
        s if s < 90 => t("just now"),
        s if s < 90 * 60 => tr!("{n} min ago", n = s / 60),
        s if s < 36 * 3600 => tr!("{n} h ago", n = s / 3600),
        s => tr!("{n} days ago", n = s / 86400),
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
