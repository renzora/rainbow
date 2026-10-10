//! Local search: Bridge's quick search, instrument/difficulty pickers and advanced search,
//! evaluated against the cached catalogue.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::api::{Chart, DRUMS, flag};
use crate::games;
use crate::query::{self, Cond};

/// Text fields in Bridge's "Search by" order.
pub const TEXT_FIELDS: [&str; 6] = ["Name", "Artist", "Album", "Genre", "Year", "Charter"];

/// Advanced search's on/off/any toggles, as (label, flag, which instruments it applies to).
pub const TOGGLES: [(&str, u16, Applies); 11] = [
    ("Forced Notes", flag::FORCED, Applies::NotDrums),
    ("Open Notes", flag::OPEN, Applies::NotDrums),
    ("Tap Notes", flag::TAP, Applies::NotDrums),
    ("Solo Sections", flag::SOLO, Applies::All),
    ("Lyrics", flag::LYRICS, Applies::All),
    ("Vocals", flag::VOCALS, Applies::All),
    ("Chart Issues", flag::ISSUES, Applies::All),
    ("Video Background", flag::VIDEO, Applies::All),
    ("Modchart", flag::MODCHART, Applies::All),
    ("Roll Lanes", flag::ROLL, Applies::Drums),
    ("2x Kick", flag::KICK2X, Applies::Drums),
];

#[derive(Clone, Copy, PartialEq)]
pub enum Applies {
    All,
    Drums,
    NotDrums,
}

impl Applies {
    pub fn enabled(self, instrument: Option<u8>) -> bool {
        match (self, instrument) {
            (Applies::All, _) | (_, None) => true,
            (Applies::Drums, Some(i)) => i == DRUMS,
            (Applies::NotDrums, Some(i)) => i != DRUMS,
        }
    }
}

#[derive(Clone, Default, PartialEq)]
pub struct TextFilter {
    pub value: String,
    pub exact: bool,
    pub exclude: bool,
}

/// A min/max pair as typed (blank = no limit).
#[derive(Clone, Default, PartialEq)]
pub struct Range {
    pub min: String,
    pub max: String,
}

impl Range {
    fn parse(&self) -> (Option<f64>, Option<f64>) {
        (self.min.trim().parse().ok(), self.max.trim().parse().ok())
    }
    fn is_set(&self) -> bool {
        let (a, b) = self.parse();
        a.is_some() || b.is_some()
    }
}

fn within((min, max): (Option<f64>, Option<f64>), v: f64) -> bool {
    min.is_none_or(|m| v >= m) && max.is_none_or(|m| v <= m)
}

#[derive(Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum SortKey {
    #[default]
    Newest,
    /// Most played on Clone Hero (its top 100 songs, in `Filters::popular`), then newest.
    Popular,
    Name,
    Artist,
    Album,
    Genre,
    Year,
    Charter,
    Length,
    Difficulty,
    Uploaded,
}

#[derive(Clone, PartialEq, Default)]
pub struct Filters {
    pub query: String,
    pub instrument: Option<u8>,
    pub difficulty: Option<u8>,
    pub drum_type: Option<u8>,

    pub text: [TextFilter; 6],
    pub length_min: Range,
    pub intensity: Range,
    pub avg_nps: Range,
    pub max_nps: Range,
    pub year: Range,
    /// YYYY-MM-DD
    pub modified_after: String,
    /// Comma-separated md5s (or chart hashes).
    pub hash: String,
    /// Comma-separated track hashes; resolved to md5s by the server (see `track_md5s`).
    pub track_hash: String,
    pub track_md5s: Option<HashSet<String>>,
    /// For `user:` searches, each looked-up player's charts (name, artist, charter, lower case),
    /// by the lower-case name searched for; filled in by the app as lookups finish.
    pub played: std::collections::BTreeMap<String, Vec<(String, String, String)>>,
    /// Clone Hero's most played songs, most first (name, artist, lower case), for that sort.
    pub popular: Vec<(String, String)>,
    pub toggles: [Option<bool>; 11],

    // Quick options.
    pub no_video: bool,
    pub no_broken: bool,
    /// Only charts from the official Guitar Hero / Rock Band games.
    pub legacy: bool,
    /// Only charts from this game (a `games::game_key`).
    pub game: Option<String>,

    pub sort: SortKey,
    pub descending: bool,
}

impl Filters {
    /// Whether any advanced-search field is set (to badge the Advanced button).
    pub fn advanced_count(&self) -> usize {
        self.text.iter().filter(|t| !t.value.trim().is_empty()).count()
            + [&self.length_min, &self.intensity, &self.avg_nps, &self.max_nps, &self.year]
                .iter()
                .filter(|r| r.is_set())
                .count()
            + [&self.modified_after, &self.hash, &self.track_hash].iter().filter(|s| !s.trim().is_empty()).count()
            + self.toggles.iter().filter(|t| t.is_some()).count()
    }

