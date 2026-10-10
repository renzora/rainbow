//! Clone Hero's track hashes: a port of scan-chart 8's `calculateTrackHash`
//! (https://github.com/Geomitron/scan-chart), which the CH leaderboards use to identify a chart.
//!
//! A track hash is blake3 over a binary dump of one instrument + difficulty after the chart's
//! rules (HOPOs, taps, opens, cymbals, sustain cutoffs, ...) have been applied, so the parsing
//! has to match scan-chart's exactly; comments name the scan-chart function each part mirrors.
//! Encore stores track hashes too, but only charts it scanned since scan-chart 8 have current ones.

mod chart;
mod midi;

use std::collections::HashMap;

use anyhow::Result;

/// scan-chart's `difficulties`, in its order.
pub const DIFFICULTIES: [&str; 4] = ["expert", "hard", "medium", "easy"];

/// The parts of song.ini that change how notes are read.
#[derive(Clone, Copy)]
pub struct Modifiers {
    pub hopo_frequency: i64,
    pub eighthnote_hopo: bool,
    pub multiplier_note: i64,
    pub sustain_cutoff_threshold: i64,
    pub chord_snap_threshold: i64,
    pub five_lane_drums: bool,
    pub pro_drums: bool,
}

impl Default for Modifiers {
    fn default() -> Self {
        Self {
            hopo_frequency: 0,
            eighthnote_hopo: false,
            multiplier_note: 0,
            sustain_cutoff_threshold: -1,
            chord_snap_threshold: 0,
            five_lane_drums: false,
            pro_drums: false,
        }
    }
}

impl Modifiers {
    /// Reads them from song.ini's `[song]` values, like scan-chart's `extractSongMetadata`.
    pub fn from_ini(values: &HashMap<String, String>) -> Self {
        let d = Self::default();
        let int = |key: &str, legacy: Option<&str>, default: i64| {
            let v = ini_number(values, key).or_else(|| legacy.and_then(|l| ini_number(values, l)));
            // Non-integers are rounded (Math.round).
            v.map_or(default, |v| (v + 0.5).floor() as i64)
        };
        let boolean = |key: &str, default: bool| match values.get(key).map(|v| v.trim()) {
            Some("True" | "1") => true,
            Some("False" | "0") => false,
            _ => default,
        };
        let mut m = Self {
            hopo_frequency: int("hopo_frequency", Some("hopofreq"), d.hopo_frequency),
            eighthnote_hopo: boolean("eighthnote_hopo", d.eighthnote_hopo),
            multiplier_note: int("multiplier_note", Some("star_power_note"), d.multiplier_note),
            sustain_cutoff_threshold: int("sustain_cutoff_threshold", None, d.sustain_cutoff_threshold),
            chord_snap_threshold: int("chord_snap_threshold", None, d.chord_snap_threshold),
            five_lane_drums: boolean("five_lane_drums", d.five_lane_drums),
            pro_drums: boolean("pro_drums", d.pro_drums),
        };
        if !matches!(m.multiplier_note, 0 | 103 | 116) {
            m.multiplier_note = 0;
        }
        m
    }
}

/// `parseFloat` of an ini value; -1 and non-numbers count as missing.
fn ini_number(values: &HashMap<String, String>, key: &str) -> Option<f64> {
    parse_float(values.get(key)?).filter(|&v| v != -1.0)
}

/// JavaScript's `parseFloat`: the longest number at the start of the text.
fn parse_float(s: &str) -> Option<f64> {
    let s = s.trim_start();
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        return Some(if b[0] == b'-' { f64::NEG_INFINITY } else { f64::INFINITY });
    }
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    let mut any = digits(&mut i);
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        if digits(&mut j) || any {
            any = true;
            i = j;
        }
    }
    if !any {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if digits(&mut j) {
            i = j;
        }
    }
    s[..i].parse().ok()
}

