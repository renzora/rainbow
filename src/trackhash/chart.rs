//! scan-chart's `parseNotesFromChart`: notes.chart to `RawChart`.

use std::collections::HashMap;

use anyhow::{Result, bail};

use super::{Event, RawChart, RawTrack, ev, is_js_space, to_uint32};

/// .chart section names, in the order scan-chart lists them.
const TRACKS: [(&str, &str, &str); 40] = [
    ("ExpertSingle", "guitar", "expert"),
    ("HardSingle", "guitar", "hard"),
    ("MediumSingle", "guitar", "medium"),
    ("EasySingle", "guitar", "easy"),
    ("ExpertDoubleGuitar", "guitarcoop", "expert"),
    ("HardDoubleGuitar", "guitarcoop", "hard"),
    ("MediumDoubleGuitar", "guitarcoop", "medium"),
    ("EasyDoubleGuitar", "guitarcoop", "easy"),
    ("ExpertDoubleRhythm", "rhythm", "expert"),
    ("HardDoubleRhythm", "rhythm", "hard"),
    ("MediumDoubleRhythm", "rhythm", "medium"),
    ("EasyDoubleRhythm", "rhythm", "easy"),
    ("ExpertDoubleBass", "bass", "expert"),
    ("HardDoubleBass", "bass", "hard"),
    ("MediumDoubleBass", "bass", "medium"),
    ("EasyDoubleBass", "bass", "easy"),
    ("ExpertDrums", "drums", "expert"),
    ("HardDrums", "drums", "hard"),
    ("MediumDrums", "drums", "medium"),
    ("EasyDrums", "drums", "easy"),
    ("ExpertKeyboard", "keys", "expert"),
    ("HardKeyboard", "keys", "hard"),
    ("MediumKeyboard", "keys", "medium"),
    ("EasyKeyboard", "keys", "easy"),
    ("ExpertGHLGuitar", "guitarghl", "expert"),
    ("HardGHLGuitar", "guitarghl", "hard"),
    ("MediumGHLGuitar", "guitarghl", "medium"),
    ("EasyGHLGuitar", "guitarghl", "easy"),
    ("ExpertGHLCoop", "guitarcoopghl", "expert"),
    ("HardGHLCoop", "guitarcoopghl", "hard"),
    ("MediumGHLCoop", "guitarcoopghl", "medium"),
    ("EasyGHLCoop", "guitarcoopghl", "easy"),
    ("ExpertGHLRhythm", "rhythmghl", "expert"),
    ("HardGHLRhythm", "rhythmghl", "hard"),
    ("MediumGHLRhythm", "rhythmghl", "medium"),
    ("EasyGHLRhythm", "rhythmghl", "easy"),
    ("ExpertGHLBass", "bassghl", "expert"),
    ("HardGHLBass", "bassghl", "hard"),
    ("MediumGHLBass", "bassghl", "medium"),
    ("EasyGHLBass", "bassghl", "easy"),
];