    pub fn clear_advanced(&mut self) {
        let keep = (self.query.clone(), self.instrument, self.difficulty, self.drum_type, self.sort, self.descending);
        *self = Filters::default();
        (self.query, self.instrument, self.difficulty, self.drum_type, self.sort, self.descending) = keep;
    }
}

/// Precomputed lowercase text for each chart, for fast matching.
#[derive(Default)]
pub struct Index {
    haystacks: Vec<String>,
    fields: Vec<[String; 6]>,
    legacy: Vec<bool>,
    game: Vec<Option<String>>,
    game_counts: HashMap<String, usize>,
}

impl Index {
    pub fn new(charts: &[Chart]) -> Self {
        let mut index = Self::default();
        for c in charts {
            index.push(c);
        }
        index
    }

    /// Adds the next chart (charts must be pushed in the same order as the chart list).
    pub fn push(&mut self, c: &Chart) {
        let lower = |f: &Option<String>| f.as_deref().unwrap_or("").to_lowercase();
        let fields = [lower(&c.name), lower(&c.artist), lower(&c.album), lower(&c.genre), lower(&c.year), lower(&c.charter)];
        self.haystacks.push(fields.join(" \u{1}"));
        self.fields.push(fields);
        let game = games::game_key(c.icon.as_deref());
        if let Some(g) = &game {
            *self.game_counts.entry(g.clone()).or_default() += 1;
        }
        self.game.push(game);
        self.legacy.push(games::is_legacy(c.icon.as_deref(), c.charter.as_deref()));
    }

    /// Official games present, with chart counts, in release order.
    pub fn games(&self) -> Vec<(String, usize)> {
        let mut games: Vec<(String, usize)> = self.game_counts.iter().map(|(g, n)| (g.clone(), *n)).collect();
        games.sort_by_key(|(g, _)| games::game_order(g));
        games
    }

    /// Indices of matching charts, sorted as requested.
    pub fn search(&self, charts: &[Chart], f: &Filters) -> Vec<usize> {
        // The search box's syntax ("exact", -exclude, field:value); see `query`. Plain MD5s,
        // Encore links and Google Drive links find those charts (any of them); other words are text.
        let (conds, words) = query::parse(&f.query);
        let (mut query_hashes, mut drive_ids, mut terms) = (Vec::new(), Vec::new(), Vec::new());
        for word in &words {
            if let Some(id) = drive_id_in(word) {
                drive_ids.push(id); // Drive ids are case-sensitive
            } else if let Some(h) = hash_in(word) {
                query_hashes.push(h);
            } else {
                terms.push(word.to_lowercase());
            }
        }
        let text: Vec<(usize, String, bool, bool)> = f
            .text
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.value.trim().is_empty())
            .map(|(k, t)| (k, t.value.trim().to_lowercase(), t.exact, t.exclude))
            .collect();
        let (length, intensity, avg_nps, max_nps, year) =
            (f.length_min.parse(), f.intensity.parse(), f.avg_nps.parse(), f.max_nps.parse(), f.year.parse());
        let hashes: HashSet<String> =
            f.hash.split(',').map(|h| h.trim().to_lowercase()).filter(|h| !h.is_empty()).collect();
        let modified_after = f.modified_after.trim();
        let toggles: Vec<(u16, bool)> = TOGGLES
            .iter()
            .zip(f.toggles)
            .filter(|((_, _, applies), _)| applies.enabled(f.instrument))
            .filter_map(|((_, bit, _), want)| want.map(|w| (*bit, w)))
            .collect();

