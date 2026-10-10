//! Importing charts from elsewhere: watching the songs folder for .zip files and Rock Band CON
//! packages, and handling files dropped onto the window.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{rockband, unzip};

/// Where converted (or unconvertible) CON files are moved when they aren't deleted.
pub const CONVERTED_DIR: &str = ".rainbow-converted";

#[derive(Clone)]
pub struct Options {
    pub unzip: bool,
    pub convert: bool,
    /// Delete originals after processing instead of moving them aside.
    pub delete: bool,
    pub folder_template: String,
}

/// Extensions that are definitely not CON packages, so we don't open every chart file.
const NOT_CON: [&str; 22] = [
    "sng", "ini", "mid", "midi", "chart", "ogg", "opus", "mp3", "wav", "flac", "png", "jpg", "jpeg", "txt", "zip",
    "rar", "7z", "json", "webm", "mp4", "part", "crdownload",
];

fn might_be_con(path: &Path) -> bool {
    let hidden = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    !hidden && path.is_file() && ext.is_none_or(|e| !NOT_CON.contains(&e.as_str()))
}

fn is_zip(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

fn name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// Converts one CON package into the songs folder, reporting the result.
fn convert_one(path: &Path, songs_dir: &Path, opts: &Options, report: &dyn Fn(String)) -> bool {
    report(format!("Converting {}…", name(path)));
    match rockband::convert(path, songs_dir, &opts.folder_template) {
        Ok(dirs) => {
            for d in dirs {
                report(format!("Converted {} to {}", name(path), d.display()));
            }
            true
        }
        Err(e) => {
            report(format!("Couldn't convert {}: {e:#}", name(path)));
            false
        }
    }
}

/// Finds CON packages inside a folder (a few levels deep).
fn find_cons(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                find_cons(&path, depth - 1, out);
            }
        } else if might_be_con(&path) && rockband::is_package(&path) {
            out.push(path);
        }
    }
}

/// Removes empty folders left behind after converting the CON files out of an extracted zip.
fn remove_empty_dirs(dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(dir) else { return false };
    let mut empty = true;
    for entry in entries.flatten() {
        let path = entry.path();
        if !(path.is_dir() && remove_empty_dirs(&path)) {
            empty = false;
        }
    }
    empty && fs::remove_dir(dir).is_ok()
}

/// Converts CON files found in a folder we just extracted, then tidies up after them.
fn convert_extracted(folder: &Path, songs_dir: &Path, opts: &Options, report: &dyn Fn(String)) {
    let mut cons = Vec::new();
    find_cons(folder, 4, &mut cons);
    for path in cons {
        convert_one(&path, songs_dir, opts, report);
        // The CON was our own extracted copy; the zip itself is kept or deleted separately.
        let _ = fs::remove_file(&path);
    }
    remove_empty_dirs(folder);
}

/// One pass over the top level of the songs folder.
pub fn process_folder(dir: &Path, opts: &Options, report: &dyn Fn(String)) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    for path in paths {
        if !unzip::settled(&path) {
            continue;
        }
        if opts.unzip && is_zip(&path) {
            match unzip::extract(&path, dir) {
                Ok(dest) => {
                    report(format!("Extracted {} to {}", name(&path), dest.display()));
                    if let Err(e) = unzip::set_aside(&path, dir, unzip::DONE_DIR, opts.delete) {
                        report(format!("Couldn't tidy up {}: {e:#}", name(&path)));
                    }
                    if opts.convert {
                        convert_extracted(&dest, dir, opts, report);
                    }
                }
                Err(e) => report(format!("Couldn't extract {}: {e:#}", name(&path))),
            }
        } else if opts.convert && might_be_con(&path) && rockband::is_package(&path) {
            convert_one(&path, dir, opts, report);
            // Move it aside even if conversion failed, so it isn't retried every few seconds.
            if let Err(e) = unzip::set_aside(&path, dir, CONVERTED_DIR, opts.delete) {
                report(format!("Couldn't tidy up {}: {e:#}", name(&path)));
            }
        }
    }
}

/// Imports files dropped onto the window: CON packages, zips, or folders containing them.
/// Dropped originals are left where they are.
pub fn import_dropped(paths: Vec<PathBuf>, songs_dir: &Path, opts: &Options, report: &dyn Fn(String)) {
    let _ = fs::create_dir_all(songs_dir);
    for path in paths {
        if path.is_dir() {
            let mut cons = Vec::new();
            find_cons(&path, 4, &mut cons);
            if cons.is_empty() {
                report(format!("No Rock Band CON files found in {}", name(&path)));
            }
            for con in cons {
                convert_one(&con, songs_dir, opts, report);
            }
        } else if is_zip(&path) {
            match unzip::extract(&path, songs_dir) {
                Ok(dest) => {
                    report(format!("Extracted {} to {}", name(&path), dest.display()));
                    convert_extracted(&dest, songs_dir, opts, report);
                }
                Err(e) => report(format!("Couldn't extract {}: {e:#}", name(&path))),
            }
        } else if rockband::is_package(&path) {
            convert_one(&path, songs_dir, opts, report);
        } else {
            report(format!("{} isn't a Rock Band CON file or a .zip", name(&path)));
        }
    }
}

#[cfg(test)]
mod tests {
    /// Run with: IMPORT_DIR=<songs dir> cargo test process_folder -- --ignored --nocapture
    #[test]
    #[ignore]
    fn process_folder_once() {
        let dir = std::env::var("IMPORT_DIR").expect("set IMPORT_DIR");
        let opts = super::Options { unzip: true, convert: true, delete: false, folder_template: "{artist} - {name} ({charter})".into() };
        super::process_folder(dir.as_ref(), &opts, &|line| println!("{line}"));
    }
}
