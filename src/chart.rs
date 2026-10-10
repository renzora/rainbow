//! Reads the notes out of a `notes.chart` or `notes.mid` for the falling-notes preview.
//! Only what the preview draws: gems, sustains, HOPO/tap/cymbal looks, star power and beat lines.

use anyhow::{Context, Result, bail};

/// Lane value for notes drawn as a bar across the highway (open notes, kicks).
pub const BAR: u8 = 255;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Strum,
    Hopo,
    Tap,
    Tom,
    Cymbal,
    Kick,
    DoubleKick,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Layout {
    /// Five-fret guitar, bass and keys.
    Five,
    /// Guitar Hero Live: lanes 0..3 white, 3..6 black, in three columns.
    Ghl,
    Drums4,
    Drums5,
}

impl Layout {
    pub fn lanes(self) -> u8 {
        match self {
            Layout::Five | Layout::Drums5 => 5,
            Layout::Ghl => 3,
            Layout::Drums4 => 4,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Note {
    /// Seconds.
    pub t: f32,
    pub end: f32,
    pub lane: u8,
    pub kind: Kind,
    pub star: bool,
}

pub struct Track {
    pub inst: u8,
    pub diff: u8,
    pub layout: Layout,
    pub notes: Vec<Note>,
}

pub struct Song {
    pub tracks: Vec<Track>,
    /// Beat lines: time and whether it starts a measure.
    pub beats: Vec<(f32, bool)>,
    pub end: f32,
}

impl Song {
    pub fn track(&self, inst: u8, diff: u8) -> Option<&Track> {
        self.tracks.iter().find(|t| t.inst == inst && t.diff == diff)
    }
}

/// Ticks to seconds.
struct TempoMap {
    /// (tick, seconds at that tick, seconds per tick from there).
    points: Vec<(u32, f64, f64)>,
}

impl TempoMap {
    /// `tempos` are (tick, microseconds per beat), in any order.
    fn new(res: u32, mut tempos: Vec<(u32, f64)>) -> Self {
        tempos.sort_by_key(|t| t.0);
        if tempos.first().is_none_or(|t| t.0 != 0) {
            tempos.insert(0, (0, 500_000.0));
        }
        let mut points: Vec<(u32, f64, f64)> = Vec::new();
        for (tick, us) in tempos {
            let secs = points.last().map_or(0.0, |&(t0, s0, spt)| s0 + (tick - t0) as f64 * spt);
            points.push((tick, secs, us / 1_000_000.0 / res as f64));
        }
        Self { points }
    }

    fn secs(&self, tick: u32) -> f32 {
        let i = self.points.partition_point(|p| p.0 <= tick).saturating_sub(1);
        let (t0, s0, spt) = self.points[i];
        (s0 + (tick - t0) as f64 * spt) as f32
    }
}

/// Notes of one instrument + difficulty before timing, in ticks, plus the markers that change
/// how they look. Ranges are (start, end) ticks.
#[derive(Default)]
struct Raw {
    notes: Vec<(u32, u32, u8)>,
    star: Vec<(u32, u32)>,
    /// MIDI-style ranges forcing HOPOs/strums/taps/opens, and tom markers (yellow, blue, green).
    hopo: Vec<(u32, u32)>,
    strum: Vec<(u32, u32)>,
    tap: Vec<(u32, u32)>,
    open: Vec<(u32, u32)>,
    toms: [Vec<(u32, u32)>; 3],
    /// .chart-style single-tick markers: flip HOPO, tap, and cymbal (lane) at a tick.
    flip: Vec<u32>,
    tap_at: Vec<u32>,
    cymbal_at: Vec<(u32, u8)>,
    /// MIDI pro drums: yellow/blue/green are cymbals unless a tom marker says otherwise.
    midi: bool,
}

fn in_ranges(ranges: &[(u32, u32)], tick: u32) -> bool {
    ranges.iter().any(|&(s, e)| tick >= s && tick < e.max(s + 1))
}

/// .chart drum lanes and MIDI drum offsets both count red as 1.
const KICK: u8 = 0;

fn finish(raw: Raw, inst: u8, diff: u8, layout: Layout, res: u32, pro: bool, tempo: &TempoMap) -> Option<Track> {
    let mut notes = raw.notes;
    if notes.is_empty() {
        return None;
    }
    notes.sort_by_key(|n| (n.0, n.2));
    let threshold = res * 65 / 192;
    let mut out = Vec::with_capacity(notes.len());
    let drums = matches!(layout, Layout::Drums4 | Layout::Drums5);
    let mut prev: Option<(u32, Vec<u8>)> = None;
    let mut i = 0;
    while i < notes.len() {
        let tick = notes[i].0;
        let j = i + notes[i..].iter().take_while(|n| n.0 == tick).count();
        let mut chord: Vec<(u32, u32, u8)> = notes[i..j].to_vec();
        chord.dedup_by_key(|n| n.2);
        i = j;
        let star = in_ranges(&raw.star, tick);
        let t = tempo.secs(tick);

        if drums {
            for &(_, _, lane) in &chord {
                let (lane, kind) = match lane {
                    KICK => (BAR, Kind::Kick),
                    100 => (BAR, Kind::DoubleKick),
                    l => {
                        let lane = l - 1;
                        // Yellow, blue and green (4-lane) can be cymbals on pro drums.
                        let pad = (layout == Layout::Drums4 && (1..=3).contains(&lane)).then(|| lane as usize - 1);
                        let cymbal = (pro || !raw.midi)
                            && pad.is_some_and(|p| {
                                if raw.midi { !in_ranges(&raw.toms[p], tick) } else { raw.cymbal_at.contains(&(tick, lane)) }
                            });
                        (lane, if cymbal { Kind::Cymbal } else { Kind::Tom })
                    }
                };
                out.push(Note { t, end: t, lane, kind, star });
            }
            continue;
        }

        // Guitar-style: work out strum/HOPO/tap for the whole chord.
        let lanes: Vec<u8> = chord.iter().map(|n| n.2).collect();
        let natural = prev.as_ref().is_some_and(|(pt, pl)| {
            lanes.len() == 1 && *pl != lanes && !pl.contains(&lanes[0]) && tick - pt <= threshold
        });
        let mut hopo = natural ^ raw.flip.contains(&tick);
        if in_ranges(&raw.hopo, tick) {
            hopo = true;
        } else if in_ranges(&raw.strum, tick) {
            hopo = false;
        }
        let tap = raw.tap_at.contains(&tick) || in_ranges(&raw.tap, tick);
        let kind = if tap { Kind::Tap } else if hopo { Kind::Hopo } else { Kind::Strum };
        let open_range = in_ranges(&raw.open, tick);
        for &(_, len, lane) in &chord {
            let lane = if open_range && lane == 0 { BAR } else { lane };
            out.push(Note { t, end: tempo.secs(tick + len), lane, kind, star });
        }
        prev = Some((tick, lanes));
    }
    Some(Track { inst, diff, layout, notes: out })
}

fn beat_lines(res: u32, sigs: &mut Vec<(u32, u32)>, last_tick: u32, tempo: &TempoMap) -> Vec<(f32, bool)> {
    sigs.sort_by_key(|s| s.0);
    if sigs.first().is_none_or(|s| s.0 != 0) {
        sigs.insert(0, (0, 4));
    }
    let mut beats = Vec::new();
    for (k, &(start, num)) in sigs.iter().enumerate() {
        let stop = sigs.get(k + 1).map_or(last_tick + res * 4, |s| s.0);
        let num = num.max(1);
        let mut tick = start;
        let mut n = 0;
        while tick < stop {
            beats.push((tempo.secs(tick), n % num == 0));
            tick += res;
            n += 1;
        }
    }
    beats
}

// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€ .chart â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn chart_instrument(name: &str) -> Option<(u8, u8)> {
    let (diff, rest) = [("Expert", 0), ("Hard", 1), ("Medium", 2), ("Easy", 3)]
        .iter()
        .find_map(|(d, k)| name.strip_prefix(d).map(|r| (*k, r)))?;
    let inst = match rest {
        "Single" => 0,
        "DoubleGuitar" => 1,
        "DoubleRhythm" => 2,
        "DoubleBass" => 3,
        "Drums" => 4,
        "Keyboard" => 5,
        "GHLGuitar" => 6,
        "GHLCoop" => 7,
        "GHLRhythm" => 8,
        "GHLBass" => 9,
        _ => return None,
    };
    Some((inst, diff))
}

pub fn parse_chart(text: &str, pro_drums: bool) -> Result<Song> {
    let mut res = 192;
    let mut tempos = Vec::new();
    let mut sigs = Vec::new();
    let mut raws: Vec<(u8, u8, Raw)> = Vec::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_owned();
            if let Some((inst, diff)) = chart_instrument(&section) {
                raws.push((inst, diff, Raw::default()));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        let mut words = value.split_whitespace();
        match section.as_str() {
            "Song" if key == "Resolution" => res = value.parse().unwrap_or(192).max(1),
            "SyncTrack" => {
                let Ok(tick) = key.parse::<u32>() else { continue };
                match (words.next(), words.next().and_then(|w| w.parse::<f64>().ok())) {
                    (Some("B"), Some(bpm)) if bpm > 0.0 => tempos.push((tick, 60_000_000_000.0 / bpm)),
                    (Some("TS"), Some(num)) => sigs.push((tick, num as u32)),
                    _ => {}
                }
            }
            _ => {
                let Some((inst, _, raw)) = raws.last_mut().filter(|_| chart_instrument(&section).is_some()) else { continue };
                let Ok(tick) = key.parse::<u32>() else { continue };
                let kind = words.next();
                let (Some(a), len) = (words.next().and_then(|w| w.parse::<u32>().ok()), words.next().and_then(|w| w.parse::<u32>().ok()).unwrap_or(0))
                else {
                    continue;
                };
                match (kind, *inst) {
                    (Some("S"), _) if a == 2 => raw.star.push((tick, tick + len)),
                    (Some("N"), 4) => match a {
                        0 => raw.notes.push((tick, 0, KICK)),
                        1..=5 => raw.notes.push((tick, 0, a as u8)),
                        32 => raw.notes.push((tick, 0, 100)),
                        66..=68 => raw.cymbal_at.push((tick, (a - 65) as u8)),
                        _ => {}
                    },
                    (Some("N"), 6..=9) => match a {
                        0..=2 => raw.notes.push((tick, len, a as u8)),
                        3 | 4 => raw.notes.push((tick, len, a as u8)),
                        8 => raw.notes.push((tick, len, 5)),
                        7 => raw.notes.push((tick, len, BAR)),
                        5 => raw.flip.push(tick),
                        6 => raw.tap_at.push(tick),
                        _ => {}
                    },
                    (Some("N"), _) => match a {
                        0..=4 => raw.notes.push((tick, len, a as u8)),
                        7 => raw.notes.push((tick, len, BAR)),
                        5 => raw.flip.push(tick),
                        6 => raw.tap_at.push(tick),
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
    let tempo = TempoMap::new(res, tempos);
    build(raws, res, pro_drums, &tempo, &mut sigs)
}

fn build(raws: Vec<(u8, u8, Raw)>, res: u32, pro_drums: bool, tempo: &TempoMap, sigs: &mut Vec<(u32, u32)>) -> Result<Song> {
    let mut last_tick = 0;
    let mut tracks = Vec::new();
    for (inst, diff, raw) in raws {
        last_tick = last_tick.max(raw.notes.iter().map(|n| n.0 + n.1).max().unwrap_or(0));
        let layout = match inst {
            4 if raw.notes.iter().any(|n| n.2 == 5) => Layout::Drums5,
            4 => Layout::Drums4,
            6..=9 => Layout::Ghl,
            _ => Layout::Five,
        };
        if let Some(t) = finish(raw, inst, diff, layout, res, pro_drums, tempo) {
            tracks.push(t);
        }
    }
    if tracks.is_empty() {
        bail!("the chart has no playable notes");
    }
    let end = tracks.iter().flat_map(|t| t.notes.iter().map(|n| n.end)).fold(0.0, f32::max);
    Ok(Song { beats: beat_lines(res, sigs, last_tick, tempo), tracks, end })
}

// â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€ .mid â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

enum Ev {
    On(u8),
    Off(u8),
    Tempo(f64),
    Sig(u32),
    Name(String),
    Text(String),
    Sysex(Vec<u8>),
}

struct Bytes<'a> {
    d: &'a [u8],
    p: usize,
}

impl Bytes<'_> {
    fn u8(&mut self) -> Result<u8> {
        let b = *self.d.get(self.p).context("truncated MIDI file")?;
        self.p += 1;
        Ok(b)
    }
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.p.checked_add(n).filter(|&e| e <= self.d.len()).context("truncated MIDI file")?;
        let s = &self.d[self.p..end];
        self.p = end;
        Ok(s)
    }
    fn var(&mut self) -> Result<u32> {
        let mut v = 0u32;
        for _ in 0..4 {
            let b = self.u8()?;
            v = (v << 7) | (b & 0x7f) as u32;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        bail!("bad MIDI number")
    }
}

/// Every track's events as (absolute tick, event).
fn read_midi(data: &[u8]) -> Result<(u32, Vec<Vec<(u32, Ev)>>)> {
    let mut b = Bytes { d: data, p: 0 };
    if b.take(4)? != b"MThd" {
        bail!("not a MIDI file");
    }
    let len = u32::from_be_bytes(b.take(4)?.try_into()?) as usize;
    let header = b.take(len)?;
    if header.len() < 6 {
        bail!("bad MIDI header");
    }
    let division = u16::from_be_bytes([header[4], header[5]]);
    if division & 0x8000 != 0 || division == 0 {
        bail!("SMPTE-timed MIDI isn't supported");
    }
    let mut tracks = Vec::new();
    while b.p + 8 <= data.len() {
        let id: [u8; 4] = b.take(4)?.try_into()?;
        let len = u32::from_be_bytes(b.take(4)?.try_into()?) as usize;
        let body = b.take(len.min(data.len() - b.p))?;
        if &id != b"MTrk" {
            continue;
        }
        let mut t = Bytes { d: body, p: 0 };
        let mut events = Vec::new();
        let (mut tick, mut running) = (0u32, 0u8);
        while t.p < body.len() {
            tick = tick.saturating_add(t.var()?);
            let mut status = t.u8()?;
            if status < 0x80 {
                // Running status: this byte is already the first data byte.
                t.p -= 1;
                status = running;
            }
            match status {
                0xff => {
                    let kind = t.u8()?;
                    let len = t.var()? as usize;
                    let d = t.take(len)?;
                    match kind {
                        0x51 if d.len() == 3 => events.push((tick, Ev::Tempo(u32::from_be_bytes([0, d[0], d[1], d[2]]) as f64))),
                        0x58 if !d.is_empty() => events.push((tick, Ev::Sig(d[0] as u32))),
                        0x03 => events.push((tick, Ev::Name(String::from_utf8_lossy(d).into_owned()))),
                        0x01 => events.push((tick, Ev::Text(String::from_utf8_lossy(d).into_owned()))),
                        0x2f => break,
                        _ => {}
                    }
                }
                0xf0 | 0xf7 => {
                    let len = t.var()? as usize;
                    events.push((tick, Ev::Sysex(t.take(len)?.to_vec())));
                }
                0x80..=0xef => {
                    running = status;
                    let a = t.u8()?;
                    let kind = status & 0xf0;
                    let v = if matches!(kind, 0xc0 | 0xd0) { 0 } else { t.u8()? };
                    match kind {
                        0x90 if v > 0 => events.push((tick, Ev::On(a))),
                        0x80 | 0x90 => events.push((tick, Ev::Off(a))),
                        _ => {}
                    }
                }
                _ => bail!("bad MIDI event"),
            }
        }
        tracks.push(events);
    }
    Ok((division as u32, tracks))
}

fn midi_instrument(name: &str) -> Option<u8> {
    Some(match name.trim() {
        "PART GUITAR" | "T1 GEMS" => 0,
        "PART GUITAR COOP" => 1,
        "PART RHYTHM" => 2,
        "PART BASS" => 3,
        "PART DRUMS" => 4,
        "PART KEYS" => 5,
        "PART GUITAR GHL" => 6,
        "PART GUITAR COOP GHL" => 7,
        "PART RHYTHM GHL" => 8,
        "PART BASS GHL" => 9,
        _ => return None,
    })
}

/// Pairs note-ons with note-offs: (start tick, length, key).
fn midi_notes(events: &[(u32, Ev)]) -> Vec<(u32, u32, u8)> {
    let mut open: [Option<u32>; 128] = [None; 128];
    let mut out = Vec::new();
    for (tick, ev) in events {
        match *ev {
            Ev::On(k) if k < 128 => {
                if let Some(s) = open[k as usize].take() {
                    out.push((s, tick - s, k));
                }
                open[k as usize] = Some(*tick);
            }
            Ev::Off(k) if k < 128 => {
                if let Some(s) = open[k as usize].take() {
                    out.push((s, tick - s, k));
                }
            }
            _ => {}
        }
    }
    out
}

pub fn parse_midi(data: &[u8], pro_drums: bool) -> Result<Song> {
    let (res, tracks) = read_midi(data)?;
    let mut tempos = Vec::new();
    let mut sigs = Vec::new();
    for events in &tracks {
        for (tick, ev) in events {
            match ev {
                Ev::Tempo(us) if *us > 0.0 => tempos.push((*tick, *us)),
                Ev::Sig(n) => sigs.push((*tick, *n)),
                _ => {}
            }
        }
    }
    let tempo = TempoMap::new(res, tempos);
    // Sustains shorter than this are just notes.
    let min_sustain = res / 3;
    let mut raws = Vec::new();
    for events in &tracks {
        let Some(inst) = events.iter().find_map(|(_, e)| if let Ev::Name(n) = e { midi_instrument(n) } else { None }) else {
            continue;
        };
        let notes = midi_notes(events);
        let ranges = |key: u8| notes.iter().filter(|n| n.2 == key).map(|n| (n.0, n.0 + n.1)).collect::<Vec<_>>();
        let star = ranges(116);
        let enhanced_opens = events.iter().any(|(_, e)| matches!(e, Ev::Text(t) if t.contains("ENHANCED_OPENS")));
        // Phase Shift sysex: [P S 0 0 difficulty type on]; difficulty 3 = expert .. 0 = easy, 0xff = all.
        let sysex = |kind: u8, diff: u8| -> Vec<(u32, u32)> {
            let mut out = Vec::new();
            let mut start = None;
            for (tick, e) in events {
                if let Ev::Sysex(d) = e
                    && d.len() >= 7
                    && d[..4] == [0x50, 0x53, 0, 0]
                    && d[5] == kind
                    && (d[4] == 0xff || d[4] == 3 - diff)
                {
                    match (d[6], start) {
                        (1, None) => start = Some(*tick),
                        (0, Some(s)) => {
                            out.push((s, *tick));
                            start = None;
                        }
                        _ => {}
                    }
                }
            }
            out
        };
        for diff in 0..4u8 {
            let mut raw = Raw { star: star.clone(), midi: true, ..Raw::default() };
            let ghl = (6..=9).contains(&inst);
            let base = if ghl { [94, 82, 70, 58][diff as usize] } else { [96, 84, 72, 60][diff as usize] };
            for &(tick, len, key) in &notes {
                let len = if len < min_sustain { 0 } else { len };
                if inst == 4 {
                    match key {
                        k if k >= base && k <= base + 5 => raw.notes.push((tick, 0, k - base)),
                        95 if diff == 0 => raw.notes.push((tick, 0, 100)),
                        _ => {}
                    }
                } else if ghl {
                    // GHL: open, then white 1-3, then black 1-3.
                    match key {
                        k if k == base => raw.notes.push((tick, len, BAR)),
                        k if k > base && k <= base + 6 => raw.notes.push((tick, len, k - base - 1)),
                        _ => {}
                    }
                } else {
                    match key {
                        k if k >= base && k <= base + 4 => raw.notes.push((tick, len, k - base)),
                        k if k + 1 == base && enhanced_opens => raw.notes.push((tick, len, BAR)),
                        _ => {}
                    }
                }
            }
            if inst == 4 {
                raw.toms = [ranges(110), ranges(111), ranges(112)];
            } else {
                let (hopo, strum) = if ghl { (base + 7, base + 8) } else { (base + 5, base + 6) };
                raw.hopo = ranges(hopo);
                raw.strum = ranges(strum);
                raw.tap = ranges(104);
                raw.tap.extend(sysex(4, diff));
                raw.open = sysex(1, diff);
            }
            raws.push((inst, diff, raw));
        }
    }
    build(raws, res, pro_drums, &tempo, &mut sigs)
}

#[cfg(test)]
mod tests {
    /// Run with: CHART_SNG=<file.sng> CHART_MID=<notes.mid> cargo test chart_samples -- --ignored --nocapture
    #[test]
    #[ignore]
    fn chart_samples() {
        let summary = |song: &super::Song| {
            for t in &song.tracks {
                println!("  inst {} diff {}: {} notes", t.inst, t.diff, t.notes.len());
            }
            println!("  {} beat lines, ends at {:.1}s", song.beats.len(), song.end);
        };
        if let Ok(path) = std::env::var("CHART_SNG") {
            let data = std::fs::read(path).unwrap();
            let (_, file) = crate::sng::read_file(&data, &["notes.chart", "notes.mid"]).unwrap();
            let (name, bytes) = file.expect("no chart in .sng");
            println!("{name}:");
            let song = if name.ends_with(".chart") {
                super::parse_chart(&String::from_utf8_lossy(&bytes), false).unwrap()
            } else {
                super::parse_midi(&bytes, false).unwrap()
            };
            summary(&song);
        }
        if let Ok(path) = std::env::var("CHART_MID") {
            println!("converted notes.mid:");
            summary(&super::parse_midi(&std::fs::read(path).unwrap(), true).unwrap());
        }
    }
}