        let mut out: Vec<usize> = (0..charts.len())
            .filter(|&i| {
                let c = &charts[i];
                let fields = &self.fields[i];
                if !terms.iter().all(|t| self.haystacks[i].contains(t.as_str())) {
                    return false;
                }
                for term in &conds {
                    let hit = match &term.cond {
                        Cond::Text {
                            field,
                            value,
                            whole,
                        } => {
                            let hay =
                                field.map_or(self.haystacks[i].as_str(), |k| fields[k].as_str());
                            if *whole {
                                query::whole_words(hay, value)
                            } else {
                                hay.contains(value.as_str())
                            }
                        }
                        // A player's charts: same name and artist (and charter, when known).
                        Cond::User(name) => f.played.get(&name.to_lowercase()).is_some_and(|charts| {
                            charts.iter().any(|(n, a, ch)| *n == fields[0] && *a == fields[1] && (ch.is_empty() || *ch == fields[5]))
                        }),
                        cond => query::matches_chart(cond, c, f.instrument, f.difficulty),
                    };
                    if hit == term.negate {
                        return false;
                    }
                }
                if !(query_hashes.is_empty() && drive_ids.is_empty()) {
                    let hash_hit = query_hashes.iter().any(|h| {
                        c.md5 == *h
                            || (h.len() < 32 && c.md5.starts_with(h.as_str()))
                            || c.chart_hash
                                .as_ref()
                                .is_some_and(|ch| ch.eq_ignore_ascii_case(h))
                    });
                    let drive_hit = drive_ids.iter().any(|id| {
                        [&c.drive_file, &c.drive_folder, &c.drive_root]
                            .iter()
                            .any(|d| d.as_deref() == Some(id.as_str()))
                    });
                    if !hash_hit && !drive_hit {
                        return false;
                    }
                }
                if f.no_video && c.has(flag::VIDEO) || f.no_broken && c.has(flag::BROKEN) {
                    return false;
                }
                if f.legacy && !self.legacy[i] || f.game.is_some() && self.game[i] != f.game {
                    return false;
                }
                // Instrument / difficulty: the chart must have notes for them.
                let stats: Vec<_> = c
                    .notes
                    .iter()
                    .filter(|s| f.instrument.is_none_or(|i| s.i == i) && f.difficulty.is_none_or(|d| s.d == d))
                    .collect();
                if (f.instrument.is_some() || f.difficulty.is_some()) && stats.is_empty() {
                    return false;
                }
                if f.instrument == Some(DRUMS) && f.drum_type.is_some() && c.drum_type != f.drum_type {
                    return false;
                }
                for (k, value, exact, exclude) in &text {
                    let hit = if *exact { fields[*k] == *value } else { fields[*k].contains(value.as_str()) };
                    if hit == *exclude {
                        return false;
                    }
                }
                if f.length_min.is_set() && !c.length().is_some_and(|ms| within(length, ms as f64 / 60000.0)) {
                    return false;
                }
                if f.intensity.is_set() {
                    let ok = match f.instrument {
                        Some(i) => c.diffs[i as usize] >= 0 && within(intensity, c.diffs[i as usize] as f64),
                        None => c.diffs.iter().any(|&d| d >= 0 && within(intensity, d as f64)),
                    };
                    if !ok {
                        return false;
                    }
                }
                if f.avg_nps.is_set() {
                    let secs = c.effective_ms.unwrap_or(0.0) / 1000.0;
                    if secs <= 0.0 || !stats.iter().any(|s| within(avg_nps, s.n as f64 / secs)) {
                        return false;
                    }
                }
                if f.max_nps.is_set() && !stats.iter().any(|s| within(max_nps, s.nps as f64)) {
                    return false;
                }
                if f.year.is_set() {
                    let y = c.year.as_deref().and_then(|y| y.trim().get(..4)?.parse::<f64>().ok());
                    if !y.is_some_and(|y| within(year, y)) {
                        return false;
                    }
                }
                if !modified_after.is_empty() && c.modified.as_deref().is_none_or(|m| m < modified_after) {
                    return false;
                }
                if !hashes.is_empty()
                    && !hashes.contains(&c.md5)
                    && !c.chart_hash.as_ref().is_some_and(|h| hashes.contains(&h.to_lowercase()))
                {
                    return false;
                }
                if let Some(md5s) = &f.track_md5s
                    && !md5s.contains(&c.md5)
                {
                    return false;
                }
                toggles.iter().all(|&(bit, want)| c.has(bit) == want)
            })
            .collect();

        if f.sort != SortKey::Newest || f.descending {
            let diff = |c: &Chart| match f.instrument {
                Some(i) => c.diffs[i as usize],
                None => c.diffs.iter().copied().max().unwrap_or(-1),
            };
            let key = |i: usize| -> &str { &self.fields[i][sort_field(f.sort)] };
            let popular: HashMap<(&str, &str), usize> =
                f.popular.iter().enumerate().map(|(k, (n, a))| ((n.as_str(), a.as_str()), k)).collect();
            let rank = |i: usize| popular.get(&(self.fields[i][0].as_str(), self.fields[i][1].as_str())).copied().unwrap_or(usize::MAX);
            out.sort_by(|&a, &b| {
                let (ca, cb) = (&charts[a], &charts[b]);
                let ord = match f.sort {
                    SortKey::Newest => a.cmp(&b),
                    SortKey::Popular => rank(a).cmp(&rank(b)),
                    SortKey::Length => ca.length().cmp(&cb.length()),
                    SortKey::Difficulty => diff(ca).cmp(&diff(cb)),
                    SortKey::Uploaded => ca.modified.cmp(&cb.modified),
                    _ => key(a).cmp(key(b)),
                };
                // Blanks last, ties by catalogue order (newest first).
                let ord = ord.then(a.cmp(&b));
                if f.descending { ord.reverse() } else { ord }
            });
            if matches!(f.sort, SortKey::Name | SortKey::Artist | SortKey::Album | SortKey::Genre | SortKey::Year | SortKey::Charter) {
                let k = sort_field(f.sort);
                out.sort_by(|&a, &b| match (self.fields[a][k].is_empty(), self.fields[b][k].is_empty()) {
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    _ => Ordering::Equal,
                });
            }
        }
        out
    }
}

