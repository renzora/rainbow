//! Google-style search box syntax: `"exact phrase"`, `-exclude`, and `field:value` filters
//! like `artist:queen`, `year:1990..1999`, `length:>5`, `notes:>1000` or `has:lyrics`. Keys have
//! short forms (`a:`, `ch:`, `y:`, `d:`...), and a space after the colon is fine: after
//! `artist: the killers` the value runs on to the next key.

use crate::api::{Chart, INSTRUMENTS, flag};

/// Text fields as indexed by `filter::Index` (name, artist, album, genre, year, charter).
const FIELDS: [(&[&str], usize); 6] = [
    (&["name", "song", "title", "n", "s"], 0),
    (&["artist", "band", "a"], 1),
    (&["album", "al"], 2),
    (&["genre", "g"], 3),
    (&["year", "y"], 4),
    (&["charter", "author", "by", "ch", "c"], 5),
];

/// The other keys (and their short forms), as `understand` reads them.
const KEYS: [&str; 22] = [
    "length", "len", "minutes", "l", "diff", "difficulty", "intensity", "d", "has", "is", "with", "inst",
    "instrument", "plays", "i", "drums", "drumtype", "type", "notes", "nc", "user", "u",
];

fn known_key(k: &str) -> bool {
    let k = k.to_lowercase();
    KEYS.contains(&k.as_str()) || FIELDS.iter().any(|(names, _)| names.contains(&k.as_str()))
}

/// Keys whose value is text, which can be several words after "key: ".
fn text_key(k: &str) -> bool {
    let k = k.to_lowercase();
    k != "year" && k != "y" && (matches!(k.as_str(), "user" | "u") || FIELDS.iter().any(|(names, _)| names.contains(&k.as_str())))
}

/// `has:` / `is:` words and the chart flag each means.
const FLAGS: [(&[&str], u16); 12] = [
    (&["solo", "solos"], flag::SOLO),
    (&["forced"], flag::FORCED),
    (&["open", "opens"], flag::OPEN),
    (&["tap", "taps"], flag::TAP),
    (&["lyrics"], flag::LYRICS),
    (&["vocals"], flag::VOCALS),
    (&["roll", "rolls"], flag::ROLL),
    (&["2xkick", "kick2x", "doublekick"], flag::KICK2X),
    (&["issues"], flag::ISSUES),
    (&["video"], flag::VIDEO),
    (&["modchart"], flag::MODCHART),
    (&["broken"], flag::BROKEN),
];

/// A min/max pair; either end may be open.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Bounds {
    pub fn contains(self, v: f64) -> bool {
        self.min.is_none_or(|m| v >= m) && self.max.is_none_or(|m| v <= m)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    /// Text anywhere (`field` = None) or in one field. `whole` = quoted: whole words only.
    Text {
        field: Option<usize>,
        value: String,
        whole: bool,
    },
    Year(Bounds),
    /// Minutes.
    Length(Bounds),
    /// Difficulty rating (0–6) on the picked instrument, or any instrument.
    Diff(Bounds),
    /// Has notes for this instrument.
    Instrument(u8),
    Flag(u16),
    /// Drum type (index into `DRUM_TYPES`).
    DrumType(u8),
    /// Note count on the picked instrument and difficulty (or any).
    Notes(Bounds),
    /// Charts a Clone Hero player has played (by username), matched by the caller.
    User(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub cond: Cond,
    pub negate: bool,
}

/// One piece of the box before it's understood: `-key:"value"`.
struct Token<'a> {
    negate: bool,
    key: Option<&'a str>,
    value: String,
    quoted: bool,
    raw: &'a str,
}