/// scan-chart's `eventTypes`: what the .chart and .mid parsers produce.
mod ev {
    pub const STAR_POWER: u8 = 0;
    pub const SOLO_SECTION: u8 = 1;
    pub const REJECTED_STAR_POWER: u8 = 2;
    pub const SOLO_SECTION_START: u8 = 3;
    pub const SOLO_SECTION_END: u8 = 4;
    pub const OPEN: u8 = 5;
    pub const GREEN: u8 = 6;
    pub const RED: u8 = 7;
    pub const YELLOW: u8 = 8;
    pub const BLUE: u8 = 9;
    pub const ORANGE: u8 = 10;
    pub const BLACK1: u8 = 11;
    pub const BLACK2: u8 = 12;
    pub const BLACK3: u8 = 13;
    pub const WHITE1: u8 = 14;
    pub const WHITE2: u8 = 15;
    pub const WHITE3: u8 = 16;
    pub const KICK: u8 = 17;
    pub const KICK2X: u8 = 18;
    pub const RED_DRUM: u8 = 19;
    pub const YELLOW_DRUM: u8 = 20;
    pub const BLUE_DRUM: u8 = 21;
    pub const FIVE_ORANGE_FOUR_GREEN_DRUM: u8 = 22;
    pub const FIVE_GREEN_DRUM: u8 = 23;
    pub const FLEX_LANE_SINGLE: u8 = 24;
    pub const FLEX_LANE_DOUBLE: u8 = 25;
    pub const FREESTYLE_SECTION: u8 = 26;
    pub const FORCE_OPEN: u8 = 27;
    pub const FORCE_TAP: u8 = 28;
    pub const FORCE_STRUM: u8 = 29;
    pub const FORCE_HOPO: u8 = 30;
    pub const FORCE_UNNATURAL: u8 = 31;
    pub const FORCE_FLAM: u8 = 32;
    pub const YELLOW_TOM_MARKER: u8 = 33;
    pub const BLUE_TOM_MARKER: u8 = 34;
    pub const GREEN_TOM_MARKER: u8 = 35;
    pub const YELLOW_CYMBAL_MARKER: u8 = 36;
    pub const BLUE_CYMBAL_MARKER: u8 = 37;
    pub const GREEN_CYMBAL_MARKER: u8 = 38;
    pub const RED_GHOST: u8 = 39;
    pub const YELLOW_GHOST: u8 = 40;
    pub const BLUE_GHOST: u8 = 41;
    pub const FIVE_ORANGE_FOUR_GREEN_GHOST: u8 = 42;
    pub const FIVE_GREEN_GHOST: u8 = 43;
    pub const KICK_GHOST: u8 = 44;
    pub const RED_ACCENT: u8 = 45;
    pub const YELLOW_ACCENT: u8 = 46;
    pub const BLUE_ACCENT: u8 = 47;
    pub const FIVE_ORANGE_FOUR_GREEN_ACCENT: u8 = 48;
    pub const FIVE_GREEN_ACCENT: u8 = 49;
    pub const KICK_ACCENT: u8 = 50;
    pub const DISCO_FLIP_OFF: u8 = 51;
    pub const DISCO_FLIP_ON: u8 = 52;
    pub const DISCO_NO_FLIP_ON: u8 = 53;
    pub const ENABLE_CHART_DYNAMICS: u8 = 54;
    /// Number of event types.
    pub const COUNT: usize = 55;
}

/// scan-chart's `noteTypes`; these values are part of the hash.
mod note {
    pub const OPEN: u32 = 1;
    pub const GREEN: u32 = 2;
    pub const RED: u32 = 3;
    pub const YELLOW: u32 = 4;
    pub const BLUE: u32 = 5;
    pub const ORANGE: u32 = 6;
    pub const BLACK1: u32 = 7;
    pub const BLACK2: u32 = 8;
    pub const BLACK3: u32 = 9;
    pub const WHITE1: u32 = 10;
    pub const WHITE2: u32 = 11;
    pub const WHITE3: u32 = 12;
    pub const KICK: u32 = 13;
    pub const RED_DRUM: u32 = 14;
    pub const YELLOW_DRUM: u32 = 15;
    pub const BLUE_DRUM: u32 = 16;
    pub const GREEN_DRUM: u32 = 17;
}

/// scan-chart's `noteFlags`; these values are part of the hash.
mod flag {
    pub const NONE: u32 = 0;
    pub const STRUM: u32 = 1;
    pub const HOPO: u32 = 2;
    pub const TAP: u32 = 4;
    pub const DOUBLE_KICK: u32 = 8;
    pub const TOM: u32 = 16;
    pub const CYMBAL: u32 = 32;
    pub const DISCO_NOFLIP: u32 = 64;
    pub const DISCO: u32 = 128;
    pub const FLAM: u32 = 256;
    pub const GHOST: u32 = 512;
    pub const ACCENT: u32 = 1024;
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Chart,
    Mid,
}

#[derive(Clone, Copy, PartialEq)]
enum DrumType {
    FourLane,
    FourLanePro,
    FiveLane,
}

#[derive(Clone, Copy)]
struct Event {
    tick: i64,
    length: i64,
    kind: u8,
}