/// The chart MD5 (or chart hash) in a search word: the word itself if it's one, or the one in an
/// Encore link like `https://www.enchor.us/chart/<md5>` or `files.enchor.us/<md5>_novideo.sng`.
pub fn hash_in(word: &str) -> Option<String> {
    let is_hash = |s: &str| s.len() >= 32 && s.bytes().all(|b| b.is_ascii_hexdigit());
    if is_hash(word) {
        return Some(word.to_lowercase());
    }
    // Rainbow's own links (rainbow:// or the web page) can have just the start of an MD5.
    if let Some(id) = crate::links::chart_id(word) {
        return Some(id);
    }
    if !word.to_lowercase().contains("enchor.us") {
        return None;
    }
    word.split(|c: char| !c.is_ascii_alphanumeric())
        .find(|p| is_hash(p))
        .map(str::to_lowercase)
}

/// The file or folder id in a Google Drive link: `drive.google.com/file/d/<id>/view`,
/// `.../drive/folders/<id>`, `.../open?id=<id>`, `.../uc?id=<id>&export=download`, and the
/// same on `docs.google.com` or `drive.usercontent.google.com`.
pub fn drive_id_in(word: &str) -> Option<String> {
    let lower = word.to_lowercase();
    if !(lower.contains("drive.google.com")
        || lower.contains("docs.google.com")
        || lower.contains("drive.usercontent.google.com"))
    {
        return None;
    }
    let is_id = |s: &str| {
        s.len() >= 20
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    // ?id=<id> (or &id=<id>)
    if let Some(i) = lower
        .find("id=")
        .filter(|&i| i > 0 && matches!(&lower[i - 1..i], "?" | "&"))
    {
        let id: String = word[i + 3..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if is_id(&id) {
            return Some(id);
        }
    }
    // .../d/<id>/..., .../folders/<id>
    let parts: Vec<&str> = word.split(['/', '?', '#', '&']).collect();
    parts
        .windows(2)
        .find(|w| matches!(w[0], "d" | "folders") && is_id(w[1]))
        .map(|w| w[1].to_owned())
}

fn sort_field(s: SortKey) -> usize {
    match s {
        SortKey::Name => 0,
        SortKey::Artist => 1,
        SortKey::Album => 2,
        SortKey::Genre => 3,
        SortKey::Year => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::hash_in;

    #[test]
    fn finds_hashes_in_words_and_encore_links() {
        let md5 = "2cc7370c95d16a1b2701bfabca131add";
        assert_eq!(hash_in(md5).as_deref(), Some(md5));
        assert_eq!(hash_in(&md5.to_uppercase()).as_deref(), Some(md5));
        assert_eq!(
            hash_in(&format!("https://www.enchor.us/chart/{md5}")).as_deref(),
            Some(md5)
        );
        assert_eq!(
            hash_in(&format!("https://files.enchor.us/{md5}_novideo.sng")).as_deref(),
            Some(md5)
        );
        assert_eq!(
            hash_in(&format!("enchor.us/chart/{md5}?instrument=guitar")).as_deref(),
            Some(md5)
        );
        assert_eq!(hash_in("rainbow://2cc7370c95d1").as_deref(), Some("2cc7370c95d1"));
        assert_eq!(hash_in("https://renzora.github.io/rainbow/c/?2cc7370c95d1").as_deref(), Some("2cc7370c95d1"));
        assert_eq!(hash_in("metallica"), None);
        assert_eq!(
            hash_in("https://example.com/2cc7370c95d16a1b2701bfabca131add"),
            None
        );
    }

    #[test]
    fn finds_ids_in_google_drive_links() {
        use super::drive_id_in;
        let id = "1XAT_rzBweGWLSieddXeFn2Zh0Pml2kKM";
        for link in [
            format!("https://drive.google.com/file/d/{id}/view?usp=sharing"),
            format!("https://drive.google.com/drive/folders/{id}"),
            format!("https://drive.google.com/drive/u/0/folders/{id}?usp=drive_link"),
            format!("https://drive.google.com/open?id={id}"),
            format!("https://drive.google.com/uc?export=download&id={id}"),
            format!("https://drive.usercontent.google.com/download?id={id}&export=download"),
        ] {
            assert_eq!(drive_id_in(&link).as_deref(), Some(id), "{link}");
        }
        assert_eq!(drive_id_in(id), None);
        assert_eq!(drive_id_in("https://drive.google.com/drive/my-drive"), None);
    }
}