pub(super) fn parse(data: &[u8]) -> Result<RawChart> {
    let text = decode(data);
    let sections = file_sections(&text)?;
    if sections.is_empty() {
        bail!("Invalid .chart file: no sections were found.");
    }
    let lines = |name: &str| sections.get(name).map(Vec::as_slice).unwrap_or_default();

    let mut metadata: HashMap<&str, &str> = HashMap::new();
    for line in lines("Song") {
        // ^(.+?) = "?(.*?)"?$
        let key_start = line.chars().next().map_or(0, char::len_utf8);
        if let Some(pos) = line[key_start..].find(" = ").map(|p| p + key_start) {
            let value = &line[pos + 3..];
            let value = value.strip_prefix('"').unwrap_or(value);
            let value = value.strip_suffix('"').unwrap_or(value);
            metadata.insert(&line[..pos], value);
        }
    }
    let resolution = metadata.get("Resolution").map_or(0.0, |r| js_number(r));
    if resolution.is_nan() || resolution == 0.0 {
        bail!("Invalid .chart file: resolution not found.");
    }

    let first_coda = lines("Events").iter().find_map(|line| {
        let (tick, rest) = tick_line(line)?;
        let inner = rest.strip_prefix("E \"")?.strip_suffix('"')?;
        let inner = inner.trim_start_matches(is_js_space).trim_end_matches(is_js_space);
        let inner = inner.strip_prefix('[').unwrap_or(inner);
        let inner = inner.strip_suffix(']').unwrap_or(inner);
        (inner == "coda").then_some(tick)
    });

    let mut tempos = Vec::new();
    let mut time_signatures = Vec::new();
    for line in lines("SyncTrack") {
        let Some((tick, rest)) = tick_line(line) else { continue };
        if let Some(bpm) = rest.strip_prefix("B ").filter(|v| is_digits(v)) {
            let bpm = js_number(bpm) / 1000.0;
            if bpm == 0.0 {
                bail!("Invalid .chart file: Tempo at tick {tick} was zero.");
            }
            tempos.push((tick, bpm));
        } else if let Some(ts) = rest.strip_prefix("TS ") {
            let (numerator, exponent) = match ts.split_once(' ') {
                Some((n, e)) if is_digits(n) && is_digits(e) => (n, Some(e)),
                None if is_digits(ts) => (ts, None),
                _ => continue,
            };
            let numerator = js_number(numerator);
            if numerator == 0.0 {
                bail!("Invalid .chart file: Time signature numerator at tick {tick} was zero.");
            }
            let denominator = exponent.map_or(4, |e| to_uint32(2f64.powf(js_number(e))));
            time_signatures.push((tick, to_uint32(numerator), denominator));
        }
    }
    if tempos.first().is_none_or(|t| t.0 != 0) {
        tempos.insert(0, (0, 120.0));
    }
    if time_signatures.first().is_none_or(|t| t.0 != 0) {
        time_signatures.insert(0, (0, 4, 4));
    }

    let mut tracks = Vec::new();
    for (name, instrument, difficulty) in TRACKS {
        let Some(lines) = sections.get(name) else { continue };
        let mut events: Vec<Event> = lines
            .iter()
            .filter_map(|line| {
                let (tick, code, value, length) = track_line(line)?;
                let kind = event_type(code, value, instrument, difficulty)?;
                Some(Event { tick, length, kind })
            })
            .collect();
        events.sort_by_key(|e| e.tick);
        merge_solo_events(&mut events);
        let mut track = RawTrack::new(instrument, difficulty);
        for e in events {
            track.push(e, first_coda);
        }
        tracks.push(track);
    }

    Ok(RawChart { resolution, tempos, time_signatures, tracks })
}

/// `TextDecoder` with scan-chart's `getEncoding`: UTF-16 if there's a UTF-16 byte order mark,
/// otherwise UTF-8; a leading byte order mark is dropped.
fn decode(data: &[u8]) -> String {
    let utf16 = |be: bool| {
        let units = data[2..].chunks_exact(2).map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) });
        let mut s: String = char::decode_utf16(units).map(|c| c.unwrap_or('\u{fffd}')).collect();
        if data.len() % 2 == 1 {
            s.push('\u{fffd}');
        }
        s
    };
    match data {
        [0xff, 0xfe, ..] => utf16(false),
        [0xfe, 0xff, ..] => utf16(true),
        _ => {
            let s = String::from_utf8_lossy(data);
            s.strip_prefix('\u{feff}').unwrap_or(&s).to_owned()
        }
    }
}