#[derive(Clone, Copy)]
struct Phrase {
    tick: i64,
    length: i64,
}

#[derive(Clone, Copy)]
struct FlexLane {
    tick: i64,
    length: i64,
    double: bool,
}

#[derive(Clone, Copy)]
struct Freestyle {
    tick: i64,
    length: i64,
    coda: bool,
}

/// One instrument + difficulty as the format parsers leave it (scan-chart's `RawChartData` track).
struct RawTrack {
    instrument: &'static str,
    difficulty: &'static str,
    star_power: Vec<Phrase>,
    solos: Vec<Phrase>,
    flex_lanes: Vec<FlexLane>,
    freestyle: Vec<Freestyle>,
    /// Notes and note modifiers only.
    events: Vec<Event>,
}

impl RawTrack {
    fn new(instrument: &'static str, difficulty: &'static str) -> Self {
        Self { instrument, difficulty, star_power: Vec::new(), solos: Vec::new(), flex_lanes: Vec::new(), freestyle: Vec::new(), events: Vec::new() }
    }

    /// Files an event under its phrase list, or as a note/modifier.
    fn push(&mut self, e: Event, first_coda: Option<i64>) {
        match e.kind {
            ev::STAR_POWER => self.star_power.push(Phrase { tick: e.tick, length: e.length }),
            ev::REJECTED_STAR_POWER => {}
            ev::SOLO_SECTION => self.solos.push(Phrase { tick: e.tick, length: e.length }),
            ev::FLEX_LANE_SINGLE | ev::FLEX_LANE_DOUBLE => {
                self.flex_lanes.push(FlexLane { tick: e.tick, length: e.length, double: e.kind == ev::FLEX_LANE_DOUBLE })
            }
            ev::FREESTYLE_SECTION => {
                self.freestyle.push(Freestyle { tick: e.tick, length: e.length, coda: first_coda.is_some_and(|c| e.tick >= c) })
            }
            _ => self.events.push(e),
        }
    }
}

struct RawChart {
    resolution: f64,
    /// (tick, beats per minute)
    tempos: Vec<(i64, f64)>,
    /// (tick, numerator, denominator)
    time_signatures: Vec<(i64, u32, u32)>,
    tracks: Vec<RawTrack>,
}

#[derive(Clone, Copy)]
struct Note {
    tick: i64,
    length: i64,
    kind: u32,
    flags: u32,
}

/// One track after the chart rules have been applied (scan-chart's `ParsedChart` track).
struct Track {
    instrument: &'static str,
    difficulty: &'static str,
    star_power: Vec<Phrase>,
    solos: Vec<Phrase>,
    flex_lanes: Vec<FlexLane>,
    freestyle: Vec<Freestyle>,
    note_groups: Vec<Vec<Note>>,
}

/// The track hash of every instrument + difficulty in a notes.chart or notes.mid, as
/// (scan-chart instrument, difficulty, url-safe base64 hash without padding).
pub fn track_hashes(data: &[u8], mid: bool, modifiers: &Modifiers) -> Result<Vec<(&'static str, &'static str, String)>> {
    let format = if mid { Format::Mid } else { Format::Chart };
    let raw = match format {
        Format::Mid => midi::parse(data, modifiers)?,
        Format::Chart => chart::parse(data)?,
    };
    let tracks = resolve(&raw, modifiers, format);
    let mut out: Vec<(&'static str, &'static str, String)> = Vec::new();
    for t in &tracks {
        // A .mid can name the same part twice; scan-chart hashes the first one.
        if !out.iter().any(|o| o.0 == t.instrument && o.1 == t.difficulty) {
            out.push((t.instrument, t.difficulty, hash(&raw, t)));
        }
    }
    Ok(out)
}

