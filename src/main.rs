//! Rainbow: a small desktop alternative to Bridge for Chorus Encore (enchor.us), which also
//! runs in the browser (see web/ and src/web.rs).

#![windows_subsystem = "windows"]

mod api;
mod catalogue;
mod chart;
mod covers;
mod drums;
mod datepicker;
mod flags;
mod filter;
mod games;
mod i18n;
mod leaderboard;
mod links;
mod net;
mod optimal;
mod players;
mod pathchart;
mod preview;
mod query;
mod scores;
mod settings;
mod sng;
mod store;
mod task;
mod theme;
mod trackhash;
#[cfg_attr(target_arch = "wasm32", path = "updater_web.rs")]
mod updater;

// Desktop only: importing zips and Rock Band packages from the songs folder.
#[cfg(not(target_arch = "wasm32"))]
mod dta;
#[cfg(not(target_arch = "wasm32"))]
mod import;
#[cfg(not(target_arch = "wasm32"))]
mod rockband;
#[cfg(not(target_arch = "wasm32"))]
mod stfs;
#[cfg(not(target_arch = "wasm32"))]
mod unzip;
#[cfg(target_arch = "wasm32")]
mod web;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use web_time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use egui::{Color32, CornerRadius, CursorIcon, RichText, Sense, vec2};
use egui_extras::{Column, TableBuilder};

use api::{Chart, Client, DIFFICULTIES, DRUM_TYPES, DRUMS, INSTRUMENTS, Manifest, flag};
use i18n::t;

