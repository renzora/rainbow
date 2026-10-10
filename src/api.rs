//! Chorus Encore client, using the same (undocumented) endpoints Bridge does:
//!   POST https://api.enchor.us/search             -> paged chart metadata
//!   POST https://api.enchor.us/search/advanced    -> same, with Bridge's advanced filters
//!   GET  https://files.enchor.us/<md5>.sng        -> chart as a .sng file
//!   GET  https://files.enchor.us/<md5>_novideo.sng

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use reqwest::StatusCode;
use reqwest::blocking::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::settings::{DownloadFormat, Settings};
use crate::sng;

const API_URL: &str = "https://api.enchor.us";
const FILES_URL: &str = "https://files.enchor.us";
const MANIFEST: &str = ".rainbow-downloaded.txt";
pub const MAX_PER_PAGE: u32 = 250;

/// Bridge's instrument identifiers, display names, and the matching `diff_*` API field.
pub const INSTRUMENTS: [(&str, &str, &str); 10] = [
    ("guitar", "Lead Guitar", "diff_guitar"),
    ("guitarcoop", "Co-op Guitar", "diff_guitar_coop"),
    ("rhythm", "Rhythm Guitar", "diff_rhythm"),
    ("bass", "Bass Guitar", "diff_bass"),
    ("drums", "Drums", "diff_drums"),
    ("keys", "Keys", "diff_keys"),
    ("guitarghl", "GHL Lead Guitar", "diff_guitarghl"),
    ("guitarcoopghl", "GHL Co-op Guitar", "diff_guitar_coop_ghl"),
    ("rhythmghl", "GHL Rhythm Guitar", "diff_rhythm_ghl"),
    ("bassghl", "GHL Bass Guitar", "diff_bassghl"),
];
pub const DRUMS: u8 = 4;
pub const DIFFICULTIES: [(&str, &str); 4] = [("expert", "Expert"), ("hard", "Hard"), ("medium", "Medium"), ("easy", "Easy")];
/// scan-chart's DrumType values.
pub const DRUM_TYPES: [&str; 3] = ["4-Lane", "4-Lane Pro", "5-Lane"];

/// Chart feature flags (bits of `Chart::flags`).
pub mod flag {
    pub const SOLO: u16 = 1 << 0;
    pub const FORCED: u16 = 1 << 1;
    pub const OPEN: u16 = 1 << 2;
    pub const TAP: u16 = 1 << 3;
    pub const LYRICS: u16 = 1 << 4;
    pub const VOCALS: u16 = 1 << 5;
    pub const ROLL: u16 = 1 << 6;
    pub const KICK2X: u16 = 1 << 7;
    pub const ISSUES: u16 = 1 << 8;
    pub const VIDEO: u16 = 1 << 9;
    pub const MODCHART: u16 = 1 << 10;
    pub const BROKEN: u16 = 1 << 11;
}

/// Problems that make a chart unplayable (missing/unreadable chart or audio, no notes),
/// as opposed to the many cosmetic issues Encore also reports.
const BROKEN_ISSUES: [&str; 11] = [
    "noChart", "invalidChart", "badChart", "noAudio", "invalidAudio", "badAudio",
    "noMetadata", "invalidIni", "noNotes", "noResolution", "noSyncTrackSection",
];

/// Notes on one instrument + difficulty.
#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct NoteStat {
    pub i: u8,
    pub d: u8,
    /// Note count.
    pub n: u32,
    /// Max notes per second.
    pub nps: f32,
}

/// The chart metadata we use and cache: a compact version of what the API returns.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Chart {
    pub md5: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart_hash: Option<String>,
    /// ISO 8601 time the chart was last modified in Google Drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(default)]
    pub length_ms: Option<u64>,
    #[serde(default)]
    pub effective_ms: Option<f64>,
    /// Difficulty ("intensity") per instrument, in `INSTRUMENTS` order; -1 = none.
    pub diffs: [i8; 10],
    #[serde(default)]
    pub notes: Vec<NoteStat>,
    #[serde(default)]
    pub drum_type: Option<u8>,
    #[serde(default)]
    pub flags: u16,
}

