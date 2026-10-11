//! Charts from RhythmVerse (rhythmverse.co), searched alongside Encore's in the Charts tab.
//!
//! RhythmVerse hosts Clone Hero charts (chart folders in .zip or .7z files) and Rock Band 3 songs
//! (Xbox CON files, which Rainbow converts; see `rockband`). Its song list comes from its API:
//!   POST https://rhythmverse.co/api/<list>/songfiles/list
//! with form fields for the page, its size and the order. "chm" lists what Clone Hero plays as
//! it is, "rb3xbox" the Rock Band songs.
//!
//! Rainbow doesn't ask RhythmVerse for the list itself. The GitHub workflow that publishes the
//! web version keeps a copy (`--update-rhythmverse`): it fetches the whole list once a week and
//! what's changed every hour, and publishes it as two files next to the web version:
//!   rhythmverse.json         every chart
//!   rhythmverse-recent.json  what changed in the last few days
//! Rainbow downloads the whole list once, then just the recent changes, so RhythmVerse sees one
//! visitor however many people use Rainbow. Downloading a chart still goes to RhythmVerse (or
//! wherever its file is). On the web, RhythmVerse doesn't let other sites fetch its files, so
//! its charts open on RhythmVerse instead.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
#[cfg(not(target_arch = "wasm32"))]
use anyhow::bail;
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use serde_json::Value;
use web_time::{SystemTime, UNIX_EPOCH};

use crate::api::{Chart, RvFile};
#[cfg(not(target_arch = "wasm32"))]
use crate::api;
use crate::net::Http;
use crate::store;

pub const SITE: &str = "https://rhythmverse.co";
/// RhythmVerse's song lists Rainbow covers.
#[cfg(not(target_arch = "wasm32"))]
const LISTS: [&str; 2] = ["chm", "rb3xbox"];
/// Bump when the saved format changes; older copies are downloaded again.
const VERSION: u32 = 1;
/// How far back `rhythmverse-recent.json` goes. A copy last updated longer ago than this
/// downloads the whole list again.
pub const RECENT_DAYS: u64 = 3;
/// How often a full fetch is made (which also drops charts that were removed).
#[cfg(not(target_arch = "wasm32"))]
const FULL_EVERY_DAYS: u64 = 7;
/// How often Rainbow looks for changes (the published copy changes hourly).
pub const CHECK_EVERY_SECS: u64 = 3600;

/// RhythmVerse's instrument ids, and where each goes in `api::INSTRUMENTS`.
#[cfg(not(target_arch = "wasm32"))]
const PARTS: [(&str, usize); 9] = [
    ("guitar", 0),
    ("guitar_coop", 1),
    ("rhythm", 2),
    ("bass", 3),
    ("drums", 4),
    ("prodrums", 4),
    ("keys", 5),
    ("guitarghl", 6),
    ("bassghl", 9),
];

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// RhythmVerse's list of charts (a saved or published copy).
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Catalogue {
    pub version: u32,
    /// When the whole list was last fetched (Unix time). Copies from an older fetch can't be
    /// brought up to date from the recent changes, so they're replaced.
    pub generation: u64,
    /// When it was last brought up to date (Unix time).
    pub updated: u64,
    /// Per list, the newest update time seen ("YYYY-MM-DD HH:MM:SS"), where the next update
    /// starts from.
    #[serde(default)]
    pub newest: BTreeMap<String, String>,
    /// Newest first.
    pub charts: Vec<Chart>,
}

pub fn path() -> std::path::PathBuf {
    crate::catalogue::data_dir().join("rhythmverse.json")
}

/// Where the published copies are.
fn published(name: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    return crate::web::page_url(name);
    #[cfg(not(target_arch = "wasm32"))]
    return format!("{}{name}", crate::links::WEB_APP);
}

impl Catalogue {
    pub fn load() -> Option<Self> {
        let cat: Self = serde_json::from_slice(&store::read(&path())?).ok()?;
        (cat.version == VERSION).then_some(cat)
    }