/// scan-chart's `parseChartFile`, minus the timing it doesn't hash.
fn resolve(raw: &RawChart, m: &Modifiers, format: Format) -> Vec<Track> {
    let drum_tracks = || raw.tracks.iter().filter(|t| t.instrument == "drums");
    let drum_type = if m.pro_drums {
        DrumType::FourLanePro
    } else if m.five_lane_drums {
        DrumType::FiveLane
    } else if drum_tracks().any(|t| t.events.iter().any(|e| is_cymbal_or_tom_marker(e.kind))) {
        DrumType::FourLanePro
    } else if drum_tracks().any(|t| t.events.iter().any(|e| e.kind == ev::FIVE_GREEN_DRUM)) {
        DrumType::FiveLane
    } else {
        DrumType::FourLane
    };

    raw.tracks
        .iter()
        .map(|t| {
            let mut events = t.events.clone();
            trim_sustains(&mut events, m.sustain_cutoff_threshold, raw.resolution, format);
            let groups = group_by_tick(events);
            let groups = if t.instrument == "drums" {
                resolve_drum_modifiers(groups, drum_type, format)
            } else {
                resolve_fret_modifiers(groups, m, raw.resolution, format)
            };
            let mut note_groups = snap_chords(groups, m.chord_snap_threshold, t.instrument == "drums");
            fix_note_overlaps(&mut note_groups);
            Track {
                instrument: t.instrument,
                difficulty: t.difficulty,
                star_power: fix_phrase_overlaps(t.star_power.clone()),
                solos: fix_phrase_overlaps(t.solos.clone()),
                flex_lanes: fix_flex_lane_overlaps(t.flex_lanes.clone()),
                freestyle: t.freestyle.clone(),
                note_groups,
            }
        })
        .collect()
}

/// lodash `groupBy(tick)` + `values()`: groups in ascending tick order, events in their original order.
fn group_by_tick(mut events: Vec<Event>) -> Vec<Vec<Event>> {
    events.sort_by_key(|e| e.tick);
    let mut groups: Vec<Vec<Event>> = Vec::new();
    for e in events {
        match groups.last_mut() {
            Some(g) if g[0].tick == e.tick => g.push(e),
            _ => groups.push(vec![e]),
        }
    }
    groups
}

fn is_cymbal_or_tom_marker(kind: u8) -> bool {
    matches!(
        kind,
        ev::YELLOW_CYMBAL_MARKER | ev::BLUE_CYMBAL_MARKER | ev::GREEN_CYMBAL_MARKER | ev::YELLOW_TOM_MARKER | ev::BLUE_TOM_MARKER | ev::GREEN_TOM_MARKER
    )
}

fn trim_sustains(events: &mut [Event], cutoff: i64, resolution: f64, format: Format) {
    let threshold = if cutoff != -1 {
        cutoff as f64
    } else if format == Format::Mid {
        (resolution / 3.0).floor() + 1.0
    } else {
        0.0
    };
    if threshold > 0.0 {
        for e in events {
            if e.length as f64 <= threshold {
                e.length = 0;
            }
        }
    }
}

fn is_drum_note(kind: u8) -> bool {
    matches!(kind, ev::KICK | ev::KICK2X | ev::RED_DRUM | ev::YELLOW_DRUM | ev::BLUE_DRUM | ev::FIVE_ORANGE_FOUR_GREEN_DRUM | ev::FIVE_GREEN_DRUM)
}

fn is_kick(kind: u8) -> bool {
    matches!(kind, ev::KICK | ev::KICK2X)
}

fn resolve_drum_modifiers(groups: Vec<Vec<Event>>, drum_type: DrumType, format: Format) -> Vec<Vec<Note>> {
    let mut out = Vec::new();
    let mut disco = ev::DISCO_FLIP_OFF;
    for events in groups {
        let notes: Vec<&Event> = events.iter().filter(|e| is_drum_note(e.kind) && !is_kick(e.kind)).collect();
        let kicks: Vec<&Event> = events.iter().filter(|e| is_kick(e.kind)).collect();
        let modifiers: Vec<u8> = events.iter().filter(|e| !is_drum_note(e.kind)).map(|e| e.kind).collect();
        // Takes effect from this tick on.
        if let Some(&d) = modifiers.iter().filter(|&&k| matches!(k, ev::DISCO_FLIP_OFF | ev::DISCO_FLIP_ON | ev::DISCO_NO_FLIP_ON)).min() {
            disco = d;
        }
        if notes.is_empty() && kicks.is_empty() {
            continue;
        }
        let mut group = Vec::new();
        let flam = if modifiers.contains(&ev::FORCE_FLAM) { flag::FLAM } else { flag::NONE };
        for k in kicks {
            let double = if k.kind == ev::KICK { flag::NONE } else { flag::DOUBLE_KICK };
            group.push(Note { tick: k.tick, length: k.length, kind: note::KICK, flags: flam | double | ghost_or_accent(k.kind, &modifiers) });
        }
        let orange_and_green = notes.iter().any(|e| e.kind == ev::FIVE_GREEN_DRUM) && notes.iter().any(|e| e.kind == ev::FIVE_ORANGE_FOUR_GREEN_DRUM);
        for n in notes {
            let kind = match n.kind {
                ev::RED_DRUM => note::RED_DRUM,
                ev::YELLOW_DRUM => note::YELLOW_DRUM,
                ev::BLUE_DRUM => note::BLUE_DRUM,
                ev::FIVE_ORANGE_FOUR_GREEN_DRUM => note::GREEN_DRUM,
                _ if orange_and_green => note::BLUE_DRUM,
                _ => note::GREEN_DRUM,
            };
            let can_be_disco = kind == note::RED_DRUM || kind == note::YELLOW_DRUM;
            let disco_flag = if !can_be_disco || disco == ev::DISCO_FLIP_OFF {
                flag::NONE
            } else if disco == ev::DISCO_FLIP_ON {
                flag::DISCO
            } else {
                flag::DISCO_NOFLIP
            };
            let flags = flam | disco_flag | tom_or_cymbal(n.kind, &modifiers, drum_type, format) | ghost_or_accent(n.kind, &modifiers);
            group.push(Note { tick: n.tick, length: n.length, kind, flags });
        }
        out.push(group);
    }
    out
}