fn text(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

impl Chart {
    /// Converts one chart from the API's JSON, tolerating missing or oddly-typed fields.
    pub fn from_api(v: &Value) -> Option<Self> {
        let md5 = v.get("md5")?.as_str()?.to_owned();
        let nd = v.get("notesData").cloned().unwrap_or(Value::Null);
        let inst_index = |s: Option<&str>| INSTRUMENTS.iter().position(|(id, ..)| Some(*id) == s).map(|i| i as u8);
        let diff_index = |s: Option<&str>| DIFFICULTIES.iter().position(|(id, _)| Some(*id) == s).map(|i| i as u8);

        let mut notes: Vec<NoteStat> = Vec::new();
        for nc in nd.get("noteCounts").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(i), Some(d)) = (inst_index(nc["instrument"].as_str()), diff_index(nc["difficulty"].as_str())) {
                notes.push(NoteStat { i, d, n: nc["count"].as_u64().unwrap_or(0) as u32, nps: 0.0 });
            }
        }
        for m in nd.get("maxNps").and_then(Value::as_array).into_iter().flatten() {
            let (i, d) = (inst_index(m["instrument"].as_str()), diff_index(m["difficulty"].as_str()));
            if let Some(s) = notes.iter_mut().find(|s| Some(s.i) == i && Some(s.d) == d) {
                s.nps = m["nps"].as_f64().unwrap_or(0.0) as f32;
            }
        }

        let mut diffs = [-1i8; 10];
        for (k, (_, _, field)) in INSTRUMENTS.iter().enumerate() {
            diffs[k] = v.get(*field).and_then(Value::as_i64).unwrap_or(-1).clamp(-1, 100) as i8;
        }

        let issue_names = |list: Option<&Value>| -> Vec<String> {
            list.and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(|issue| {
                    issue.as_object().into_iter().flatten().filter_map(|(k, v)| {
                        (k.ends_with("Issue") || k == "issue").then(|| v.as_str().map(str::to_owned)).flatten()
                    })
                })
                .collect()
        };
        let chart_issues = issue_names(nd.get("chartIssues"));
        let all_issues: Vec<String> =
            chart_issues.iter().cloned().chain(issue_names(v.get("folderIssues"))).collect();

        let b = |val: Option<&Value>| val.and_then(Value::as_bool).unwrap_or(false);
        let mut flags = 0;
        for (on, f) in [
            (b(nd.get("hasSoloSections")), flag::SOLO),
            (b(nd.get("hasForcedNotes")), flag::FORCED),
            (b(nd.get("hasOpenNotes")), flag::OPEN),
            (b(nd.get("hasTapNotes")), flag::TAP),
            (b(nd.get("hasLyrics")), flag::LYRICS),
            (b(nd.get("hasVocals")), flag::VOCALS),
            (b(nd.get("hasFlexLanes")), flag::ROLL),
            (b(nd.get("has2xKick")), flag::KICK2X),
            (!chart_issues.is_empty(), flag::ISSUES),
            (b(v.get("hasVideoBackground")), flag::VIDEO),
            (b(v.get("modchart")), flag::MODCHART),
            (all_issues.iter().any(|i| BROKEN_ISSUES.contains(&i.as_str())), flag::BROKEN),
        ] {
            if on {
                flags |= f;
            }
        }

        Some(Chart {
            md5,
            name: text(v, "name"),
            artist: text(v, "artist"),
            album: text(v, "album"),
            genre: text(v, "genre"),
            year: text(v, "year"),
            charter: text(v, "charter"),
            chart_hash: text(v, "chartHash"),
            modified: text(v, "modifiedTime"),
            length_ms: v.get("song_length").and_then(Value::as_u64).filter(|&l| l > 0),
            effective_ms: nd.get("effectiveLength").and_then(Value::as_f64),
            diffs,
            notes,
            drum_type: nd.get("drumType").and_then(Value::as_u64).map(|d| d as u8),
            flags,
        })
    }

    pub fn has(&self, f: u16) -> bool {
        self.flags & f != 0
    }

    pub fn title(&self) -> String {
        format!(
            "{} - {} ({})",
            self.artist.as_deref().unwrap_or("Unknown Artist"),
            self.name.as_deref().unwrap_or("Unknown Name"),
            self.charter.as_deref().unwrap_or("Unknown Charter"),
        )
    }

    /// Length in ms: song_length, or the chart's note span if that's missing.
    pub fn length(&self) -> Option<u64> {
        self.length_ms.or(self.effective_ms.map(|e| e as u64))
    }

    pub fn length_text(&self) -> String {
        self.length().map(|ms| format!("{}:{:02}", ms / 60000, ms / 1000 % 60)).unwrap_or_default()
    }

    pub fn uploaded(&self) -> &str {
        self.modified.as_deref().map(|m| m.get(..10).unwrap_or(m)).unwrap_or("")
    }
}

pub struct SearchPage {
    pub found: u64,
    pub charts: Vec<Chart>,
}

