//! Safe .zip extraction for charts downloaded from other sites, so Clone Hero can see them.

use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use web_time::{Duration, SystemTime};

use anyhow::{Context, Result};

/// Where extracted zips are moved when they aren't deleted.
pub const DONE_DIR: &str = ".rainbow-extracted-zips";

/// Skip files modified in the last few seconds; they may still be downloading.
pub fn settled(path: &Path) -> bool {
    let modified = fs::metadata(path).and_then(|m| m.modified()).unwrap_or(SystemTime::now());
    SystemTime::now().duration_since(modified).unwrap_or_default() > Duration::from_secs(5)
}

/// Extracts into `<dir>/<zip name>/`. If everything in the zip is inside one folder, that
/// folder's contents go there directly instead of being nested a level deeper.
pub fn extract(zip_path: &Path, dir: &Path) -> Result<PathBuf> {
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

/// Deletes a processed file, or moves it into `<dir>/<subdir>/` so it isn't processed again.
pub fn set_aside(path: &Path, dir: &Path, subdir: &str, delete: bool) -> Result<()> {
    if delete {
        fs::remove_file(path)?;
        return Ok(());
    }
    let done = dir.join(subdir);
    fs::create_dir_all(&done)?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut target = done.join(format!("{stem}{ext}"));
    let mut n = 2;
    while target.exists() {
        target = done.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    fs::rename(path, target)?;
    Ok(())
}

pub fn unique_dir(dir: &Path, name: &str) -> PathBuf {
    let mut path = dir.join(name);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{name} ({n})"));
        n += 1;
    }
    path
}
