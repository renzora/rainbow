//! Updates Rainbow from its GitHub releases.
//!
//! A release is a `vX.Y.Z` tag whose release has the files the Build workflow makes
//! (rainbow-windows-x64.zip, rainbow-macos-universal.zip, rainbow-linux-x64.tar.gz) and a
//! SHA256SUMS file. Rainbow checks the latest release, and if it's newer, downloads this
//! platform's file, checks its SHA-256, and puts the new version in place of the running one:
//! the running copy is renamed to `.old` (which works even while it runs) and removed on the
//! next launch.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};

const REPO: &str = "renzora/rainbow";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// A release newer than this one.
#[derive(Clone)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    asset: String,
    sums: Option<String>,
}

fn client() -> Client {
    Client::builder()
        .user_agent(concat!("rainbow/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(600))
        .build()
        .unwrap_or_default()
}

/// This platform's file in a release.
fn asset_name() -> Option<&'static str> {
    if cfg!(target_os = "windows") {
        Some("rainbow-windows-x64.zip")
    } else if cfg!(target_os = "macos") {
        Some("rainbow-macos-universal.zip")
    } else if cfg!(target_os = "linux") {
        Some("rainbow-linux-x64.tar.gz")
    } else {
        None
    }
}

/// "1.2.3" (or "v1.2.3", ignoring anything after a "-") as numbers, for comparing.
fn version(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split('-').next().unwrap_or_default().split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// The latest release, if it's newer than this version and has a file for this platform.
pub fn check() -> Result<Option<Release>> {
    let resp = client()
        .get(format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("couldn't reach GitHub")?;
    // No releases published yet.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let v: Value = resp.error_for_status().context("couldn't reach GitHub")?.json()?;
    let tag = v["tag_name"].as_str().unwrap_or_default();
    if version(tag) <= version(CURRENT) {
        return Ok(None);
    }
    let Some(name) = asset_name() else { return Ok(None) };
    let url_of = |wanted: &str| {
        v["assets"].as_array().into_iter().flatten().find(|a| a["name"].as_str() == Some(wanted)).and_then(|a| a["browser_download_url"].as_str()).map(str::to_owned)
    };
    let Some(asset) = url_of(name) else { return Ok(None) };
    Ok(Some(Release {
        version: tag.trim_start_matches('v').to_owned(),
        notes: v["body"].as_str().unwrap_or_default().trim().to_owned(),
        page: v["html_url"].as_str().unwrap_or_default().to_owned(),
        asset,
        sums: url_of("SHA256SUMS"),
    }))
}

/// Downloads and installs `release` in place of the running copy. `progress` gets
/// (bytes so far, total). Takes effect when Rainbow next starts; returns what to launch.
pub fn install(release: &Release, mut progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    let http = client();
    let name = asset_name().context("updates aren't available for this system")?;
    let mut resp = http.get(&release.asset).send()?.error_for_status()?;
    let total = resp.content_length();
    let mut data = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        progress(data.len() as u64, total);
    }

    // The download must match the release's published checksum.
    let sums_url = release.sums.as_ref().context("the release has no SHA256SUMS file to check the download against")?;
    let sums = http.get(sums_url).send()?.error_for_status()?.text()?;
    let expected = sums
        .lines()
        .find_map(|l| {
            let (hash, file) = l.split_once(char::is_whitespace)?;
            (file.trim().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
        })
        .context("SHA256SUMS doesn't list this file")?;
    let actual: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        bail!("the download is damaged (its checksum doesn't match), so it wasn't installed");
    }

    let exe = std::env::current_exe()?.canonicalize()?;
    if cfg!(target_os = "macos")
        && let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app"))
    {
        return replace_bundle(&data, bundle);
    }
    replace_exe(&unpack_exe(&data)?, &exe)?;
    Ok(exe)
}

/// The program file from the downloaded archive.
fn unpack_exe(data: &[u8]) -> Result<Vec<u8>> {
    #[cfg(target_os = "linux")]
    {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(data));
        for entry in archive.entries()? {
            let mut entry = entry?;
            if entry.path()?.file_name().is_some_and(|n| n == "rainbow") {
                let mut out = Vec::new();
                entry.read_to_end(&mut out)?;
                return Ok(out);
            }
        }
        bail!("the download has no rainbow program in it")
    }
    #[cfg(not(target_os = "linux"))]
    {
        let wanted = if cfg!(target_os = "windows") { "rainbow.exe" } else { "rainbow" };
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))?;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            let name = file.name()?.into_owned();
            if Path::new(&name).file_name().is_some_and(|n| n == wanted) {
                let mut out = Vec::new();
                file.read_to_end(&mut out)?;
                return Ok(out);
            }
        }
        bail!("the download has no {wanted} in it")
    }
}

