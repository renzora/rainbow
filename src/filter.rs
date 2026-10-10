//! Local search: Bridge's quick search, instrument/difficulty pickers and advanced search,
//! evaluated against the cached catalogue.

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::api::{Chart, DRUMS, flag};

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

#[derive(Clone, Copy, PartialEq, Default)]
pub enum SortKey {
    #[default]
    Newest,
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
    pub toggles: [Option<bool>; 11],

    // Quick options.
    pub no_video: bool,
    pub no_broken: bool,

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
pub struct Index {
    haystacks: Vec<String>,
    fields: Vec<[String; 6]>,
}

impl Index {
    pub fn new(charts: &[Chart]) -> Self {
        let lower = |f: &Option<String>| f.as_deref().unwrap_or("").to_lowercase();
        let fields: Vec<[String; 6]> = charts
            .iter()
            .map(|c| [lower(&c.name), lower(&c.artist), lower(&c.album), lower(&c.genre), lower(&c.year), lower(&c.charter)])
            .collect();
        let haystacks = fields.iter().map(|f| f.join(" \u{1}")).collect();
        Self { haystacks, fields }
    }

    /// Indices of matching charts, sorted as requested.
    pub fn search(&self, charts: &[Chart], f: &Filters) -> Vec<usize> {
        let terms: Vec<String> = f.query.split_whitespace().map(str::to_lowercase).collect();
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
                if f.no_video && c.has(flag::VIDEO) || f.no_broken && c.has(flag::BROKEN) {
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
            out.sort_by(|&a, &b| {
                let (ca, cb) = (&charts[a], &charts[b]);
                let ord = match f.sort {
                    SortKey::Newest => a.cmp(&b),
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
