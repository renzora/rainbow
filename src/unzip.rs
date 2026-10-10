//! Automatic extraction of .zip files dropped into the songs folder (e.g. charts downloaded
//! from other sites), so Clone Hero can see them.

use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

/// Where extracted zips are moved when they aren't deleted.
pub const DONE_DIR: &str = ".rainbow-extracted-zips";

/// Extracts every finished .zip at the top level of `dir`. Returns a log line per archive.
pub fn extract_all(dir: &Path, delete: bool) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut log = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_zip = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
        if !is_zip || !path.is_file() || !settled(&path) {
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        match extract(&path, dir).and_then(|dest| finish(&path, dir, delete).map(|()| dest)) {
            Ok(dest) => log.push(format!("Extracted {name} to {}", dest.display())),
            Err(e) => log.push(format!("Couldn't extract {name}: {e:#}")),
        }
    }
    log
}

/// Skip files modified in the last few seconds; they may still be downloading.
fn settled(path: &Path) -> bool {
    let modified = fs::metadata(path).and_then(|m| m.modified()).unwrap_or(SystemTime::now());
    SystemTime::now().duration_since(modified).unwrap_or_default() > Duration::from_secs(5)
}

/// Extracts into `<dir>/<zip name>/`. If everything in the zip is inside one folder, that
/// folder's contents go there directly instead of being nested a level deeper.
fn extract(zip_path: &Path, dir: &Path) -> Result<PathBuf> {
    let mut archive = zip::ZipArchive::new(File::open(zip_path)?).context("not a valid zip file")?;

    // Safe relative paths only (no "..", no absolute paths).
    let names: Vec<Option<PathBuf>> = (0..archive.len())
        .map(|i| archive.by_index(i).ok().and_then(|f| f.enclosed_name()))
        .collect();
    let roots: Vec<_> = names
        .iter()
        .flatten()
        .filter_map(|p| p.components().next())
        .filter(|c| matches!(c, Component::Normal(_)))
        .collect();
    let single_root = roots.first().filter(|first| {
        roots.iter().all(|c| c == *first) && names.iter().flatten().any(|p| p.components().count() > 1)
    });
    let strip: Option<PathBuf> = single_root.map(|c| PathBuf::from(c.as_os_str()));

    let stem = zip_path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let folder = strip
        .as_ref()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or(stem);
    let dest = unique_dir(dir, &crate::api::sanitize_filename(&folder));
    fs::create_dir_all(&dest)?;

    for (i, name) in names.iter().enumerate() {
        let Some(name) = name else { continue };
        let rel = match &strip {
            Some(root) => name.strip_prefix(root).unwrap_or(name).to_path_buf(),
            None => name.clone(),
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&rel);
        let mut file = archive.by_index(i)?;
        if file.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut file, &mut File::create(&out)?)?;
    }
    Ok(dest)
}

/// Deletes the zip, or moves it out of the way so it isn't extracted again.
fn finish(zip_path: &Path, dir: &Path, delete: bool) -> Result<()> {
    if delete {
        fs::remove_file(zip_path)?;
    } else {
        let done = dir.join(DONE_DIR);
        fs::create_dir_all(&done)?;
        let name = zip_path.file_name().unwrap_or_default();
        let mut target = done.join(name);
        let mut n = 2;
        while target.exists() {
            target = done.join(format!("{} ({n}).zip", zip_path.file_stem().unwrap_or_default().to_string_lossy()));
            n += 1;
        }
        fs::rename(zip_path, target)?;
    }
    Ok(())
}

fn unique_dir(dir: &Path, name: &str) -> PathBuf {
    let mut path = dir.join(name);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{name} ({n})"));
        n += 1;
    }
    path
}