fn tom_or_cymbal(kind: u8, modifiers: &[u8], drum_type: DrumType, format: Format) -> u32 {
    let has = |m: u8| modifiers.contains(&m);
    match drum_type {
        DrumType::FourLane => flag::TOM,
        DrumType::FourLanePro => {
            let (marker, tom_marker) = match kind {
                ev::YELLOW_DRUM => (ev::YELLOW_CYMBAL_MARKER, ev::YELLOW_TOM_MARKER),
                ev::BLUE_DRUM => (ev::BLUE_CYMBAL_MARKER, ev::BLUE_TOM_MARKER),
                ev::FIVE_ORANGE_FOUR_GREEN_DRUM => (ev::GREEN_CYMBAL_MARKER, ev::GREEN_TOM_MARKER),
                ev::RED_DRUM | ev::FIVE_GREEN_DRUM => return flag::TOM,
                _ => return flag::NONE,
            };
            // .mid marks toms (cymbal by default); .chart marks cymbals (tom by default).
            let cymbal = if format == Format::Mid { !has(tom_marker) } else { has(marker) };
            if cymbal { flag::CYMBAL } else { flag::TOM }
        }
        DrumType::FiveLane => match kind {
            ev::RED_DRUM | ev::BLUE_DRUM | ev::FIVE_GREEN_DRUM => flag::TOM,
            ev::YELLOW_DRUM | ev::FIVE_ORANGE_FOUR_GREEN_DRUM => flag::CYMBAL,
            _ => flag::NONE,
        },
    }
}

fn ghost_or_accent(kind: u8, modifiers: &[u8]) -> u32 {
    let (accent, ghost) = match kind {
        ev::RED_DRUM => (ev::RED_ACCENT, ev::RED_GHOST),
        ev::YELLOW_DRUM => (ev::YELLOW_ACCENT, ev::YELLOW_GHOST),
        ev::BLUE_DRUM => (ev::BLUE_ACCENT, ev::BLUE_GHOST),
        ev::FIVE_ORANGE_FOUR_GREEN_DRUM => (ev::FIVE_ORANGE_FOUR_GREEN_ACCENT, ev::FIVE_ORANGE_FOUR_GREEN_GHOST),
        ev::FIVE_GREEN_DRUM => (ev::FIVE_GREEN_ACCENT, ev::FIVE_GREEN_GHOST),
        ev::KICK | ev::KICK2X => (ev::KICK_ACCENT, ev::KICK_GHOST),
        _ => return flag::NONE,
    };
    if modifiers.contains(&accent) {
        flag::ACCENT
    } else if modifiers.contains(&ghost) {
        flag::GHOST
    } else {
        flag::NONE
    }
}

fn is_fret_note(kind: u8) -> bool {
    matches!(
        kind,
        ev::OPEN | ev::GREEN | ev::RED | ev::YELLOW | ev::BLUE | ev::ORANGE | ev::BLACK1 | ev::BLACK2 | ev::BLACK3 | ev::WHITE1 | ev::WHITE2 | ev::WHITE3
    )
}

fn fret_note_type(kind: u8) -> u32 {
    match kind {
        ev::OPEN => note::OPEN,
        ev::GREEN => note::GREEN,
        ev::RED => note::RED,
        ev::YELLOW => note::YELLOW,
        ev::BLUE => note::BLUE,
        ev::ORANGE => note::ORANGE,
        ev::BLACK1 => note::BLACK1,
        ev::BLACK2 => note::BLACK2,
        ev::BLACK3 => note::BLACK3,
        ev::WHITE1 => note::WHITE1,
        ev::WHITE2 => note::WHITE2,
        _ => note::WHITE3,
    }
}