/// scan-chart's `getFileSections`: each `[Section] { ... }` as its trimmed, non-empty lines.
fn file_sections(text: &str) -> Result<HashMap<String, Vec<String>>> {
    let mut sections = HashMap::new();
    let (mut skip_line, mut reading_section) = (false, false);
    let mut read_start = 0;
    let mut this_section: Option<&str> = None;
    for (i, c) in text.char_indices() {
        if reading_section {
            if c == ']' {
                reading_section = false;
                this_section = Some(&text[read_start..i]);
            }
            if c == '\n' {
                bail!("Invalid .chart file: unexpected new line when parsing section at index {i}");
            }
            continue;
        }
        // Skip everything after '=' on a line, so values can't open or close sections.
        if c == '=' {
            skip_line = true;
        }
        if c == '\n' {
            skip_line = false;
        }
        if skip_line {
            continue;
        }
        match c {
            '{' => {
                skip_line = true;
                read_start = i + 1;
            }
            '}' => {
                let Some(name) = this_section.filter(|s| !s.is_empty()) else {
                    bail!("Invalid .chart file: end of section reached before a section name was found at index {i}");
                };
                let lines = text[read_start..i]
                    .split('\n')
                    .map(|l| l.trim_matches(is_js_space).to_owned())
                    .filter(|l| !l.is_empty())
                    .collect();
                sections.insert(name.to_owned(), lines);
            }
            '[' => {
                read_start = i + 1;
                reading_section = true;
            }
            _ => {}
        }
    }
    Ok(sections)
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `^(\d+) = (.*)$`: the tick and the rest.
fn tick_line(line: &str) -> Option<(i64, &str)> {
    let (tick, rest) = line.split_once(" = ")?;
    is_digits(tick).then(|| (js_number(tick) as i64, rest))
}

/// `^(\d+) = ([A-Z]+) ([\w\s[\]]+?)( \d+)?$`: tick, type code, value and length.
fn track_line(line: &str) -> Option<(i64, &str, &str, i64)> {
    let (tick, rest) = tick_line(line)?;
    let (code, value) = rest.split_once(' ')?;
    if code.is_empty() || !code.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || is_js_space(c) || c == '[' || c == ']') {
        return None;
    }
    // The value is as short as it can be, so a trailing " <digits>" is the length.
    if let Some((v, length)) = value.rsplit_once(' ')
        && !v.is_empty()
        && is_digits(length)
    {
        return Some((tick, code, v, js_number(length) as i64));
    }
    Some((tick, code, value, 0))
}

/// `Number(s)` for the plain decimal numbers the regexes allow.
fn js_number(s: &str) -> f64 {
    let s = s.trim_matches(is_js_space);
    if s.is_empty() { 0.0 } else { s.parse().unwrap_or(f64::NAN) }
}

/// `[mix <difficulty> drums<n><flag>]` disco flip events, as (difficulty index, flag).
pub(super) fn disco_flip(text: &str) -> Option<(usize, &str)> {
    let s = text.trim_start_matches(is_js_space).trim_end_matches(is_js_space);
    let s = s.strip_prefix('[').unwrap_or(s);
    let s = s.strip_suffix(']').unwrap_or(s);
    let s = s.strip_prefix("mix")?;
    let b = s.as_bytes();
    if b.len() < 9 || !matches!(b[0], b' ' | b'_') || !(b'0'..=b'3').contains(&b[1]) || !matches!(b[2], b' ' | b'_') {
        return None;
    }
    let rest = s[3..].strip_prefix("drums")?;
    let (n, flag) = rest.split_at_checked(1)?;
    if !matches!(n, "0" | "1" | "2" | "3" | "4" | "5") || !matches!(flag, "" | "d" | "dnoflip" | "easy" | "easynokick") {
        return None;
    }
    Some(((b[1] - b'0') as usize, flag))
}

/// Difficulties as disco flip events number them.
pub(super) const DISCO_DIFFICULTIES: [&str; 4] = ["easy", "medium", "hard", "expert"];

