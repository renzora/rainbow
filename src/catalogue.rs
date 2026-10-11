//! Local cache of Encore's entire chart list, for instant offline searching.
//!
//! The search API returns charts roughly newest-first, so after the first full build an
//! update only has to fetch pages until it reaches charts it already knows.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use web_time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::api::{Chart, Client, MAX_PER_PAGE};
use crate::store;


/// Save progress every this many pages while building, so it can resume if interrupted.
const SAVE_EVERY_PAGES: u32 = 10;
/// Bump when `Chart`'s cached fields change; older caches are rebuilt.
const VERSION: u32 = 5;
/// Catalogues from before the Google Drive ids; still usable while they're refreshed.
const VERSION_WITHOUT_DRIVE: u32 = 4;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Catalogue {
    #[serde(default)]
    pub version: u32,
    /// Unix time of the last successful update.
    pub updated: u64,
    /// True once every page has been fetched at least once.
    pub complete: bool,
    /// While building, the page to resume from.
    pub next_page: u32,
    pub charts: Vec<Chart>,
    /// Where `save` writes it, if not the usual place.
    #[serde(skip)]
    pub file: Option<PathBuf>,
}

pub enum Progress {
    /// A page was fetched; `new` holds the charts that weren't in the catalogue yet, so the
    /// UI can make them searchable straight away.
    Page { page: u32, pages: u32, charts: usize, new: Vec<Chart> },
}

/// The app's folder name: capitalised on macOS, where that's the convention.
#[cfg(not(target_arch = "wasm32"))]
const APP_DIR: &str = if cfg!(target_os = "macos") { "Rainbow" } else { "rainbow" };

/// Where settings, the catalogue and language overrides live:
/// Windows `%LOCALAPPDATA%/rainbow`, macOS `~/Library/Application Support/Rainbow`,
/// Linux `$XDG_DATA_HOME/rainbow` (usually `~/.local/share/rainbow`).
/// (On the web these are just names for keys in the browser's storage; see store.rs.)
pub fn data_dir() -> PathBuf {
    #[cfg(not(target_arch = "wasm32"))]
    return dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR);
    #[cfg(target_arch = "wasm32")]
    return PathBuf::from("data");
}

/// Where downloaded files that can be thrown away (album covers) are kept:
/// Windows `%LOCALAPPDATA%/rainbow`, macOS `~/Library/Caches/Rainbow`,
/// Linux `$XDG_CACHE_HOME/rainbow` (usually `~/.cache/rainbow`).
pub fn cache_dir() -> PathBuf {
    #[cfg(not(target_arch = "wasm32"))]
    return dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR);
    #[cfg(target_arch = "wasm32")]
    return PathBuf::from("cache");
}

pub fn path() -> PathBuf {
    data_dir().join("catalogue.json")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Catalogue {
    pub fn load() -> Option<Self> {
        Self::from_json(&store::read(&path())?)
    }

    /// A catalogue saved as JSON, if it's one this version can use.
    pub fn from_json(bytes: &[u8]) -> Option<Self> {
        let mut cat = serde_json::from_slice::<Self>(bytes).ok()?;
        if cat.version == VERSION_WITHOUT_DRIVE {
            // Keep it searchable, and fetch every page again in the background to fill in the
            // Drive ids (see the full build in `update`, which refreshes charts it already has).
            cat.complete = false;
            cat.next_page = 1;
            return Some(cat);
        }
        (cat.version == VERSION).then_some(cat)
    }

    pub fn save(&self) -> Result<()> {
        store::write(self.file.as_deref().unwrap_or(&path()), &serde_json::to_vec(self)?)?;
        Ok(())
    }

    /// The copy of the whole catalogue published with the web version (refreshed daily by
    /// .github/workflows/pages.yml), so a new visitor needn't fetch all of Encore page by page.
    #[cfg(target_arch = "wasm32")]
    pub async fn snapshot() -> Result<Self> {
        let http = crate::net::Http::new(None);
        let cat: Self = http.get(&crate::web::page_url("catalogue.json")).send().await?.error_for_status()?.json().await?;
        anyhow::ensure!(cat.version == VERSION && cat.complete, "the published catalogue is out of date");
        Ok(cat)
    }

    /// Brings the catalogue up to date: resumes or starts a full build if it isn't complete,
    /// otherwise fetches only the newest pages. Returns how many charts were added.
    pub async fn update(
        &mut self,
        client: &Client,
        stop: &AtomicBool,
        mut progress: impl FnMut(Progress),
    ) -> Result<usize> {
        let mut known: HashSet<String> = self.charts.iter().map(|c| c.md5.clone()).collect();
        let before = self.charts.len();
        self.version = VERSION;

        if self.complete {
            // Incremental: newest charts are first; stop at the first page with nothing new.
            let mut new = Vec::new();
            let mut page = 1;
            loop {
                if stop.load(Ordering::Relaxed) {
                    return Ok(0);
                }
                let resp = client.newest(page).await?;
                let pages = resp.found.div_ceil(MAX_PER_PAGE as u64) as u32;
                let fresh: Vec<Chart> = resp.charts.into_iter().filter(|c| known.insert(c.md5.clone())).collect();
                let charts = before + new.len() + fresh.len();
                progress(Progress::Page { page, pages, charts, new: fresh.clone() });
                if fresh.is_empty() || page >= pages {
                    new.extend(fresh);
                    break;
                }
                new.extend(fresh);
                page += 1;
            }
            new.append(&mut self.charts);
            self.charts = new;
        } else {
            // Full build, resuming where a previous run left off. Charts it already has are
            // replaced with the fresh copy (which matters when refreshing an older catalogue).
            let mut index: HashMap<String, usize> = self
                .charts
                .iter()
                .enumerate()
                .map(|(i, c)| (c.md5.clone(), i))
                .collect();
            let mut page = self.next_page.max(1);
            loop {
                if stop.load(Ordering::Relaxed) {
                    self.next_page = page;
                    self.save()?;
                    return Ok(self.charts.len() - before);
                }
                let resp = client.search("*", page, None, None).await?;
                let pages = resp.found.div_ceil(MAX_PER_PAGE as u64) as u32;
                let empty = resp.charts.is_empty();
                let mut fresh = Vec::new();
                for c in resp.charts {
                    match index.get(&c.md5) {
                        Some(&i) => self.charts[i] = c,
                        None => {
                            index.insert(c.md5.clone(), self.charts.len());
                            self.charts.push(c.clone());
                            fresh.push(c);
                        }
                    }
                }
                progress(Progress::Page {
                    page,
                    pages,
                    charts: self.charts.len(),
                    new: fresh,
                });
                if empty || page >= pages {
                    break;
                }
                page += 1;
                if page % SAVE_EVERY_PAGES == 0 {
                    self.next_page = page;
                    self.save()?;
                }
            }
            self.complete = true;
            self.next_page = 0;
        }

        self.updated = now();
        self.save()?;
        Ok(self.charts.len() - before)
    }
}