/// More than one distinct fret.
fn is_chord(notes: &[Event]) -> bool {
    notes.iter().any(|n| n.kind != notes[0].kind)
}

fn resolve_fret_modifiers(groups: Vec<Vec<Event>>, m: &Modifiers, resolution: f64, format: Format) -> Vec<Vec<Note>> {
    let threshold = if m.hopo_frequency != 0 {
        m.hopo_frequency as f64
    } else if m.eighthnote_hopo {
        (1.0 + resolution / 2.0).floor()
    } else if format == Format::Mid {
        (1.0 + resolution / 3.0).floor()
    } else {
        ((65.0 / 192.0) * resolution).floor()
    };

    let mut out = Vec::new();
    let mut last: Option<Vec<Event>> = None;
    for mut events in groups {
        if events.iter().any(|e| e.kind == ev::FORCE_OPEN) {
            // The longest note becomes an open note (first of equals, like lodash `maxBy`).
            let mut longest: Option<Event> = None;
            for e in events.iter().filter(|e| is_fret_note(e.kind)) {
                if longest.is_none_or(|l| e.length > l.length) {
                    longest = Some(*e);
                }
            }
            events.retain(|e| !is_fret_note(e.kind) && e.kind != ev::FORCE_OPEN);
            if let Some(mut l) = longest {
                l.kind = ev::OPEN;
                events.push(l);
            }
        }
        let notes: Vec<Event> = events.iter().filter(|e| is_fret_note(e.kind)).copied().collect();
        if notes.is_empty() {
            continue;
        }
        let has = |k: u8| events.iter().any(|e| e.kind == k);

        let natural = last.as_ref().is_some_and(|last| {
            let same_note = notes.iter().chain(last.iter()).all(|n| n.kind == notes[0].kind);
            // For compatibility with older games that mostly use .mid.
            let mid_exception = format == Format::Mid && is_chord(last) && notes.iter().all(|n| last.iter().any(|l| l.kind == n.kind));
            (notes[0].tick - last[0].tick) as f64 <= threshold && !is_chord(&notes) && !same_note && !mid_exception
        });
        let unnatural = has(ev::FORCE_UNNATURAL);
        let flags = if has(ev::FORCE_TAP) {
            flag::TAP
        } else if has(ev::FORCE_HOPO) {
            flag::HOPO
        } else if has(ev::FORCE_STRUM) || unnatural == natural {
            // forceUnnatural flips the natural HOPO/strum.
            flag::STRUM
        } else {
            flag::HOPO
        };
        out.push(notes.iter().map(|n| Note { tick: n.tick, length: n.length, kind: fret_note_type(n.kind), flags }).collect());
        last = Some(notes);
    }
    out
}

fn snap_chords(groups: Vec<Vec<Note>>, threshold: i64, drums: bool) -> Vec<Vec<Note>> {
    if threshold <= 0 || groups.is_empty() {
        return groups;
    }
    let mut out: Vec<Vec<Note>> = Vec::new();
    for mut group in groups {
        let Some(prev) = out.last_mut() else {
            out.push(group);
            continue;
        };
        if group[0].tick - prev[0].tick >= threshold {
            out.push(group);
            continue;
        }
        // Resolve flag differences with the group it's snapped to.
        if drums {
            let flags_of = |prev: &[Note], kind: u32| prev.iter().find(|n| n.kind == kind).map(|n| n.flags);
            for n in &mut group {
                if matches!(n.kind, note::KICK | note::RED_DRUM | note::YELLOW_DRUM | note::BLUE_DRUM | note::GREEN_DRUM)
                    && let Some(f) = flags_of(prev, n.kind)
                {
                    n.flags = f;
                }
                if n.kind == note::RED_DRUM || n.kind == note::YELLOW_DRUM {
                    let (red, yellow) = (flags_of(prev, note::RED_DRUM), flags_of(prev, note::YELLOW_DRUM));
                    if red.is_some() || yellow.is_some() {
                        let disco = flag::DISCO | flag::DISCO_NOFLIP;
                        n.flags = (n.flags & !disco) | ((red.unwrap_or(0) | yellow.unwrap_or(0)) & disco);
                    }
                }
            }
        } else {
            let flags = prev[0].flags;
            for n in &mut group {
                n.flags = flags;
            }
        }
        let tick = prev[0].tick;
        for mut n in group {
            n.tick = tick;
            prev.push(n);
        }
    }
    out
}

