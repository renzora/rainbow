//! Chorus Encore client, using the same (undocumented) endpoints Bridge does:
//!   POST https://api.enchor.us/search             -> paged chart metadata
//!   POST https://api.enchor.us/search/advanced    -> same, with Bridge's advanced filters
//!   GET  https://files.enchor.us/<md5>.sng        -> chart as a .sng file
//!   GET  https://files.enchor.us/<md5>_novideo.sng

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use web_time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use web_time::{SystemTime, UNIX_EPOCH};

use crate::net::{Http, Resp, StatusCode};
use crate::settings::Settings;
use crate::task::sleep;
use crate::{sng, store};

const API_URL: &str = "https://api.enchor.us";
pub const FILES_URL: &str = "https://files.enchor.us";
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
    /// md5 of the album art; the image is at files.enchor.us/<art>.jpg
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    /// Source-game icon, e.g. "gh3" or "rb2dlc" for official charts (see games.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack: Option<String>,
    /// ISO 8601 time the chart was last modified in Google Drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    /// Google Drive ids: the chart's own file (or the pack it's in), the folder holding it,
    /// and the charter's shared top-level folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_ms: Option<f64>,
    /// Difficulty ("intensity") per instrument, in `INSTRUMENTS` order; -1 = none.
    pub diffs: [i8; 10],
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<NoteStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drum_type: Option<u8>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub flags: u16,
    /// Set for charts from RhythmVerse rather than Encore (their `md5` is then "rv-<file id>").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rv: Option<RvFile>,
}

fn is_zero(n: &u16) -> bool {
    *n == 0
}

/// Where a RhythmVerse chart's file is (see rhythmverse.rs).
#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct RvFile {
    /// RhythmVerse's id for the file.
    pub id: String,
    /// Its format: "ch" or "rv" (a chart folder in a .zip or .7z), "rb3xbox" (a Rock Band CON
    /// file, converted after downloading)…
    pub format: String,
    /// The download: a path on rhythmverse.co, or a link to another site.
    pub url: String,
    /// The file's name, as uploaded.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
}