#[derive(Deserialize)]
struct RawPage {
    found: u64,
    data: Vec<Value>,
}

/// The server told us to slow down; wait this long before trying again.
#[derive(Debug)]
struct RateLimited(Duration);

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "rate limited for {}s", self.0.as_secs())
    }
}

impl std::error::Error for RateLimited {}

/// How long the server wants us to wait, from `Retry-After` or `X-Ratelimit-Reset` (a Unix time).
fn rate_limit_wait(resp: &Response) -> Duration {
    let header = |name| resp.headers().get(name)?.to_str().ok()?.parse::<u64>().ok();
    let secs = header("retry-after").or_else(|| {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some(header("x-ratelimit-reset")?.saturating_sub(now))
    });
    Duration::from_secs(secs.unwrap_or(60).clamp(1, 900) + 1)
}

/// Callback for status messages, e.g. "waiting for rate limit".
pub type StatusFn = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone)]
pub struct Client {
    http: reqwest::blocking::Client,
    status: Option<StatusFn>,
}

impl Client {
    pub fn new() -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .user_agent(concat!("rainbow/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self { http, status: None })
    }

    /// A copy of this client that reports rate-limit waits to `f`.
    pub fn with_status(&self, f: StatusFn) -> Self {
        Self { http: self.http.clone(), status: Some(f) }
    }

    /// Turns 429s into `RateLimited`, other bad statuses into errors, and pauses when
    /// the API says we've used up the current rate-limit window.
    fn check(&self, resp: Response) -> Result<Response> {
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(RateLimited(rate_limit_wait(&resp)).into());
        }
        let resp = resp.error_for_status()?;
        let remaining =
            resp.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
        if remaining == Some(0) {
            let wait = rate_limit_wait(&resp);
            self.report(format!("Used up Encore's rate limit, pausing {}s", wait.as_secs()));
            sleep(wait);
        }
        Ok(resp)
    }

    fn report(&self, msg: String) {
        if let Some(f) = &self.status {
            f(msg);
        }
    }

    fn retry<T>(&self, mut f: impl FnMut() -> Result<T>) -> Result<T> {
        let mut wait = Duration::from_secs(2);
        let mut failures = 0;
        let mut rate_limits = 0;
        loop {
            match f() {
                Ok(v) => return Ok(v),
                // Rate limits don't count as failures; just wait them out (within reason).
                Err(e) if rate_limits < 30 && e.downcast_ref::<RateLimited>().is_some() => {
                    let d = e.downcast_ref::<RateLimited>().unwrap().0;
                    rate_limits += 1;
                    self.report(format!("Rate limited by Encore, waiting {}s", d.as_secs()));
                    sleep(d);
                }
                // 4xx other than 429 means the request itself is wrong; retrying won't help.
                Err(e) if e.downcast_ref::<reqwest::Error>().and_then(|e| e.status()).is_some_and(|s| s.is_client_error()) => {
                    return Err(e);
                }
                Err(e) if failures < 4 => {
                    failures += 1;
                    self.report(format!("Request failed ({e:#}), retrying in {}s", wait.as_secs()));
                    sleep(wait);
                    wait *= 2;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn post_search(&self, endpoint: &str, body: &Value) -> Result<SearchPage> {
        self.retry(|| {
            let resp = self.check(
                self.http.post(format!("{API_URL}{endpoint}")).json(body).timeout(Duration::from_secs(60)).send()?,
            )?;
            let raw: RawPage = resp.json()?;
            Ok(SearchPage { found: raw.found, charts: raw.data.iter().filter_map(Chart::from_api).collect() })
        })
    }

    pub fn search(&self, query: &str, page: u32, instrument: Option<u8>, difficulty: Option<u8>) -> Result<SearchPage> {
        let body = json!({
            "search": query,
            "per_page": MAX_PER_PAGE,
            "page": page,
            "instrument": instrument.map(|i| INSTRUMENTS[i as usize].0),
            "difficulty": difficulty.map(|d| DIFFICULTIES[d as usize].0),
            "drumType": null,
            "drumsReviewed": false,
            "sort": null,
            "source": "bridge",
        });
        self.post_search("/search", &body)
    }

    /// md5s of every chart with one of these track hashes (comma separated).
    /// Track hashes aren't in the local catalogue, so this asks the server.
    pub fn md5s_for_track_hash(&self, track_hash: &str) -> Result<HashSet<String>> {
        let mut md5s = HashSet::new();
        for page in 1..=20 {
            let body = json!({ "trackHash": track_hash, "per_page": MAX_PER_PAGE, "page": page, "drumsReviewed": false });
            let resp = self.post_search("/search/advanced", &body)?;
            let done = resp.charts.len() < MAX_PER_PAGE as usize;
            md5s.extend(resp.charts.into_iter().map(|c| c.md5));
            if done {
                break;
            }
        }
        Ok(md5s)
    }

    /// Downloads a chart into `settings.dir` (as a .sng file or an extracted chart folder,
    /// named by `settings.folder_name`) and returns the path.
    /// `progress` is called with (bytes so far, total bytes if known).
    pub fn download(&self, chart: &Chart, settings: &Settings, mut progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
        let novideo = chart.has(flag::VIDEO) && !settings.video;
        let url = format!("{FILES_URL}/{}{}.sng", chart.md5, if novideo { "_novideo" } else { "" });

        let rel = resolve_folder_name(&settings.folder_name, chart);
        let (parent, stem) = match rel.rsplit_once('/') {
            Some((p, s)) => (settings.dir.join(p), s.to_owned()),
            None => (settings.dir.clone(), rel),
        };
        fs::create_dir_all(&parent)?;
        let ext = match settings.format {
            DownloadFormat::Sng => ".sng",
            DownloadFormat::Folder => "",
        };
        let dest = unique_path(&parent, &stem, ext);
        let tmp = parent.join(format!(".{}.part", chart.md5));

        self.retry(|| {
            let mut resp = self.check(self.http.get(&url).send()?)?;
            let total = resp.content_length();
            let mut file = File::create(&tmp)?;
            let mut buf = vec![0u8; 64 * 1024];
            let mut done = 0u64;
            loop {
                let n = resp.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n])?;
                done += n as u64;
                progress(done, total);
            }
            file.sync_all()?;
            Ok(())
        })
        .inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })?;

        let finished = match settings.format {
            DownloadFormat::Sng => fs::rename(&tmp, &dest).map_err(Into::into),
            DownloadFormat::Folder => sng::extract(&tmp, &dest).inspect_err(|_| {
                let _ = fs::remove_dir_all(&dest);
            }),
        };
        let _ = fs::remove_file(&tmp);
        finished?;
        Ok(dest)
    }
}