/// scan-chart's `sortAndFixInvalidNoteOverlaps`.
fn fix_note_overlaps(groups: &mut [Vec<Note>]) {
    for g in groups.iter_mut() {
        // Longest sustain is kept for duplicates.
        g.sort_by(|a, b| a.kind.cmp(&b.kind).then(b.length.cmp(&a.length)).then(b.flags.cmp(&a.flags)));
        g.dedup_by(|b, a| a.kind == b.kind);
    }
    let mut previous: HashMap<u32, (usize, usize)> = HashMap::new();
    for gi in 0..groups.len() {
        for ni in 0..groups[gi].len() {
            let n = groups[gi][ni];
            if let Some((pg, pn)) = previous.insert(n.kind, (gi, ni)) {
                let p = groups[pg][pn];
                if p.tick + p.length > n.tick {
                    groups[gi][ni].length = n.length.max(p.length - (n.tick - p.tick));
                    groups[pg][pn].length = n.tick - p.tick;
                }
            }
        }
    }
}

/// scan-chart's `sortAndFixInvalidEventOverlaps`.
fn fix_phrase_overlaps(mut phrases: Vec<Phrase>) -> Vec<Phrase> {
    // Longest phrase is kept for duplicates.
    phrases.sort_by(|a, b| a.tick.cmp(&b.tick).then(b.length.cmp(&a.length)));
    phrases.dedup_by(|b, a| a.tick == b.tick);
    for i in 1..phrases.len() {
        let (p, e) = (phrases[i - 1], phrases[i]);
        if p.tick + p.length > e.tick {
            phrases[i].length = e.length.max(p.length - (e.tick - p.tick));
            phrases[i - 1].length = e.tick - p.tick;
        }
    }
    phrases
}

/// scan-chart's `sortAndFixInvalidFlexLaneOverlaps`.
fn fix_flex_lane_overlaps(mut lanes: Vec<FlexLane>) -> Vec<FlexLane> {
    lanes.sort_by(|a, b| a.tick.cmp(&b.tick).then(a.double.cmp(&b.double)).then(b.length.cmp(&a.length)));
    lanes.dedup_by(|b, a| a.tick == b.tick && a.double == b.double);
    lanes
}

/// scan-chart's `pruneEmptyPhrases`: drops phrases with no notes in them.
fn prune_empty<T: Copy>(phrases: &[T], span: impl Fn(&T) -> (i64, i64), groups: &[Vec<Note>]) -> Vec<T> {
    let mut out = Vec::new();
    let mut i = 0;
    for p in phrases {
        let (tick, length) = span(p);
        while i < groups.len() && groups[i][0].tick < tick {
            i += 1;
        }
        if i < groups.len() && groups[i][0].tick < tick + if length == 0 { 1 } else { length } {
            out.push(*p);
        }
    }
    out
}

/// The last of each run of equal ticks, in tick order (lodash sortBy, reverse, sortedUniqBy, reverse).
fn last_per_tick<T: Copy>(items: &[T], tick: impl Fn(&T) -> i64) -> Vec<T> {
    let mut sorted = items.to_vec();
    sorted.sort_by_key(&tick);
    let mut out: Vec<T> = Vec::new();
    for item in sorted {
        match out.last_mut() {
            Some(last) if tick(last) == tick(&item) => *last = item,
            _ => out.push(item),
        }
    }
    out
}

