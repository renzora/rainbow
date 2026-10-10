//! scan-chart's `parseNotesFromMidi`: notes.mid to `RawChart`, reading the file the way the
//! `midi-file` package does.

use anyhow::{Result, bail};

use super::chart::{DISCO_DIFFICULTIES, disco_flip};
use super::{DIFFICULTIES, Event, Modifiers, RawChart, RawTrack, ev, is_js_space};

/// Track names scan-chart reads, and the instrument each part is.
const TRACK_NAMES: [(&str, Option<&str>); 13] = [
    ("T1 GEMS", Some("guitar")),
    ("PART GUITAR", Some("guitar")),
    ("PART GUITAR COOP", Some("guitarcoop")),
    ("PART RHYTHM", Some("rhythm")),
    ("PART BASS", Some("bass")),
    ("PART DRUMS", Some("drums")),
    ("PART KEYS", Some("keys")),
    ("PART GUITAR GHL", Some("guitarghl")),
    ("PART GUITAR COOP GHL", Some("guitarcoopghl")),
    ("PART RHYTHM GHL", Some("rhythmghl")),
    ("PART BASS GHL", Some("bassghl")),
    ("PART VOCALS", None),
    ("EVENTS", None),
];

#[derive(Clone, Copy, PartialEq)]
enum InstrumentType {
    FiveFret,
    SixFret,
    Drums,
}

fn instrument_type(instrument: &str) -> InstrumentType {
    match instrument {
        "drums" => InstrumentType::Drums,
        "guitarghl" | "guitarcoopghl" | "rhythmghl" | "bassghl" => InstrumentType::SixFret,
        _ => InstrumentType::FiveFret,
    }
}

/// The MIDI events scan-chart looks at.
enum Kind {
    NoteOn { note: u8, velocity: u8, channel: u8 },
    NoteOff { note: u8, channel: u8 },
    /// Meta event 0x01.
    Text(String),
    TrackName(String),
    SetTempo(u32),
    TimeSignature { numerator: u8, denominator: u32 },
    SysEx(Vec<u8>),
    Other,
}

struct MidiEvent {
    /// Absolute tick.
    tick: i64,
    kind: Kind,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn eof(&self) -> bool {
        self.pos >= self.buf.len()
    }
    fn u8(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes([self.u8()?, self.u8()?]))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes([self.u8()?, self.u8()?, self.u8()?, self.u8()?]))
    }
    /// Like `midi-file`, a read past the end returns what's there.
    fn bytes(&mut self, len: usize) -> &'a [u8] {
        let start = self.pos.min(self.buf.len());
        let end = self.pos.saturating_add(len).min(self.buf.len());
        self.pos = self.pos.saturating_add(len);
        &self.buf[start..end]
    }
    /// `midi-file`'s readVarInt, including its 32-bit shifts.
    fn var_int(&mut self) -> i64 {
        let mut result: i64 = 0;
        while let Some(b) = self.u8() {
            if b & 0x80 != 0 {
                result = ((result + (b & 0x7f) as i64) as i32).wrapping_shl(7) as i64;
            } else {
                return result + b as i64;
            }
        }
        result
    }
}

/// `midi-file` decodes text one byte per character.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Reads a .mid into (ticks per beat, tracks of events with absolute ticks).
fn read(data: &[u8]) -> Result<(u16, Vec<Vec<MidiEvent>>)> {
    let mut p = Reader { buf: data, pos: 0 };
    let (id, header) = chunk(&mut p);
    if id != b"MThd" {
        bail!("Bad MIDI file: expected 'MThd'");
    }
    let mut h = Reader { buf: header, pos: 0 };
    let (Some(format), Some(track_count), Some(division)) = (h.u16(), h.u16(), h.u16()) else {
        bail!("Bad MIDI file: truncated header");
    };
    if format != 1 {
        bail!("Invalid .mid file: unsupported header format \"{format}\"");
    }
    if division & 0x8000 != 0 || division == 0 {
        bail!("Invalid .mid file: resolution in ticks per SMPTE frame is not supported");
    }
    let mut tracks = Vec::new();
    while tracks.len() < track_count as usize && !p.eof() {
        let (id, data) = chunk(&mut p);
        if id != b"MTrk" {
            bail!("Bad MIDI file: expected 'MTrk'");
        }
        tracks.push(read_track(data)?);
    }
    if tracks.is_empty() {
        bail!("Invalid .mid file: no tracks detected");
    }
    Ok((division, tracks))
}