/// Fills in a Bridge-style folder name template like `{artist} - {name} ({charter})`.
/// `/` makes subfolders; each part is made safe for Windows.
pub fn resolve_folder_name(template: &str, chart: &Chart) -> String {
    let template = if template.trim().is_empty() { "{artist} - {name} ({charter})" } else { template };
    let get = |f: &Option<String>| f.clone().unwrap_or_default();
    let parts: Vec<String> = template
        .split(['/', '\\'])
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            sanitize_filename(
                &part
                    .replace("{name}", &get(&chart.name))
                    .replace("{artist}", &get(&chart.artist))
                    .replace("{album}", &get(&chart.album))
                    .replace("{genre}", &get(&chart.genre))
                    .replace("{year}", &get(&chart.year))
                    .replace("{charter}", &get(&chart.charter)),
            )
        })
        .collect();
    if parts.is_empty() { chart.md5.clone() } else { parts.join("/") }
}

pub fn default_songs_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Documents").join("Clone Hero").join("Songs")
}

/// Makes a string safe to use as a Windows file name.
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let mut cleaned: String = cleaned.trim().chars().take(180).collect();
    while cleaned.ends_with(['.', ' ']) {
        cleaned.pop();
    }
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "LPT1", "LPT2", "LPT3"];
    if cleaned.is_empty() || reserved.contains(&cleaned.to_uppercase().as_str()) {
        cleaned.insert(0, '_');
    }
    cleaned
}

/// `<dir>/<stem><ext>`, or `<stem> (2)<ext>` etc. if that already exists.
fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut path = dir.join(format!("{stem}{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    path
}

/// md5s already downloaded into a folder, so they're skipped next time.
pub struct Manifest {
    pub seen: HashSet<String>,
    path: PathBuf,
}

impl Manifest {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(MANIFEST);
        let seen = match File::open(&path) {
            Ok(f) => BufReader::new(f)
                .lines()
                .map_while(Result::ok)
                .filter_map(|l| l.split('\t').next().map(str::to_owned))
                .filter(|s| !s.is_empty())
                .collect(),
            Err(_) => HashSet::new(),
        };
        Self { seen, path }
    }

    pub fn contains(&self, md5: &str) -> bool {
        self.seen.contains(md5)
    }

    pub fn add(&mut self, chart: &Chart, file: &Path) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(f, "{}\t{}", chart.md5, file.display())?;
        self.seen.insert(chart.md5.clone());
        Ok(())
    }
}