fn text(v: &Value, key: &str) -> Option<String> {
    let s = match v.get(key)? {
        Value::Null => return None,
        Value::String(s) => strip_tags(s),
        other => other.to_string(),
    };
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// Removes rich-text tags like `<color=#00ffff>S</color>` that charters put in names.
pub fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        match rest[start..].find('>') {
            // Only strip things that look like tags, not e.g. "<3" in a song name.
            Some(len) if rest[start + 1..start + len].chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '/') => {
                out.push_str(&rest[..start]);
                rest = &rest[start + len + 1..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
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
            art: text(v, "albumArtMd5"),
            icon: text(v, "icon").map(|i| i.to_lowercase()),
            pack: text(v, "packName"),
            modified: text(v, "modifiedTime"),
            drive_file: text(v, "driveFileId"),
            drive_folder: text(v, "parentFolderId"),
            drive_root: text(v, "applicationDriveId"),
            length_ms: v
                .get("song_length")
                .and_then(Value::as_u64)
                .filter(|&l| l > 0),
            effective_ms: nd.get("effectiveLength").and_then(Value::as_f64),
            diffs,
            notes,
            drum_type: nd.get("drumType").and_then(Value::as_u64).map(|d| d as u8),
            flags,
            rv: None,
        })
    }

    pub fn has(&self, f: u16) -> bool {
        self.flags & f != 0
    }

    /// From Encore, so it has a preview, optimal scores and leaderboards (which need its notes).
    pub fn on_encore(&self) -> bool {
        self.rv.is_none()
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
fn rate_limit_wait(resp: &Resp) -> Duration {
    let header = |name| resp.header(name)?.parse::<u64>().ok();
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
    http: Http,
    status: Option<StatusFn>,
}

impl Client {
    pub fn new() -> Result<Self> {
        Ok(Self { http: Http::new(None), status: None })
    }

    /// A copy of this client that reports rate-limit waits to `f`.
    pub fn with_status(&self, f: StatusFn) -> Self {
        Self { http: self.http.clone(), status: Some(f) }
    }

    /// Turns 429s into `RateLimited`, other bad statuses into errors, and pauses when
    /// the API says we've used up the current rate-limit window.
    async fn check(&self, resp: Resp) -> Result<Resp> {
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(RateLimited(rate_limit_wait(&resp)).into());
        }
        let resp = resp.error_for_status()?;
        let remaining = resp.header("x-ratelimit-remaining").and_then(|v| v.parse::<u64>().ok());
        if remaining == Some(0) {
            let wait = rate_limit_wait(&resp);
            self.report(format!("Used up Encore's rate limit, pausing {}s", wait.as_secs()));
            sleep(wait).await;
        }
        Ok(resp)
    }

    fn report(&self, msg: String) {
        if let Some(f) = &self.status {
            f(msg);
        }
    }

    async fn retry<T>(&self, mut f: impl AsyncFnMut() -> Result<T>) -> Result<T> {
        let mut wait = Duration::from_secs(2);
        let mut failures = 0;
        let mut rate_limits = 0;
        loop {
            match f().await {
                Ok(v) => return Ok(v),
                // Rate limits don't count as failures; just wait them out (within reason).
                Err(e) if rate_limits < 30 && e.downcast_ref::<RateLimited>().is_some() => {
                    let d = e.downcast_ref::<RateLimited>().unwrap().0;
                    rate_limits += 1;
                    self.report(format!("Rate limited by Encore, waiting {}s", d.as_secs()));
                    sleep(d).await;
                }
                // 4xx other than 429 means the request itself is wrong; retrying won't help.
                Err(e) if e.downcast_ref::<reqwest::Error>().and_then(|e| e.status()).is_some_and(|s| s.is_client_error()) => {
                    return Err(e);
                }
                Err(e) if failures < 4 => {
                    failures += 1;
                    self.report(format!("Request failed ({e:#}), retrying in {}s", wait.as_secs()));
                    sleep(wait).await;
                    wait *= 2;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn post_search(&self, endpoint: &str, body: &Value) -> Result<SearchPage> {
        self.retry(async || {
            let url = format!("{API_URL}{endpoint}");
            let resp = self.check(self.http.post(&url).json(body).timeout(Duration::from_secs(60)).send().await?).await?;
            let raw: RawPage = resp.json().await?;
            Ok(SearchPage { found: raw.found, charts: raw.data.iter().filter_map(Chart::from_api).collect() })
        })
        .await
    }

    pub async fn search(&self, query: &str, page: u32, instrument: Option<u8>, difficulty: Option<u8>) -> Result<SearchPage> {
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
        self.post_search("/search", &body).await
    }

    /// A page of every chart, newest first, for catalogue updates. The web version gets the first
    /// page from the proxy (tools/clonehero-proxy), which keeps it for a few minutes, so all its
    /// visitors share one request to Encore; it falls back to asking Encore itself.
    pub async fn newest(&self, page: u32) -> Result<SearchPage> {
        #[cfg(target_arch = "wasm32")]
        if page == 1
            && let Some(root) = crate::players::proxy_root()
        {
            let url = format!("{root}/encore/newest");
            let shared = async {
                let raw: RawPage = self.http.get(&url).timeout(Duration::from_secs(30)).send().await?.error_for_status()?.json().await?;
                anyhow::Ok(SearchPage { found: raw.found, charts: raw.data.iter().filter_map(Chart::from_api).collect() })
            };
            if let Ok(page) = shared.await {
                return Ok(page);
            }
        }
        self.search("*", page, None, None).await
    }

    /// Charts with this md5 or chart hash, straight from Encore (for ones newer than the catalogue).
    pub async fn find_by_hash(&self, hash: &str) -> Result<Vec<Chart>> {
        let body =
            json!({ "hash": hash, "per_page": MAX_PER_PAGE, "page": 1, "drumsReviewed": false });
        Ok(self.post_search("/search/advanced", &body).await?.charts)
    }

    /// md5s of every chart with one of these track hashes (comma separated).
    /// Track hashes aren't in the local catalogue, so this asks the server.
    pub async fn md5s_for_track_hash(&self, track_hash: &str) -> Result<HashSet<String>> {
        let mut md5s = HashSet::new();
        for page in 1..=20 {
            let body = json!({ "trackHash": track_hash, "per_page": MAX_PER_PAGE, "page": page, "drumsReviewed": false });
            let resp = self.post_search("/search/advanced", &body).await?;
            let done = resp.charts.len() < MAX_PER_PAGE as usize;
            md5s.extend(resp.charts.into_iter().map(|c| c.md5));
            if done {
                break;
            }
        }
        Ok(md5s)
    }

    /// A chart's song.ini text and its notes file (name, bytes), for previews and optimal scores.
    /// Only the .sng's header and notes are downloaded (with range requests, not the audio),
    /// and they're kept on disk so the chart opens straight away next time. `progress` goes
    /// from 0 to 1000 as it downloads.
    pub async fn fetch_notes(&self, chart: &Chart, progress: &std::sync::atomic::AtomicU32) -> Result<(String, String, Vec<u8>)> {
        use std::sync::atomic::Ordering::Relaxed;
        let dir = crate::catalogue::cache_dir().join("charts");
        let ini_path = dir.join(format!("{}.ini", chart.md5));
        for name in ["notes.chart", "notes.mid"] {
            let path = dir.join(format!("{}.{}", chart.md5, name.trim_start_matches("notes.")));
            if let (Some(ini), Some(bytes)) = (store::read_string(&ini_path), store::read(&path)) {
                progress.store(1000, Relaxed);
                return Ok((ini, name.to_owned(), bytes));
            }
        }
        let novideo = if chart.has(flag::VIDEO) { "_novideo" } else { "" };
        let url = format!("{FILES_URL}/{}{novideo}.sng", chart.md5);
        // A byte range of the file, reporting progress from `from` to `to` (per mille).
        let fetch = async |start: u64, len: u64, from: u32, to: u32| -> Result<Vec<u8>> {
            self.retry(async || {
                let range = format!("bytes={start}-{}", start + len.max(1) - 1);
                let resp = self.http.get(&url).header("range", &range).send().await?;
                let partial = resp.status() == StatusCode::PARTIAL_CONTENT;
                let mut resp = self.check(resp).await?;
                let mut out = Vec::with_capacity(len as usize);
                while let Some(piece) = resp.chunk().await? {
                    out.extend_from_slice(&piece);
                    progress.store(from + ((to - from) as u64 * (out.len() as u64).min(len) / len.max(1)) as u32, Relaxed);
                    // A server that ignores the range sends the whole file; stop once we have ours.
                    if !partial && out.len() as u64 >= start + len {
                        break;
                    }
                }
                Ok(if partial { out } else { out.get(start as usize..).unwrap_or_default().iter().take(len as usize).copied().collect() })
            })
            .await
        };
        let head = fetch(0, 64 * 1024, 0, 100).await?;
        let contents = match sng::contents(&head) {
            Ok(c) => c,
            Err(_) => sng::contents(&fetch(0, 8 * 1024 * 1024, 0, 100).await?)?,
        };
        let Some((name, offset, len)) = ["notes.chart", "notes.mid"]
            .iter()
            .find_map(|want| contents.files.iter().find(|f| f.0.eq_ignore_ascii_case(want)))
        else {
            anyhow::bail!("it has no notes.chart or notes.mid");
        };
        let bytes = contents.unmask(&fetch(*offset as u64, *len as u64, 100, 1000).await?);
        if bytes.len() != *len {
            anyhow::bail!("couldn't download the notes");
        }
        let name = name.to_lowercase();
        let ini = contents.ini().to_owned();
        let _ = store::write(&dir.join(format!("{}.{}", chart.md5, name.trim_start_matches("notes."))), &bytes);
        let _ = store::write(&ini_path, ini.as_bytes());
        Ok((ini, name, bytes))
    }

    /// Downloads a chart into `settings.dir` (as a .sng file or an extracted chart folder,
    /// named by `settings.folder_name`) and returns the path.
    /// `progress` is called with (bytes so far, total bytes if known).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download(&self, chart: &Chart, settings: &Settings, mut progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
        use crate::settings::DownloadFormat;
        use std::fs::{self, File};
        use std::io::Write;

        if chart.rv.is_some() {
            return crate::rhythmverse::download(chart, settings, progress).await;
        }

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

        self.retry(async || {
            let mut resp = self.check(self.http.get(&url).send().await?).await?;
            let total = resp.content_length();
            let mut file = File::create(&tmp)?;
            let mut done = 0u64;
            while let Some(piece) = resp.chunk().await? {
                file.write_all(&piece)?;
                done += piece.len() as u64;
                progress(done, total);
            }
            file.sync_all()?;
            Ok(())
        })
        .await
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

    /// Downloads a chart into the songs folder the browser was given, or, without one, as an
    /// ordinary download. Either way it's a .sng file (see below).
    /// Returns where it went, relative to the songs folder.
    #[cfg(target_arch = "wasm32")]
    pub async fn download(&self, chart: &Chart, settings: &Settings, mut progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
        use crate::web;

        // (RhythmVerse's files can't be fetched from other sites' pages; see rhythmverse.rs.)
        anyhow::ensure!(chart.on_encore(), "RhythmVerse charts can only be downloaded in the desktop app");

        let novideo = chart.has(flag::VIDEO) && !settings.video;
        let url = format!("{FILES_URL}/{}{}.sng", chart.md5, if novideo { "_novideo" } else { "" });
        let data = self
            .retry(async || {
                let mut resp = self.check(self.http.get(&url).send().await?).await?;
                let total = resp.content_length();
                let mut data = Vec::with_capacity(total.unwrap_or(0) as usize);
                while let Some(piece) = resp.chunk().await? {
                    data.extend_from_slice(&piece);
                    progress(data.len() as u64, total);
                }
                Ok(data)
            })
            .await?;

        let rel = resolve_folder_name(&settings.folder_name, chart);
        let Some(folder) = web::folder_name() else {
            let name = format!("{}.sng", rel.rsplit('/').next().unwrap_or(&rel));
            web::save_file(&name, &data);
            return Ok(PathBuf::from(name));
        };
        // (Asked for when the download starts; see App::start_download.)
        anyhow::ensure!(web::folder_allowed().await, "the browser didn't allow saving into the folder {folder}");
        let (parent, stem) = match rel.rsplit_once('/') {
            Some((p, s)) => (format!("{p}/"), s.to_owned()),
            None => (String::new(), rel.clone()),
        };
        // Always a .sng: browsers won't let a page create song.ini (Chrome blocks .ini files as
        // risky), which a chart folder needs.
        let mut dest = format!("{parent}{stem}.sng");
        let mut n = 2;
        while web::folder_has(&dest).await {
            dest = format!("{parent}{stem} ({n}).sng");
            n += 1;
        }
        web::folder_write(&dest, &data, false).await?;
        Ok(PathBuf::from(dest))
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
            tidy_name(
                &part
                    .replace("{name}", &get(&chart.name))
                    .replace("{artist}", &get(&chart.artist))
                    .replace("{album}", &get(&chart.album))
                    .replace("{genre}", &get(&chart.genre))
                    .replace("{year}", &get(&chart.year))
                    .replace("{charter}", &get(&chart.charter)),
            )
        })
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() { chart.md5.clone() } else { parts.join("/") }
}

/// Tidies a folder name after blank fields were filled in: drops empty brackets ("()" from a
/// missing charter), doubled spaces and a dangling " -".
fn tidy_name(name: &str) -> String {
    let mut s = name.replace("()", "").replace("[]", "");
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }
    let s = s.trim().trim_end_matches(" -").trim_start_matches("- ").trim();
    if s.is_empty() { String::new() } else { sanitize_filename(s) }
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
#[cfg(not(target_arch = "wasm32"))]
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut path = dir.join(format!("{stem}{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    path
}

/// Charts already downloaded into a folder (md5 -> where it was saved), so they're skipped
/// next time and can be shown in the file manager.
///
/// The desktop keeps the list in the songs folder. The web keeps it in the browser (and adds
/// to the one in the songs folder too, when it has the folder).
pub struct Manifest {
    files: std::collections::HashMap<String, PathBuf>,
    path: PathBuf,
    /// The web's copy of the list, and how many charts were added since it was last saved.
    #[cfg(target_arch = "wasm32")]
    text: String,
    #[cfg(target_arch = "wasm32")]
    unsaved: usize,
}

/// Separates the md5 and the saved path on each line of the download list.
const SEP: char = '\t';

/// The download list's name in the songs folder.
#[cfg(target_arch = "wasm32")]
pub const MANIFEST_NAME: &str = MANIFEST;

fn manifest_lines(text: &str) -> impl Iterator<Item = (String, PathBuf)> + '_ {
    text.lines().filter_map(|l| {
        let (md5, file) = l.split_once(SEP).unwrap_or((l, ""));
        (!md5.is_empty()).then(|| (md5.to_owned(), PathBuf::from(file)))
    })
}

impl Manifest {
    pub fn load(dir: &Path) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let path = dir.join(MANIFEST);
        #[cfg(target_arch = "wasm32")]
        let path = {
            let _ = dir;
            crate::catalogue::data_dir().join("downloaded.txt")
        };
        let text = store::read_string(&path).unwrap_or_default();
        Self {
            files: manifest_lines(&text).collect(),
            path,
            #[cfg(target_arch = "wasm32")]
            text,
            #[cfg(target_arch = "wasm32")]
            unsaved: 0,
        }
    }

    pub fn contains(&self, md5: &str) -> bool {
        self.files.contains_key(md5)
    }

    /// Where a downloaded chart was saved, if it's still there.
    pub fn location(&self, md5: &str) -> Option<PathBuf> {
        #[cfg(target_arch = "wasm32")]
        return {
            let _ = md5;
            None
        };
        #[cfg(not(target_arch = "wasm32"))]
        self.files.get(md5).filter(|p| !p.as_os_str().is_empty() && p.exists()).cloned()
    }

    /// The line `add` writes for a chart.
    pub fn line(chart: &Chart, file: &Path) -> String {
        format!("{}{SEP}{}\n", chart.md5, file.display())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn add(&mut self, chart: &Chart, file: &Path) -> Result<()> {
        use std::io::Write;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        f.write_all(Self::line(chart, file).as_bytes())?;
        self.files.insert(chart.md5.clone(), file.to_path_buf());
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub fn add(&mut self, chart: &Chart, file: &Path) -> Result<()> {
        self.text.push_str(&Self::line(chart, file));
        self.files.insert(chart.md5.clone(), file.to_path_buf());
        // Saved every so often (it's rewritten whole), and at the end of each download.
        self.unsaved += 1;
        if self.unsaved >= 25 {
            self.flush();
        }
        Ok(())
    }

    /// Saves charts added since the list was last saved.
    pub fn flush(&mut self) {
        #[cfg(target_arch = "wasm32")]
        if self.unsaved > 0 {
            self.unsaved = 0;
            let _ = store::write(&self.path, self.text.as_bytes());
        }
    }

    /// Adds the charts in another download list (the songs folder's, on the web).
    #[cfg(target_arch = "wasm32")]
    pub fn merge(&mut self, text: &str) {
        for (md5, file) in manifest_lines(text) {
            if !self.files.contains_key(&md5) {
                self.text.push_str(&format!("{md5}{SEP}{}\n", file.display()));
                self.files.insert(md5, file);
                self.unsaved += 1;
            }
        }
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_skip_blank_fields() {
        let chart = Chart { artist: Some("WANDS".into()), name: Some("Sekai".into()), md5: "x".into(), ..Default::default() };
        assert_eq!(resolve_folder_name("{artist} - {name} ({charter})", &chart), "WANDS - Sekai");
        assert_eq!(resolve_folder_name("{charter}/{artist} - {name}", &chart), "WANDS - Sekai");
        let full = Chart { charter: Some("Nadir".into()), ..chart };
        assert_eq!(resolve_folder_name("{artist} - {name} ({charter})", &full), "WANDS - Sekai (Nadir)");
    }
}