fn chunk<'a>(p: &mut Reader<'a>) -> (&'a [u8], &'a [u8]) {
    let id = p.bytes(4);
    let len = p.u32().unwrap_or(0) as usize;
    (id, p.bytes(len))
}

fn read_track(data: &[u8]) -> Result<Vec<MidiEvent>> {
    let mut p = Reader { buf: data, pos: 0 };
    let mut events = Vec::new();
    let mut tick = 0;
    let mut running: Option<u8> = None;
    while !p.eof() {
        tick += p.var_int();
        // A track cut off part way through an event just ends there.
        let Some(kind) = read_event(&mut p, &mut running)? else { break };
        events.push(MidiEvent { tick, kind });
    }
    Ok(events)
}

fn read_event(p: &mut Reader, running: &mut Option<u8>) -> Result<Option<Kind>> {
    let Some(status) = p.u8() else { return Ok(None) };
    if status == 0xff {
        let Some(meta) = p.u8() else { return Ok(None) };
        let len = p.var_int().max(0) as usize;
        let expect = |ok: bool, what: &str| if ok { Ok(()) } else { Err(anyhow::anyhow!("Bad MIDI file: wrong length for {what} event")) };
        return Ok(Some(match meta {
            0x00 => {
                expect(len == 2, "sequenceNumber")?;
                p.bytes(2);
                Kind::Other
            }
            0x01 => Kind::Text(latin1(p.bytes(len))),
            0x03 => Kind::TrackName(latin1(p.bytes(len))),
            0x20 | 0x21 => {
                expect(len == 1, "channelPrefix/portPrefix")?;
                p.bytes(1);
                Kind::Other
            }
            0x2f => {
                expect(len == 0, "endOfTrack")?;
                Kind::Other
            }
            0x51 => {
                expect(len == 3, "setTempo")?;
                let b = p.bytes(3);
                let Ok(b) = <[u8; 3]>::try_from(b) else { return Ok(None) };
                Kind::SetTempo(u32::from_be_bytes([0, b[0], b[1], b[2]]))
            }
            0x54 => {
                expect(len == 5, "smpteOffset")?;
                p.bytes(5);
                Kind::Other
            }
            0x58 => {
                expect(len == 2 || len == 4, "timeSignature")?;
                let b = p.bytes(len);
                if b.len() < 2 {
                    return Ok(None);
                }
                // JavaScript's `1 << n` uses the low five bits of n and gives a signed result.
                Kind::TimeSignature { numerator: b[0], denominator: 1i32.wrapping_shl(b[1] as u32) as u32 }
            }
            0x59 => {
                expect(len == 2, "keySignature")?;
                p.bytes(2);
                Kind::Other
            }
            _ => {
                p.bytes(len);
                Kind::Other
            }
        }));
    }
    if status == 0xf0 || status == 0xf7 {
        let len = p.var_int().max(0) as usize;
        return Ok(Some(Kind::SysEx(p.bytes(len).to_vec())));
    }
    if status & 0xf0 == 0xf0 {
        bail!("Unrecognised MIDI event type byte: {status}");
    }
    let (status, param) = if status & 0x80 == 0 {
        // Running status: this byte is the first parameter.
        let Some(last) = *running else { bail!("Unrecognised MIDI event type: 0") };
        (last, status)
    } else {
        let Some(param) = p.u8() else { return Ok(None) };
        *running = Some(status);
        (status, param)
    };
    let channel = status & 0x0f;
    Ok(Some(match status >> 4 {
        0x8 => {
            let Some(_) = p.u8() else { return Ok(None) };
            Kind::NoteOff { note: param, channel }
        }
        0x9 => {
            let Some(velocity) = p.u8() else { return Ok(None) };
            if velocity == 0 { Kind::NoteOff { note: param, channel } } else { Kind::NoteOn { note: param, velocity, channel } }
        }
        0xa | 0xb | 0xe => {
            let Some(_) = p.u8() else { return Ok(None) };
            Kind::Other
        }
        _ => Kind::Other,
    }))
}