/// Marks a string for translation where it's defined; it's translated where it's shown.
pub(crate) const fn n(s: &'static str) -> &'static str {
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

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    // `rainbow --update-catalogue <file>`: brings a catalogue file up to date and quits (this is
    // how the copy published with the web version is refreshed; see .github/workflows/pages.yml).
    let args: Vec<String> = std::env::args().collect();
    if let Some(k) = args.iter().position(|a| a == "--update-catalogue") {
        let Some(file) = args.get(k + 1) else {
            eprintln!("usage: rainbow --update-catalogue <file>");
            std::process::exit(2);
        };
        if let Err(e) = update_catalogue_file(std::path::Path::new(file)) {
            eprintln!("couldn't update {file}: {e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    // Opened from a rainbow:// link while Rainbow is already running: pass it over and stop.
    let link = links::from_args();
    if let Some(id) = &link
        && links::hand_over(id)
    {
        return Ok(());
    }
    std::thread::spawn(links::register);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Rainbow")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([1100.0, 600.0])
            .with_icon(eframe::icon_data::from_png_bytes(LOGO_PNG).unwrap_or_default()),
        ..Default::default()
    };
    eframe::run_native("Rainbow", options, Box::new(move |cc| Ok(Box::new(App::new(&cc.egui_ctx, link)))))
}

/// Updates (or builds) the catalogue in `file`, printing progress.
#[cfg(not(target_arch = "wasm32"))]
fn update_catalogue_file(file: &std::path::Path) -> anyhow::Result<()> {
    let mut cat = std::fs::read(file).ok().and_then(|b| Catalogue::from_json(&b)).unwrap_or_default();
    cat.file = Some(file.to_owned());
    eprintln!("{} charts to start with", cat.charts.len());
    let client = Client::new()?.with_status(Arc::new(|m| eprintln!("{m}")));
    let stop = AtomicBool::new(false);
    let added = pollster::block_on(cat.update(&client, &stop, |p| {
        let catalogue::Progress::Page { page, pages, charts, .. } = p;
        eprintln!("page {page}/{pages}: {charts} charts");
    }))?;
    anyhow::ensure!(cat.complete, "the catalogue is still incomplete");
    eprintln!("{} charts ({added} new)", cat.charts.len());
    Ok(())
}

/// The web version: reads its saved files, then runs in the page's canvas (see web/index.html).
#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;
    console_error_panic_hook::set_once();
    wasm_bindgen_futures::spawn_local(async {
        store::load().await;
        let link = web::link_from_url();
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("rainbow"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("the page has no canvas for Rainbow");
        let started = eframe::WebRunner::new()
            .start(canvas, eframe::WebOptions::default(), Box::new(move |cc| Ok(Box::new(App::new(&cc.egui_ctx, link)))))
            .await;
        web::loaded();
        if let Err(e) = started {
            web_sys::console::error_1(&e);
        }
    });
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
    /// A rainbow:// link to open (a chart id: an MD5 or the start of one).
    OpenLink(String),
    /// The web version has downloaded a font it leaves out at first (see theme::noto_sans_jp).
    #[cfg(target_arch = "wasm32")]
    FontReady,
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
    /// For the time left: when it started, and how many charts weren't downloaded already.
    started: Instant,
    to_get: usize,
}

/// A download that was still running, saved so it carries on when Rainbow is reopened.
#[derive(serde::Serialize, serde::Deserialize)]
struct Pending {
    title: String,
    charts: Vec<Chart>,
}

impl Pending {
    fn path() -> std::path::PathBuf {
        catalogue::data_dir().join("pending_download.json")
    }

    fn load() -> Option<Self> {
        store::read(&Self::path()).and_then(|b| serde_json::from_slice(&b).ok())
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_vec(self) {
            let _ = store::write(&Self::path(), &json);
        }
    }

    fn clear() {
        store::remove(&Self::path());
    }
}

struct Confirm {
    title: String,
    charts: Vec<Chart>,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Browse,
    Players,
    Leaderboards,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Col {
    Check,
    /// The favourite star.
    Fav,
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
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Reveal(usize),
    Favourite(usize),
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
    /// Measured width of the controls beside the search box.
    search_controls_w: f32,
    /// Row to scroll into view (keyboard navigation).
    scroll_to: Option<usize>,
    /// The oldest chart's year, for the date picker: (catalogue size it's from, year).
    oldest: (usize, i16),
    /// A short message at the bottom of the window: text, an optional link (label, URL), and
    /// when it appeared.
    toast: Option<(String, Option<(String, String)>, Instant)>,
    /// Open the top result in the details panel once the search has run.
    open_first: bool,
    /// The chart the details panel showed last frame (a new one starts scrolled to the top).
    detail_shown: Option<String>,
    /// The first results row drawn last frame, to keep in place while the cover column (and
    /// with it the row height) is being resized.
    first_row: usize,
    cover_dragging: bool,
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
    detail: Option<Chart>,
    preview: Option<preview::Preview>,
    /// Optimal scores shown in the details panel.
    scores: scores::Scores,
    leaderboard: leaderboard::Panel,
    /// Clone Hero player search and profiles.
    players: players::Players,
    /// The tab a profile was opened from, for the Players tab's Back button.
    prev_tab: Option<Tab>,
    /// A chart from a link, waiting to be found in the catalogue.
    pending_link: Option<String>,
    /// Bring the window to the front (a link was opened).
    focus_window: bool,
    /// How compact the header is drawn (0 = in full), and its width at each level.
    header_level: usize,
    header_widths: [f32; 4],
    /// How compact the results toolbar and status bar are drawn (0 = in full), and the width
    /// each needed at each level when last drawn (0 = not measured yet).
    toolbar_level: usize,
    toolbar_widths: [f32; 4],
    status_level: usize,
    status_widths: [f32; 4],
    /// Search history for Back and Forward: earlier and later searches, the one being shown,
    /// and when the filters last changed (a change counts once it's been left alone a moment).
    nav_back: Vec<Place>,
    nav_forward: Vec<Place>,
    nav_current: Place,
    nav_changed: Option<Instant>,
    /// New versions from GitHub.
    updates: updater::Updates,
    /// The Report an issue pop-up.
    show_report: bool,
    /// How many error lines the log had when it was last open.
    errors_seen: usize,
    scroll_to_top: bool,
    manifest: Arc<Mutex<Manifest>>,

    job: Option<Job>,
    stop: Arc<AtomicBool>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    unzip_running: Arc<AtomicBool>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    unzip_last: Option<Instant>,
    confirm: Option<Confirm>,
    /// Asking whether to quit while a download is running.
    confirm_quit: bool,
    /// The user said yes, so the next close request goes through.
    quit_confirmed: bool,
    status: Option<(String, Instant)>,
    log: Vec<String>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl App {
    fn new(ctx: &egui::Context, link: Option<String>) -> Self {
        let (tx, rx) = channel();
        // Links opened while Rainbow is running come from the copies they start.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (link_tx, link_ctx) = (tx.clone(), ctx.clone());
            links::listen(move |id| {
                let _ = link_tx.send(Msg::OpenLink(id));
                link_ctx.request_repaint();
            });
        }
        if let Some(id) = link {
            let _ = tx.send(Msg::OpenLink(id));
        }
        let status_tx = tx.clone();
        let repaint = ctx.clone();
        let client = Client::new().expect("failed to create HTTP client").with_status(Arc::new(move |m| {
            let _ = status_tx.send(Msg::Status(m));
            repaint.request_repaint();
        }));
        let settings = Settings::load();
        let default_sort = settings.default_sort;
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
            client: client.clone(),
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
            filters: Filters { sort: default_sort, ..Filters::default() },
            show_advanced: false,
            show_log: false,
            show_shortcuts: false,
            backdrop: None,
            backdrop_requested: false,
            search_controls_w: 560.0,
            scroll_to: None,
            table_gen: 0,
            table_density: None,
            first_row: 0,
            detail_shown: None,
            open_first: false,
            toast: None,
            oldest: (usize::MAX, 2010),
            cover_dragging: false,
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
            preview: None,
            scores: scores::Scores::new(client.clone()),
            leaderboard: leaderboard::Panel::new(),
            players: players::Players::new(),
            prev_tab: None,
            pending_link: None,
            focus_window: false,
            header_level: 0,
            header_widths: [0.0; 4],
            toolbar_level: 0,
            toolbar_widths: [0.0; 4],
            status_level: 0,
            status_widths: [0.0; 4],
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            nav_current: Place { tab: Tab::Browse, filters: Filters { sort: default_sort, ..Filters::default() }, profile: None },
            nav_changed: None,
            updates: updater::Updates::new(),
            show_report: false,
            errors_seen: 0,
            scroll_to_top: false,
            job: None,
            stop: Arc::new(AtomicBool::new(false)),
            unzip_running: Arc::new(AtomicBool::new(false)),
            unzip_last: None,
            confirm: None,
            confirm_quit: false,
            quit_confirmed: false,
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
                if store::exists(&catalogue::path()) {
                    app.log("Catalogue format changed; downloading it again with the new fields".to_owned());
                }
                app.update_catalogue(false);
            }
        }
        if let Some(pending) = Pending::load() {
            if app.settings.resume_downloads && !pending.charts.is_empty() {
                app.log(format!("Resuming unfinished download: {}", pending.title));
                app.start_download(pending.title, pending.charts);
            } else {
                Pending::clear();
            }
        }
        // The web version leaves the big Japanese font out of the first download.
        #[cfg(target_arch = "wasm32")]
        {
            let tx = app.tx.clone();
            let ctx = ctx.clone();
            task::spawn(move || async move {
                let http = net::Http::new(None);
                let font = match http.get(&web::page_url("fonts/NotoSansJP.ttf")).send().await {
                    Ok(resp) => resp.error_for_status().ok(),
                    Err(_) => None,
                };
                if let Some(resp) = font
                    && let Ok(bytes) = resp.bytes().await
                    && theme::WEB_NOTO_SANS_JP.set(Vec::leak(bytes)).is_ok()
                {
                    let _ = tx.send(Msg::FontReady);
                    ctx.request_repaint();
                }
            });
        }
        // Tidy up after an update, and look for a newer version.
        updater::clean_up();
        if app.settings.check_updates {
            app.updates.check(ctx);
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
            // The details panel belongs to the results: a search that leaves its chart out closes it.
            if let Some(d) = &self.detail
                && !self.results.iter().any(|&i| self.charts[i].md5 == d.md5)
            {
                self.detail = None;
            }
        }
        if self.open_first {
            if let Some(&first) = self.results.first() {
                let c = self.charts[first].clone();
                self.selected = HashSet::from([c.md5.clone()]);
                self.detail = Some(c);
                self.open_first = false;
            } else if self.cat_complete && !self.live_searching {
                self.open_first = false;
            }
        }
        self.searched = Some((self.filters.clone(), self.charts.len()));
    }

    /// While the catalogue is still downloading, also ask Encore directly for the typed query
    /// so results show up right away.
    /// Also, an MD5 or Encore link for a chart the catalogue doesn't have yet is looked up on Encore.
    fn live_search(&mut self) {
        if self.live_searching || self.live_edited.elapsed() < LIVE_SEARCH_DEBOUNCE {
            return;
        }
        // Encore gets the words without the search syntax; our own filter applies the rest.
        let query = query::plain_text(self.filters.query.trim());
        let key = format!(
            "{query}|{:?}|{:?}",
            self.filters.instrument, self.filters.difficulty
        );
        if query.is_empty() || key == self.live_sent {
            return;
        }
        let hashes: Vec<String> = query
            .split_whitespace()
            .filter_map(filter::hash_in)
            // Encore can only look up whole MD5s (a link may have just the start of one).
            .filter(|h| h.len() == 32 && !self.known.contains(h))
            .collect();
        if self.cat_complete && hashes.is_empty() {
            return;
        }
        self.live_sent = key;
        self.live_searching = true;
        let (client, tx) = (self.client.clone(), self.tx.clone());
        let (inst, diff) = (self.filters.instrument, self.filters.difficulty);
        task::spawn(move || async move {
            let found = if hashes.is_empty() {
                client.search(&query, 1, inst, diff).await.map(|r| r.charts)
            } else {
                let mut found = Vec::new();
                let mut failed = None;
                for h in &hashes {
                    match client.find_by_hash(h).await {
                        Ok(charts) => found.extend(charts),
                        Err(e) => {
                            failed = Some(e);
                            break;
                        }
                    }
                }
                failed.map_or(Ok(found), Err)
            };
            let msg = match found {
                Ok(charts) => Msg::LiveResults(charts),
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
        task::spawn(move || async move {
            let _ = tx.send(Msg::TrackHash(client.md5s_for_track_hash(&hash).await.map_err(|e| format!("{e:#}"))));
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
                file: None,
            }
        };
        self.cat_stop.store(false, Ordering::Relaxed);
        self.cat_status = CatStatus::Building { page: cat.next_page, pages: 0, charts: cat.charts.len() };
        let (client, tx, stop) = (self.client.clone(), self.tx.clone(), self.cat_stop.clone());
        task::spawn(move || async move {
            // A new visitor to the web version starts from the published copy of the catalogue.
            #[cfg(target_arch = "wasm32")]
            if cat.charts.is_empty() {
                match Catalogue::snapshot().await {
                    Ok(snapshot) => {
                        let pages = snapshot.charts.len().div_ceil(api::MAX_PER_PAGE as usize) as u32;
                        let _ = tx.send(Msg::CataloguePage { page: pages, pages, charts: snapshot.charts.len(), new: snapshot.charts.clone() });
                        cat = snapshot;
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Log(format!("Couldn't load the published catalogue ({e:#}); fetching it from Encore")));
                    }
                }
            }
            let result = cat
                .update(&client, &stop, |p| {
                    let catalogue::Progress::Page { page, pages, charts, new } = p;
                    let _ = tx.send(Msg::CataloguePage { page, pages, charts, new });
                })
                .await;
            let _ = tx.send(Msg::CatalogueDone(result.map(|added| (cat, added)).map_err(|e| format!("{e:#}"))));
        });
    }

#[cfg(not(target_arch = "wasm32"))]
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
        // The preview has its own keys (Space, arrows, Esc…).
        if self.preview.is_some() {
            return;
        }
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

    fn modal_open(&self) -> bool {
        self.preview.is_some() || self.confirm.is_some() || self.show_shortcuts || self.confirm_quit || self.updates.dialog_open() || self.show_report
    }

    /// While a pop-up (chart preview, download confirmation, shortcuts) is open, covers the app
    /// with a blurred, dimmed copy of itself. Clicking it closes the pop-up. Returns false on the frame the screenshot for the blur
    /// is taken, so the preview isn't in it.
    fn modal_backdrop(&mut self, ctx: &egui::Context) -> bool {
        if !self.modal_open() {
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
            if self.confirm_quit {
                self.confirm_quit = false;
            } else if self.show_shortcuts {
                self.show_shortcuts = false;
            } else if self.confirm.is_some() {
                self.confirm = None;
            } else if self.updates.dialog_open() {
                self.updates.close_dialog();
            } else if self.show_report {
                self.show_report = false;
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
                (format!("{ctrl}+1 / {ctrl}+2"), n("Charts / Settings")),
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
    #[cfg(not(target_arch = "wasm32"))]
    fn reporter(&self) -> impl Fn(String) + Send + 'static {
        let tx = self.tx.clone();
        move |line: String| {
            let _ = tx.send(Msg::Status(line));
        }
    }

    /// Checks the songs folder for .zip files and Rock Band CON packages every few seconds
    /// (if enabled), or right away when forced.
    fn auto_unzip(&mut self, force: bool) {
        #[cfg(target_arch = "wasm32")]
        let _ = force;
        #[cfg(not(target_arch = "wasm32"))]
        self.auto_unzip_native(force);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn auto_unzip_native(&mut self, force: bool) {
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
        std::thread::spawn(move || {
            import::process_folder(&dir, &opts, &report);
            running.store(false, Ordering::SeqCst);
        });
    }

    /// Files dropped onto the window: convert CON files, import zips.
    fn handle_drops(&mut self, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
        let _ = ctx;
        #[cfg(not(target_arch = "wasm32"))]
        self.handle_drops_native(ctx);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn handle_drops_native(&mut self, ctx: &egui::Context) {
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        if dropped.is_empty() {
            return;
        }
        let (opts, dir, report) = (self.import_options(), self.settings.dir.clone(), self.reporter());
        std::thread::spawn(move || import::import_dropped(dropped, &dir, &opts, &report));
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
        let to_get = {
            let m = self.manifest.lock().unwrap();
            charts.iter().filter(|c| !m.contains(&c.md5)).count()
        };
        self.job = Some(Job {
            title: title.clone(),
            total: charts.len(),
            checked: 0,
            downloaded: 0,
            failed: 0,
            file: None,
            started: Instant::now(),
            to_get,
        });
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
        task::spawn(move || async move {
            // Already-downloaded charts are skipped, so resuming just runs the same list again.
            let pending = Pending { title, charts };
            pending.save();
            let charts = pending.charts;
            for (k, chart) in charts.iter().enumerate() {
                if w.stopped() {
                    break;
                }
                if w.download(chart).await {
                    w.wait(delay).await;
                } else if k % 500 == 499 {
                    // (Skipping a long run of charts already downloaded mustn't hold up the web page.)
                    task::breathe().await;
                }
            }
            w.manifest.lock().unwrap().flush();
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

    fn handle_messages(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = ctx;
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
                    }
                }
                Msg::Status(s) => {
                    self.log(s.clone());
                    self.status = Some((s, Instant::now()));
                }
                Msg::Log(line) => self.log(line),
                #[cfg(target_arch = "wasm32")]
                Msg::FontReady => {
                    theme::install_fonts(ctx, self.settings.font, &i18n::current());
                }
                Msg::OpenLink(id) => {
                    self.log(format!("Opening shared chart {id}"));
                    self.pending_link = Some(id);
                    self.tab = Tab::Browse;
                    self.preview = None;
                    self.focus_window = true;
                }
                Msg::JobDone => {
                    // Quitting mid-download leaves it saved for next time; finishing or Stop clears it.
                    if !self.quit_confirmed {
                        Pending::clear();
                    }
                    if let Some(job) = self.job.take() {
                        let how = if self.stop.load(Ordering::Relaxed) { "Stopped" } else { "Finished" };
                        self.log(format!(
                            "{how}: {}: {} downloaded, {} in library, {} failed",
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
        // As fully as fits: then "Renzora Studios" becomes just its logo and the progress bar
        // shortens, then the RAINBOW title goes (the logo stays).
        let full = ui.available_width();
        self.header_level = pick_level(&self.header_widths, full - 8.0).min(2);
        let level = self.header_level;
        crate::theme::hrow(ui, |ui| {
            let start = ui.cursor();
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::new(logo).fit_to_exact_size(vec2(36.0, 36.0)).corner_radius(CornerRadius::same(9)));
            }
            // The gradient drifts slowly through the word (once every 12 s), unless motion is
            // reduced; a few repaints a second are enough for something this slow.
            let phase = if self.settings.reduce_motion {
                0.0
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(60));
                (ui.input(|i| i.time) / 12.0).fract() as f32
            };
            if level < 2 {
                ui.label(theme::title(24.0, phase));
            }
            ui.add_space(18.0);
            for (tab, label) in [
                (Tab::Browse, n("🎵  Charts")),
                (Tab::Players, n("👤  Players")),
                (Tab::Leaderboards, n("🏆  Leaderboards")),
                (Tab::Settings, n("⚙  Settings")),
            ] {
                // (The web version needs a proxy for Clone Hero's API; see players::API_URL.)
                if matches!(tab, Tab::Players | Tab::Leaderboards) && !players::available() {
                    continue;
                }
                let on = self.tab == tab;
                // Each tab has its own colour: filled when it's open, its label tinted otherwise.
                let color = match tab {
                    Tab::Browse => theme::accent(),
                    Tab::Players => Color32::from_rgb(38, 150, 104),
                    Tab::Leaderboards => Color32::from_rgb(196, 122, 20),
                    Tab::Settings => Color32::from_rgb(92, 104, 136),
                };
                if tab_button(ui, &t(label), on, color).clicked() {
                    self.tab = tab;
                }
            }
            let left = (ui.cursor().left() - start.left()).abs().max((start.right() - ui.cursor().right()).abs());
            let right = ui
                .with_layout(crate::theme::trailing(), |ui| {
                    let from = ui.cursor();
                    credit(ui, self.renzora.as_ref(), 26.0, level >= 1);
                    ui.add_space(12.0);
                    if let Some(job) = &self.job {
                        let bar_w = if level == 0 { 140.0 } else { 80.0 };
                        ui.add(crate::theme::Bar::new(job.checked as f32 / job.total.max(1) as f32).desired_width(bar_w).show_percentage());
                        ui.label(RichText::new(job.downloaded.to_string()).color(theme::good()));
                        ui.add(download_icon(14.0, theme::good()));
                    }
                    (from.right() - ui.cursor().right()).abs().max((ui.cursor().left() - from.left()).abs())
                })
                .inner;
            self.header_widths[level] = left + right + 24.0;
        });
        
    }

    // ───────────────────────────── Browse tab ─────────────────────────────

    fn search_bar(&mut self, ui: &mut egui::Ui) {
        crate::theme::hrow(ui, |ui| {
            // Back and Forward through earlier searches (greyed out when there's nowhere to go).
            let back = self.nav_back.len() + usize::from(self.place() != self.nav_current) > 0;
            if nav_arrow(ui, true, back).on_hover_text(t("Back (Alt+←)")).clicked() {
                self.go_back(ui.ctx());
            }
            if nav_arrow(ui, false, !self.nav_forward.is_empty()).on_hover_text(t("Forward (Alt+→)")).clicked() {
                self.go_forward(ui.ctx());
            }
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.filters.query)
                    .id(egui::Id::new("search"))
                    .hint_text(t("🔍  Search songs, artists, albums, charters…  (Ctrl+F)"))
                    .font(egui::FontId::proportional(16.0))
                    // Extra room on the right for the clear button.
                    .margin(egui::Margin {
                        left: 10,
                        right: 34,
                        top: 7,
                        bottom: 7,
                    })
                    .desired_width(
                        (ui.available_width() - self.search_controls_w - 16.0).max(200.0),
                    ),
            );
            // Measure the controls after the search box, so the box leaves exactly enough room.
            let controls_start = ui.cursor().left();
            if text_menu(&resp, &mut self.filters.query) || resp.changed() {
                self.live_edited = Instant::now();
            }
            if !self.filters.query.is_empty() && clear_button(ui, resp.rect, "charts") {
                self.filters.query.clear();
                self.live_edited = Instant::now();
                resp.request_focus();
            }
            // Shown as soon as it's hovered, without the usual tooltip delay.
            let help = ui
                .add(egui::Label::new(RichText::new("❔").size(16.0)).sense(Sense::hover()))
                .on_hover_cursor(CursorIcon::Help);
            if help.hovered() {
                help.show_tooltip_ui(search_syntax_help);
            }
            // Earlier searches, newest first.
            let mut again = None;
            let recent = &mut self.settings.recent_searches;
            let history = ui.add_enabled_ui(!recent.is_empty(), |ui| {
                ui.menu_button(RichText::new("🕘").size(16.0), |ui| {
                    // A fixed-size list with the app's scroll bar (Clear history stays below it).
                    const W: f32 = 420.0;
                    let rows = recent.len() as f32 * (ui.spacing().interact_size.y + ui.spacing().item_spacing.y);
                    let height = rows.min(300.0);
                    ui.allocate_ui(vec2(W, height), |ui| {
                        ui.set_min_size(vec2(W, height));
                        crate::fancy_scroll(ui, "recent_searches", |ui| {
                            let row_w = ui.available_width();
                            for q in recent.iter() {
                                let item = egui::Button::new(q.as_str()).frame_when_inactive(false).truncate().min_size(vec2(row_w, 0.0));
                                if ui.add(item).on_hover_text(q.as_str()).clicked() {
                                    again = Some(q.clone());
                                    ui.close();
                                }
                            }
                        });
                    });
                    ui.separator();
                    if ui.button(t("🗑  Clear history")).clicked() {
                        recent.clear();
                        ui.close();
                    }
                })
            });
            history.response.on_hover_text(t("Previous searches"));
            if let Some(q) = again {
                // A past search starts fresh: just its text, with every other filter cleared
                // (the sort order stays).
                let f = &mut self.filters;
                *f = Filters { query: q, sort: f.sort, descending: f.descending, ..Filters::default() };
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
            }).response.on_hover_cursor(CursorIcon::PointingHand);
            let diff_text = difficulty_label(self.filters.difficulty);
            egui::ComboBox::from_id_salt("difficulty").selected_text(diff_text).width(130.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.filters.difficulty, None, difficulty_label(None));
                for k in 0..DIFFICULTIES.len() as u8 {
                    ui.selectable_value(&mut self.filters.difficulty, Some(k), difficulty_label(Some(k)));
                }
            }).response.on_hover_cursor(CursorIcon::PointingHand);
            if self.filters.instrument == Some(DRUMS) {
                let dt = self.filters.drum_type.map_or(t("Any Drum Type"), |d| t(DRUM_TYPES[d as usize]));
                egui::ComboBox::from_id_salt("drumtype").selected_text(dt).width(110.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.filters.drum_type, None, t("Any Drum Type"));
                    for (k, name) in DRUM_TYPES.iter().enumerate() {
                        ui.selectable_value(&mut self.filters.drum_type, Some(k as u8), t(name));
                    }
                }).response.on_hover_cursor(CursorIcon::PointingHand);
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
        crate::theme::hrow(ui, |ui| {
            let f = &mut self.filters;
            let all = !f.no_video && !f.no_broken && !f.legacy && f.game.is_none() && f.favourites.is_none() && f.advanced_count() == 0;
            if pill(ui, all, &t("🌈  All songs")).on_hover_text(t("Clear all filters")).clicked() {
                f.no_video = false;
                f.no_broken = false;
                f.legacy = false;
                f.game = None;
                f.favourites = None;
                f.clear_advanced();
            }
            // Only favourites (kept up to date as stars are added and removed).
            let favs = &self.settings.favourites;
            if f.favourites.as_ref().is_some_and(|shown| shown != favs) {
                f.favourites = Some(favs.clone());
            }
            if pill(ui, f.favourites.is_some(), &tr!("⭐  Favourites ({count})", count = favs.len())).on_hover_text(t("Show only your favourite charts")).clicked() {
                f.favourites = if f.favourites.is_some() { None } else { Some(favs.clone()) };
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
            }).response.on_hover_cursor(CursorIcon::PointingHand);
        });
        self.user_searches(ui);
        self.remember_search();
    }

    /// `user:name` searches: looks the players up and lists which charts they've played for the
    /// filter. Says how each lookup is going; returns whether there were any.
    fn user_searches(&mut self, ui: &mut egui::Ui) -> bool {
        let (terms, _) = query::parse(&self.filters.query);
        let names: Vec<String> = terms
            .iter()
            .filter_map(|t| if let query::Cond::User(n) = &t.cond { Some(n.to_lowercase()) } else { None })
            .collect();
        self.filters.played.retain(|k, _| names.contains(k));
        if names.is_empty() {
            return false;
        }
        crate::theme::hrow_wrapped(ui, |ui| {
            for name in &names {
                match self.players.played(ui.ctx(), name) {
                    None => {
                        ui.spinner();
                        ui.weak(tr!("Looking up {name}…", name = name));
                    }
                    Some(Err(e)) => {
                        ui.colored_label(theme::bad(), tr!("Couldn't look up {name}: {error}", name = name, error = e));
                    }
                    Some(Ok(None)) => {
                        self.filters.played.entry(name.clone()).or_default();
                        ui.colored_label(theme::warn(), tr!("No Clone Hero player called {name}", name = name));
                    }
                    Some(Ok(Some((shown, charts)))) => {
                        if !self.filters.played.contains_key(name) {
                            self.filters.played.insert(name.clone(), charts.clone());
                        }
                        ui.label(RichText::new(format!("👤 {shown}")).strong());
                        ui.weak(t("Charts from their latest scores and most played songs (the leaderboards don't share their full history)"));
                    }
                }
            }
        });
        true
    }

    /// Moves a column to where its header was dropped (at pointer `x`), and remembers the order.
    fn move_column(&mut self, col: Col, x: f32, headers: &[(Col, egui::Rect)]) {
        // Shown columns in their on-screen order, left to right.
        let mut shown: Vec<(Col, egui::Rect)> = headers.iter().filter(|h| !matches!(h.0, Col::Check)).copied().collect();
        shown.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
        let Some(from) = shown.iter().position(|h| h.0 == col) else { return };
        // The insertion point: before the first header whose middle is right of the pointer.
        let mut to = shown.iter().position(|h| h.1.center().x > x).unwrap_or(shown.len());
        if to > from {
            to -= 1;
        }
        let moved = shown.remove(from).0;
        shown.insert(to.min(shown.len()), (moved, egui::Rect::NOTHING));
        let mut order: Vec<Col> = shown.into_iter().map(|h| h.0).collect();
        if i18n::rtl() {
            order.reverse(); // the table is mirrored
        }
        // Hidden columns keep their place after the shown ones.
        let mut names: Vec<String> = order.iter().map(|c| format!("{c:?}")).collect();
        for c in [Col::Name, Col::Artist, Col::Album, Col::Genre, Col::Year, Col::Charter, Col::Length, Col::Difficulty, Col::Uploaded, Col::Tags] {
            let name = format!("{c:?}");
            if !names.contains(&name) {
                names.push(name);
            }
        }
        self.settings.column_order = names;
    }

    /// The year of the oldest modified date in the catalogue (worked out when it changes size).
    fn oldest_year(&mut self) -> i16 {
        if self.oldest.0 != self.charts.len() {
            let year = self.charts.iter().filter_map(|c| c.modified.as_deref()?.get(..4)?.parse::<i16>().ok()).min().unwrap_or(2010);
            self.oldest = (self.charts.len(), year);
        }
        self.oldest.1
    }

    /// Where the app is, for Back and Forward: the tab, the search, and the player profile.
    fn place(&self) -> Place {
        Place { tab: self.tab, filters: nav_key(&self.filters), profile: self.players.current_profile() }
    }

    /// Records where the app is for Back and Forward: a new tab or profile straight away, a new
    /// search once it's been left alone briefly (so typing a word is one step, not one per
    /// letter). Also handles the mouse's back/forward buttons and Alt+arrows.
    fn track_history(&mut self, ctx: &egui::Context) {
        let now = self.place();
        if now == self.nav_current {
            self.nav_changed = None;
        } else {
            let moved = now.tab != self.nav_current.tab || now.profile != self.nav_current.profile;
            let since = *self.nav_changed.get_or_insert_with(Instant::now);
            if moved || since.elapsed() >= Duration::from_millis(800) {
                self.nav_back.push(std::mem::replace(&mut self.nav_current, now));
                if self.nav_back.len() > 100 {
                    self.nav_back.remove(0);
                }
                self.nav_forward.clear();
                self.nav_changed = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(850));
            }
        }
        let (back, forward) = ctx.input(|i| {
            (
                i.pointer.button_clicked(egui::PointerButton::Extra1) || i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft),
                i.pointer.button_clicked(egui::PointerButton::Extra2) || i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight),
            )
        });
        if back {
            self.go_back(ctx);
        } else if forward {
            self.go_forward(ctx);
        }
    }

    fn go_back(&mut self, ctx: &egui::Context) {
        let now = self.place();
        if now != self.nav_current {
            // Unsettled changes (still typing): back to the last settled place.
            self.nav_forward.push(now);
            self.restore(ctx, self.nav_current.clone());
        } else if let Some(prev) = self.nav_back.pop() {
            self.nav_forward.push(std::mem::replace(&mut self.nav_current, prev.clone()));
            self.restore(ctx, prev);
        }
    }

    fn go_forward(&mut self, ctx: &egui::Context) {
        let Some(next) = self.nav_forward.pop() else { return };
        self.nav_back.push(std::mem::replace(&mut self.nav_current, next.clone()));
        self.restore(ctx, next);
    }

    /// Goes back to a place from the history (lookups already made, like `user:` players, are kept).
    fn restore(&mut self, ctx: &egui::Context, place: Place) {
        let (played, popular) = (std::mem::take(&mut self.filters.played), std::mem::take(&mut self.filters.popular));
        self.filters = Filters { played, popular, ..place.filters };
        self.tab = place.tab;
        self.players.return_to(ctx, place.profile);
        self.nav_changed = None;
        self.live_edited = Instant::now();
    }

    /// Adds the search text to the history once it's been left alone for a few seconds.
    fn remember_search(&mut self) {
        let query = self.filters.query.trim();
        if query.is_empty() || self.live_edited.elapsed() < Duration::from_secs(3) {
            return;
        }
        let recent = &mut self.settings.recent_searches;
        if recent.first().is_some_and(|q| q == query) {
            return;
        }
        recent.retain(|q| q != query);
        recent.insert(0, query.to_owned());
        recent.truncate(15);
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
        // Inset a little from the window's sides, so both rounded corners show the same.
        let inset = 8.0;
        let width = screen.width() - 2.0 * inset;
        let max_height = (screen.bottom() - self.search_bottom - 70.0).max(150.0);
        egui::Area::new(egui::Id::new("advanced_search"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(screen.left() + inset, self.search_bottom))
            .show(ctx, |ui| {
                // An Area's room is last frame's size; let it grow so the cards never get
                // clipped into a scroll area they don't need.
                ui.set_max_height(screen.bottom() - self.search_bottom);
                // Full width, flush under the search bar, like a drop-down sheet.
                let radius = CornerRadius::ZERO;
                let frame = egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .shadow(egui::epaint::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(90) })
                    .corner_radius(radius)
                    .inner_margin(egui::Margin::symmetric(16, 12));
                let shown = frame.show(ui, |ui| {
                    ui.set_width(width - 32.0);
                    crate::theme::hrow(ui, |ui| {
                        ui.label(RichText::new(t("🎛  Advanced search")).size(17.0).strong().color(theme::accent_hover()));
                        ui.weak(trn!(self.results.len(), "{count} result", "{count} results", count = self.results.len()));
                        ui.with_layout(crate::theme::trailing(), |ui| {
                            if ui.button(t("✖  Close")).on_hover_text(t("Esc")).clicked() {
                                self.show_advanced = false;
                            }
                            if ui.button(t("↺  Reset")).on_hover_text(t("Clear all advanced filters")).clicked() {
                                self.filters.clear_advanced();
                            }
                        });
                    });
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical()
                        .max_height(max_height)
                        .auto_shrink([false, true])
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                        .show(ui, |ui| {
                        self.advanced_search(ui, width - 32.0 - 16.0);
                    });
                });
                // A border along the bottom, where the panel ends over the results.
                let r = shown.response.rect;
                let stroke = egui::Stroke::new(2.0, ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(1.6));
                ui.painter().line_segment([r.left_bottom() + vec2(0.0, -1.0), r.right_bottom() + vec2(0.0, -1.0)], stroke);
            });
    }

    fn advanced_search(&mut self, ui: &mut egui::Ui, avail: f32) {
        const GAP: f32 = 10.0;
        const MIN_CARD: f32 = 280.0;
        let columns = (((avail + GAP) / (MIN_CARD + GAP)).floor() as usize).clamp(1, 4);
        let card_w = (avail - GAP * (columns - 1) as f32) / columns as f32;
        let mut lookup = false;
        let sections = [Section::Text, Section::Ranges, Section::Chart, Section::Features];
        for (r, row) in sections.chunks(columns).enumerate() {
            if r > 0 {
                ui.add_space(GAP);
            }
            // Cards in a row share the height of the tallest one (measured last frame).
            let id = ui.id().with(("adv_row", columns, r));
            let row_h = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
            let mut tallest: f32 = 0.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for section in row {
                    let (wants_lookup, h) = self.advanced_card(ui, *section, card_w, row_h);
                    lookup |= wants_lookup;
                    tallest = tallest.max(h);
                }
            });
            if (tallest - row_h).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(id, tallest));
                ui.ctx().request_repaint();
            }
        }
        if lookup && !self.track_lookup {
            self.lookup_track_hash();
        }
    }

    /// One advanced-search card, at least `min_height` tall. Returns whether a track hash
    /// lookup was requested, and the height its contents need.
    fn advanced_card(&mut self, ui: &mut egui::Ui, section: Section, width: f32, min_height: f32) -> (bool, f32) {
        let this_year = jiff::Zoned::now().year();
        let oldest_year = self.oldest_year().min(this_year);
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
        let shown = egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(inner);
                let natural = crate::theme::vcol(ui, |ui| {
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
                                // These fields are the search box's `artist:` (etc.) terms: typing here
                                // writes them into it, and terms typed there show here.
                                for (k, label) in TEXT_FIELDS.iter().enumerate() {
                                    let id = egui::Id::new(("adv_text_field", k));
                                    let field = &mut f.text[k];
                                    if !ui.memory(|m| m.has_focus(id)) {
                                        let (value, exact, exclude) = query::field_in_query(&f.query, k);
                                        if value != field.value.trim() || exact != field.exact || exclude != field.exclude {
                                            (field.value, field.exact, field.exclude) = (value, exact, exclude);
                                        }
                                    }
                                    let resp = ui.add_sized([field_w, 24.0], egui::TextEdit::singleline(&mut field.value).id(id).hint_text(t(label)));
                                    let mut changed = text_menu(&resp, &mut field.value) || resp.changed();
                                    changed |= ui.vertical_centered(|ui| theme::checkbox(ui, &mut field.exact, "").changed()).inner;
                                    changed |= ui.vertical_centered(|ui| theme::checkbox(ui, &mut field.exclude, "").changed()).inner;
                                    ui.end_row();
                                    if changed {
                                        let (value, exact, exclude) = (field.value.trim().to_owned(), field.exact, field.exclude);
                                        f.query = query::with_field(&f.query, k, &value, exact, exclude);
                                    }
                                }
                            });
                        }
                        Section::Ranges => {
                            let label_w = inner - 2.0 * 64.0 - 16.0;
                            let this_year = jiff::Zoned::now().year() as f64;
                            // Each range: its boxes, then a slider underneath (the boxes win).
                            for (label, range, tip, lo, hi, step) in [
                                (n("⏱  Length (min)"), &mut f.length_min, "", 0.0, 20.0, 0.5),
                                (n("🔥  Intensity"), &mut f.intensity, n("Also known as chart difficulty. Typically a number between 0 and 6."), 0.0, 6.0, 1.0),
                                (n("⚡  Average NPS"), &mut f.avg_nps, n("Average notes per second, for the chosen instrument/difficulty."), 0.0, 20.0, 0.5),
                                (n("🚀  Max NPS"), &mut f.max_nps, n("Highest notes per second, for the chosen instrument/difficulty."), 0.0, 40.0, 1.0),
                                (n("📅  Year"), &mut f.year, "", 1960.0, this_year, 1.0),
                            ] {
                                crate::theme::hrow(ui, |ui| range_row(ui, label, range, tip, label_w));
                                range_slider(ui, label, range, lo, hi, step);
                                ui.add_space(4.0);
                            }
                        }
                        Section::Chart => {
                            let field_w = inner - 110.0;
                            egui::Grid::new("adv_misc").num_columns(2).spacing([8.0, 5.0]).show(ui, |ui| {
                                ui.label(t("Modified after")).on_hover_text(t("The date of the last time this chart was modified in Google Drive."));
                                // A calendar, from the year of the oldest chart in the catalogue.
                                datepicker::date_field(ui, "modified_after", &mut f.modified_after, oldest_year..=this_year, field_w);
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
                                crate::theme::hrow(ui, |ui| {
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
                            crate::theme::hrow_wrapped(ui, |ui| {
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
                    ui.min_rect().height()
                });
                let natural = natural.inner;
                ui.set_min_height(min_height.max(natural));
                natural
            });
        (lookup, shown.inner)
    }

    /// The fixed status bar at the bottom: catalogue progress, rate-limit notices, the log
    /// and links to Encore.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        // Drawn as fully as fits (like the results toolbar): links lose their names, then the
        // less important bits go.
        let full_width = ui.available_width();
        self.status_level = pick_level(&self.status_widths, full_width - 8.0).min(3);
        let level = self.status_level;
        crate::theme::hrow(ui, |ui| {
            // The links on the right go first, so the catalogue progress and status message get
            // only the space that's left and shorten instead of running underneath them.
            ui.with_layout(crate::theme::trailing(), |ui| {
                let start = ui.cursor();
                if level < 2 {
                    ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).monospace().weak())
                        .on_hover_text(t("Rainbow version"));
                }
                self.updates.status_bar(ui);
                ui.separator();
                let named = level == 0;
                web_link(
                    ui,
                    Some(egui::include_image!("../assets/icons/github.svg")),
                    if named { "GitHub" } else { "" },
                    "https://github.com/renzora/rainbow",
                    n("Rainbow's source code, releases and issue tracker"),
                );
                // Right to left, so they read Encore, Patreon, Discord, GitHub.
                web_link(
                    ui,
                    Some(egui::include_image!("../assets/icons/discord.svg")),
                    if named { "Discord" } else { "" },
                    "https://discord.gg/x6KHzEe2j2",
                    n("Join the Renzora Discord: news, help and feedback for Rainbow"),
                );
                web_link(
                    ui,
                    Some(egui::include_image!("../assets/icons/patreon.svg")),
                    if named { "Patreon" } else { "" },
                    "https://www.patreon.com/cw/ChorusEncore701?l=en-GB",
                    n("Support Chorus Encore: it's donation-funded and pays for the servers and file hosting Rainbow uses"),
                );
                web_link(ui, None, if named { "🌐 Encore" } else { "🌐" }, "https://www.enchor.us", n("Chorus Encore, the chart search engine Rainbow is built on"));
                ui.separator();
                self.language_picker(ui);
                ui.separator();
                let report = if level == 0 { t("🐞 Report an issue") } else { "🐞".to_owned() };
                if ui.button(report).on_hover_text(t("Get help, or report a problem with Rainbow")).clicked() {
                    self.show_report = true;
                }
                // Errors since the log was last looked at, as a red count beside its button.
                let errors = self.log.iter().filter(|l| log_level(l) == LogLevel::Error).count();
                if self.show_log {
                    self.errors_seen = errors;
                }
                let unseen = errors.saturating_sub(self.errors_seen);
                if unseen > 0 {
                    ui.label(RichText::new(format!("✖ {unseen}")).color(theme::bad()).strong())
                        .on_hover_text(tr!("{count} new error(s): open the log to see them", count = unseen));
                }
                let log = if level == 0 { tr!("📜 Log ({count})", count = self.log.len()) } else { "📜".to_owned() };
                if ui
                    .selectable_label(self.show_log, log)
                    .on_hover_text(t("Show activity and errors"))
                    .clicked()
                {
                    self.show_log = !self.show_log;
                }
                let right = (start.right() - ui.cursor().right()).abs().max((ui.cursor().left() - start.left()).abs());
                let left = ui.with_layout(if i18n::rtl() { egui::Layout::right_to_left(egui::Align::Center) } else { egui::Layout::left_to_right(egui::Align::Center) }, |ui| {
                    match &self.cat_status {
                        CatStatus::Building { page, pages, charts } => {
                            ui.spinner();
                            if self.cat_complete {
                                ui.label(tr!("Checking for new charts… {count} charts", count = charts));
                            } else {
                                if *pages > 0 {
                                    ui.add(
                                        crate::theme::Bar::new(*page as f32 / (*pages).max(1) as f32)
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
                            ui.label(tr!("📚 {count} charts", count = self.charts.len()))
                                .on_hover_text(tr!("updated {when}", when = ago(self.cat_updated)));
                            if level < 2 {
                                ui.weak(tr!("updated {when}", when = ago(self.cat_updated)));
                            }
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
                    // What this side needs (a status message just shortens to fit).
                    let needed = ui.min_rect().width();
                    if let Some((s, at)) = &self.status
                        && at.elapsed() < Duration::from_secs(60)
                    {
                        ui.separator();
                        let label = egui::Label::new(RichText::new(format!("⏳ {s}")).color(theme::warn())).truncate();
                        ui.add(label).on_hover_text(s);
                    }
                    needed
                });
                self.status_widths[level] = right + left.inner + 16.0;
            });
        });
    }

    /// The interface language picker in the status bar: the current language's flag and name,
    /// opening a menu of every language (each shown in itself, with its flag).
    fn language_picker(&mut self, ui: &mut egui::Ui) {
        let languages = &self.languages;
        let shown_code = if self.settings.language == "auto" { i18n::system_language() } else { self.settings.language.clone() };
        let name = languages.iter().find(|l| l.code == shown_code).map_or("English", |l| l.name.as_str());
        let current = if self.settings.language == "auto" { tr!("Auto ({language})", language = name) } else { name.to_owned() };
        let flag_size = vec2(19.0, 14.0);
        let flag = |ui: &egui::Ui, code: &str| flags::flag(ui.ctx(), code).map(|tex| egui::Image::new(&tex).fit_to_exact_size(flag_size).corner_radius(2));
        // Just the flag when the status bar is short of room.
        let label = if self.status_level >= 2 { "⏷".to_owned() } else { format!("{current}  ⏷") };
        let button = match flag(ui, &shown_code) {
            Some(img) => egui::Button::image_and_text(img, label),
            None => egui::Button::new(format!("🌐 {label}")),
        };
        let mut picked = None;
        let (resp, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
            // A fixed size, with the same scroll bar as the song list.
            const W: f32 = 270.0;
            let height = (ui.ctx().content_rect().height() - 120.0).clamp(160.0, 420.0);
            // Room between each flag and its name.
            ui.spacing_mut().icon_spacing = 10.0;
            ui.allocate_ui(vec2(W, height), |ui| {
                ui.set_min_size(vec2(W, height));
                crate::fancy_scroll(ui, "language_menu", |ui| {
                let row_w = ui.available_width();
                let auto = egui::Button::new(format!("🌐  {}", t("Auto (system language)")))
                    .selected(self.settings.language == "auto")
                    .frame_when_inactive(false)
                    .min_size(vec2(row_w, 0.0));
                if ui.add(auto).clicked() {
                    picked = Some("auto".to_owned());
                }
                ui.separator();
                for lang in languages {
                    let label = if lang.name == lang.english_name { lang.name.clone() } else { format!("{}  ·  {}", lang.name, lang.english_name) };
                    let on = self.settings.language == lang.code;
                    let button = match flag(ui, &lang.code) {
                        Some(img) => egui::Button::image_and_text(img, label),
                        None => egui::Button::new(label),
                    };
                    let button = button.selected(on).frame_when_inactive(false).min_size(vec2(row_w, 0.0));
                    if ui.add(button).clicked() {
                        picked = Some(lang.code.clone());
                    }
                }
                });
            });
            if picked.is_some() {
                ui.close();
            }
        });
        resp.on_hover_text(t("Language"));
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
        crate::theme::hrow(ui, |ui| {
            // Titles like "Entire Encore library" are translated (a resumed download keeps the
            // title it started with).
            ui.add(download_icon(15.0, ui.visuals().strong_text_color()));
            ui.strong(t(&job.title));
            ui.label(tr!("{done} / {total} checked", done = job.checked, total = job.total));
            ui.colored_label(theme::good(), tr!("✔ {count} downloaded", count = job.downloaded));
            ui.weak(tr!("{count} in library", count = job.checked - job.downloaded - job.failed));
            if job.failed > 0 {
                ui.colored_label(theme::bad(), tr!("✖ {count} failed", count = job.failed));
            }
            let stop = egui::Button::new(RichText::new(t("Stop")).color(Color32::WHITE).strong()).fill(theme::bad()).small();
            if ui.add(stop).clicked() {
                self.stop.store(true, Ordering::Relaxed);
            }
            // Time left, at the far end: the average time per chart fetched so far, times the
            // charts still to get (ones already downloaded are skipped in no time).
            let fetched = job.downloaded + job.failed;
            let left = job.to_get.saturating_sub(fetched);
            if fetched >= 2 && left > 0 {
                let secs = job.started.elapsed().as_secs_f64() / fetched as f64 * left as f64;
                ui.with_layout(crate::theme::trailing(), |ui| {
                    ui.weak(tr!("⏳ about {time} left ({count} to go)", time = duration_words(secs), count = preview::thousands(left as u64)))
                        .on_hover_text(t("Estimated from how long each download has taken so far"));
                });
            }
        });
        // One bar for the whole job, naming the song being downloaded (the last one stays
        // named until the next starts, so the text doesn't flicker).
        let frac = job.checked as f32 / job.total.max(1) as f32;
        let percent = if frac < 0.1 { format!("{:.1}%", frac * 100.0) } else { format!("{:.0}%", frac * 100.0) };
        let text = match &job.file {
            Some((title, done, _)) => format!("{percent}   ·   {title}  ({:.1} MB)", *done as f64 / 1_048_576.0),
            None => percent,
        };
        ui.add(crate::theme::Bar::new(frac).text(text).fill(theme::accent()));
        ui.add_space(4.0);
    }

    /// The log slides up above the status bar; drag its top edge to resize it.
    fn log_panel(&mut self, ui: &mut egui::Ui, normal: &std::sync::Arc<egui::Style>) {
        let log = &self.log;
        let mut close = false;
        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(220.0)
            .min_size(80.0)
            .max_size(600.0)
            .drag_to_open(false)
            .show_collapsible(ui, &mut self.show_log, |ui| {
                ui.set_style(normal.clone());
                ui.add_space(6.0);
                crate::theme::hrow(ui, |ui| {
                    ui.label(RichText::new(tr!("📜 Log ({count})", count = log.len())).strong());
                    ui.with_layout(crate::theme::trailing(), |ui| {
                        if ui.small_button(t("✖ Close")).on_hover_text(t("Hide the log")).clicked() {
                            close = true;
                        }
                        if ui.small_button(format!("📋  {}", t("Copy all"))).on_hover_text(t("Copy the whole log")).clicked() {
                            ui.ctx().copy_text(log.join("\n"));
                        }
                    });
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    for line in log {
                        // An icon column for the kind of line; errors and warnings are coloured too.
                        let kind = log_level(line);
                        crate::theme::hrow(ui, |ui| {
                            let (rect, _) = ui.allocate_exact_size(vec2(22.0, 16.0), Sense::hover());
                            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, kind.icon(), egui::FontId::proportional(13.0), kind.color(ui));
                            let text = RichText::new(line).monospace();
                            let text = if matches!(kind, LogLevel::Error | LogLevel::Warning) { text.color(kind.color(ui)) } else { text };
                            // One line each; the full text shows on hover when it's cut short.
                            ui.add(egui::Label::new(text).truncate());
                        });
                    }
                });
            });
        if close {
            self.show_log = false;
        }
    }

    /// The columns to show at this table width: the ones enabled in Settings, minus the least
    /// important ones while they don't fit. Name then stretches to fill the rest.
    fn columns(&self, avail: f32) -> Vec<Col> {
        let c = &self.settings.columns;
        let mut cols = vec![Col::Check, Col::Fav];
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
        // The order set by dragging headers (the tick box and cover always come first).
        let order = &self.settings.column_order;
        if !order.is_empty() {
            let rank = |c: &Col| order.iter().position(|o| *o == format!("{c:?}")).unwrap_or(usize::MAX);
            let fixed = cols.iter().take_while(|c| matches!(c, Col::Check | Col::Fav | Col::Cover)).count();
            cols[fixed..].sort_by_key(rank);
        }

        let width = |cols: &[Col]| cols.iter().map(|&col| self.column_width(col) + 8.0).sum::<f32>();
        for drop in [Col::Uploaded, Col::Year, Col::Genre, Col::Tags, Col::Length, Col::Album, Col::Charter, Col::Difficulty] {
            if width(&cols) <= avail {
                break;
            }
            cols.retain(|&c| c != drop);
        }
        // Right to left: the tick boxes and names start on the right.
        if i18n::rtl() {
            cols.reverse();
        }
        cols
    }

    /// Starting width of each column (Name's is its minimum; it grows to fill).
    fn column_width(&self, col: Col) -> f32 {
        match col {
            Col::Check => 36.0,
            Col::Fav => 26.0,
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
        let mut f = Filters { sort: self.filters.sort, descending: self.filters.descending, ..Default::default() };
        match nav {
            Nav::Text(k, value, exact) => f.query = query::with_field("", k, &value, exact, false),
            Nav::Album { artist, album } => {
                f.query = query::with_field("", 2, &album, true, false);
                if let Some(artist) = artist {
                    f.query = query::with_field(&f.query, 1, &artist, true, false);
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

    /// The toast, floating above the status bar for a few seconds (longer while hovered).
    fn show_toast(&mut self, ctx: &egui::Context) {
        let Some((text, link, at)) = self.toast.clone() else { return };
        let screen = ctx.content_rect();
        let mut close = false;
        let area = egui::Area::new(egui::Id::new("toast"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(egui::pos2(screen.center().x, screen.bottom() - 70.0))
            .show(ctx, |ui| {
                // Amber, so it stands out from the rest of the interface.
                let amber = Color32::from_rgb(222, 172, 92);
                let ink = Color32::from_rgb(40, 24, 0);
                egui::Frame::popup(ui.style())
                    .fill(amber)
                    .stroke(egui::Stroke::new(1.0, amber.lerp_to_gamma(Color32::WHITE, 0.3)))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_max_width(520.0);
                        crate::theme::hrow(ui, |ui| {
                            ui.label(RichText::new("⚠").size(16.0).strong().color(ink));
                            ui.add(egui::Label::new(RichText::new(text.as_str()).color(ink).strong()).wrap());
                            if let Some((label, url)) = &link {
                                let button = egui::Button::new(RichText::new(label.as_str()).color(Color32::WHITE).strong()).fill(Color32::from_rgb(60, 38, 6));
                                if ui.add(button).on_hover_text(url.as_str()).clicked() {
                                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                                    close = true;
                                }
                            }
                            let x = egui::Button::new(RichText::new("✖").color(ink)).frame(false);
                            if ui.add(x).on_hover_text(t("Close")).clicked() {
                                close = true;
                            }
                        });
                    });
            });
        // Gone after 8 seconds, unless the pointer is on it.
        let hovered = area.response.hovered() || area.response.contains_pointer();
        if hovered {
            self.toast = Some((text, link, Instant::now()));
        } else if close || at.elapsed() > Duration::from_secs(8) {
            self.toast = None;
            return;
        }
        if close {
            self.toast = None;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// Removable chips for the active filters.
    fn active_filters(&mut self, ui: &mut egui::Ui) {
        let f = &self.filters;
        let mut chips: Vec<(String, Box<dyn Fn(&mut Filters)>)> = Vec::new();
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
        if chips.is_empty() {
            return;
        }
        ui.add_space(4.0);
        crate::theme::hrow_wrapped(ui, |ui| {
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

    fn toolbar_left(&mut self, ui: &mut egui::Ui) {
        let level = self.toolbar_level;
        let count = self.results.len();
        ui.label(RichText::new(trn!(count, "{count} result", "{count} results", count = count)).strong());
        if !self.cat_complete && level == 0 {
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
        // The order, including ones no column sorts by.
        let f = &mut self.filters;
        let current = SORTS.iter().find(|s| s.0 == f.sort).map_or_else(|| t("Custom order"), |s| t(s.1));
        // Narrower as space runs out: the full name, a shorter box, then just the icon.
        let (shown, width) = match level {
            0 => (format!("⇅  {current}"), 190.0),
            1 => (format!("⇅  {current}"), 140.0),
            _ => ("⇅".to_owned(), 30.0),
        };
        egui::ComboBox::from_id_salt("list_order")
            .selected_text(shown)
            .width(width)
            .show_ui(ui, |ui| {
                ui.set_min_width(190.0);
                for (key, name) in SORTS {
                    if ui.selectable_label(f.sort == key, t(name)).clicked() {
                        (f.sort, f.descending) = (key, false);
                    }
                }
            })
            .response
            .on_hover_text(current);
        if f.sort == SortKey::Popular && f.popular.is_empty() {
            match self.players.popular_songs(ui.ctx()) {
                Some(songs) => f.popular = songs,
                None => {
                    ui.spinner();
                }
            }
        }
        ui.separator();
        for (d, icon, tip) in [
            (Density::Compact, "☰", n("Compact rows")),
            (Density::Default, "▤", n("Default rows")),
            (Density::Large, "▦", n("Large rows with bigger covers")),
        ] {
            if ui
                .selectable_label(self.settings.density == d, icon)
                .on_hover_text(t(tip))
                .clicked()
            {
                self.settings.density = d;
                self.settings.cover_size = d.cover_size();
            }
        }
    }

    /// Download buttons, laid out right to left.
    fn toolbar_right(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        let count = self.results.len();
        let level = self.toolbar_level;
        // Shorter labels as space runs out (the tooltips always say it in full).
        let library = if level < 2 { t("⬇ Entire library") } else { "📚".to_owned() };
        let text = ui.visuals().text_color();
        let resp = ui
            .add_enabled(!busy && self.cat_complete, icon_button(&library, text))
            .on_hover_text(t("Download every chart on Encore"))
            .on_disabled_hover_text(t("Available once the catalogue has finished downloading"));
        if resp.clicked() {
            let charts = self.charts.clone();
            self.request_download(t("Entire Encore library"), charts);
        }
        let all = match level {
            0 => trn!(count, "⬇ All {count} results", "⬇ All {count} results", count = count),
            1 => t("⬇ All results"),
            _ => t("⬇ All"),
        };
        let resp = ui.add_enabled(!busy && count > 0, icon_button(&all, text)).on_hover_text(tr!("⬇ All {count} results", count = count));
        if resp.clicked() {
            let charts: Vec<Chart> = self.results.iter().map(|&i| self.charts[i].clone()).collect();
            self.request_download(t("All current results"), charts);
        }
        let n = self.selected.len();
        let selected = if level == 0 { tr!("⬇ Download selected ({count})", count = n) } else { tr!("⬇ Selected ({count})", count = n) };
        if ui
            .add_enabled(!busy && n > 0, download_button(&selected))
            .on_hover_text(tr!("⬇ Download selected ({count})", count = n))
            .clicked()
        {
            let charts = self.selected_charts();
            self.selected.clear();
            self.request_download(tr!("{count} selected chart(s)", count = charts.len()), charts);
        }
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let count = self.results.len();
        // The visible results area, for centring the floating "Back to top" button.
        let visible = ui.clip_rect().intersect(ui.max_rect());
        // One row, drawn as fully as fits: each level of compactness has its width remembered
        // from when it was last drawn, and the fullest one that fits is used. Only if even the
        // most compact one doesn't fit do the download buttons move to a second row.
        let width = ui.available_width();
        self.toolbar_level = pick_level(&self.toolbar_widths, width - 24.0);
        if self.toolbar_level < 3 {
            crate::theme::hrow(ui, |ui| {
                let left = ui.scope(|ui| self.toolbar_left(ui)).response.rect.width();
                let right = ui
                    .with_layout(crate::theme::trailing(), |ui| {
                        let start = ui.cursor();
                        self.toolbar_right(ui);
                        (start.right() - ui.cursor().right()).abs().max((ui.cursor().left() - start.left()).abs())
                    })
                    .inner;
                self.toolbar_widths[self.toolbar_level] = left + right;
            });
        } else {
            crate::theme::hrow(ui, |ui| self.toolbar_left(ui));
            ui.add_space(2.0);
            crate::theme::hrow(ui, |ui| {
                let right = ui.with_layout(crate::theme::trailing(), |ui| {
                    let start = ui.cursor().right();
                    self.toolbar_right(ui);
                    start - ui.min_rect().left()
                });
                let _ = right;
            });
        }
        ui.add_space(4.0);

        // The table, with a strip on the right for the custom scroll bar.
        let full = ui.available_rect_before_wrap();
        // The scroll bar on the right, or the left in a right-to-left language.
        let rtl = i18n::rtl();
        let table_rect = if rtl { full.with_min_x(full.left() + theme::SCROLLBAR_W + 6.0) } else { full.with_max_x(full.right() - theme::SCROLLBAR_W - 6.0) };
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
        let mut first_row = usize::MAX;
        // The cover column's own resize handle: how far it was dragged, and whether it still is.
        let (mut cover_delta, mut cover_active) = (0.0f32, false);
        // Dragging a header moves its column: each movable header's rect, and the one dropped.
        let mut header_rects: Vec<(Col, egui::Rect)> = Vec::new();
        let mut dropped: Option<(Col, f32)> = None;
        let drag_id = egui::Id::new("column_drag");
        let mut toggle_all = false;
        let any_selected = !self.selected.is_empty();
        let all_selected = count > 0 && self.results.iter().all(|&i| self.selected.contains(&self.charts[i].md5));
        let show_cover = self.settings.columns.cover;
        let cover_px = self.settings.cover_size;
        let row_h = if show_cover { cover_px + 8.0 } else { self.settings.density.row_height() };

        // Split borrows: the table body reads results while loading covers mutably.
        let (charts, results) = (&self.charts, &self.results);
        let covers = &mut self.covers;
        let selected = &self.selected;
        let favourites = &self.settings.favourites;
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
        } else if self.cover_dragging {
            table = table.scroll_to_row(self.first_row, Some(egui::Align::Min));
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
                Col::Check | Col::Fav => Column::exact(w).resizable(false),
                _ if k == last => Column::remainder().at_least(w.min(120.0)).clip(true),
                // Sized by its own handle (in the header), not the table's.
                Col::Cover => Column::exact(w).resizable(false),
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
                            if theme::select_box(ui, &mut all, any_selected && !all_selected).on_hover_text(tip).clicked() {
                                toggle_all = true;
                            }
                        });
                        continue;
                    }
                    let (title, key) = match col {
                        Col::Check | Col::Cover | Col::Fav => ("", None),
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
                        // The whole header can be dragged (clicking the name still sorts).
                        let cell = ui.max_rect();
                        if matches!(col, Col::Cover) {
                            // Dragging the cover column's edge sizes the covers (and rows).
                            // Its inner edge: the right, or the left when the table is mirrored.
                            let x = if i18n::rtl() { cell.left() } else { cell.right() };
                            let edge = egui::Rect::from_center_size(egui::pos2(x, cell.center().y), vec2(12.0, cell.height()));
                            let handle = ui.interact(edge, egui::Id::new("cover_resize"), Sense::drag());
                            handle.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, t("Cover size")));
                            let hot = handle.hovered() || handle.dragged();
                            if hot {
                                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                            }
                            let w = ui.visuals().widgets.clone();
                            let stroke = if hot { egui::Stroke::new(4.0, w.hovered.bg_stroke.color) } else { egui::Stroke::new(3.0, w.noninteractive.bg_stroke.color) };
                            ui.painter().with_clip_rect(ui.ctx().content_rect()).line_segment([egui::pos2(x, cell.top()), egui::pos2(x, cell.bottom())], stroke);
                            if handle.dragged() {
                                cover_delta += handle.drag_delta().x;
                                cover_active = true;
                            }
                        }
                        if !matches!(col, Col::Cover | Col::Fav) {
                            header_rects.push((*col, cell));
                            // Kept off the edges, which resize the column.
                            let grab = ui.interact(cell.shrink2(vec2(8.0, 0.0)), egui::Id::new(("column_header", *col as u8)), Sense::drag());
                            if grab.drag_started() {
                                ui.ctx().data_mut(|d| d.insert_temp(drag_id, *col as u8));
                            }
                            if grab.dragged() {
                                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                                ui.painter().rect_filled(cell, CornerRadius::same(4), theme::accent().linear_multiply(0.25));
                            } else if grab.hovered() {
                                ui.ctx().set_cursor_icon(CursorIcon::Grab);
                            }
                            if grab.drag_stopped()
                                && let Some(pos) = grab.interact_pointer_pos()
                            {
                                dropped = Some((*col, pos.x));
                            }
                        }
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
                    first_row = first_row.min(i);
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
                                if theme::select_box(ui, &mut checked, false).changed() {
                                    click = Some(RowClick::Checkbox(i));
                                }
                            }
                            Col::Fav => {
                                // A star: gold when it's a favourite; click to add or remove it.
                                let on = favourites.contains(&c.md5);
                                let (rect, star) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                                let color = if on {
                                    Color32::from_rgb(240, 190, 40)
                                } else if star.hovered() {
                                    ui.visuals().text_color()
                                } else {
                                    ui.visuals().weak_text_color().linear_multiply(0.6)
                                };
                                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, if on { "★" } else { "☆" }, egui::FontId::proportional(17.0), color);
                                star.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, t("Favourite")));
                                let tip = if on { t("Remove from favourites") } else { t("Add to favourites") };
                                if star.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip).clicked() {
                                    row_menu = Some(RowAction::Favourite(i));
                                }
                            }
                            Col::Cover => {
                                let rect = cover(ui, covers, c.art.as_deref(), cover_px, covers::THUMB);
                                // Hovering the cover shows a play button that opens the preview.
                                let play = ui.interact(rect, ui.id().with(("cover_play", i)), Sense::click());
                                play.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t("Preview")));
                                if play.hovered() {
                                    let p = ui.painter();
                                    p.rect_filled(rect, CornerRadius::same(5), Color32::from_black_alpha(110));
                                    let r = (rect.width() * 0.28).clamp(7.0, 22.0);
                                    p.circle_filled(rect.center(), r, Color32::from_rgb(124, 84, 210));
                                    let c0 = rect.center() + vec2(r * 0.12, 0.0);
                                    let tri = vec![c0 + vec2(-r * 0.4, -r * 0.5), c0 + vec2(r * 0.55, 0.0), c0 + vec2(-r * 0.4, r * 0.5)];
                                    p.add(egui::Shape::convex_polygon(tri, Color32::WHITE, egui::Stroke::NONE));
                                }
                                if play.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(t("Preview")).clicked() {
                                    row_menu = Some(RowAction::Preview(i));
                                }
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
                    let fav = favourites.contains(&c.md5);
                    resp.context_menu(|ui| row_context_menu(ui, c, i, is_checked, have, fav, &mut row_menu));
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
        if self.results.is_empty() && !self.live_searching {
            let message = egui::Rect::from_center_size(table_rect.center(), vec2(table_rect.width().min(420.0), 160.0));
            let mut box_ui = ui.new_child(egui::UiBuilder::new().max_rect(message).layout(egui::Layout::top_down(egui::Align::Center)));
            // Showing favourites with none saved says how to add them.
            let no_favourites = self.filters.favourites.as_ref().is_some_and(|f| f.is_empty());
            let (icon, title, hint) = if no_favourites {
                ("☆", t("You haven't added any favourites yet"), t("Click the ☆ next to a chart, or right-click it, to add it to your favourites."))
            } else if self.cat_complete {
                ("🔍", t("No charts found"), t("Try fewer words, or check the spelling and filters."))
            } else {
                ("🔍", t("No charts found"), t("The catalogue is still downloading, so more charts may turn up."))
            };
            box_ui.label(RichText::new(icon).size(34.0).weak());
            box_ui.label(RichText::new(title).size(18.0).strong());
            box_ui.add(egui::Label::new(RichText::new(hint).weak()).wrap());
            box_ui.add_space(6.0);
            if no_favourites {
                if box_ui.button(t("🌈  Show all songs")).clicked() {
                    self.filters.favourites = None;
                }
            } else if box_ui.button(t("✖  Clear search and filters")).clicked() {
                let f = &mut self.filters;
                *f = Filters { sort: f.sort, descending: f.descending, ..Filters::default() };
                self.live_edited = Instant::now();
            }
        }

        let bar_x = if rtl { full.left() } else { full.right() - theme::SCROLLBAR_W };
        let bar = egui::Rect::from_min_max(egui::pos2(bar_x, scroll.inner_rect.top()), egui::pos2(bar_x + theme::SCROLLBAR_W, scroll.inner_rect.bottom()));
        // A solid strip behind the scroll bar, from the table's edge to the panel's.
        let strip = if rtl {
            egui::Rect::from_min_max(egui::pos2(full.left(), bar.top()), egui::pos2(table_rect.left(), bar.bottom()))
        } else {
            egui::Rect::from_min_max(egui::pos2(table_rect.right(), bar.top()), egui::pos2(full.right(), bar.bottom()))
        };
        ui.painter().rect_filled(strip, CornerRadius::ZERO, ui.visuals().panel_fill);
        let view = scroll.inner_rect.height();
        if let Some(offset) = theme::scrollbar(ui, bar, scroll.id.with("bar"), scroll.state.offset.y, scroll.content_size.y, view) {
            let mut state = scroll.state;
            state.offset.y = offset;
            state.store(ui.ctx(), scroll.id);
            ui.ctx().request_repaint();
        }
        ui.advance_cursor_after_rect(full);

        // Floating "back to top" button once the list is scrolled down. It floats above everything,
        // so it's hidden while a pop-up's blurred backdrop is up.
        if scroll.state.offset.y > 300.0 && !self.modal_open() {
            let pos = egui::pos2(
                visible.center().x,
                visible.bottom().min(scroll.inner_rect.bottom()) - 18.0,
            );
            egui::Area::new(egui::Id::new("scroll_top"))
                // Above the table, but under menus and pop-ups (which are Foreground).
                .order(egui::Order::Middle)
                .pivot(egui::Align2::CENTER_BOTTOM)
                .fixed_pos(pos)
                .show(ui.ctx(), |ui| {
                    if theme::back_to_top(ui).on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        self.scroll_to_top = true;
                    }
                });
        }

        // The cover column's handle was dragged: resize the covers, keeping the top row in place
        // (see `first_row`) so the table doesn't jump as the rows change height.
        if first_row != usize::MAX {
            self.first_row = first_row;
        }
        self.cover_dragging = cover_active;
        if cover_delta != 0.0 {
            let dir = if i18n::rtl() { -1.0 } else { 1.0 };
            self.settings.cover_size = (self.settings.cover_size + cover_delta * dir).clamp(16.0, 234.0);
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
            Some(RowAction::Favourite(i)) => {
                let md5 = self.charts[self.results[i]].md5.clone();
                let favs = &mut self.settings.favourites;
                if !favs.remove(&md5) {
                    favs.insert(md5);
                }
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
        // While dragging a header, a line shows where its column will go.
        let dragging = ui.ctx().data(|d| d.get_temp::<u8>(drag_id));
        if let (Some(_), Some(pos)) = (dragging, ui.ctx().pointer_interact_pos())
            && ui.ctx().input(|i| i.pointer.primary_down())
            && let Some(x) = column_gap(&header_rects, pos.x)
            && let (Some(first), Some(last)) = (header_rects.first(), header_rects.last())
        {
            let (top, bottom) = (first.1.top(), last.1.bottom());
            ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, drag_id)).line_segment(
                [egui::pos2(x, top), egui::pos2(x, bottom + 6.0)],
                egui::Stroke::new(3.0, theme::accent_hover()),
            );
        }
        if let Some((col, x)) = dropped {
            ui.ctx().data_mut(|d| d.remove::<u8>(drag_id));
            self.move_column(col, x, &header_rects);
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
    /// Shows the chart from a link once it's in the catalogue: just it in the list, with its
    /// details open. A full MD5 Rainbow doesn't have yet is looked up on Encore.
    fn open_pending_link(&mut self) {
        let Some(id) = self.pending_link.clone() else { return };
        let found: Vec<Chart> = self.charts.iter().filter(|c| c.md5.starts_with(&id)).take(2).cloned().collect();
        if let Some(c) = found.first().cloned() {
            if found.len() > 1 {
                self.log(format!("More than one chart starts with {id}; showing the first"));
            }
            self.filters = Filters { query: c.md5.clone(), sort: self.filters.sort, descending: self.filters.descending, ..Filters::default() };
            self.live_edited = Instant::now();
            self.selected = HashSet::from([c.md5.clone()]);
            self.detail = Some(c);
            self.pending_link = None;
        } else if id.len() == 32 && self.filters.query != id {
            // Search for it, which asks Encore if the catalogue doesn't have it.
            self.filters = Filters { query: id, sort: self.filters.sort, descending: self.filters.descending, ..Filters::default() };
            self.live_edited = Instant::now();
        } else if self.cat_complete && id.len() < 32 {
            self.log(format!("FAILED to open shared chart {id}: no chart on Encore starts with it"));
            self.pending_link = None;
        }
    }

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
        // The MD5 stays put at the top; the rest scrolls (from the top again for a new chart).
        ui.add_space(8.0);
        crate::theme::hrow(ui, |ui| {
            ui.weak(t("MD5"));
            // The copy button sits at the right edge; the hash takes what's left and
            // shortens with "…" if needed (right-click or hover shows it in full).
            ui.with_layout(crate::theme::trailing(), |ui| {
                ui.menu_button("🔗", |ui| copy_link_items(ui, &c.md5)).response.on_hover_text(t("Copy a link to this chart that opens it in Rainbow"));
                if ui.small_button("📋").on_hover_text(t("Copy MD5")).clicked() {
                    ui.ctx().copy_text(c.md5.clone());
                }
                let hash = ui.add(egui::Label::new(RichText::new(&c.md5).monospace().small()).truncate());
                copy_menu(&hash.on_hover_text(&c.md5), &[(n("Copy MD5"), c.md5.clone())]);
            });
        });
        ui.add_space(6.0);
        let new_chart = self.detail_shown.as_deref() != Some(c.md5.as_str());
        self.detail_shown = Some(c.md5.clone());
        fancy_scroll_ex(ui, "detail", new_chart, |ui| {
            let size = ui.available_width().min(320.0);
            let art = ui.vertical_centered(|ui| cover(ui, &mut self.covers, c.art.as_deref(), size, covers::LARGE)).inner;
            // Close button floating in the cover's top-right corner.
            let label = RichText::new(t("✖  Close")).color(Color32::WHITE);
            let text = egui::WidgetText::from(label.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
            let button_size = text.size() + 2.0 * ui.spacing().button_padding;
            let corner = art.right_top() + vec2(-4.0, 4.0);
            let close = egui::Button::new(label).fill(Color32::from_black_alpha(170)).corner_radius(CornerRadius::same(8));
            if ui.place(egui::Rect::from_min_size(corner - vec2(button_size.x, 0.0), button_size), close).on_hover_text(t("Hide the song details")).clicked() {
                self.detail = None;
            }
            ui.add_space(8.0);
            let title = ui.label(RichText::new(c.name.clone().unwrap_or_else(|| t("Unknown"))).heading().strong());
            copy_menu(&title, &[(n("Copy song name"), c.name.clone().unwrap_or_default()), (n("Copy \"Artist - Song\""), c.title())]);
            let mut nav = None;
            if let Some(a) = &c.artist
                && link(ui, RichText::new(a).size(16.0).color(theme::accent()), n("Show all songs by this artist"))
            {
                nav = Some(Nav::Text(1, a.clone(), true));
            }
            crate::theme::hrow_wrapped(ui, |ui| {
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
            ui.add_space(10.0);
            // Download and Preview side by side, half the panel each, right under the title.
            let (mut download, mut open_preview, mut reveal_it) = (false, false, false);
            let half = ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).max(60.0);
            let busy = self.job.is_some();
            crate::theme::hrow(ui, |ui| {
                if downloaded {
                    let good = theme::good();
                    // (A web page can't open the file manager, so there it only says it's downloaded.)
                    let label = if cfg!(target_arch = "wasm32") { t("✔  Downloaded") } else { t("📂  Show in folder") };
                    let button = egui::Button::new(RichText::new(label).strong().color(good))
                        .fill(good.linear_multiply(0.15))
                        .stroke(egui::Stroke::new(1.0, good.linear_multiply(0.7)))
                        .min_size(vec2(half, 34.0));
                    let resp = ui.add(button);
                    reveal_it = !cfg!(target_arch = "wasm32") && resp.on_hover_text(t("Downloaded ✔  Open it in your file manager")).clicked();
                } else {
                    download = glossy(ui, download_button(&t("⬇  Download")).min_size(vec2(half, 34.0)), theme::good().lerp_to_gamma(Color32::BLACK, 0.2), !busy).clicked();
                }
                let preview = egui::Button::new(RichText::new(t("▶  Preview")).size(14.5).strong().color(Color32::WHITE)).min_size(vec2(half, 34.0));
                open_preview = glossy(ui, preview, Color32::from_rgb(124, 84, 210), true)
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

            ui.add_space(10.0);
            // One line each: what it is on the left, the value on the right (a link where it
            // leads somewhere).
            // Alternate lines are shaded, like the table.
            let mut line = 0;
            let mut info = |ui: &mut egui::Ui, label: &str, value: &str, color: Color32, tip: Option<&str>| -> bool {
                let mut clicked = false;
                let fill = if line % 2 == 0 { ui.visuals().widgets.inactive.bg_fill } else { Color32::TRANSPARENT };
                line += 1;
                egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(5)).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                crate::theme::hrow(ui, |ui| {
                    ui.label(RichText::new(label).weak());
                    ui.with_layout(crate::theme::trailing(), |ui| {
                        let text = RichText::new(value).color(color).strong();
                        match tip {
                            Some(tip) => clicked = link(ui, text, tip),
                            None => {
                                ui.add(egui::Label::new(text).truncate());
                            }
                        }
                    });
                });
                });
                clicked
            };
            let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);
            if let Some(g) = games::game_key(c.icon.as_deref())
                && info(ui, &t("🏆  Game"), &games::game_name(&g), theme::warn(), Some(n("Show this game's charts")))
            {
                nav = Some(Nav::Game(g));
            }
            if let Some(g) = &c.genre
                && info(ui, &t("🎼  Genre"), g, tag_color(g), Some(n("Show this genre")))
            {
                nav = Some(Nav::Text(3, g.clone(), true));
            }
            if let Some(ch) = &c.charter
                && info(ui, &t("✏  Charter"), ch, theme::RAINBOW[4], Some(n("Show everything charted by this charter")))
            {
                nav = Some(Nav::Text(5, ch.clone(), false));
            }
            if !c.length_text().is_empty() {
                info(ui, &t("⏱  Length"), &c.length_text(), ui.visuals().text_color(), None);
            }
            if !c.uploaded().is_empty() {
                info(ui, &t("📅  Uploaded"), &c.uploaded(), ui.visuals().text_color(), None);
            }
            ui.spacing_mut().item_spacing.y = gap;

            ui.add_space(12.0);
            section(ui, "instruments", &t("Instruments"), None, |ui| {
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
                            crate::theme::hrow(ui, |ui| {
                                if link_ex(ui, RichText::new(format!("{}  {}", instrument_icon(k as u8), t(name))).strong(), n("Show charts for this instrument"), false) {
                                    nav = Some(Nav::Instrument(k as u8));
                                }
                                if d >= 0 {
                                    ui.with_layout(crate::theme::trailing(), |ui| {
                                        let resp = ui.add(egui::Label::new(theme::difficulty_dots(d)).sense(Sense::click()));
                                        if resp.on_hover_text(tr!("Intensity {level}: show {instrument} charts at this intensity", level = d, instrument = t(name))).clicked() {
                                            nav = Some(Nav::Difficulty(k as u8, d));
                                        }
                                    });
                                }
                            });
                            if let Some(s) = notes {
                                ui.label(RichText::new(trn!(s.n, "{notes} note  ·  {nps} max NPS", "{notes} notes  ·  {nps} max NPS", notes = s.n, nps = format!("{:.0}", s.nps))).size(12.5).weak());
                            }
                        });
                }
            });

            ui.spacing_mut().item_spacing.y = 6.0;
            self.scores.show(ui, &c, self.filters.difficulty.unwrap_or(0));
            ui.add_space(10.0);
            section(ui, "features", &t("Features"), None, |ui| {
                crate::theme::hrow_wrapped(ui, |ui| {
                    let any = feature_tags(ui, &c, true, &mut nav);
                    if !any {
                        ui.weak(t("None"));
                    }
                });
            });

            ui.add_space(12.0);
            self.leaderboard.show(ui, &c, self.filters.instrument, self.filters.difficulty);

            ui.add_space(8.0);
            if let Some(nav) = nav {
                self.navigate(nav);
            }
        });
    }

    /// Where to get help or report a problem, with the details a report needs.
    fn report_window(&mut self, ctx: &egui::Context) {
        if !self.show_report {
            return;
        }
        let mut open = true;
        // Our own title row instead of egui's, whose ✖ can't show a pointer cursor.
        egui::Window::new(t("🐞  Report an issue"))
            .id(egui::Id::new("report_issue"))
            .order(egui::Order::Foreground)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(520.0)
            .show(ctx, |ui| {
                crate::theme::hrow(ui, |ui| {
                    ui.label(RichText::new(t("🐞  Report an issue")).strong().size(15.0));
                    ui.with_layout(crate::theme::trailing(), |ui| {
                        let close = ui.add(egui::Button::new(RichText::new("✖").size(13.0)).frame(false)).on_hover_text(t("Close"));
                        if close.clicked() {
                            open = false;
                        }
                    });
                });
                ui.separator();
                ui.add_space(6.0);
                ui.vertical_centered(|ui| ui.label(t("Something not working, or an idea for Rainbow? Let us know.")));
                ui.add_space(12.0);
                // Three tiles side by side: a big icon, the title and a line about it.
                // Each its own colour: Discord's blurple, GitHub red for bugs, green for helping.
                let choices = [
                    (t("💬  Ask on Discord"), t("Quick help and chat in the Renzora Discord."), "https://discord.gg/x6KHzEe2j2", Color32::from_rgb(88, 101, 242)),
                    (t("🐛  Report on GitHub"), t("Open an issue for a bug or a feature request."), "https://github.com/renzora/rainbow/issues/new", Color32::from_rgb(218, 74, 72)),
                    (t("🤝  Contribute"), t("How to help with code or translations."), "https://github.com/renzora/rainbow/blob/main/CONTRIBUTING.md", Color32::from_rgb(38, 150, 104)),
                ];
                ui.columns(3, |cols| {
                    for (ui, (title, about, url, color)) in cols.iter_mut().zip(&choices) {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 128.0), Sense::click());
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title.as_str()));
                        let fill = color.linear_multiply(if resp.hovered() { 0.42 } else { 0.24 });
                        ui.painter().rect_filled(rect, 8.0, fill);
                        ui.painter().rect_stroke(rect, 8.0, egui::Stroke::new(1.0, color.linear_multiply(0.7)), egui::StrokeKind::Inside);
                        let (icon, name) = title.split_once("  ").unwrap_or(("", title.as_str()));
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(10.0)), |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add(egui::Label::new(RichText::new(icon).size(26.0)).selectable(false));
                                ui.add(egui::Label::new(RichText::new(name).strong().size(14.5)).selectable(false));
                                ui.add(egui::Label::new(RichText::new(about.as_str()).size(12.0).weak()).selectable(false));
                            });
                        });
                        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(*url);
                        if resp.clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(*url));
                        }
                    }
                });
                ui.add_space(10.0);
                ui.separator();
                crate::theme::hrow(ui, |ui| {
                    if ui.button(t("📋  Copy details")).on_hover_text(t("Rainbow's version, your system and the latest log lines, to paste into a report")).clicked() {
                        let lines: Vec<&str> = self.log.iter().rev().take(25).rev().map(String::as_str).collect();
                        let details = format!(
                            "Rainbow {}\nSystem: {} {}\n\nRecent log:\n{}",
                            env!("CARGO_PKG_VERSION"),
                            std::env::consts::OS,
                            std::env::consts::ARCH,
                            lines.join("\n")
                        );
                        ui.ctx().copy_text(details);
                    }
                    ui.add(egui::Label::new(RichText::new(t("Paste it into your report so the problem is easier to fix.")).weak()).truncate());
                });
            });
        if !open {
            self.show_report = false;
        }
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
                crate::theme::hrow(ui, |ui| {
                    ui.label(t("Format:"));
                    ui.radio_value(&mut self.settings.format, DownloadFormat::Sng, ".sng");
                    ui.radio_value(&mut self.settings.format, DownloadFormat::Folder, t("Chart Folder"));
                });
                crate::theme::hrow(ui, |ui| {
                    ui.label(t("Wait between downloads:"));
                    ui.add(egui::Slider::new(&mut self.settings.delay_secs, 0.5..=10.0).suffix(" s"));
                });
                let hours = todo as f32 * (self.settings.delay_secs + 1.0) / 3600.0;
                ui.weak(tr!("Roughly {hours}+ hours at that pace (plus download time).", hours = format!("{hours:.0}")));
                ui.weak(t("Encore is donation-funded; please keep the delay reasonable."));
                ui.add_space(8.0);
                crate::theme::hrow(ui, |ui| {
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

    /// Holds the window open while a download is running and asks first.
    fn guard_quit(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested())
            && self.job.is_some()
            && !self.quit_confirmed
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_quit = true;
        }
    }

    fn quit_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_quit {
            return;
        }
        if self.job.is_none() {
            // The download finished while we were asking.
            self.confirm_quit = false;
            return;
        }
        let mut open = true;
        let (mut quit, mut keep) = (false, false);
        egui::Window::new(t("Quit while downloading?"))
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(ctx.content_rect().center())
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(t(
                    "A download is still running. Are you sure you want to quit?",
                ));
                if self.settings.resume_downloads {
                    ui.weak(t(
                        "It will carry on automatically next time you open Rainbow.",
                    ));
                } else {
                    ui.weak(t("Next time it picks up where it left off."));
                }
                ui.add_space(8.0);
                crate::theme::hrow(ui, |ui| {
                    if ui.add(theme::primary(t("Keep downloading"))).clicked() {
                        keep = true;
                    }
                    if ui.button(t("Quit")).clicked() {
                        quit = true;
                    }
                });
            });
        if quit {
            self.stop.store(true, Ordering::Relaxed);
            self.quit_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if quit || keep || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.confirm_quit = false;
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
                // Columns exactly a card wide (Library and Downloads first: on the left, or the
                // right in a right-to-left language).
                let column = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
                    ui.allocate_ui_with_layout(vec2(card_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| add(ui));
                };
                if two {
                    let updates = &mut self.updates;
                    let mut first = |ui: &mut egui::Ui| {
                        library_card(ui, s, card_w, busy, example.as_ref(), &mut actions);
                        downloads_card(ui, s, card_w, busy, &mut actions);
                    };
                    if i18n::rtl() {
                        let mut second = |ui: &mut egui::Ui| {
                            appearance_card(ui, s, card_w);
                            accessibility_card(ui, s, card_w, &mut actions);
                            catalogue_card(ui, s, card_w, building, &mut actions);
                            updates_card(ui, s, card_w, updates);
                        };
                        column(ui, &mut second);
                        ui.add_space(16.0);
                        let mut first = |ui: &mut egui::Ui| {
                            library_card(ui, s, card_w, busy, example.as_ref(), &mut actions);
                            downloads_card(ui, s, card_w, busy, &mut actions);
                        };
                        column(ui, &mut first);
                    } else {
                        column(ui, &mut first);
                        ui.add_space(16.0);
                        let mut second = |ui: &mut egui::Ui| {
                            appearance_card(ui, s, card_w);
                            accessibility_card(ui, s, card_w, &mut actions);
                            catalogue_card(ui, s, card_w, building, &mut actions);
                            updates_card(ui, s, card_w, updates);
                        };
                        column(ui, &mut second);
                    }
                } else {
                    ui.vertical(|ui| {
                        library_card(ui, s, card_w, busy, example.as_ref(), &mut actions);
                        downloads_card(ui, s, card_w, busy, &mut actions);
                        appearance_card(ui, s, card_w);
                        accessibility_card(ui, s, card_w, &mut actions);
                        catalogue_card(ui, s, card_w, building, &mut actions);
                        updates_card(ui, s, card_w, &mut self.updates);
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
        #[cfg(target_arch = "wasm32")]
        if actions.pick_folder {
            self.pick_folder();
        }
        #[cfg(target_arch = "wasm32")]
        if actions.forget_folder {
            web::forget_folder();
        }
    }

    /// The web version's songs folder: asks the browser for one, then reads the download list
    /// in it (so charts the desktop app downloaded there count as downloaded).
    #[cfg(target_arch = "wasm32")]
    fn pick_folder(&mut self) {
        let (tx, manifest) = (self.tx.clone(), self.manifest.clone());
        task::spawn(move || async move {
            let Some(name) = web::pick_folder().await else { return };
            if let Some(list) = web::folder_read(api::MANIFEST_NAME).await {
                manifest.lock().unwrap().merge(&String::from_utf8_lossy(&list));
            }
            let _ = tx.send(Msg::Log(format!("Saving charts to the folder {name}")));
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
fn credit(ui: &mut egui::Ui, logo: Option<&egui::TextureHandle>, height: f32, compact: bool) {
    // Drawn by hand at its exact size, so it works in the header's right-to-left layout.
    // Compact (a narrow window): just the logo.
    let compact = compact && logo.is_some();
    let text = ui.painter().layout_no_wrap(
        if compact { String::new() } else { "Renzora Studios".to_owned() },
        egui::FontId::new(13.5, egui::FontFamily::Name("bold".into())),
        ui.visuals().text_color(),
    );
    let logo_size = vec2(height * 110.0 / 128.0, height);
    let (pad_l, gap, pad_r, pad_y) = if compact { (6.0, 0.0, 6.0, 3.0) } else { (6.0, 8.0, 12.0, 3.0) };
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
    /// The web version's Choose… (or Forget) for the songs folder.
    #[cfg(target_arch = "wasm32")]
    pick_folder: bool,
    #[cfg(target_arch = "wasm32")]
    forget_folder: bool,
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
            crate::theme::vcol(ui, |ui| {
                let title_text = RichText::new(without_arrow(title)).size(19.0).strong().color(theme::accent_hover());
                if title.starts_with('⬇') {
                    // The drawn download icon rather than the ⬇ character.
                    crate::theme::hrow(ui, |ui| {
                        ui.add(download_icon(19.0, theme::accent_hover()));
                        ui.label(title_text);
                    });
                } else {
                    ui.label(title_text);
                }
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
    crate::theme::hrow(ui, |ui| {
        let text_w = (ui.available_width() - control_w - 16.0).max(140.0);
        let align = if i18n::rtl() { egui::Align::Max } else { egui::Align::Min };
        ui.allocate_ui_with_layout(vec2(text_w, 0.0), egui::Layout::top_down(align), |ui| {
            ui.set_width(text_w);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add(egui::Label::new(RichText::new(label).size(15.5).strong()).wrap());
            if !about.is_empty() {
                ui.add(egui::Label::new(RichText::new(about).size(13.0).weak()).wrap());
            }
        });
        let measured = ui
            .with_layout(crate::theme::trailing(), |ui| {
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

/// The songs folder's path (or name), in a box under its setting.
fn folder_box(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(text).monospace().size(13.5)).truncate());
        });
}

fn library_card(ui: &mut egui::Ui, s: &mut Settings, width: f32, busy: bool, example: Option<&Chart>, actions: &mut SettingsActions) {
    let _ = actions;
    settings_card(ui, width, &t("📁  Chart library"), &t("Where Rainbow puts the charts it downloads."), |ui| {
        #[cfg(not(target_arch = "wasm32"))]
        {
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
            folder_box(ui, &s.dir.display().to_string());
        }
        // The browser can write into a folder only where it has the File System Access API.
        #[cfg(target_arch = "wasm32")]
        if web::has_folder_api() {
            let folder = web::folder_name();
            setting_row(ui, &t("Songs folder"), &t("Point this at your Clone Hero or YARG songs folder. The browser asks once each visit before Rainbow saves into it."), |ui| {
                if ui.add_enabled(!busy, theme::primary(t("Choose…")).min_size(vec2(0.0, 32.0))).clicked() {
                    actions.pick_folder = true;
                }
                if folder.is_some() && ui.add_enabled(!busy, egui::Button::new(RichText::new(t("✖ Forget")).size(14.5)).min_size(vec2(0.0, 32.0))).on_hover_text(t("Save charts as ordinary downloads instead")).clicked() {
                    actions.forget_folder = true;
                }
            });
            folder_box(ui, &folder.unwrap_or_else(|| t("None chosen: charts are saved as ordinary downloads")));
        } else {
            setting_row(ui, &t("Songs folder"), &t("This browser saves charts as ordinary downloads (always as .sng files). In Chrome or Edge, Rainbow can save straight into your songs folder."), |_| {});
        }
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
            setting_row(
                ui,
                &t("Wait between bulk downloads"),
                &t("Encore is donation-funded, so please keep this reasonable."),
                |ui| {
                    ui.add(egui::Slider::new(&mut s.delay_secs, 0.5..=10.0).suffix(" s"));
                },
            );
            setting_row(
                ui,
                &t("Resume unfinished downloads"),
                &t("If Rainbow is closed mid-download, carry on when it's reopened."),
                |ui| {
                    theme::checkbox(ui, &mut s.resume_downloads, "");
                },
            );
            #[cfg(not(target_arch = "wasm32"))]
            import_rows(ui, s);
        });
        // Importing zips and Rock Band songs is desktop only (it needs to watch the songs folder).
        #[cfg(not(target_arch = "wasm32"))]
        if ui.add(egui::Button::new(RichText::new(t("📦  Process songs folder now")).size(14.5)).min_size(vec2(0.0, 32.0))).clicked() {
            actions.extract_now = true;
        }
        #[cfg(target_arch = "wasm32")]
        let _ = actions;
    });
}

/// The Downloads card's settings for importing zips and Rock Band songs (desktop only).
#[cfg(not(target_arch = "wasm32"))]
fn import_rows(ui: &mut egui::Ui, s: &mut Settings) {
    setting_row(
        ui,
        &t("Convert Rock Band songs automatically"),
        &t("CON files (e.g. from RhythmVerse) in your songs folder become Clone Hero charts. You can also drop them onto the window."),
        |ui| {
            theme::checkbox(ui, &mut s.auto_convert, "");
        },
    );
    setting_row(
        ui,
        &t("Extract .zip files automatically"),
        &t("Unpack zips that appear in your songs folder (e.g. from other sites)."),
        |ui| {
            theme::checkbox(ui, &mut s.auto_extract, "");
        },
    );
    ui.add_enabled_ui(s.auto_extract || s.auto_convert, |ui| {
        let about = tr!(
            "Otherwise they're moved to {zips} or {cons} in your songs folder.",
            zips = unzip::DONE_DIR,
            cons = import::CONVERTED_DIR
        );
        setting_row(ui, &t("Delete originals after processing"), &about, |ui| {
            theme::checkbox(ui, &mut s.delete_zips, "");
        });
    });
}

fn appearance_card(ui: &mut egui::Ui, s: &mut Settings, width: f32) {
    settings_card(ui, width, &t("🎨  Appearance"), &t("Theme, size and what the results table shows."), |ui| {
        setting_row(ui, &t("Theme"), &t("Colours for the whole app."), |_| {});
        crate::theme::hrow_wrapped(ui, |ui| {
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
        setting_row(ui, &t("Preview speed"), &t("How playback speeds are shown in the chart preview."), |ui| {
            segmented(ui, &mut s.speed_percent, &[(false, n("Multiplier (1.5×)")), (true, n("Percent (150%)"))]);
        });
        setting_row(ui, &t("Row size"), &t("Drag the cover column's edge to fine-tune."), |ui| {
            if segmented(ui, &mut s.density, &[(Density::Compact, n("Compact")), (Density::Default, n("Default")), (Density::Large, n("Large"))]) {
                s.cover_size = s.density.cover_size();
            }
        });
        setting_row(ui, &t("Default order"), &t("How the song list is ordered when Rainbow opens."), |ui| {
            let current = SORTS.iter().find(|o| o.0 == s.default_sort).map_or_else(String::new, |o| t(o.1));
            egui::ComboBox::from_id_salt("default_order").selected_text(current).width(200.0).show_ui(ui, |ui| {
                for (key, name) in SORTS {
                    ui.selectable_value(&mut s.default_sort, key, t(name));
                }
            });
        });
        setting_row(ui, &t("Table columns"), &t("Columns also hide automatically when the window is narrow."), |_| {});
        crate::theme::hrow_wrapped(ui, |ui| {
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
                theme::checkbox(ui, on, &t(label));
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
            let resp = crate::theme::hrow(ui, |ui| {
                let mut on = selected;
                let clicked = theme::checkbox(ui, &mut on, "").clicked();
                crate::theme::vcol(ui, |ui| {
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
        setting_row(ui, &t("Scroll speed"), &t("How far the mouse wheel and touchpad scroll lists and panels."), |ui| {
            ui.add(egui::Slider::new(&mut s.scroll_speed, 0.25..=4.0).step_by(0.05).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
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
        setting_row(ui, &t("Keyboard shortcuts"), &t("Everything in Charts can be used from the keyboard."), |ui| {
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

fn updates_card(ui: &mut egui::Ui, s: &mut Settings, width: f32, updates: &mut updater::Updates) {
    settings_card(ui, width, &t("⬆  Updates"), &t("New versions of Rainbow from GitHub."), |ui| {
        updates.settings(ui);
        setting_row(ui, &t("Check for updates on startup"), &t("Looks for a newer version each time Rainbow opens."), |ui| {
            theme::checkbox(ui, &mut s.check_updates, "");
        });
        setting_row(ui, &t("Install updates automatically"), &t("Downloads and installs new versions in the background; they're used after a restart."), |ui| {
            theme::checkbox(ui, &mut s.auto_update, "");
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
pub(crate) fn fancy_scroll<R>(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    fancy_scroll_ex(ui, id, false, add)
}

/// [`fancy_scroll`], back at the top when `to_top`.
pub(crate) fn fancy_scroll_ex<R>(ui: &mut egui::Ui, id: &str, to_top: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let full = ui.available_rect_before_wrap();
    // The scroll bar goes on the right, or the left in a right-to-left language. Its room is
    // only kept when there's something to scroll (as last frame found), so content that fits
    // has the same margin on both sides.
    let rtl = i18n::rtl();
    let needs_id = ui.id().with(("fancy_scroll_needs_bar", id));
    let needs_bar = ui.data(|d| d.get_temp::<bool>(needs_id)).unwrap_or(true);
    let room = if needs_bar { theme::SCROLLBAR_W + 6.0 } else { 0.0 };
    let area = if rtl { full.with_min_x(full.left() + room) } else { full.with_max_x(full.right() - room) };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    // Content wider than the area (in a narrow panel) is cut off here, not drawn under the bar.
    child.set_clip_rect(area.intersect(ui.clip_rect()));
    let mut area = egui::ScrollArea::vertical()
        .id_salt(id)
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
    if to_top {
        area = area.vertical_scroll_offset(0.0);
    }
    let out = area.show(&mut child, add);
    let bar_x = if rtl { full.left() } else { full.right() - theme::SCROLLBAR_W };
    let bar = egui::Rect::from_min_max(egui::pos2(bar_x, out.inner_rect.top()), egui::pos2(bar_x + theme::SCROLLBAR_W, out.inner_rect.bottom()));
    // A solid strip behind the bar, so nothing shows through it.
    if needs_bar {
        let strip = if rtl { bar.with_max_x(bar.right() + 6.0) } else { bar.with_min_x(bar.left() - 6.0) };
        ui.painter().rect_filled(strip, CornerRadius::ZERO, if ui.layer_id().order == egui::Order::Background { ui.visuals().panel_fill } else { ui.visuals().window_fill });
    }
    let view = out.inner_rect.height();
    let needed = out.content_size.y > view + 1.0;
    if needed != needs_bar {
        ui.data_mut(|d| d.insert_temp(needs_id, needed));
        ui.ctx().request_repaint();
    }
    // When the content gets shorter (e.g. a section is closed at the bottom), pull the scroll
    // position back so the bar and content still line up.
    let max_offset = (out.content_size.y - view).max(0.0);
    if out.state.offset.y > max_offset + 0.5 {
        let mut state = out.state;
        state.offset.y = max_offset;
        state.store(ui.ctx(), out.id);
        ui.ctx().request_repaint();
    }
    if let Some(offset) = theme::scrollbar(ui, bar, out.id.with("bar"), out.state.offset.y.min(max_offset), out.content_size.y, view) {
        let mut state = out.state;
        state.offset.y = offset;
        state.store(ui.ctx(), out.id);
        ui.ctx().request_repaint();
    }
    ui.advance_cursor_after_rect(full);
    out.inner
}

/// A toggle-style rounded button.
/// A details panel section under a header band (accent-tinted, with diagonal stripes) that
/// collapses it when clicked. Sections start open and stay as they were left for the rest of the
/// session; `id` keeps that the same whatever the language. `tip` is shown when hovering the header.
pub(crate) fn section(ui: &mut egui::Ui, id: &str, title: &str, tip: Option<&str>, add: impl FnOnce(&mut egui::Ui)) {
    section_ex(ui, id, title, tip, None, add);
}

/// A [`section`] with a small button at the far end of its header (e.g. "Refresh"); returns
/// whether it was clicked.
pub(crate) fn section_ex(ui: &mut egui::Ui, id: &str, title: &str, tip: Option<&str>, action: Option<&str>, add: impl FnOnce(&mut egui::Ui)) -> bool {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(("detail_section", id)), true);
    let (rect, mut resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    if let Some(tip) = tip {
        resp = resp.on_hover_text(tip);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, true, title));
    // Opening a section scrolls its contents into view once they've slid open.
    let reveal_id = ui.make_persistent_id(("detail_section_reveal", id));
    if resp.clicked() {
        state.toggle(ui);
        if state.is_open() {
            ui.data_mut(|d| d.insert_temp(reveal_id, true));
        }
    }

    let accent = theme::accent();
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let radius = CornerRadius::same(6);
    painter.rect_filled(rect, radius, accent.linear_multiply(if resp.hovered() { 0.24 } else { 0.15 }));
    let stripe = egui::Stroke::new(5.0, accent.linear_multiply(0.08));
    let mut x = rect.left() + 10.0;
    while x < rect.right() + rect.height() {
        painter.line_segment([egui::pos2(x - rect.height(), rect.bottom()), egui::pos2(x, rect.top())], stripe);
        x += 14.0;
    }
    // Mirrored in right-to-left languages: the accent edge, arrow and title start on the right.
    let rtl = i18n::rtl();
    let edge = if rtl {
        (egui::Rect::from_min_max(egui::pos2(rect.right() - 4.0, rect.top()), rect.max), CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 })
    } else {
        (egui::Rect::from_min_size(rect.min, vec2(4.0, rect.height())), CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 })
    };
    painter.rect_filled(edge.0, edge.1, accent);

    // Arrow: points towards the text's end when closed (right, or left), down when open.
    let openness = state.openness(ui.ctx());
    let start = |d: f32| if rtl { rect.right() - d } else { rect.left() + d };
    let center = egui::pos2(start(20.0), rect.center().y);
    let closed = if rtl { std::f32::consts::FRAC_PI_2 } else { -std::f32::consts::FRAC_PI_2 };
    let rot = egui::emath::Rot2::from_angle((1.0 - openness) * closed);
    let arrow = [vec2(-5.0, -3.0), vec2(5.0, -3.0), vec2(0.0, 4.0)].map(|v| center + rot * v);
    let text = ui.visuals().strong_text_color();
    painter.add(egui::Shape::convex_polygon(arrow.to_vec(), text, egui::Stroke::NONE));
    let align = if rtl { egui::Align2::RIGHT_CENTER } else { egui::Align2::LEFT_CENTER };
    painter.text(egui::pos2(start(34.0), rect.center().y), align, title, egui::FontId::proportional(16.0), text);

    // The header's button, over the header (so a click on it doesn't fold the section).
    let mut clicked = false;
    if let Some(label) = action {
        let font = if label.chars().count() == 1 { 17.0 } else { 13.0 };
        let galley = ui.painter().layout_no_wrap(label.to_owned(), egui::FontId::proportional(font), text);
        let size = vec2(galley.size().x + 12.0, rect.height() - 6.0);
        let x = if rtl { rect.left() + 6.0 + size.x / 2.0 } else { rect.right() - 4.0 - size.x / 2.0 };
        let button = egui::Rect::from_center_size(egui::pos2(x, rect.center().y), size);
        // Plain text that lights up on hover (no button frame), on one line.
        let link = egui::Button::new(RichText::new(label).size(font)).frame(false).wrap_mode(egui::TextWrapMode::Extend);
        clicked = ui.put(button, link).on_hover_cursor(CursorIcon::PointingHand).clicked();
    }
    let openness = state.openness(ui.ctx());
    let body = state.show_body_unindented(ui, |ui| {
        ui.add_space(4.0);
        add(ui);
    });
    if ui.data(|d| d.get_temp::<bool>(reveal_id).unwrap_or(false)) {
        if let Some(body) = body {
            // The header and as much of the body as fits; egui scrolls only as far as needed.
            ui.scroll_to_rect(rect.union(body.response.rect), None);
        }
        if openness >= 1.0 {
            ui.data_mut(|d| d.remove::<bool>(reveal_id));
        }
    }
    clicked
}

/// A main tab. Drawn by hand at a fixed size so nothing moves on hover: filled with `color`
/// when open, otherwise its label is tinted with it and the background lightens on hover.
fn tab_button(ui: &mut egui::Ui, label: &str, on: bool, color: Color32) -> egui::Response {
    let text_color = if on { Color32::WHITE } else { color.lerp_to_gamma(ui.visuals().text_color(), 0.35) };
    let font = egui::FontId::proportional(15.0);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, text_color);
    let size = vec2((galley.size().x + 28.0).max(110.0), 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
    let radius = CornerRadius::same(16);
    if on {
        ui.painter().rect_filled(rect, radius, color);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, radius, color.linear_multiply(0.16));
    }
    let pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(pos, galley, text_color);
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// What a log line is about, from its wording; each has an icon (and errors and warnings a colour).
#[derive(PartialEq, Clone, Copy)]
enum LogLevel {
    Error,
    Warning,
    Downloaded,
    Finished,
    Files,
    Catalogue,
    Search,
    Update,
    Info,
}

impl LogLevel {
    fn icon(self) -> &'static str {
        match self {
            LogLevel::Error => "✖",
            LogLevel::Warning => "⚠",
            LogLevel::Downloaded => "🎵",
            LogLevel::Finished => "✔",
            LogLevel::Files => "📦",
            LogLevel::Catalogue => "📚",
            LogLevel::Search => "🔍",
            LogLevel::Update => "⬆",
            LogLevel::Info => "•",
        }
    }

    fn color(self, ui: &egui::Ui) -> Color32 {
        match self {
            LogLevel::Error => theme::bad(),
            LogLevel::Warning => theme::warn(),
            LogLevel::Downloaded | LogLevel::Finished => theme::good(),
            LogLevel::Info => ui.visuals().weak_text_color(),
            _ => ui.visuals().text_color(),
        }
    }
}

fn log_level(line: &str) -> LogLevel {
    let lower = line.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    // How a line starts says the most (e.g. "Finished: ... 0 failed" isn't an error).
    if line.starts_with("FAILED") {
        LogLevel::Error
    } else if lower.contains("retrying") {
        // A failed attempt that's being retried; the final FAILED is the error.
        LogLevel::Warning
    } else if line.starts_with("Finished:") {
        LogLevel::Finished
    } else if line.starts_with("Stopped:") {
        LogLevel::Warning
    } else if line.starts_with("Downloaded") {
        if has(&["couldn't"]) { LogLevel::Warning } else { LogLevel::Downloaded }
    } else if has(&["failed", "couldn't", "error", "damaged"]) {
        LogLevel::Error
    } else if has(&["rate limit", "pausing", "paused", "incomplete", "warning"]) {
        LogLevel::Warning
    } else if line.starts_with("Extracted") || line.starts_with("Converted") {
        LogLevel::Files
    } else if lower.contains("catalogue") {
        LogLevel::Catalogue
    } else if line.starts_with("Search") || line.starts_with("Track hash") {
        LogLevel::Search
    } else if has(&["update", "version"]) {
        LogLevel::Update
    } else {
        LogLevel::Info
    }
}
pub(crate) fn pill(ui: &mut egui::Ui, on: bool, text: &str) -> egui::Response {
    let text = if on { RichText::new(text).color(theme::on_accent()).strong() } else { RichText::new(text) };
    let fill = if on { theme::accent() } else { ui.visuals().widgets.inactive.bg_fill };
    ui.add(egui::Button::new(text).fill(fill).corner_radius(CornerRadius::same(14)))
}

/// Album art, or a placeholder while it loads (or if the chart has none).
/// Shows a chart's album art (or a placeholder) and returns where it was drawn.
pub(crate) fn cover(ui: &mut egui::Ui, covers: &mut Covers, art: Option<&str>, px: f32, size: u32) -> egui::Rect {
    let radius = CornerRadius::same(if px > 100.0 { 10 } else { 5 });
    if let Some(tex) = art.and_then(|a| covers.get(a, size)) {
        return ui.add(egui::Image::new(tex).fit_to_exact_size(vec2(px, px)).corner_radius(radius)).rect;
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
    rect
}

pub(crate) fn instrument_icon(i: u8) -> &'static str {
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
/// The two kinds of link to a chart: a web link (clickable anywhere, it opens Rainbow) and the
/// rainbow:// link itself.
fn copy_link_items(ui: &mut egui::Ui, md5: &str) {
    if ui.button(t("🌐  Web link")).on_hover_text(links::share_link(md5)).clicked() {
        ui.ctx().copy_text(links::share_link(md5));
        ui.close();
    }
    if ui.button(t("🌈  rainbow:// link")).on_hover_text(links::app_link(md5)).clicked() {
        ui.ctx().copy_text(links::app_link(md5));
        ui.close();
    }
}

/// The menu's line icons (drawn, so they match), by name.
fn menu_icon(name: &str) -> egui::ImageSource<'static> {
    match name {
        "info" => egui::include_image!("../assets/icons/info.svg"),
        "folder" => egui::include_image!("../assets/icons/folder.svg"),
        "download" => egui::include_image!("../assets/icons/download.svg"),
        "play" => egui::include_image!("../assets/icons/play.svg"),
        "square" => egui::include_image!("../assets/icons/square.svg"),
        "square-check" => egui::include_image!("../assets/icons/square-check.svg"),
        "star" => egui::include_image!("../assets/icons/star.svg"),
        "star-off" => egui::include_image!("../assets/icons/star-off.svg"),
        "link" => egui::include_image!("../assets/icons/link.svg"),
        "copy" => egui::include_image!("../assets/icons/copy.svg"),
        "user" => egui::include_image!("../assets/icons/user.svg"),
        _ => egui::include_image!("../assets/icons/pencil.svg"),
    }
}

/// Text without its leading emoji (the translations have one; menus draw an icon instead).
fn without_icon(text: &str) -> String {
    // The icon is a symbol or two before a double space ("ℹ  Show details").
    match text.split_once("  ") {
        Some((first, rest)) if first.chars().count() <= 2 => rest.trim_start().to_owned(),
        _ => text.to_owned(),
    }
}

/// A menu item: a line icon and the text.
fn menu_item(ui: &egui::Ui, icon: &str, text: &str) -> egui::Button<'static> {
    let image = egui::Image::new(menu_icon(icon)).fit_to_exact_size(vec2(15.0, 15.0)).tint(ui.visuals().text_color());
    egui::Button::image_and_text(image, without_icon(text))
}

fn row_context_menu(ui: &mut egui::Ui, c: &Chart, i: usize, checked: bool, downloaded: bool, favourite: bool, action: &mut Option<RowAction>) {
    ui.set_min_width(220.0);
    ui.spacing_mut().icon_spacing = 10.0;
    let mut pick = |ui: &mut egui::Ui, icon: &str, label: &str, a: RowAction| {
        if ui.add(menu_item(ui, icon, label)).clicked() {
            *action = Some(a);
            ui.close();
        }
    };
    pick(ui, "info", &t("ℹ  Show details"), RowAction::Details(i));
    if downloaded {
        // (A web page can't open the file manager.)
        #[cfg(not(target_arch = "wasm32"))]
        pick(ui, "folder", &t("📂  Show in folder"), RowAction::Reveal(i));
    } else {
        pick(ui, "download", &t("⬇  Download"), RowAction::Download(i));
    }
    pick(ui, "play", &t("▶  Preview"), RowAction::Preview(i));
    if checked {
        pick(ui, "square", &t("☐  Deselect"), RowAction::Toggle(i));
    } else {
        pick(ui, "square-check", &t("☑  Select"), RowAction::Toggle(i));
    }
    if favourite {
        pick(ui, "star-off", &t("☆  Remove from favourites"), RowAction::Favourite(i));
    } else {
        pick(ui, "star", &t("⭐  Add to favourites"), RowAction::Favourite(i));
    }
    // Submenus show a caret.
    egui::containers::menu::SubMenuButton::from_button(menu_item(ui, "link", &t("🔗  Copy link")).right_text("⏵")).ui(ui, |ui| copy_link_items(ui, &c.md5));
    ui.separator();
    egui::containers::menu::SubMenuButton::from_button(menu_item(ui, "copy", &t("📋  Copy")).right_text("⏵")).ui(ui, |ui| {
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
        pick(ui, "user", &t("🔍  More by this artist"), RowAction::Nav(Nav::Text(1, a.clone(), true)));
    }
    if let Some(ch) = &c.charter {
        pick(ui, "pencil", &t("🔍  More by this charter"), RowAction::Nav(Nav::Text(5, ch.clone(), false)));
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
        // (In the browser, pasting is Ctrl+V: pages can't read the clipboard on their own.)
        #[cfg(not(target_arch = "wasm32"))]
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

/// A round ✖ inside the right end of a text box (`field`). Returns true if clicked.
/// A difficulty's name after a dot in its colour (green Easy up to purple Expert); a hollow
/// dot for any difficulty.
fn difficulty_label(d: Option<u8>) -> egui::text::LayoutJob {
    let (dot, color, name) = match d {
        Some(d) => {
            // DIFFICULTIES runs Expert, Hard, Medium, Easy.
            let colors = [Color32::from_rgb(170, 95, 235), Color32::from_rgb(235, 90, 60), Color32::from_rgb(240, 190, 40), Color32::from_rgb(60, 190, 90)];
            ("●", colors[d as usize % 4], t(DIFFICULTIES[d as usize].1))
        }
        None => ("○", Color32::from_gray(150), t("Any Difficulty")),
    };
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::proportional(14.0);
    job.append(dot, 0.0, egui::TextFormat { font_id: font.clone(), color, ..Default::default() });
    // PLACEHOLDER takes the widget's own text colour.
    job.append(&name, 6.0, egui::TextFormat { font_id: font, color: Color32::PLACEHOLDER, ..Default::default() });
    job
}

/// Where a dragged column would go: the x of the boundary between headers nearest `x`.
fn column_gap(headers: &[(Col, egui::Rect)], x: f32) -> Option<f32> {
    let mut edges: Vec<f32> = headers.iter().map(|h| h.1.left()).collect();
    edges.push(headers.last()?.1.right());
    edges.into_iter().min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs()))
}

/// The fullest of a bar's levels of compactness (0 = in full) that fits in `width`, from the
/// widths each needed when last drawn; a level not measured yet is tried. Returns one past the
/// last level if none fits.
fn pick_level(widths: &[f32], width: f32) -> usize {
    widths.iter().position(|&w| w == 0.0 || w <= width).unwrap_or(widths.len() - 1)
}

/// A button in `base` with a top-to-bottom gradient and a small soft shadow under it, painted
/// behind the button (whose own fill is cleared).
fn glossy(ui: &mut egui::Ui, button: egui::Button, base: Color32, enabled: bool) -> egui::Response {
    let behind = ui.painter().add(egui::Shape::Noop);
    let resp = ui.add_enabled(enabled, button.fill(Color32::TRANSPARENT).stroke(egui::Stroke::NONE));
    let r = resp.rect;
    let radius = ui.visuals().widgets.inactive.corner_radius;
    let base = if !enabled {
        base.gamma_multiply(0.45)
    } else if resp.hovered() {
        base.lerp_to_gamma(Color32::WHITE, 0.12)
    } else {
        base
    };
    let shadow = egui::epaint::Shadow { offset: [0, 2], blur: 6, spread: 0, color: Color32::from_black_alpha(if enabled { 90 } else { 30 }) };
    let top = base.lerp_to_gamma(Color32::WHITE, 0.16);
    let bottom = base.lerp_to_gamma(Color32::BLACK, 0.14);
    ui.painter().set(behind, egui::Shape::Vec(vec![shadow.as_shape(r, radius).into(), theme::vertical_gradient(r, radius.nw as f32, top, bottom)]));
    resp
}

/// The download icon (an arrow into a tray), drawn rather than the boxy ⬇ character.
fn download_icon(size: f32, color: Color32) -> egui::Image<'static> {
    egui::Image::new(egui::include_image!("../assets/icons/download.svg")).fit_to_exact_size(vec2(size, size)).tint(color)
}

/// Text without a leading ⬇ (the translations have it; the icon is drawn instead).
fn without_arrow(text: &str) -> String {
    text.trim_start_matches('⬇').trim_start().to_owned()
}

/// A button with the download icon before its text.
fn icon_button(text: &str, color: Color32) -> egui::Button<'static> {
    egui::Button::image_and_text(download_icon(14.0, color), without_arrow(text))
}

/// The green download button, with the icon.
fn download_button(text: &str) -> egui::Button<'static> {
    let fill = theme::good().lerp_to_gamma(Color32::BLACK, 0.2);
    let on = theme::text_on(fill);
    egui::Button::image_and_text(download_icon(15.0, on), RichText::new(without_arrow(text)).color(on).strong()).fill(fill)
}

/// A place in the app for Back and Forward: a tab, the search, and the player profile shown.
#[derive(Clone, PartialEq)]
struct Place {
    tab: Tab,
    filters: Filters,
    profile: Option<(u64, String)>,
}

/// A search as Back and Forward remember it: without data looked up for it along the way.
fn nav_key(f: &Filters) -> Filters {
    Filters { played: Default::default(), popular: Vec::new(), track_md5s: None, ..f.clone() }
}

/// A Back (`left`) or Forward button: a thick arrow on a rounded square, greyed out when
/// there's nowhere to go. In right-to-left languages the arrows point the other way.
fn nav_arrow(ui: &mut egui::Ui, left: bool, enabled: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, if left { t("Back") } else { t("Forward") }));
    let visuals = ui.visuals();
    let hovered = enabled && resp.hovered();
    let fill = if hovered { visuals.widgets.hovered.bg_fill } else { visuals.widgets.inactive.bg_fill };
    ui.painter().rect_filled(rect, CornerRadius::same(8), if enabled { fill } else { fill.linear_multiply(0.5) });
    let color = if !enabled {
        visuals.weak_text_color().linear_multiply(0.5)
    } else if hovered {
        theme::accent_hover()
    } else {
        visuals.text_color()
    };
    let dir = if left != i18n::rtl() { -1.0 } else { 1.0 };
    let c = rect.center();
    // Shaft, then a solid head.
    let tip = c + vec2(9.0 * dir, 0.0);
    ui.painter().line_segment([c + vec2(-8.0 * dir, 0.0), c + vec2(1.0 * dir, 0.0)], egui::Stroke::new(4.0, color));
    let head = vec![tip, c + vec2(0.0, -7.5), c + vec2(0.0, 7.5)];
    ui.painter().add(egui::Shape::convex_polygon(head, color, egui::Stroke::NONE));
    if enabled { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp }
}

/// Orders the song list can be in (the order menu and the Default order setting).
const SORTS: [(SortKey, &str); 9] = [
    (SortKey::Newest, n("Newest on Encore")),
    (SortKey::Popular, n("Most played on Clone Hero")),
    (SortKey::Uploaded, n("Recently updated")),
    (SortKey::Name, n("Name")),
    (SortKey::Artist, n("Artist")),
    (SortKey::Charter, n("Charter")),
    (SortKey::Year, n("Year")),
    (SortKey::Length, n("Length")),
    (SortKey::Difficulty, n("Difficulty")),
];

pub(crate) fn clear_button(ui: &mut egui::Ui, field: egui::Rect, id: &str) -> bool {
    let size = 22.0;
    let rect = egui::Rect::from_center_size(
        egui::pos2(field.right() - 6.0 - size / 2.0, field.center().y),
        vec2(size, size),
    );
    let resp = ui.interact(rect, egui::Id::new(("search_clear", id)), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t("Clear search"))
    });
    let painter = ui.painter();
    let color = if resp.hovered() {
        painter.circle_filled(
            rect.center(),
            size / 2.0,
            ui.visuals().widgets.hovered.bg_fill,
        );
        theme::accent_hover()
    } else {
        ui.visuals().weak_text_color()
    };
    let (c, r) = (rect.center(), 4.5);
    let stroke = egui::Stroke::new(1.8, color);
    painter.line_segment([c + vec2(-r, -r), c + vec2(r, r)], stroke);
    painter.line_segment([c + vec2(-r, r), c + vec2(r, -r)], stroke);
    resp.on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(t("Clear search"))
        .clicked()
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
pub(crate) fn tags(ui: &mut egui::Ui, c: &Chart, downloaded: bool) {
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
}

/// A two-handle slider for a range from `lo` to `hi`. It follows the Min and Max boxes (an empty
/// or out-of-range box puts its handle at that end); dragging a handle writes the box, and
/// dragging it to the end clears it (no limit).
/// A rough length of time: "45 s", "12 min", "3 h 20 min", "2 days 5 h".
fn duration_words(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    match s {
        0..60 => tr!("{n} s", n = s),
        60..3600 => tr!("{n} min", n = s.div_ceil(60)),
        3600..86400 => tr!("{h} h {m} min", h = s / 3600, m = s % 3600 / 60),
        _ => trn!(s / 86400, "{d} day {h} h", "{d} days {h} h", d = s / 86400, h = s % 86400 / 3600),
    }
}

fn range_slider(ui: &mut egui::Ui, label: &str, r: &mut Range, lo: f64, hi: f64, step: f64) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
    let track = rect.shrink2(vec2(8.0, 0.0));
    let x_of = |v: f64| track.left() + ((v.clamp(lo, hi) - lo) / (hi - lo)) as f32 * track.width();
    let v_of = |x: f32| {
        let v = lo + ((x - track.left()) / track.width()).clamp(0.0, 1.0) as f64 * (hi - lo);
        (v / step).round() * step
    };
    let min = r.min.trim().parse::<f64>().ok();
    let max = r.max.trim().parse::<f64>().ok();
    let (a, b) = (x_of(min.unwrap_or(lo)), x_of(max.unwrap_or(hi)));
    let accent = theme::accent();
    let p = ui.painter();
    let y = rect.center().y;
    p.line_segment([egui::pos2(track.left(), y), egui::pos2(track.right(), y)], egui::Stroke::new(4.0, ui.visuals().widgets.inactive.bg_fill));
    p.line_segment([egui::pos2(a, y), egui::pos2(b, y)], egui::Stroke::new(4.0, accent.linear_multiply(0.8)));
    let fmt = |v: f64| if step >= 1.0 { format!("{v:.0}") } else { format!("{v}") };
    for (k, x) in [a, b].into_iter().enumerate() {
        let handle = egui::Rect::from_center_size(egui::pos2(x, y), vec2(16.0, 16.0));
        let resp = ui.interact(handle, ui.id().with((label, k)), Sense::drag());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, if k == 0 { t("Min") } else { t("Max") }));
        let hot = resp.hovered() || resp.dragged();
        ui.painter().circle(egui::pos2(x, y), if hot { 7.5 } else { 6.5 }, accent, egui::Stroke::new(2.0, Color32::WHITE));
        let resp = resp.on_hover_cursor(CursorIcon::Grab);
        if resp.dragged()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            // Handles can't cross; at the ends the box is cleared.
            if k == 0 {
                let v = v_of(pos.x).min(max.unwrap_or(hi));
                r.min = if v <= lo { String::new() } else { fmt(v) };
            } else {
                let v = v_of(pos.x).max(min.unwrap_or(lo));
                r.max = if v >= hi { String::new() } else { fmt(v) };
            }
        }
    }
}

/// Opens the file manager with `path` selected (or its folder, where selecting isn't supported).
fn reveal(path: &std::path::Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let native = path.to_string_lossy().replace('/', "\\");
        // Explorer wants `/select,"C:\path with spaces"`; Rust's own quoting would wrap the
        // whole thing, which Explorer can't read, so it opens Documents instead.
        let _ = std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{native}\""))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(not(any(windows, target_os = "macos")))]
    if let Some(parent) = path.parent() {
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
    /// Applies the scroll speed setting to wheel and touchpad scrolling (but not Ctrl+wheel
    /// zooming).
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let speed = self.settings.scroll_speed;
        if (speed - 1.0).abs() < f32::EPSILON {
            return;
        }
        for event in &mut raw_input.events {
            if let egui::Event::MouseWheel { delta, modifiers, .. } = event
                && !modifiers.command
                && !modifiers.ctrl
            {
                *delta *= speed;
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.handle_messages(ui.ctx());
        self.covers.poll();
        self.auto_unzip(false);
        let ctx = ui.ctx().clone();
        self.handle_drops(&ctx);
        self.keyboard(&ctx);
        self.guard_quit(&ctx);

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
        {
            // The log's resize line looks like the table's column bars (thick, the same colour).
            let normal = ui.style().clone();
            let w = &mut ui.visuals_mut().widgets;
            w.noninteractive.bg_stroke.width = 3.0;
            w.hovered.fg_stroke = w.hovered.bg_stroke;
            w.hovered.fg_stroke.width = 4.0;
            w.active.fg_stroke = w.active.bg_stroke;
            w.active.fg_stroke.width = 4.0;
            ui.style_mut().interaction.resize_grab_radius_side = 7.0;
            self.log_panel(ui, &normal);
            ui.set_style(normal);
        }
        match self.tab {
            Tab::Browse => {
                egui::Panel::top("search").show(ui, |ui| {
                    ui.add_space(8.0);
                    self.search_bar(ui);
                    // The Advanced search panel opens from here, above the active-filter chips,
                    // so a chip appearing as you set a filter doesn't push the panel down.
                    self.search_bottom = ui.min_rect().bottom() + 4.0;
                    self.active_filters(ui);
                    // The same gap below as above (the row spacing already adds some).
                    ui.add_space((8.0 - ui.spacing().item_spacing.y).max(0.0));
                });
                if self.detail.is_some() {
                    // A fixed width (no resize handle).
                    let side = if i18n::rtl() { egui::Panel::left("detail") } else { egui::Panel::right("detail") };
                    side.resizable(false).exact_size(350.0).show(ui, |ui| self.detail_panel(ui));
                }
                self.live_search();
                self.refresh_results();
                egui::CentralPanel::default_margins().show(ui, |ui| self.results(ui));
                self.advanced_overlay(&ctx);
            }
            Tab::Players => {
                if self.players.players_tab(ui, self.prev_tab.is_some(), &self.charts, &mut self.covers)
                    && let Some(tab) = self.prev_tab.take()
                {
                    self.tab = tab;
                }
            }
            Tab::Leaderboards => {
                egui::CentralPanel::default_margins().show(ui, |ui| self.players.leaderboards_tab(ui, &self.charts, &mut self.covers));
            }
            Tab::Settings => {
                egui::CentralPanel::default_margins().show(ui, |ui| self.settings_tab(ui));
            }
        }
        // Pop-ups sit over a blurred copy of the app; they're held back for the one frame the
        // screenshot for the blur is taken.
        // Requests from the player profile: open another player, or search the catalogue.
        self.players.take_requests(&ctx);
        self.open_pending_link();
        self.track_history(&ctx);
        if std::mem::take(&mut self.focus_window) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if let Some((text, link)) = ctx.data_mut(|d| d.remove_temp::<(String, Option<(String, String)>)>(players::toast_id())) {
            self.toast = Some((text, link, Instant::now()));
        }
        self.show_toast(&ctx);
        if let Some(q) = ctx.data_mut(|d| d.remove_temp::<String>(players::search_request_id())) {
            // A fresh search: just this text, every other filter cleared (the order stays). A
            // chart's MD5 (New on Encore) also opens its details, like a shared link.
            if filter::hash_in(&q).is_some_and(|h| h.len() == 32) {
                self.pending_link = Some(q.to_lowercase());
            } else {
                let f = &mut self.filters;
                *f = Filters { query: q, sort: f.sort, descending: f.descending, ..Filters::default() };
                // A song found from Players or Leaderboards opens in the details panel (the top
                // result if there are several).
                self.open_first = true;
            }
            self.tab = Tab::Browse;
            self.live_edited = Instant::now();
        }
        if self.players.take_opened() && self.tab != Tab::Players {
            // Back on the Players tab returns here.
            self.prev_tab = Some(self.tab);
            self.tab = Tab::Players;
        }
        self.players.set_density(self.settings.density);
        preview::SPEED_PERCENT.store(self.settings.speed_percent, Ordering::Relaxed);
        self.updates.poll(&ctx, self.settings.auto_update);
        if self.modal_backdrop(&ctx) {
            self.updates.dialog(&ctx);
            self.report_window(&ctx);
            if let Some(p) = &mut self.preview
                && !p.show(&ctx)
            {
                self.preview = None;
            }
            self.confirm_dialog(&ctx);
            self.shortcuts_window(&ctx);
            self.quit_dialog(&ctx);
        }
        self.drop_overlay(&ctx);
        self.sync_settings(&ctx);

        let building = matches!(self.cat_status, CatStatus::Building { .. });
        if self.live_searching || self.job.is_some() || building || self.track_lookup {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if self.settings.auto_extract || self.settings.auto_convert {
            ctx.request_repaint_after(Duration::from_secs(10));
        }
        if !self.filters.query.is_empty() && self.live_edited.elapsed() < LIVE_SEARCH_DEBOUNCE * 2 {
            // Wake up after the typing pause so the live search (or hash lookup) fires.
            ctx.request_repaint_after(LIVE_SEARCH_DEBOUNCE);
        }
    }
}

/// The search box's syntax, shown when hovering the ❔ next to it.
fn search_syntax_help(ui: &mut egui::Ui) {
    ui.set_max_width(420.0);
    ui.strong(t("Search tips"));
    ui.add_space(4.0);
    let rows: [(&str, &str); 12] = [
        ("\"one\"", n("Exact words or phrase")),
        ("-live", n("Leave out results with this word")),
        (
            "artist:queen  charter:harmonix",
            n("Search one field: name, artist, album, genre, charter"),
        ),
        (
            "year:1999  year:1990s  year:2000..2005",
            n("A year, decade or range"),
        ),
        ("length:>5  length:3..6", n("Song length in minutes")),
        ("diff:5..6", n("Difficulty rating (0–6)")),
        ("notes:>1000", n("Note count (for the instrument and difficulty picked)")),
        ("user:nadxa  u:nadxa", n("Charts a Clone Hero player has played recently or most")),
        (
            "has:lyrics  has:solo  has:video",
            n("Charts with this feature"),
        ),
        (
            "has:drums  has:keys  type:pro",
            n("Instrument parts and drum type"),
        ),
        (
            "-has:video  -artist:\"tribute\"",
            n("Put - in front of any of these to exclude"),
        ),
        ("a: the killers  ch:  y:  d:  l:  n:", n("Short forms: artist, charter, year, difficulty, length, name; a space after the colon is fine")),
    ];
    egui::Grid::new("search_syntax")
        .num_columns(2)
        .striped(true)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            for (example, what) in rows {
                ui.label(RichText::new(example).monospace());
                ui.label(t(what));
                ui.end_row();
            }
        });
}

fn ago(unix: u64) -> String {
    match now().saturating_sub(unix) {
        s if s < 90 => t("just now"),
        s if s < 90 * 60 => tr!("{n} min ago", n = s / 60),
        s if s < 36 * 3600 => tr!("{n} h ago", n = s / 3600),
        s => trn!(s / 86400, "{n} day ago", "{n} days ago", n = s / 86400),
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
    async fn wait(&self, d: Duration) {
        let end = Instant::now() + d;
        while !self.stopped() && Instant::now() < end {
            task::sleep(Duration::from_millis(100).min(end - Instant::now())).await;
        }
    }

    fn send_queue(&self) {
        let _ = self.tx.send(Msg::Queue { checked: self.checked, downloaded: self.downloaded, failed: self.failed });
    }

    /// Downloads one chart unless it's already in the manifest. Returns true if it hit the network.
    async fn download(&mut self, chart: &Chart) -> bool {
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
        let result = self
            .client
            .download(chart, &self.settings, |done, total| {
                let _ = tx.send(Msg::FileProgress { title: title.clone(), done, total });
            })
            .await;
        let line = match result {
            Ok(path) => {
                self.downloaded += 1;
                // The web version also adds it to the songs folder's own download list (which
                // the desktop app reads).
                #[cfg(target_arch = "wasm32")]
                if crate::web::folder_name().is_some() && crate::web::folder_allowed().await {
                    let line = Manifest::line(chart, &path);
                    let _ = crate::web::folder_write(api::MANIFEST_NAME, line.as_bytes(), true).await;
                }
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