/// scan-chart's `calculateTrackHash`.
fn hash(raw: &RawChart, t: &Track) -> String {
    let tempos = last_per_tick(&raw.tempos, |t| t.0);
    let time_signatures = last_per_tick(&raw.time_signatures, |t| t.0);
    let star_power = prune_empty(&t.star_power, |p| (p.tick, p.length), &t.note_groups);
    let solos = prune_empty(&t.solos, |p| (p.tick, p.length), &t.note_groups);
    let flex_lanes = prune_empty(&t.flex_lanes, |p| (p.tick, p.length), &t.note_groups);

    // Big endian for the format header, little endian for everything else.
    let mut b: Vec<u8> = Vec::new();
    b.extend(0x43484e46u32.to_be_bytes());
    b.extend(20240320u32.to_le_bytes());
    b.extend(to_uint32(raw.resolution).to_le_bytes());
    b.extend((tempos.len() as u32).to_le_bytes());
    for (tick, bpm) in &tempos {
        b.extend(tick.to_le_bytes());
        b.extend(bpm.to_le_bytes());
    }
    b.extend((time_signatures.len() as u32).to_le_bytes());
    for (tick, numerator, denominator) in &time_signatures {
        b.extend(tick.to_le_bytes());
        b.extend(numerator.to_le_bytes());
        b.extend(denominator.to_le_bytes());
    }
    for phrases in [&star_power, &solos] {
        b.extend((phrases.len() as u32).to_le_bytes());
        for p in phrases {
            b.extend(p.tick.to_le_bytes());
            b.extend(p.length.to_le_bytes());
        }
    }
    b.extend((flex_lanes.len() as u32).to_le_bytes());
    for f in &flex_lanes {
        b.extend(f.tick.to_le_bytes());
        b.extend(f.length.to_le_bytes());
        b.push(f.double as u8);
    }
    b.extend((t.freestyle.len() as u32).to_le_bytes());
    for f in &t.freestyle {
        b.extend(f.tick.to_le_bytes());
        b.extend(f.length.to_le_bytes());
        b.push(f.coda as u8);
    }
    let notes: Vec<&Note> = t.note_groups.iter().flatten().collect();
    b.extend((notes.len() as u32).to_le_bytes());
    for n in notes {
        b.extend(n.tick.to_le_bytes());
        b.extend(n.length.to_le_bytes());
        b.extend(n.kind.to_le_bytes());
        b.extend(n.flags.to_le_bytes());
    }
    base64url(blake3::hash(&b).as_bytes())
}

/// JavaScript's ToUint32, as `DataView.setUint32` applies it.
fn to_uint32(v: f64) -> u32 {
    if !v.is_finite() {
        return 0;
    }
    v.trunc().rem_euclid(4294967296.0) as u32
}

fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// JavaScript's `String.prototype.trim` whitespace (Unicode White_Space plus the BOM).
fn is_js_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_rfc4648_without_padding() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn parse_float_like_javascript() {
        assert_eq!(parse_float(" 170"), Some(170.0));
        assert_eq!(parse_float("12.5abc"), Some(12.5));
        assert_eq!(parse_float(".5"), Some(0.5));
        assert_eq!(parse_float("5."), Some(5.0));
        assert_eq!(parse_float("1e3x"), Some(1000.0));
        assert_eq!(parse_float("abc"), None);
        assert_eq!(parse_float("-"), None);
    }

    /// Compares every track with scan-chart's hashes. Run with:
    /// TRACKHASH_DIR=<folder> cargo test trackhash -- --ignored --nocapture
    /// where the folder has, per chart, `<id>.chart` or `<id>.mid`, its song.ini values in
    /// `<id>.ini`, and scan-chart's `notesData.trackHashes` (padding removed) in `<id>.json`.
    #[test]
    #[ignore]
    fn matches_scan_chart() {
        let dir = std::env::var("TRACKHASH_DIR").expect("set TRACKHASH_DIR");
        let (mut good, mut bad) = (0, 0);
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let ext = path.extension().unwrap_or_default().to_string_lossy().to_string();
            if ext != "chart" && ext != "mid" {
                continue;
            }
            let mut values = HashMap::new();
            for line in std::fs::read_to_string(path.with_extension("ini")).unwrap_or_default().lines() {
                if let Some((k, v)) = line.split_once('=') {
                    values.insert(k.trim().to_owned(), v.trim().to_owned());
                }
            }
            let expected: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path.with_extension("json")).unwrap()).unwrap();
            let got = track_hashes(&std::fs::read(&path).unwrap(), ext == "mid", &Modifiers::from_ini(&values));
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let Some(expected) = expected.as_array() else {
                if got.is_ok() {
                    println!("{name}: scan-chart failed ({}) but we didn't", expected["error"]);
                }
                continue;
            };
            let got = match got {
                Ok(got) => got,
                Err(e) => {
                    bad += expected.len();
                    println!("{name}: failed: {e:#}");
                    continue;
                }
            };
            for e in expected {
                let (i, d, h) = (e["instrument"].as_str().unwrap(), e["difficulty"].as_str().unwrap(), e["hash"].as_str().unwrap());
                match got.iter().find(|g| g.0 == i && g.1 == d) {
                    Some(g) if g.2 == h => good += 1,
                    other => {
                        bad += 1;
                        println!("{name} {i} {d}: expected {h}, got {:?}", other.map(|g| &g.2));
                    }
                }
            }
            if got.len() != expected.len() {
                println!("{name}: {} tracks, scan-chart has {}", got.len(), expected.len());
            }
        }
        println!("{good} match, {bad} differ");
        assert_eq!(bad, 0);
    }
}