/// A note or phrase start or end, before they're paired up.
#[derive(Clone, Copy)]
struct EventEnd {
    tick: i64,
    kind: u8,
    velocity: u8,
    channel: u8,
    start: bool,
}

#[derive(Clone, Copy)]
struct TrackEvent {
    tick: i64,
    length: i64,
    kind: u8,
    velocity: u8,
    channel: u8,
}

/// Index into per-difficulty arrays: expert, hard, medium, easy (scan-chart's `difficulties`).
type PerDifficulty<T> = [T; 4];

fn difficulty_index(name: &str) -> usize {
    DIFFICULTIES.iter().position(|d| *d == name).unwrap_or(0)
}

pub(super) fn parse(data: &[u8], modifiers: &Modifiers) -> Result<RawChart> {
    let (resolution, midi) = read(data)?;

    // Each track named on its first tick.
    let mut named: Vec<(&'static str, &[MidiEvent])> = Vec::new();
    for track in &midi {
        let mut name = None;
        for e in track.iter().take_while(|e| e.tick == 0) {
            if let Kind::TrackName(n) = &e.kind
                && let Some((known, _)) = TRACK_NAMES.iter().find(|(t, _)| t == n)
            {
                name = Some(*known);
            }
        }
        if let Some(name) = name {
            named.push((name, track));
        }
    }

    let first_coda = named.iter().find(|(n, _)| *n == "EVENTS").and_then(|(_, events)| {
        events.iter().find_map(|e| match &e.kind {
            Kind::Text(t) if matches!(t.trim_matches(is_js_space), "coda" | "[coda]") => Some(e.tick),
            _ => None,
        })
    });

    let mut tempos: Vec<(i64, f64)> = midi[0]
        .iter()
        .filter_map(|e| match e.kind {
            Kind::SetTempo(us) => Some((e.tick, 60_000_000.0 / us as f64)),
            _ => None,
        })
        .collect();
    if tempos.first().is_none_or(|t| t.0 != 0) {
        tempos.insert(0, (0, 120.0));
    }
    let mut time_signatures: Vec<(i64, u32, u32)> = Vec::new();
    for e in &midi[0] {
        if let Kind::TimeSignature { numerator, denominator } = e.kind {
            if numerator == 0 {
                bail!("Invalid .mid file: Time signature numerator at tick {} was zero.", e.tick);
            }
            time_signatures.push((e.tick, numerator as u32, denominator));
        }
    }
    if time_signatures.first().is_none_or(|t| t.0 != 0) {
        time_signatures.insert(0, (0, 4, 4));
    }

    let mut tracks = Vec::new();
    for (name, events) in named {
        let Some(instrument) = TRACK_NAMES.iter().find(|(t, _)| *t == name).and_then(|(_, i)| *i) else { continue };
        let kind = instrument_type(instrument);
        let ends = track_event_ends(events, kind)?;
        let ends = distribute_instrument_events(ends);
        let mut per_difficulty = pair_event_ends(ends);
        split_modifier_sustains(&mut per_difficulty, kind);
        fix_legacy_gh_star_power(&mut per_difficulty, kind, modifiers);
        fix_flex_lane_lds(&mut per_difficulty);
        for (d, events) in per_difficulty.into_iter().enumerate() {
            let mut track = RawTrack::new(instrument, DIFFICULTIES[d]);
            for e in events {
                track.push(Event { tick: e.tick, length: e.length, kind: e.kind }, first_coda);
            }
            if !track.events.is_empty() {
                tracks.push(track);
            }
        }
    }

    Ok(RawChart { resolution: resolution as f64, tempos, time_signatures, tracks })
}

/// scan-chart's `getTrackEventEnds`: [expert, hard, medium, easy, all].
fn track_event_ends(events: &[MidiEvent], kind: InstrumentType) -> Result<[Vec<EventEnd>; 5]> {
    const ALL: usize = 4;
    let mut ends: [Vec<EventEnd>; 5] = Default::default();
    let mut enhanced_opens = false;
    for e in events {
        match &e.kind {
            Kind::SysEx(d) if d.len() > 6 && d[..4] == [0x50, 0x53, 0x00, 0x00] => {
                // Phase Shift: [P S 0 0 difficulty type on]
                let kind = match d[5] {
                    0x01 => ev::FORCE_OPEN,
                    0x04 => ev::FORCE_TAP,
                    _ => continue,
                };
                let target = match d[4] {
                    0xff => ALL,
                    n if (n as usize) < 4 => difficulty_index(DISCO_DIFFICULTIES[n as usize]),
                    n => bail!("Invalid .mid file: Phase Shift SysEx event for difficulty {n}"),
                };
                ends[target].push(EventEnd { tick: e.tick, kind, velocity: 127, channel: 1, start: d[6] == 0x01 });
            }
            &Kind::NoteOn { note, velocity, channel } => note_end(&mut ends, e.tick, note, velocity, channel, true, kind, enhanced_opens),
            &Kind::NoteOff { note, channel } => note_end(&mut ends, e.tick, note, 0, channel, false, kind, enhanced_opens),
            Kind::Text(text) => {
                if kind == InstrumentType::Drums
                    && let Some((diff, flag)) = disco_flip(text)
                {
                    let kind = match flag {
                        "" => Some(ev::DISCO_FLIP_OFF),
                        "d" => Some(ev::DISCO_FLIP_ON),
                        "dnoflip" => Some(ev::DISCO_NO_FLIP_ON),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        // A start and an end, so it's paired up like everything else.
                        let target = difficulty_index(DISCO_DIFFICULTIES[diff]);
                        for start in [true, false] {
                            ends[target].push(EventEnd { tick: e.tick, kind, velocity: 127, channel: 1, start });
                        }
                    }
                }
                if text == "ENHANCED_OPENS" || text == "[ENHANCED_OPENS]" {
                    enhanced_opens = true;
                } else if text == "ENABLE_CHART_DYNAMICS" || text == "[ENABLE_CHART_DYNAMICS]" {
                    for start in [true, false] {
                        ends[ALL].push(EventEnd { tick: e.tick, kind: ev::ENABLE_CHART_DYNAMICS, velocity: 127, channel: 1, start });
                    }
                }
            }
            _ => {}
        }
    }
    Ok(ends)
}

#[allow(clippy::too_many_arguments)]
fn note_end(ends: &mut [Vec<EventEnd>; 5], tick: i64, note: u8, velocity: u8, channel: u8, start: bool, kind: InstrumentType, enhanced_opens: bool) {
    // easy, medium, hard, expert, then instrument-wide events.
    let (target, offset_base) = match note {
        0..=66 => (3, [59, 58, 60]),
        67..=78 => (2, [71, 70, 72]),
        79..=90 => (1, [83, 82, 84]),
        91..=102 => (0, [95, 94, 96]),
        _ => {
            if let Some(kind) = instrument_event_type(note) {
                ends[4].push(EventEnd { tick, kind, velocity, channel, start });
            }
            return;
        }
    };
    let offset = |base: i32| note as i32 - base;
    let event = match kind {
        InstrumentType::FiveFret => match offset(offset_base[0]) {
            0 if enhanced_opens => Some(ev::OPEN),
            1 => Some(ev::GREEN),
            2 => Some(ev::RED),
            3 => Some(ev::YELLOW),
            4 => Some(ev::BLUE),
            5 => Some(ev::ORANGE),
            6 => Some(ev::FORCE_HOPO),
            7 => Some(ev::FORCE_STRUM),
            _ => None,
        },
        InstrumentType::SixFret => match offset(offset_base[1]) {
            0 => Some(ev::OPEN),
            1 => Some(ev::WHITE1),
            2 => Some(ev::WHITE2),
            3 => Some(ev::WHITE3),
            4 => Some(ev::BLACK1),
            5 => Some(ev::BLACK2),
            6 => Some(ev::BLACK3),
            7 => Some(ev::FORCE_HOPO),
            8 => Some(ev::FORCE_STRUM),
            _ => None,
        },
        InstrumentType::Drums => match offset(offset_base[2]) {
            -1 => Some(ev::KICK2X),
            0 => Some(ev::KICK),
            1 => Some(ev::RED_DRUM),
            2 => Some(ev::YELLOW_DRUM),
            3 => Some(ev::BLUE_DRUM),
            4 => Some(ev::FIVE_ORANGE_FOUR_GREEN_DRUM),
            5 => Some(ev::FIVE_GREEN_DRUM),
            _ => None,
        },
    };
    if let Some(kind) = event {
        ends[target].push(EventEnd { tick, kind, velocity, channel, start });
    }
}

/// Events that apply to every difficulty of the instrument.
fn instrument_event_type(note: u8) -> Option<u8> {
    match note {
        103 => Some(ev::SOLO_SECTION),
        104 => Some(ev::FORCE_TAP),
        109 => Some(ev::FORCE_FLAM),
        110 => Some(ev::YELLOW_TOM_MARKER),
        111 => Some(ev::BLUE_TOM_MARKER),
        112 => Some(ev::GREEN_TOM_MARKER),
        116 => Some(ev::STAR_POWER),
        120 => Some(ev::FREESTYLE_SECTION),
        126 => Some(ev::FLEX_LANE_SINGLE),
        127 => Some(ev::FLEX_LANE_DOUBLE),
        _ => None,
    }
}

/// Copies instrument-wide events into each charted difficulty, then sorts by tick (ascending)
/// and event type (descending), keeping the original order otherwise.
fn distribute_instrument_events(mut ends: [Vec<EventEnd>; 5]) -> PerDifficulty<Vec<EventEnd>> {
    let all = std::mem::take(&mut ends[4]);
    for e in all {
        for d in ends.iter_mut().take(4) {
            if !d.is_empty() {
                d.push(e);
            }
        }
    }
    let [mut expert, mut hard, mut medium, mut easy, _] = ends;
    for d in [&mut expert, &mut hard, &mut medium, &mut easy] {
        d.sort_by(|a, b| a.tick.cmp(&b.tick).then(b.kind.cmp(&a.kind)));
    }
    [expert, hard, medium, easy]
}

/// scan-chart's `getTrackEvents`: pairs each end with the latest start of its type on the same
/// channel. Starts that never end are dropped.
fn pair_event_ends(ends: PerDifficulty<Vec<EventEnd>>) -> PerDifficulty<Vec<TrackEvent>> {
    ends.map(|ends| {
        let mut events: Vec<TrackEvent> = Vec::new();
        let mut open: Vec<Vec<usize>> = vec![Vec::new(); ev::COUNT];
        for e in ends {
            let partial = &mut open[e.kind as usize];
            if e.start {
                partial.push(events.len());
                events.push(TrackEvent { tick: e.tick, length: -1, kind: e.kind, velocity: e.velocity, channel: e.channel });
            } else if let Some(pos) = partial.iter().rposition(|&i| events[i].channel == e.channel) {
                let i = partial.remove(pos);
                events[i].length = e.tick - events[i].tick;
            }
        }
        events.retain(|e| e.length != -1);
        events
    })
}

/// scan-chart's `splitMidiModifierSustains`: turns modifier phrases (forced HOPO/strum/tap/open,
/// flams, tom markers) into a zero-length modifier on each note tick under them, like .chart.
fn split_modifier_sustains(per_difficulty: &mut PerDifficulty<Vec<TrackEvent>>, kind: InstrumentType) {
    // Not reset between difficulties.
    let mut dynamics = false;
    let sustains: &[u8] = if kind == InstrumentType::Drums {
        &[ev::FORCE_FLAM, ev::YELLOW_TOM_MARKER, ev::BLUE_TOM_MARKER, ev::GREEN_TOM_MARKER]
    } else {
        &[ev::FORCE_OPEN, ev::FORCE_TAP, ev::FORCE_STRUM, ev::FORCE_HOPO]
    };
    let notes: &[u8] = match kind {
        InstrumentType::FiveFret => &[ev::OPEN, ev::GREEN, ev::RED, ev::YELLOW, ev::BLUE, ev::ORANGE],
        InstrumentType::SixFret => &[ev::OPEN, ev::BLACK3, ev::BLACK2, ev::BLACK1, ev::WHITE3, ev::WHITE2, ev::WHITE1],
        InstrumentType::Drums => {
            &[ev::KICK, ev::KICK2X, ev::RED_DRUM, ev::YELLOW_DRUM, ev::BLUE_DRUM, ev::FIVE_ORANGE_FOUR_GREEN_DRUM, ev::FIVE_GREEN_DRUM]
        }
    };
    for events in per_difficulty.iter_mut() {
        let mut out: Vec<TrackEvent> = Vec::new();
        let mut has_notes = false;
        let mut active: Vec<TrackEvent> = Vec::new();
        let mut last_inserted: [Option<i64>; ev::COUNT] = [None; ev::COUNT];
        for &e in events.iter() {
            if e.kind == ev::ENABLE_CHART_DYNAMICS {
                dynamics = true;
                continue;
            }
            active.retain(|m| if m.length == 0 { m.tick + m.length >= e.tick } else { m.tick + m.length > e.tick });
            if sustains.contains(&e.kind) {
                active.push(e);
                continue;
            }
            if notes.contains(&e.kind) {
                has_notes = true;
                for m in &active {
                    if last_inserted[m.kind as usize].is_none_or(|t| t < e.tick) {
                        last_inserted[m.kind as usize] = Some(e.tick);
                        out.push(TrackEvent { tick: e.tick, length: 0, ..*m });
                    }
                }
                if dynamics && kind == InstrumentType::Drums && (e.velocity == 1 || e.velocity == 127) {
                    let kind = if e.velocity == 1 { drum_ghost(e.kind) } else { drum_accent(e.kind) };
                    out.push(TrackEvent { tick: e.tick, length: 0, kind, velocity: 127, channel: e.channel });
                }
            }
            out.push(e);
        }
        // Don't copy modifiers into uncharted difficulties.
        if !has_notes {
            out.clear();
        }
        *events = out;
    }
}

fn drum_ghost(kind: u8) -> u8 {
    match kind {
        ev::RED_DRUM => ev::RED_GHOST,
        ev::YELLOW_DRUM => ev::YELLOW_GHOST,
        ev::BLUE_DRUM => ev::BLUE_GHOST,
        ev::FIVE_ORANGE_FOUR_GREEN_DRUM => ev::FIVE_ORANGE_FOUR_GREEN_GHOST,
        ev::FIVE_GREEN_DRUM => ev::FIVE_GREEN_GHOST,
        _ => ev::KICK_GHOST,
    }
}

fn drum_accent(kind: u8) -> u8 {
    match kind {
        ev::RED_DRUM => ev::RED_ACCENT,
        ev::YELLOW_DRUM => ev::YELLOW_ACCENT,
        ev::BLUE_DRUM => ev::BLUE_ACCENT,
        ev::FIVE_ORANGE_FOUR_GREEN_DRUM => ev::FIVE_ORANGE_FOUR_GREEN_ACCENT,
        ev::FIVE_GREEN_DRUM => ev::FIVE_GREEN_ACCENT,
        _ => ev::KICK_ACCENT,
    }
}

/// GH1 and GH2 charts mark star power with the solo note.
fn fix_legacy_gh_star_power(per_difficulty: &mut PerDifficulty<Vec<TrackEvent>>, kind: InstrumentType, m: &Modifiers) {
    if kind == InstrumentType::Drums || m.multiplier_note == 116 {
        return;
    }
    for events in per_difficulty.iter_mut() {
        let star_power = events.iter().filter(|e| e.kind == ev::STAR_POWER).count();
        let solos = events.iter().filter(|e| e.kind == ev::SOLO_SECTION).count();
        if m.multiplier_note == 103 || (star_power == 0 && solos > 1) {
            for e in events.iter_mut() {
                e.kind = match e.kind {
                    ev::SOLO_SECTION => ev::STAR_POWER,
                    ev::STAR_POWER => ev::REJECTED_STAR_POWER,
                    k => k,
                };
            }
        }
    }
}

/// Lower difficulties only get activation lanes whose velocity says so.
fn fix_flex_lane_lds(per_difficulty: &mut PerDifficulty<Vec<TrackEvent>>) {
    for (d, max) in [(1, 50), (2, 40), (3, 30)] {
        per_difficulty[d].retain(|e| !matches!(e.kind, ev::FLEX_LANE_SINGLE | ev::FLEX_LANE_DOUBLE) || (21..=max).contains(&e.velocity));
    }
}
