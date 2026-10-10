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

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Columns {
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
        Self { artist: true, album: true, genre: true, year: true, charter: true, length: true, difficulty: true, uploaded: false }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub dir: PathBuf,
    pub folder_name: String,
    pub dark: bool,
    pub zoom: f32,
    pub format: DownloadFormat,
    pub video: bool,
    pub compact: bool,
    pub columns: Columns,
    /// Seconds between downloads in bulk downloads.
    pub delay_secs: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dir: crate::api::default_songs_dir(),
            folder_name: "{artist} - {name} ({charter})".to_owned(),
            dark: true,
            zoom: 1.0,
            format: DownloadFormat::Sng,
            video: false,
            compact: false,
            columns: Columns::default(),
            delay_secs: 2.0,
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