    pub fn save_to(&self, file: &std::path::Path) -> Result<()> {
        store::write(file, &serde_json::to_vec(self)?)?;
        Ok(())
    }

    async fn fetch(http: &Http, name: &str) -> Result<Self> {
        let cat: Self = http.get(&published(name)).send().await?.error_for_status()?.json().await?;
        anyhow::ensure!(cat.version == VERSION, "the published RhythmVerse list is in a format this version doesn't read");
        Ok(cat)
    }

    /// Brings a saved copy (or none) up to date from the published ones, and saves it. Returns
    /// the list and how many charts are new.
    pub async fn refresh(saved: Option<Self>) -> Result<(Self, usize)> {
        let http = Http::new(None);
        let full = async || -> Result<(Self, usize)> {
            let cat = Self::fetch(&http, "rhythmverse.json").await?;
            cat.save_to(&path())?;
            let n = cat.charts.len();
            Ok((cat, n))
        };
        let Some(mut cat) = saved else { return full().await };
        if now().saturating_sub(cat.updated) > RECENT_DAYS * 86400 {
            return full().await;
        }
        let recent = Self::fetch(&http, "rhythmverse-recent.json").await?;
        if recent.generation != cat.generation {
            return full().await;
        }
        let added = cat.merge(recent.charts);
        cat.updated = recent.updated;
        cat.newest = recent.newest;
        cat.save_to(&path())?;
        Ok((cat, added))
    }

    /// Adds or replaces charts, keeping the list newest first. Returns how many were new.
    fn merge(&mut self, charts: Vec<Chart>) -> usize {
        let mut index: HashMap<String, usize> = self.charts.iter().enumerate().map(|(i, c)| (c.md5.clone(), i)).collect();
        let mut added = 0;
        for c in charts {
            match index.get(&c.md5) {
                Some(&i) => self.charts[i] = c,
                None => {
                    index.insert(c.md5.clone(), self.charts.len());
                    self.charts.push(c);
                    added += 1;
                }
            }
        }
        self.charts.sort_by(|a, b| b.modified.cmp(&a.modified));
        added
    }

    /// The charts that changed in the last `RECENT_DAYS`, as published for clients to merge.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn recent(&self) -> Self {
        let since = jiff::Timestamp::from_second(self.updated.saturating_sub(RECENT_DAYS * 86400) as i64)
            .map(|t| t.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_default();
        let charts = self.charts.iter().filter(|c| c.modified.as_deref().is_some_and(|m| m >= since.as_str())).cloned().collect();
        Self { charts, ..self.clone() }
    }
}

// ─────────────────────────────── Reading RhythmVerse's API ───────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => api::strip_tags(s.trim()).trim().to_owned(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// The first of `values` that isn't empty.
#[cfg(not(target_arch = "wasm32"))]
fn first(values: &[&Value]) -> Option<String> {
    values.iter().map(|v| text(v)).find(|s| !s.is_empty())
}