/// Swaps the program file: the running one becomes `<name>.old`, the new one takes its name.
fn replace_exe(new: &[u8], exe: &Path) -> Result<()> {
    let staged = exe.with_extension("new");
    fs::write(&staged, new)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    }
    let old = old_path(exe);
    let _ = fs::remove_file(&old);
    fs::rename(exe, &old).context("couldn't move the running copy aside")?;
    if let Err(e) = fs::rename(&staged, exe) {
        // Put things back as they were.
        let _ = fs::rename(&old, exe);
        return Err(e).context("couldn't put the new version in place");
    }
    Ok(())
}

/// macOS: swaps the whole Rainbow.app bundle for the one in the download.
#[cfg(target_os = "macos")]
fn replace_bundle(data: &[u8], bundle: &Path) -> Result<PathBuf> {
    let parent = bundle.parent().context("no folder around Rainbow.app")?;
    let staging = parent.join(".rainbow-update");
    let _ = fs::remove_dir_all(&staging);
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))?;
    archive.extract(&staging)?;
    let new_app = fs::read_dir(&staging)?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == "app")).context("the download has no app in it")?;
    let old = old_path(bundle);
    let _ = fs::remove_dir_all(&old);
    fs::rename(bundle, &old).context("couldn't move the running app aside")?;
    if let Err(e) = fs::rename(&new_app, bundle) {
        let _ = fs::rename(&old, bundle);
        return Err(e).context("couldn't put the new version in place");
    }
    let _ = fs::remove_dir_all(&staging);
    Ok(bundle.join("Contents/MacOS/rainbow"))
}

#[cfg(not(target_os = "macos"))]
fn replace_bundle(_: &[u8], _: &Path) -> Result<PathBuf> {
    unreachable!()
}

fn old_path(p: &Path) -> PathBuf {
    let mut name = p.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    p.with_file_name(name)
}

/// Removes what the last update left behind. Called at startup.
pub fn clean_up() {
    let Ok(exe) = std::env::current_exe() else { return };
    let _ = fs::remove_file(old_path(&exe));
    if let Some(bundle) = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
        let _ = fs::remove_dir_all(old_path(bundle));
    }
}

/// Starts the new version and closes this one.
pub fn restart(exe: &Path) -> ! {
    let _ = std::process::Command::new(exe).spawn();
    std::process::exit(0);
}

// ─────────────────────────────── In the app ───────────────────────────────

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use eframe::egui;
use egui::RichText;

use crate::i18n::t;
use crate::{theme, tr};

enum State {
    Idle,
    Checking(Receiver<Result<Option<Release>, String>>),
    UpToDate,
    Available(Release),
    Installing(Release, Receiver<Result<PathBuf, String>>),
    /// Installed; it runs from the next start (the path to launch to restart now).
    Ready(Release, PathBuf),
    Failed(String),
}

/// Rainbow's updates: checking, the status bar button, the release window and settings.
pub struct Updates {
    state: State,
    dialog: bool,
    progress: Arc<Mutex<(u64, Option<u64>)>>,
}

impl Updates {
    pub fn new() -> Self {
        Self { state: State::Idle, dialog: false, progress: Arc::new(Mutex::new((0, None))) }
    }

