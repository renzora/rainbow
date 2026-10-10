//! Interface translations. Each language is a JSON file in `assets/lang/` that maps the
//! English text of a string to its translation:
//!
//! ```json
//! { "name": "Español", "english_name": "Spanish", "strings": { "Select all": "Seleccionar todo" } }
//! ```
//!
//! The English text is the key, so code just writes `t("Select all")`; anything missing from
//! a language falls back to English. Files in `%LOCALAPPDATA%\rainbow\lang\` are loaded too,
//! so translations can be fixed or added without rebuilding.

use std::collections::HashMap;
use std::sync::RwLock;

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
        .map(|(code, f)| Language { english_name: if f.english_name.is_empty() { f.name.clone() } else { f.english_name }, name: f.name, code })
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
/// `tr!("{count} results", count = n)`.
#[macro_export]
macro_rules! tr {
    ($text:literal $(, $key:ident = $value:expr)* $(,)?) => {{
        #[allow(unused_mut)]
        let mut s = $crate::i18n::t($text);
        $( s = s.replace(concat!("{", stringify!($key), "}"), &$value.to_string()); )*
        s
    }};
}