#[cfg(not(target_arch = "wasm32"))]
fn number(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn yes(v: &Value) -> bool {
    number(v).is_some_and(|n| n != 0) || v.as_bool() == Some(true)
}

#[cfg(not(target_arch = "wasm32"))]
fn absolute(path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") { path.to_owned() } else { format!("{SITE}{path}") }
}

/// The page for a chart on RhythmVerse.
pub fn page(rv: &RvFile) -> String {
    format!("{SITE}/songfile/{}", rv.id)
}

/// Converts one song from RhythmVerse's list, or None if it's been removed or has no file.
#[cfg(not(target_arch = "wasm32"))]
pub fn parse(v: &Value) -> Option<Chart> {
    let (d, f) = (&v["data"], &v["file"]);
    if ["deleted", "retired", "destroyed", "record_hidden", "hidden"].iter().any(|k| yes(&f[*k])) {
        return None;
    }
    let id = f["file_id"].as_str()?.to_owned();
    // (Ids look like "d565fcc2ef7c4ef48de5b5be66ebd375", or "595fa143b6b0e7.69372484" for older
    // ones; they're used in file names, so nothing else is allowed.)
    if id.is_empty() || id.starts_with('.') || !id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
        return None;
    }
    let external = text(&f["external_url"]);
    let url = if external.is_empty() { text(&f["download_url"]) } else { external };
    // (Official Rock Band songs are listed with a link to buy them on the Xbox store: nothing to
    // download.)
    if url.is_empty() || url.contains("marketplace.xbox.com") || url.contains("store.xbox.com") {
        return None;
    }
    let mut diffs = [-1i8; 10];
    let mut pro_drums = yes(&f["pro_drums"]);
    for (part, k) in PARTS {
        // RhythmVerse's tiers run from 1 (warm-up, no dots) to 7 (devil); -1 is "not charted".
        if let Some(tier) = number(&f[format!("diff_{part}")]).filter(|t| *t >= 1) {
            diffs[k] = diffs[k].max((tier - 1).clamp(0, 6) as i8);
            pro_drums |= part == "prodrums";
        }
    }
    let format = text(&f["gameformat"]);
    // (Rock Band's drums are 4-lane pro.)
    let drum_type = (diffs[api::DRUMS as usize] >= 0).then_some(if pro_drums || format.starts_with("rb") { 1 } else { 0 });
    let has_vocals = number(&f["diff_vocals"]).is_some_and(|t| t >= 1);
    // "2026-10-11 10:10:08" → "2026-10-11T10:10:08Z", like Encore's dates.
    // (Older songs have "0000-00-00 00:00:00" for some of these.)
    let modified = [&f["update_date"], &f["file_updated"], &f["release_date"], &f["record_created"], &f["upload_date"]]
        .iter()
        .map(|v| text(v))
        .find(|m| m.len() >= 10 && !m.starts_with("0000"))
        .map(|m| {
        let m = m.replace(' ', "T");
        if m.ends_with('Z') { m } else { format!("{m}Z") }
    });
    // One file can hold several songs (a pack), each listed on its own.
    let md5 = match number(&d["song_id"]) {
        Some(song) => format!("rv-{id}-{song}"),
        None => format!("rv-{id}"),
    };
    Some(Chart {
        md5,
        name: first(&[&f["file_title"], &d["title"]]),
        artist: first(&[&f["file_artist"], &d["artist"]]),
        album: first(&[&f["file_album"], &d["album"]]),
        genre: first(&[&f["file_genre"], &d["genre"]]),
        year: number(&f["file_year"]).or_else(|| number(&d["year"])).filter(|y| *y > 0).map(|y| y.to_string()),
        charter: first(&[&f["author"]["name"], &f["user"]]),
        art: first(&[&f["album_art"], &d["album_art"]]).map(|a| absolute(&a)),
        modified,
        length_ms: number(&f["file_song_length"]).or_else(|| number(&d["song_length"])).filter(|l| *l > 0).map(|l| l as u64 * 1000),
        diffs,
        drum_type,
        flags: if has_vocals { api::flag::VOCALS } else { 0 },
        rv: Some(RvFile { id, format, url, file: text(&f["file_name"]) }),
        ..Default::default()
    })
}

/// Percent-encodes a URL path (RhythmVerse's download paths have spaces and brackets in them).
fn encode_path(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A Google Drive file's id from a sharing link (folders don't count).
fn drive_id(url: &str) -> Option<String> {
    let drive = ["drive.google.com/", "docs.google.com/", "drive.usercontent.google.com/"].iter().any(|h| url.contains(h));
    if !drive || url.contains("/folders/") {
        return None;
    }
    let id = match url.split("/file/d/").nth(1) {
        Some(rest) => rest.split(['/', '?', '#']).next()?,
        None => url.split(['?', '&']).find_map(|p| p.strip_prefix("id="))?,
    };
    let ok = id.len() >= 10 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| id.to_owned())
}