    pub fn check(&mut self, ctx: &egui::Context) {
        if matches!(self.state, State::Checking(_) | State::Installing(..) | State::Ready(..)) {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(check().map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.state = State::Checking(rx);
    }

    fn install(&mut self, ctx: &egui::Context) {
        let State::Available(release) = &self.state else { return };
        let release = release.clone();
        let (tx, rx) = channel();
        let (ctx, progress, r) = (ctx.clone(), self.progress.clone(), release.clone());
        std::thread::spawn(move || {
            let result = install(&r, |done, total| {
                *progress.lock().unwrap() = (done, total);
                ctx.request_repaint();
            });
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.state = State::Installing(release, rx);
    }

    /// Picks up results; with `auto_install`, a newer version is installed straight away.
    pub fn poll(&mut self, ctx: &egui::Context, auto_install: bool) {
        match &self.state {
            State::Checking(rx) => {
                if let Ok(result) = rx.try_recv() {
                    self.state = match result {
                        Ok(Some(r)) => State::Available(r),
                        Ok(None) => State::UpToDate,
                        Err(e) => State::Failed(e),
                    };
                    if auto_install && matches!(self.state, State::Available(_)) {
                        self.install(ctx);
                    }
                }
            }
            State::Installing(release, rx) => {
                if let Ok(result) = rx.try_recv() {
                    self.state = match result {
                        Ok(exe) => State::Ready(release.clone(), exe),
                        Err(e) => State::Failed(e),
                    };
                }
            }
            _ => {}
        }
    }

    pub fn dialog_open(&self) -> bool {
        self.dialog
    }

    pub fn close_dialog(&mut self) {
        self.dialog = false;
    }

    /// The status bar's update button or progress, when there's an update.
    pub fn status_bar(&mut self, ui: &mut egui::Ui) {
        match &self.state {
            State::Available(r) => {
                let text = RichText::new(tr!("⬆  Update to {version}", version = r.version)).color(theme::on_accent()).strong();
                if ui.add(egui::Button::new(text).fill(theme::accent())).on_hover_text(t("A new version of Rainbow is out")).clicked() {
                    self.dialog = true;
                }
            }
            State::Installing(..) => {
                let (done, total) = *self.progress.lock().unwrap();
                ui.spinner();
                match total {
                    Some(total) if total > 0 => ui.weak(tr!("Updating… {percent}%", percent = done * 100 / total)),
                    _ => ui.weak(t("Updating…")),
                };
            }
            State::Ready(_, exe) => {
                let text = RichText::new(t("↻  Restart to update")).color(theme::on_accent()).strong();
                if ui.add(egui::Button::new(text).fill(theme::good())).on_hover_text(t("The new version is installed; restart Rainbow to use it")).clicked() {
                    restart(exe);
                }
            }
            _ => {}
        }
    }

    /// The window describing the new version, with Install.
    pub fn dialog(&mut self, ctx: &egui::Context) {
        if !self.dialog {
            return;
        }
        let mut open = true;
        let mut install_now = false;
        let mut later = false;
        egui::Window::new(RichText::new(t("⬆  Update Rainbow")).strong())
            .id(egui::Id::new("update_dialog"))
            .order(egui::Order::Foreground)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(460.0)
            .show(ctx, |ui| match &self.state {
                State::Available(r) => {
                    ui.label(RichText::new(tr!("Rainbow {version} is available", version = r.version)).heading());
                    ui.weak(tr!("You have {version}", version = CURRENT));
                    if !r.notes.is_empty() {
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            ui.label(&r.notes);
                        });
                    }
                    ui.add_space(10.0);
                    crate::theme::hrow(ui, |ui| {
                        install_now = ui.add(theme::primary(t("Install and restart"))).clicked();
                        later = ui.button(t("Later")).clicked();
                        if !r.page.is_empty() && ui.button(t("View on GitHub ↗")).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(&r.page));
                        }
                    });
                }
                State::Installing(..) => {
                    let (done, total) = *self.progress.lock().unwrap();
                    let frac = total.map_or(0.0, |t| done as f32 / t.max(1) as f32);
                    ui.label(t("Downloading the update…"));
                    ui.add(crate::theme::Bar::new(frac).show_percentage());
                }
                State::Ready(_, exe) => {
                    let exe = exe.clone();
                    restart(&exe);
                }
                State::Failed(e) => {
                    ui.label(tr!("Couldn't update: {error}", error = e));
                }
                _ => {
                    ui.weak(t("Rainbow is up to date."));
                }
            });
        if install_now {
            self.install(ctx);
        }
        if later || !open {
            self.dialog = false;
        }
    }

    /// The Settings card's status line and Check now button.
    pub fn settings(&mut self, ui: &mut egui::Ui) {
        crate::theme::hrow_wrapped(ui, |ui| {
            ui.label(RichText::new(tr!("Version {version}", version = CURRENT)).strong());
            match &self.state {
                State::Idle => {}
                State::Checking(_) => {
                    ui.spinner();
                    ui.weak(t("Checking…"));
                }
                State::UpToDate => {
                    ui.weak(t("✔ Up to date"));
                }
                State::Available(r) => {
                    if ui.link(tr!("{version} is available", version = r.version)).clicked() {
                        self.dialog = true;
                    }
                }
                State::Installing(..) => {
                    ui.weak(t("Updating…"));
                }
                State::Ready(r, _) => {
                    ui.weak(tr!("{version} is installed; restart to use it", version = r.version));
                }
                State::Failed(e) => {
                    ui.weak(tr!("Couldn't check: {error}", error = e));
                }
            }
            if ui.button(t("⟳  Check now")).clicked() {
                self.check(ui.ctx());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn compares_versions() {
        use super::version;
        assert!(version("v0.2.0") > version("0.1.0"));
        assert!(version("v0.1.10") > version("0.1.9"));
        assert!(version("1.0.0-beta") == version("1.0.0"));
        assert!(version("v0.1.0") <= version("0.1.0"));
    }
}