fn tokens(q: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut rest = q;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return out;
        }
        let start = rest;
        let negate = rest.len() > 1 && rest.starts_with('-');
        if negate {
            rest = &rest[1..];
        }
        // A key is letters/digits right before a colon, then something other than "//" (so links stay words).
        let mut key = None;
        let mut spaced = false;
        if let Some(colon) = rest.find(':') {
            let k = &rest[..colon];
            if !k.is_empty()
                && k.chars().all(|c| c.is_ascii_alphanumeric())
                && !rest[colon + 1..].starts_with("//")
            {
                key = Some(k);
                rest = &rest[colon + 1..];
                // "key: value" (a known key with a space after the colon) means "key:value".
                let after = rest.trim_start();
                if after.len() < rest.len() && !after.is_empty() && known_key(k) {
                    rest = after;
                    spaced = true;
                }
            }
        }
        let (value, quoted) = if let Some(inner) = rest.strip_prefix('"') {
            // An unclosed quote (still typing) runs to the end.
            let end = inner.find('"').unwrap_or(inner.len());
            let v = inner[..end].to_owned();
            rest = inner.get(end + 1..).unwrap_or("");
            (v, true)
        } else if spaced && key.is_some_and(text_key) {
            // Words up to the next key or exclusion: "artist: the killers year:2004".
            let mut end = 0;
            for (i, word) in word_spans(rest) {
                let is_key = word
                    .split_once(':')
                    .is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric()));
                if end > 0 && (is_key || word.starts_with('-')) {
                    break;
                }
                end = i + word.len();
            }
            let v = rest[..end].to_owned();
            rest = &rest[end..];
            (v, false)
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let v = rest[..end].to_owned();
            rest = &rest[end..];
            (v, false)
        };
        let raw = &start[..start.len() - rest.len()];
        out.push(Token {
            negate,
            key,
            value,
            quoted,
            raw,
        });
    }
}

/// Each whitespace-separated word with its byte offset.
fn word_spans(s: &str) -> impl Iterator<Item = (usize, &str)> {
    s.split_whitespace().map(move |w| (w.as_ptr() as usize - s.as_ptr() as usize, w))
}

/// Numbers like `2000`, `1990..1999`, `1990-1999`, `>5`, `<=3`, and decades like `1990s`.
fn bounds(s: &str) -> Option<Bounds> {
    let s = s.trim();
    let num = |t: &str| t.trim().parse::<f64>().ok();
    let b = |min, max| Some(Bounds { min, max });
    if let Some(t) = s.strip_prefix(">=") {
        return b(Some(num(t)?), None);
    }
    if let Some(t) = s.strip_prefix("<=") {
        return b(None, Some(num(t)?));
    }
    if let Some(t) = s.strip_prefix('>') {
        return b(Some(num(t)? + 1e-9), None);
    }
    if let Some(t) = s.strip_prefix('<') {
        return b(None, Some(num(t)? - 1e-9));
    }
    if let Some(d) = s
        .strip_suffix('s')
        .and_then(num)
        .filter(|d| *d >= 1000.0 && d % 10.0 == 0.0)
    {
        return b(Some(d), Some(d + 9.0));
    }
    for sep in ["..", "-"] {
        if let Some((a, z)) = s.split_once(sep).filter(|(a, _)| !a.is_empty()) {
            let (a, z) = (
                num(a)?,
                if z.trim().is_empty() {
                    None
                } else {
                    Some(num(z)?)
                },
            );
            return b(Some(a), z);
        }
    }
    let n = num(s)?;
    b(Some(n), Some(n))
}

fn instrument(word: &str) -> Option<u8> {
    let w = word.replace(['-', '_', ' '], "");
    let alias = match w.as_str() {
        "lead" | "gtr" => "guitar",
        "keyboard" | "piano" => "keys",
        "drum" => "drums",
        "coop" => "guitarcoop",
        other => other,
    };
    INSTRUMENTS
        .iter()
        .position(|(key, _, _)| *key == alias)
        .map(|i| i as u8)
}

fn drum_type(word: &str) -> Option<u8> {
    match word.replace(['-', '_', ' '], "").as_str() {
        "4lane" | "4" | "four" => Some(0),
        "pro" | "4lanepro" | "prodrums" => Some(1),
        "5lane" | "5" | "five" => Some(2),
        _ => None,
    }
}

