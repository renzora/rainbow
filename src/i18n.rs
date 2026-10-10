//! Interface translations. Each language is a JSON file in `assets/lang/` that maps the
//! English text of a string to its translation:
//!
//! ```json
//! { "name": "Español", "english_name": "Spanish", "strings": { "Select all": "Seleccionar todo" } }
//! ```
//!
//! The English text is the key, so code just writes `t("Select all")`; anything missing from
//! a language falls back to English. Files in the `lang` folder inside the data folder (see
//! `catalogue::data_dir`) are loaded too, so translations can be fixed or added without
//! rebuilding.
//!
//! Right-to-left languages (Arabic and the like): egui draws text left to right and doesn't
//! join Arabic letters, so their strings are turned into display form when the language is
//! loaded (letters joined, then put in visual order; see `visual`), and `rtl()` tells the
//! layout code to mirror rows and panels.

use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use serde::Deserialize;

#[derive(Deserialize)]
struct File {
    name: String,
    #[serde(default)]
    english_name: String,
    #[serde(default)]
    strings: HashMap<String, String>,
}

/// Built-in languages: (code, file contents).
const BUILT_IN: &[(&str, &str)] = &[
    ("ar", include_str!("../assets/lang/ar.json")),
    ("es", include_str!("../assets/lang/es.json")),
    ("pt-BR", include_str!("../assets/lang/pt-BR.json")),
    ("fr", include_str!("../assets/lang/fr.json")),
    ("de", include_str!("../assets/lang/de.json")),
    ("it", include_str!("../assets/lang/it.json")),
    ("nl", include_str!("../assets/lang/nl.json")),
    ("pl", include_str!("../assets/lang/pl.json")),
    ("sv", include_str!("../assets/lang/sv.json")),
    ("cs", include_str!("../assets/lang/cs.json")),
    ("tr", include_str!("../assets/lang/tr.json")),
    ("ru", include_str!("../assets/lang/ru.json")),
    ("uk", include_str!("../assets/lang/uk.json")),
    ("id", include_str!("../assets/lang/id.json")),
    ("vi", include_str!("../assets/lang/vi.json")),
    ("ja", include_str!("../assets/lang/ja.json")),
    ("ko", include_str!("../assets/lang/ko.json")),
    ("zh-CN", include_str!("../assets/lang/zh-CN.json")),
    ("zh-TW", include_str!("../assets/lang/zh-TW.json")),
];

/// A language the user can pick.
#[derive(Clone)]
pub struct Language {
    pub code: String,
    /// The language's name in itself, e.g. "Deutsch".
    pub name: String,
    pub english_name: String,
}

static CURRENT: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);
static CURRENT_CODE: RwLock<String> = RwLock::new(String::new());
static RTL: AtomicBool = AtomicBool::new(false);

/// The interface is in a right-to-left language, so layouts are mirrored.
pub fn rtl() -> bool {
    RTL.load(Relaxed)
}

/// Languages written right to left.
fn is_rtl_language(code: &str) -> bool {
    matches!(code.split('-').next().unwrap_or(code), "ar" | "fa" | "he" | "ur")
}

/// Whether text has right-to-left letters (Arabic, Hebrew and their presentation forms).
fn has_rtl(s: &str) -> bool {
    s.chars().any(|c| matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF))
}

/// A bracket's mirror image, as right-to-left text shows it.
fn mirrored(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        other => other,
    }
}

/// Text with right-to-left letters as egui should draw it: Arabic letters joined (as their
/// presentation forms) and each line in visual order. `{placeholders}` are kept intact so
/// `tr!` can still fill them in. Other text is returned as it is.
pub fn visual(s: &str) -> String {
    if !has_rtl(s) {
        return s.to_owned();
    }
    // Stand-ins for placeholders that the bidi algorithm leaves alone (a Latin word, with no
    // brackets for it to mirror).
    let mut names = Vec::new();
    let mut text = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        text.push_str(&rest[..open]);
        text.push_str(&format!("PH{}X", names.len()));
        names.push(rest[open..open + close + 1].to_owned());
        rest = &rest[open + close + 1..];
    }
    text.push_str(rest);
    let shaped = ar_reshaper::reshape_line(&text);
    let mut out: Vec<String> = Vec::new();
    for line in shaped.split('\n') {
        // Right to left as a whole (the placeholder stand-ins are Latin, which would
        // otherwise make a line start left to right).
        let info = unicode_bidi::BidiInfo::new(line, Some(unicode_bidi::Level::rtl()));
        let Some(para) = info.paragraphs.first() else {
            out.push(line.to_owned());
            continue;
        };
        // Runs in display order; right-to-left runs are reversed, and their brackets mirrored
        // so "(" still opens on the right.
        let (levels, runs) = info.visual_runs(para, para.range.clone());
        let mut visual = String::new();
        for run in runs {
            let text = &line[run.clone()];
            if levels[run.start].is_rtl() {
                visual.extend(text.chars().rev().map(mirrored));
            } else {
                visual.push_str(text);
            }
        }
        out.push(visual);
    }
    let mut out = out.join("\n");
    for (k, name) in names.iter().enumerate() {
        out = out.replace(&format!("PH{k}X"), name);
    }
    out
}

/// The language in use (with "auto" resolved), e.g. "de".
pub fn current() -> String {
    let code = CURRENT_CODE.read().unwrap().clone();
    if code.is_empty() { "en".to_owned() } else { code }
}