/// Where Rainbow can download a chart's file from, if it can: RhythmVerse's own files and
/// Google Drive files. Others (Drive folders, Mega, .rar files…) are opened in the browser.
pub fn file_url(rv: &RvFile) -> Option<String> {
    if rv.file.to_lowercase().ends_with(".rar") {
        return None;
    }
    if rv.url.starts_with('/') {
        return Some(format!("{SITE}{}", encode_path(&rv.url)));
    }
    if let Some(id) = drive_id(&rv.url) {
        return Some(format!("https://drive.usercontent.google.com/download?id={id}&export=download&confirm=t"));
    }
    // Dropbox links download the file itself with dl=1.
    if rv.url.starts_with("https://www.dropbox.com/") || rv.url.starts_with("https://dropbox.com/") {
        let url = rv.url.replace("dl=0", "dl=1");
        return Some(if url.contains("dl=1") { url } else if url.contains('?') { format!("{url}&dl=1") } else { format!("{url}?dl=1") });
    }
    None
}

/// Whether Rainbow can install this chart itself (otherwise its page is opened).
pub fn installable(rv: &RvFile) -> bool {
    !cfg!(target_arch = "wasm32") && file_url(rv).is_some()
}

/// Fetches pages of one of RhythmVerse's lists, newest change first, until `enough` (given
/// each page's update times) says so.
#[cfg(not(target_arch = "wasm32"))]
async fn list(http: &Http, list: &str, per_page: u32, mut enough: impl FnMut(&[String]) -> bool, log: &dyn Fn(String)) -> Result<Vec<Chart>> {
    let url = format!("{SITE}/api/{list}/songfiles/list");
    let mut out = Vec::new();
    let mut page = 1u64;
    loop {
        let body = format!("data_type=full&page={page}&records={per_page}&sort%5B0%5D%5Bsort_by%5D=update_date&sort%5B0%5D%5Bsort_order%5D=DESC");
        let mut tries = 0;
        let v: Value = loop {
            let sent = http.post(&url).header("Content-Type", "application/x-www-form-urlencoded").body(body.clone()).send().await;
            let got: Result<Value> = match sent.and_then(|r| r.error_for_status()) {
                Ok(r) => r.json().await.map_err(Into::into),
                Err(e) => Err(e.into()),
            };
            match got {
                Ok(v) => break v,
                Err(e) if tries < 3 => {
                    tries += 1;
                    log(format!("{list} page {page} failed ({e:#}); trying again"));
                    crate::task::sleep(web_time::Duration::from_secs(10 * tries)).await;
                }
                Err(e) => return Err(e),
            }
        };
        if v["data"].is_null() {
            bail!("RhythmVerse's {list} list didn't come back");
        }
        let songs = v["data"]["songs"].as_array().cloned().unwrap_or_default();
        let total = number(&v["data"]["records"]["total_filtered"]).unwrap_or(0).max(0) as u64;
        let dates: Vec<String> = songs.iter().map(|s| text(&s["file"]["update_date"])).collect();
        log(format!("{list} page {page}: {} songs ({} of {total})", songs.len(), (page * per_page as u64).min(total)));
        let done = songs.is_empty() || enough(&dates) || page * per_page as u64 >= total;
        out.extend(songs.iter().filter_map(parse));
        if done {
            return Ok(out);
        }
        page += 1;
        // (Politely: one request at a time, with a pause.)
        crate::task::sleep(web_time::Duration::from_secs(2)).await;
    }
}