fn understand(key: &str, value: &str, quoted: bool) -> Option<Cond> {
    let v = value.to_lowercase();
    let key = key.to_lowercase();
    if (key == "year" || key == "y")
        && let Some(b) = bounds(&v)
    {
        return Some(Cond::Year(b));
    }
    if let Some((_, k)) = FIELDS
        .iter()
        .find(|(names, _)| names.contains(&key.as_str()))
    {
        return (!v.is_empty()).then(|| Cond::Text {
            field: Some(*k),
            value: v,
            whole: quoted,
        });
    }
    match key.as_str() {
        "length" | "len" | "minutes" | "l" => bounds(&v).map(Cond::Length),
        "diff" | "difficulty" | "intensity" | "d" => bounds(&v).map(Cond::Diff),
        "notes" | "nc" => bounds(&v).map(Cond::Notes),
        "user" | "u" => (!v.trim().is_empty()).then(|| Cond::User(v.trim().to_owned())),
        "has" | "is" | "with" => FLAGS
            .iter()
            .find(|(names, _)| names.contains(&v.as_str()))
            .map(|(_, f)| Cond::Flag(*f))
            .or_else(|| instrument(&v).map(Cond::Instrument)),
        "inst" | "instrument" | "plays" | "i" => instrument(&v).map(Cond::Instrument),
        "drums" | "drumtype" | "type" => drum_type(&v).map(Cond::DrumType),
        _ => None,
    }
}

/// Splits the search box into conditions, plus the plain (unquoted, not negated, no field) words,
/// which the caller also checks for chart hashes and links.
pub fn parse(q: &str) -> (Vec<Term>, Vec<String>) {
    let (mut terms, mut words) = (Vec::new(), Vec::new());
    for tok in tokens(q) {
        if let Some(cond) = tok.key.and_then(|k| understand(k, &tok.value, tok.quoted)) {
            terms.push(Term {
                cond,
                negate: tok.negate,
            });
            continue;
        }
        // Not a known key: the whole thing is ordinary text (e.g. "AC/DC:", "3:00").
        let (value, quoted) = match tok.key {
            Some(_) => {
                let raw = tok
                    .raw
                    .strip_prefix('-')
                    .filter(|_| tok.negate)
                    .unwrap_or(tok.raw);
                (raw.replace('"', ""), false)
            }
            None => (tok.value, tok.quoted),
        };
        if value.is_empty() {
            continue;
        }
        if !tok.negate && !quoted {
            words.push(value);
        } else {
            terms.push(Term {
                cond: Cond::Text {
                    field: None,
                    value: value.to_lowercase(),
                    whole: quoted,
                },
                negate: tok.negate,
            });
        }
    }
    (terms, words)
}

/// Words to send to Encore's own search: just the text you're looking for, without the syntax.
pub fn plain_text(q: &str) -> String {
    let (terms, words) = parse(q);
    let mut out = words;
    for t in terms {
        if let (Cond::Text { value, .. }, false) = (t.cond, t.negate) {
            out.push(value);
        }
    }
    out.join(" ")
}

/// Whether `needle` appears in `hay` as whole words (not inside a longer word).
pub fn whole_words(hay: &str, needle: &str) -> bool {
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    hay.match_indices(needle).any(|(i, _)| {
        !word(hay[..i].chars().next_back()) && !word(hay[i + needle.len()..].chars().next())
    })
}