fn event_type(code: &str, value: &str, instrument: &str, difficulty: &str) -> Option<u8> {
    match code {
        "E" => match value {
            "solo" => Some(ev::SOLO_SECTION_START),
            "soloend" => Some(ev::SOLO_SECTION_END),
            _ => {
                let (diff, flag) = disco_flip(value)?;
                if DISCO_DIFFICULTIES[diff] != difficulty {
                    return None;
                }
                match flag {
                    "" => Some(ev::DISCO_FLIP_OFF),
                    "d" => Some(ev::DISCO_FLIP_ON),
                    "dnoflip" => Some(ev::DISCO_NO_FLIP_ON),
                    _ => None,
                }
            }
        },
        "S" => match value {
            "2" => Some(ev::STAR_POWER),
            "64" => Some(ev::FREESTYLE_SECTION),
            "65" => Some(ev::FLEX_LANE_SINGLE),
            "66" => Some(ev::FLEX_LANE_DOUBLE),
            _ => None,
        },
        "N" => match instrument {
            "drums" => match value {
                "0" => Some(ev::KICK),
                "1" => Some(ev::RED_DRUM),
                "2" => Some(ev::YELLOW_DRUM),
                "3" => Some(ev::BLUE_DRUM),
                "4" => Some(ev::FIVE_ORANGE_FOUR_GREEN_DRUM),
                "5" => Some(ev::FIVE_GREEN_DRUM),
                "32" => Some(ev::KICK2X),
                "34" => Some(ev::RED_ACCENT),
                "35" => Some(ev::YELLOW_ACCENT),
                "36" => Some(ev::BLUE_ACCENT),
                "37" => Some(ev::FIVE_ORANGE_FOUR_GREEN_ACCENT),
                "38" => Some(ev::FIVE_GREEN_ACCENT),
                "40" => Some(ev::RED_GHOST),
                "41" => Some(ev::YELLOW_GHOST),
                "42" => Some(ev::BLUE_GHOST),
                "43" => Some(ev::FIVE_ORANGE_FOUR_GREEN_GHOST),
                "44" => Some(ev::FIVE_GREEN_GHOST),
                "66" => Some(ev::YELLOW_CYMBAL_MARKER),
                "67" => Some(ev::BLUE_CYMBAL_MARKER),
                "68" => Some(ev::GREEN_CYMBAL_MARKER),
                _ => None,
            },
            "guitarghl" | "guitarcoopghl" | "rhythmghl" | "bassghl" => match value {
                "0" => Some(ev::WHITE1),
                "1" => Some(ev::WHITE2),
                "2" => Some(ev::WHITE3),
                "3" => Some(ev::BLACK1),
                "4" => Some(ev::BLACK2),
                "5" => Some(ev::FORCE_UNNATURAL),
                "6" => Some(ev::FORCE_TAP),
                "7" => Some(ev::OPEN),
                "8" => Some(ev::BLACK3),
                _ => None,
            },
            _ => match value {
                "0" => Some(ev::GREEN),
                "1" => Some(ev::RED),
                "2" => Some(ev::YELLOW),
                "3" => Some(ev::BLUE),
                "4" => Some(ev::ORANGE),
                "5" => Some(ev::FORCE_UNNATURAL),
                "6" => Some(ev::FORCE_TAP),
                "7" => Some(ev::OPEN),
                _ => None,
            },
        },
        _ => None,
    }
}

/// Pairs `solo` and `soloend` events into solo sections. .chart solos include notes on their
/// last tick, unlike other phrases, so the length gets one extra tick.
fn merge_solo_events(events: &mut Vec<Event>) {
    let mut starts: Vec<usize> = Vec::new();
    for i in 0..events.len() {
        match events[i].kind {
            ev::SOLO_SECTION_START => starts.push(i),
            ev::SOLO_SECTION_END => {
                if let Some(s) = starts.pop() {
                    events[s].kind = ev::SOLO_SECTION;
                    events[s].length = events[i].tick - events[s].tick + 1;
                }
            }
            _ => {}
        }
    }
    events.retain(|e| e.kind != ev::SOLO_SECTION_START && e.kind != ev::SOLO_SECTION_END);
}