/// Brings `cat` up to date from RhythmVerse itself: everything if it's a week since the last
/// full fetch (or `full`), otherwise what changed since the last update. Returns how many
/// charts are new. For the GitHub workflow (`--update-rhythmverse`); Rainbow itself reads the
/// published copy.
#[cfg(not(target_arch = "wasm32"))]
pub async fn crawl(cat: &mut Catalogue, full: bool, log: &dyn Fn(String)) -> Result<usize> {
    let http = Http::new(Some(web_time::Duration::from_secs(300)));
    let full = full || cat.version != VERSION || now().saturating_sub(cat.generation) > FULL_EVERY_DAYS * 86400;
    log(format!("Fetching {} from RhythmVerse", if full { "every chart" } else { "the newest changes" }));
    let before = if full { 0 } else { cat.charts.len() };
    let mut newest = BTreeMap::new();
    let mut fetched = Vec::new();
    for name in LISTS {
        let since = cat.newest.get(name).cloned().filter(|_| !full);
        let charts = match &since {
            // Pages until one reaches songs from before the last update.
            Some(since) => list(&http, name, 500, |dates| dates.iter().any(|d| d.as_str() < since.as_str()), log).await?,
            None => list(&http, name, 5000, |_| false, log).await?,
        };
        let top = charts.iter().filter_map(|c| c.modified.as_deref()).max().map(|m| m.trim_end_matches('Z').replace('T', " "));
        if let Some(top) = top.or(since) {
            newest.insert(name.to_owned(), top);
        }
        fetched.extend(charts);
    }
    if full {
        *cat = Catalogue { version: VERSION, generation: now(), ..Default::default() };
    }
    cat.merge(fetched);
    cat.updated = now();
    cat.newest = newest;
    Ok(cat.charts.len().saturating_sub(before))
}

// ─────────────────────────────── Installing ───────────────────────────────

/// Downloads a chart and installs it into the songs folder: unpacks a .zip or .7z, or converts
/// a Rock Band CON file. Returns its folder.
#[cfg(not(target_arch = "wasm32"))]
pub async fn download(chart: &Chart, settings: &crate::settings::Settings, mut progress: impl FnMut(u64, Option<u64>)) -> Result<std::path::PathBuf> {
    use anyhow::Context;
    use std::io::Write;

    let rv = chart.rv.as_ref().context("not a RhythmVerse chart")?;
    let Some(url) = file_url(rv) else { bail!("this chart's file is on another site; open it on RhythmVerse to download it") };
    let work = settings.dir.join(install::WORK_DIR);
    std::fs::create_dir_all(&work)?;
    let file = work.join(format!("{}.part", rv.id));
    let fetched = async {
        let http = Http::new(None);
        let mut resp = http.get(&url).send().await?.error_for_status()?;
        if resp.header("content-type").is_some_and(|c| c.starts_with("text/html")) {
            bail!("the file's host sent a web page instead of the file (open it on RhythmVerse to download it)");
        }
        let total = resp.content_length();
        let mut out = std::fs::File::create(&file)?;
        let mut done = 0;
        while let Some(piece) = resp.chunk().await? {
            out.write_all(&piece)?;
            done += piece.len() as u64;
            progress(done, total);
        }
        Ok(())
    }
    .await;
    let result = fetched.and_then(|()| install::install(&file, chart, settings));
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_dir(&work);
    result?.into_iter().next().context("nothing was installed")
}