/// Checks the non-text conditions against a chart. `instrument` and `difficulty` are the ones
/// picked in the bar.
pub fn matches_chart(cond: &Cond, c: &Chart, instrument: Option<u8>, difficulty: Option<u8>) -> bool {
    match *cond {
        Cond::Text { .. } | Cond::User(_) => unreachable!("text and players are matched by the caller"),
        Cond::Notes(b) => c
            .notes
            .iter()
            .any(|s| instrument.is_none_or(|i| s.i == i) && difficulty.is_none_or(|d| s.d == d) && b.contains(s.n as f64)),
        Cond::Year(b) => c
            .year
            .as_deref()
            .and_then(|y| y.trim().get(..4)?.parse::<f64>().ok())
            .is_some_and(|y| b.contains(y)),
        Cond::Length(b) => c.length().is_some_and(|ms| b.contains(ms as f64 / 60000.0)),
        Cond::Diff(b) => match instrument {
            Some(i) => c.diffs[i as usize] >= 0 && b.contains(c.diffs[i as usize] as f64),
            None => c.diffs.iter().any(|&d| d >= 0 && b.contains(d as f64)),
        },
        Cond::Instrument(i) => c.diffs[i as usize] >= 0 || c.notes.iter().any(|s| s.i == i),
        Cond::Flag(f) => c.has(f),
        Cond::DrumType(d) => c.drum_type == Some(d),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(field: Option<usize>, value: &str, whole: bool) -> Cond {
        Cond::Text {
            field,
            value: value.to_owned(),
            whole,
        }
    }

    #[test]
    fn plain_words_stay_plain() {
        let (terms, words) = parse("through the fire");
        assert!(terms.is_empty());
        assert_eq!(words, ["through", "the", "fire"]);
    }

    #[test]
    fn quotes_negation_and_fields() {
        let (terms, words) = parse(
            r#""one" -live artist:"iron maiden" -charter:harmonix year:1990s length:>5 has:lyrics -has:video"#,
        );
        assert!(words.is_empty());
        let conds: Vec<(Cond, bool)> = terms.into_iter().map(|t| (t.cond, t.negate)).collect();
        assert_eq!(
            conds,
            vec![
                (text(None, "one", true), false),
                (text(None, "live", false), true),
                (text(Some(1), "iron maiden", true), false),
                (text(Some(5), "harmonix", false), true),
                (
                    Cond::Year(Bounds {
                        min: Some(1990.0),
                        max: Some(1999.0)
                    }),
                    false
                ),
                (
                    Cond::Length(Bounds {
                        min: Some(5.0 + 1e-9),
                        max: None
                    }),
                    false
                ),
                (Cond::Flag(flag::LYRICS), false),
                (Cond::Flag(flag::VIDEO), true),
            ]
        );
    }

    #[test]
    fn links_and_unknown_keys_are_words() {
        let (terms, words) = parse("https://www.enchor.us/chart/abc ac/dc: 3:00");
        assert!(terms.is_empty());
        assert_eq!(words, ["https://www.enchor.us/chart/abc", "ac/dc:", "3:00"]);
    }

    #[test]
    fn instruments_and_drum_types() {
        let (terms, _) = parse("has:drums type:pro inst:bass");
        let conds: Vec<Cond> = terms.into_iter().map(|t| t.cond).collect();
        assert_eq!(
            conds,
            vec![Cond::Instrument(4), Cond::DrumType(1), Cond::Instrument(3)]
        );
    }

    #[test]
    fn spaces_after_colons_and_short_keys() {
        let (terms, words) = parse("artist: the killers y: 2004 length: >3 ch:nadxa notes:>1000 u: nadx mr");
        let conds: Vec<Cond> = terms.into_iter().map(|t| t.cond).collect();
        assert_eq!(
            conds,
            vec![
                text(Some(1), "the killers", false),
                Cond::Year(Bounds { min: Some(2004.0), max: Some(2004.0) }),
                Cond::Length(Bounds { min: Some(3.0 + 1e-9), max: None }),
                text(Some(5), "nadxa", false),
                Cond::Notes(Bounds { min: Some(1000.0 + 1e-9), max: None }),
                Cond::User("nadx mr".to_owned()),
            ]
        );
        assert!(words.is_empty());
        // Unknown keys keep the space: "AC/DC: back" stays two words.
        let (_, words) = parse("ac/dc: back");
        assert_eq!(words, ["ac/dc:", "back"]);
    }

    #[test]
    fn ranges() {
        assert_eq!(
            bounds("2000..2005"),
            Some(Bounds {
                min: Some(2000.0),
                max: Some(2005.0)
            })
        );
        assert_eq!(
            bounds("2000-2005"),
            Some(Bounds {
                min: Some(2000.0),
                max: Some(2005.0)
            })
        );
        assert_eq!(
            bounds("2000.."),
            Some(Bounds {
                min: Some(2000.0),
                max: None
            })
        );
        assert_eq!(
            bounds("<=3"),
            Some(Bounds {
                min: None,
                max: Some(3.0)
            })
        );
        assert_eq!(bounds("abc"), None);
    }

    #[test]
    fn whole_word_matching() {
        assert!(whole_words("one \u{1}metallica", "one"));
        assert!(!whole_words("someone", "one"));
        assert!(whole_words("iron maiden", "iron maiden"));
        assert!(!whole_words("iron maidens", "iron maiden"));
    }

    #[test]
    fn plain_text_drops_syntax() {
        assert_eq!(
            plain_text(r#"fire "through the" -live year:2000 artist:dragonforce"#),
            "fire through the dragonforce"
        );
    }
}
