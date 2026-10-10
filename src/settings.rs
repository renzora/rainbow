//! User settings, saved as JSON next to the catalogue.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
pub enum DownloadFormat {
    /// One .sng file per chart (Clone Hero and YARG read these directly).
    Sng,
    /// The .sng unpacked into a regular chart folder with song.ini.
    Folder,
}

/// Table row size.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
pub enum Density {
    Compact,
    Default,
    Large,
}

impl Density {
    /// Cover size each preset starts from (the cover column can then be dragged).
    pub fn cover_size(self) -> f32 {
        match self {
            Density::Compact => 24.0,
            Density::Default => 42.0,
            Density::Large => 72.0,
        }
    }

    /// Row height when the cover column is hidden.
    pub fn row_height(self) -> f32 {
        match self {
            Density::Compact => 26.0,
            Density::Default => 34.0,
            Density::Large => 46.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Columns {
    pub cover: bool,
    pub artist: bool,
    pub album: bool,
    pub genre: bool,
    pub year: bool,
    pub charter: bool,
    pub length: bool,
    pub difficulty: bool,
    pub uploaded: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Self { cover: true, artist: true, album: true, genre: true, year: true, charter: true, length: true, difficulty: true, uploaded: false }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub dir: PathBuf,
    pub folder_name: String,
    pub theme: crate::theme::ThemeKind,
    pub zoom: f32,
    pub format: DownloadFormat,
    pub video: bool,
    pub density: Density,
    pub columns: Columns,
    /// Seconds between downloads in bulk downloads.
    pub delay_secs: f32,
    /// Check Encore for new charts at startup if the catalogue is older than this many hours
    /// (0 = every launch, None = never; the Refresh button always works).
    pub refresh_after_hours: Option<u32>,
    /// Unpack .zip files that appear in the songs folder.
    pub auto_extract: bool,
    /// Delete zips after extracting (otherwise they're moved aside).
    pub delete_zips: bool,
    /// Cover size in the results table, in points (drag the cover column to change it).
    pub cover_size: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dir: crate::api::default_songs_dir(),
            folder_name: "{artist} - {name} ({charter})".to_owned(),
            theme: crate::theme::ThemeKind::Midnight,
            zoom: 1.0,
            format: DownloadFormat::Sng,
            video: false,
            density: Density::Default,
            columns: Columns::default(),
            delay_secs: 2.0,
            refresh_after_hours: Some(12),
            cover_size: 42.0,
            auto_extract: false,
            delete_zips: false,
        }
    }
}

fn path() -> PathBuf {
    crate::catalogue::data_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = path();
        let _ = fs::create_dir_all(path.parent().unwrap());
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = fs::write(path, json);
        }
    }
}