#[cfg(not(target_arch = "wasm32"))]
mod install {
    use std::fs;
    use std::io::Read;
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result, bail};

    use crate::api::{self, Chart};
    use crate::{import, unzip};

    /// Where downloads are unpacked (inside the songs folder, so moving the result into place
    /// is a rename).
    pub const WORK_DIR: &str = ".rainbow-rhythmverse";

    /// Whether a folder holds a chart (looking a few levels down).
    pub fn has_chart(dir: &Path, depth: usize) -> bool {
        let Ok(entries) = fs::read_dir(dir) else { return false };
        entries.flatten().any(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if path.is_dir() {
                depth > 0 && has_chart(&path, depth - 1)
            } else {
                name == "song.ini" || name == "notes.chart" || name == "notes.mid" || name.ends_with(".sng")
            }
        })
    }

    /// Moves an unpacked folder into the songs folder: its only subfolder if everything is in
    /// one, otherwise the whole folder, named `name`. Returns where it went.
    fn move_into(from: &Path, dir: &Path, name: &str) -> Result<PathBuf> {
        let entries: Vec<PathBuf> = fs::read_dir(from)?.flatten().map(|e| e.path()).collect();
        let (src, name) = match entries.as_slice() {
            [only] if only.is_dir() => (only.clone(), only.file_name().unwrap_or_default().to_string_lossy().into_owned()),
            _ => (from.to_path_buf(), name.to_owned()),
        };
        let dest = unzip::unique_dir(dir, &api::sanitize_filename(&name));
        fs::rename(&src, &dest).with_context(|| format!("couldn't move the chart into {}", dest.display()))?;
        let _ = fs::remove_dir_all(from);
        Ok(dest)
    }

    /// Unpacks or converts a downloaded file into the songs folder. Returns the chart folders.
    pub fn install(file: &Path, chart: &Chart, settings: &crate::settings::Settings) -> Result<Vec<PathBuf>> {
        let mut magic = [0u8; 8];
        let n = fs::File::open(file)?.read(&mut magic)?;
        let magic = &magic[..n];
        let dir = &settings.dir;
        let name = format!("{} - {}", chart.artist.as_deref().unwrap_or("Unknown Artist"), chart.name.as_deref().unwrap_or("Unknown Name"));
        let work = file.parent().context("no folder")?;
        let opts = import::Options { unzip: true, convert: true, delete: true, folder_template: settings.folder_name.clone() };
        // Why converting a Rock Band file failed, if it does.
        let problems = std::cell::RefCell::new(Vec::new());
        let report = |line: String| {
            if line.starts_with("Couldn't") {
                problems.borrow_mut().push(line);
            }
        };
        // Converts any Rock Band files in an unpacked folder, and checks there's a chart.
        let finish = |dest: PathBuf| -> Result<Vec<PathBuf>> {
            let converted = import::convert_extracted(&dest, dir, &opts, &report);
            if dest.exists() && has_chart(&dest, 3) {
                return Ok(vec![dest]);
            }
            if dest.exists() {
                let _ = fs::remove_dir_all(&dest);
            }
            if converted.is_empty() {
                match problems.borrow().last() {
                    Some(why) => bail!("{why}"),
                    None => bail!("there's no chart in it"),
                }
            }
            Ok(converted)
        };
        if magic.starts_with(b"PK") {
            // (unzip names the folder after the zip unless it's all in one folder already.)
            let zip = work.join(format!("{}.zip", api::sanitize_filename(&name)));
            fs::rename(file, &zip)?;
            let dest = unzip::extract(&zip, dir);
            let _ = fs::remove_file(&zip);
            finish(dest?)
        } else if magic.starts_with(b"7z\xBC\xAF\x27\x1C") {
            let out = work.join(file.file_stem().unwrap_or_default());
            let _ = fs::remove_dir_all(&out);
            if let Err(e) = sevenz_rust2::decompress_file(file, &out) {
                let _ = fs::remove_dir_all(&out);
                bail!("couldn't unpack the .7z file: {e}");
            }
            finish(move_into(&out, dir, &name)?)
        } else if magic.starts_with(b"CON ") || magic.starts_with(b"LIVE") || magic.starts_with(b"PIRS") {
            crate::rockband::convert(file, dir, &settings.folder_name, chart.charter.as_deref())
        } else if magic.starts_with(b"SNGPKG") {
            let dest = api::unique_path(dir, &api::sanitize_filename(&name), ".sng");
            fs::copy(file, &dest)?;
            Ok(vec![dest])
        } else if magic.starts_with(b"Rar!") {
            bail!("it's a .rar file, which Rainbow can't unpack (open it on RhythmVerse to download it)");
        } else {
            bail!("it isn't a .zip, .7z or Rock Band file");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_ids() {
        assert_eq!(drive_id("https://drive.google.com/file/d/1qzTV22Z_GMPN8ygyRfKODUDd5cl4cCwW/view?usp=sharing").as_deref(), Some("1qzTV22Z_GMPN8ygyRfKODUDd5cl4cCwW"));
        assert_eq!(drive_id("https://drive.google.com/open?id=1qzTV22Z_GMPN8yg").as_deref(), Some("1qzTV22Z_GMPN8yg"));
        assert_eq!(drive_id("https://drive.google.com/drive/folders/1gxpASG3DoN9SEoChYpMXaDcE_6eLRPRH?usp=drive_link"), None);
        assert_eq!(drive_id("https://mega.nz/file/abc"), None);
    }

    #[test]
    fn encodes_paths() {
        assert_eq!(encode_path("/download_file/a/b/Issues - Love (FT).7z"), "/download_file/a/b/Issues%20-%20Love%20%28FT%29.7z");
    }

    #[test]
    fn parses_a_song() {
        let v = serde_json::json!({
            "data": { "song_id": 111166, "title": "T", "artist": "A", "album_art": "/assets/a.png", "song_length": 201 },
            "file": {
                "file_id": "d565fcc2ef7c4ef48de5b5be66ebd375", "gameformat": "ch", "file_name": "x.7z",
                "download_url": "/download_file/u/d565/x.7z", "external_url": null,
                "diff_guitar": 5, "diff_drums": -1, "diff_vocals": 3, "update_date": "2026-10-11 10:10:08",
                "author": { "name": "Nadir" }, "deleted": 0
            }
        });
        let c = parse(&v).unwrap();
        assert_eq!(c.md5, "rv-d565fcc2ef7c4ef48de5b5be66ebd375-111166");
        assert_eq!((c.name.as_deref(), c.charter.as_deref(), c.length_ms), (Some("T"), Some("Nadir"), Some(201_000)));
        assert_eq!((c.diffs[0], c.diffs[4]), (4, -1));
        assert_eq!(c.modified.as_deref(), Some("2026-10-11T10:10:08Z"));
        assert_eq!(c.art.as_deref(), Some("https://rhythmverse.co/assets/a.png"));
        assert!(c.has(api::flag::VOCALS));
        assert_eq!(file_url(c.rv.as_ref().unwrap()).as_deref(), Some("https://rhythmverse.co/download_file/u/d565/x.7z"));
    }

    /// Fetches the newest songs and installs one of each kind into a temporary folder:
    /// `cargo test rhythmverse -- --ignored --nocapture`
    #[test]
    #[ignore = "downloads from rhythmverse.co"]
    fn installs_each_kind() {
        let dir = std::env::temp_dir().join("rainbow-rv-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let settings = crate::settings::Settings { dir: dir.clone(), folder_name: "{artist} - {name}".to_owned(), ..Default::default() };
        let http = Http::new(None);
        let mut picked: Vec<Chart> = Vec::new();
        for name in LISTS {
            let charts = pollster::block_on(list(&http, name, 300, |_| true, &|_| {})).unwrap();
            for want in [".zip", ".7z", "con", "drive"] {
                let found = charts.iter().filter(|c| c.length_ms.unwrap_or(0) < 400_000).find(|c| {
                    let rv = c.rv.as_ref().unwrap();
                    let hosted = rv.url.starts_with('/');
                    match want {
                        "drive" => drive_id(&rv.url).is_some() && file_url(rv).is_some(),
                        "con" => rv.format == "rb3xbox" && hosted,
                        ext => hosted && rv.file.to_lowercase().ends_with(ext),
                    }
                });
                if let Some(c) = found {
                    picked.push(c.clone());
                }
            }
        }
        for c in picked {
            let rv = c.rv.as_ref().unwrap();
            println!("{} [{} {} {}]", c.title(), rv.format, rv.file, rv.url);
            match pollster::block_on(download(&c, &settings, |_, _| {})) {
                Ok(f) => {
                    println!("  -> {}", f.display());
                    assert!(install::has_chart(&f, 3) || f.extension().is_some_and(|e| e == "sng"));
                }
                Err(e) => println!("  FAILED: {e:#}"),
            }
        }
        assert!(!dir.join(install::WORK_DIR).exists(), "the work folder was left behind");
    }
}