fn user_dir() -> std::path::PathBuf {
    crate::catalogue::data_dir().join("lang")
}

/// All languages: English, the built-in files, and any in the user's lang folder
/// (a user file with the same code replaces the built-in one).
fn all_files() -> Vec<(String, File)> {
    let mut files: Vec<(String, File)> = BUILT_IN
        .iter()
        .filter_map(|(code, src)| serde_json::from_str(src).ok().map(|f| (code.to_string(), f)))
        .collect();
    if let Ok(entries) = std::fs::read_dir(user_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Some(code) = path.file_stem().map(|s| s.to_string_lossy().into_owned())
                && let Some(file) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<File>(&b).ok())
            {
                files.retain(|(c, _)| *c != code);
                files.push((code, file));
            }
        }
    }
    files
}

pub fn languages() -> Vec<Language> {
    let mut out = vec![Language { code: "en".into(), name: "English".into(), english_name: "English".into() }];
    let mut rest: Vec<Language> = all_files()
        .into_iter()
        .map(|(code, f)| Language { english_name: if f.english_name.is_empty() { f.name.clone() } else { f.english_name }, name: visual(&f.name), code })
        .collect();
    rest.sort_by(|a, b| a.english_name.cmp(&b.english_name));
    out.extend(rest);
    out
}

/// The best language for the system locale, e.g. "pt-BR" for Brazilian Portuguese.
pub fn system_language() -> String {
    let locale = sys_locale::get_locale().unwrap_or_default();
    let codes: Vec<String> = languages().into_iter().map(|l| l.code).collect();
    let exact = codes.iter().find(|c| c.eq_ignore_ascii_case(&locale));
    let traditional = ["zh-TW", "zh-HK", "zh-Hant"].iter().any(|t| locale.starts_with(t));
    let base = locale.split(['-', '_']).next().unwrap_or("en").to_lowercase();
    exact
        .cloned()
        .or_else(|| (base == "zh").then(|| if traditional { "zh-TW".to_owned() } else { "zh-CN".to_owned() }))
        .or_else(|| codes.iter().find(|c| c.split('-').next() == Some(base.as_str())).cloned())
        .unwrap_or_else(|| "en".to_owned())
}

/// Switches the interface language ("auto" follows the system).
pub fn set_language(code: &str) {
    let code = if code == "auto" { system_language() } else { code.to_owned() };
    let strings = all_files().into_iter().find(|(c, _)| *c == code).map(|(_, f)| f.strings);
    // Right-to-left translations are put in display form once, here.
    let strings = strings.map(|m| m.into_iter().map(|(k, v)| (k, visual(&v))).collect());
    RTL.store(is_rtl_language(&code), Relaxed);
    *CURRENT.write().unwrap() = strings;
    *CURRENT_CODE.write().unwrap() = code;
}

/// Translates an English interface string (or returns it unchanged).
pub fn t(english: &str) -> String {
    CURRENT
        .read()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(english))
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| english.to_owned())
}

/// Translates a string with `{name}` placeholders and fills them in:
/// `tr!("{count} results", count = n)`. Whole numbers get thousands separators (95,394).
#[macro_export]
macro_rules! tr {
    ($text:literal $(, $key:ident = $value:expr)* $(,)?) => {{
        #[allow(unused_mut)]
        let mut s = $crate::i18n::t($text);
        $( s = s.replace(concat!("{", stringify!($key), "}"), &$crate::i18n::Shown::shown(&$value)); )*
        s
    }};
}

/// How a value fills a `tr!` placeholder: whole numbers with thousands separators, anything
/// else as it displays.
pub trait Shown {
    fn shown(&self) -> String;
}

/// "95394" as "95,394".
fn grouped(digits: &str) -> String {
    let (sign, digits) = digits.strip_prefix('-').map_or(("", digits), |d| ("-", d));
    let mut out = String::from(sign);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

macro_rules! whole_numbers {
    ($($t:ty),*) => {$(
        impl Shown for $t {
            fn shown(&self) -> String {
                grouped(&self.to_string())
            }
        }
    )*};
}
whole_numbers!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);

macro_rules! as_displayed {
    ($($t:ty),*) => {$(
        impl Shown for $t {
            fn shown(&self) -> String {
                self.to_string()
            }
        }
    )*};
}
as_displayed!(str, String, f32, f64, char, std::borrow::Cow<'_, str>, std::path::Display<'_>, anyhow::Error);

impl<T: Shown + ?Sized> Shown for &T {
    fn shown(&self) -> String {
        (**self).shown()
    }
}

impl<T: Shown + ?Sized> Shown for &mut T {
    fn shown(&self) -> String {
        (**self).shown()
    }
}

#[cfg(test)]
mod tests {
    use super::visual;

    #[test]
    fn right_to_left_text() {
        // Left-to-right text is untouched.
        assert_eq!(visual("{count} results"), "{count} results");
        // Arabic is joined (presentation forms) and reversed into display order, and the
        // placeholder survives with its braces the right way round.
        let shown = visual("{count} نتيجة");
        assert!(shown.contains("{count}"), "{shown}");
        assert!(!shown.contains('ن'), "letters should be joined forms: {shown}");
        assert!(shown.ends_with("{count}"), "the number goes on the right: {shown}");
        // Brackets around a number in Arabic text still face the right way.
        let shown = visual("السجل (114)");
        assert!(shown.starts_with("(114)"), "{shown}");
    }
}
