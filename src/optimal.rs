//! The best possible Clone Hero score for a chart: every note hit, plus star power used along
//! the best path, with squeezing (hitting notes early or late to stretch an activation) and
//! whammy. This is Rainbow's own implementation of the game's rules and a search over
//! activations; its results are meant to match the community tool CHOpt's defaults.
//!
//! Units: positions are in beats (quarter notes, `tick / resolution`); star power ("the bar")
//! runs from 0.0 to 1.0.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Work counters for tuning (starts tried, activation simulations, ends tried).
pub static STATS: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];

use crate::chart::{Parsed, TickTrack, time_signatures};

/// Clone Hero's hit window on guitar, either side of a note.
const WINDOW_SECS: f64 = 0.07;
/// A star power phrase fills a quarter of the bar.
pub(crate) const PHRASE: f64 = 0.25;
/// Whammying a star power sustain fills 1/30 of the bar per beat.
const WHAMMY_PER_BEAT: f64 = 1.0 / 30.0;
/// A full bar lasts 8 measures.
pub(crate) const MEASURES_PER_BAR: f64 = 8.0;
/// Clone Hero pays out the last quarter beat of a sustain (and its whammy) early.
const BURST_BEATS: f64 = 0.25;

#[derive(Clone, Copy)]
pub struct Options {
    /// How far into the hit window notes are hit to stretch activations, 0.0 to 1.0.
    pub squeeze: f64,
    /// How much of the hit window before a star power sustain is whammied, 0.0 to 1.0.
    pub early_whammy: f64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            squeeze: 1.0,
            early_whammy: 1.0,
        }
    }
}

/// One star power activation on the best path.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Activation {
    /// Phrases collected since the previous activation, and during this one.
    pub phrases_before: u32,
    pub phrases_during: u32,
    /// Measure numbers (1-based, fractional) of the same.
    pub first_measure: f64,
    pub last_measure: f64,
    /// Seconds into the song of the same.
    pub first_secs: f64,
    pub last_secs: f64,
    /// Extra points it earns.
    pub bonus: u64,
    /// Beats of its first and last points.
    pub first_beat: f64,
    pub last_beat: f64,
}

/// One scoring point (a note or chord, or sustain points), for drawing the path.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct ScorePoint {
    pub beat: f64,
    /// Points before the combo multiplier, and after it (without star power).
    pub base: u64,
    pub value: u64,
    /// Solo and clean-play bonus points it earns, which nothing multiplies.
    pub extra: u64,
    /// The activation (index into `Outcome::activations`) that doubles it, if any.
    pub activation: Option<usize>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    /// Every note hit, with no star power (includes solo bonuses).
    pub no_sp: u64,
    /// The best total with star power.
    pub total: u64,
    /// CHOpt-style path, e.g. "2-3(+1)-2": phrases before each activation, (+phrases during).
    pub path: String,
    pub activations: Vec<Activation>,
    pub points: Vec<ScorePoint>,
    /// The star power bar at the end of each measure along the path: (beat, bar 0..1).
    pub measure_bars: Vec<(f64, f64)>,
}

/// Works out the optimal score for one instrument and difficulty (five-fret or GHL guitar,
/// bass, rhythm, keys, and drums, which have their own rules in `drums.rs`). `sustain_cutoff` is song.ini's `sustain_cutoff_threshold`, if any.
/// Returns None if the track has no notes.
#[cfg_attr(not(test), allow(dead_code))]
pub fn optimise(
    chart: &Parsed,
    inst: u8,
    diff: u8,
    sustain_cutoff: Option<i64>,
    opts: Options,
) -> Option<Outcome> {
    optimise_with_progress(chart, inst, diff, sustain_cutoff, opts, &|_| {})
}

/// Bump when results change, so results saved on disk by an older version aren't used.
const CACHE_VERSION: u32 = 1;

/// `optimise` with default options, kept on disk by chart md5 so it's only worked out once.
/// `progress` is told how far along it is (0.0 to 1.0) when it does have to work it out.
pub fn optimise_cached(
    md5: &str,
    chart: &Parsed,
    inst: u8,
    diff: u8,
    sustain_cutoff: Option<i64>,
    progress: &(dyn Fn(f32) + Sync),
) -> Option<Outcome> {
    let dir = crate::catalogue::cache_dir().join("optimal");
    let path = dir.join(format!("{md5}_{inst}_{diff}_v{CACHE_VERSION}.json"));
    if let Some(saved) = crate::store::read(&path)
        && let Ok(outcome) = serde_json::from_slice::<Option<Outcome>>(&saved)
    {
        return outcome;
    }
    let outcome = optimise_with_progress(chart, inst, diff, sustain_cutoff, Options::default(), progress);
    if let Ok(json) = serde_json::to_vec(&outcome) {
        let _ = crate::store::write(&path, &json);
    }
    outcome
}

/// `optimise`, telling `progress` how far along the search is (0.0 to 1.0, roughly).
pub fn optimise_with_progress(
    chart: &Parsed,
    inst: u8,
    diff: u8,
    sustain_cutoff: Option<i64>,
    opts: Options,
    progress: &(dyn Fn(f32) + Sync),
) -> Option<Outcome> {
    if inst == 4 {
        return crate::drums::optimise(chart, diff, opts, progress);
    }
    let track = chart.track(inst, diff)?;
    let timing = Timing::new(chart);
    let song = Scored::new(&track, &timing, chart.res, sustain_cutoff, opts);
    if song.points.is_empty() {
        return None;
    }
    Some(Search::new(&song, &timing, progress).run())
}

// ─────────────────────────────── Timing ───────────────────────────────

/// Converts between beats, seconds and measures.
pub(crate) struct Timing {
    /// (beat, seconds at that beat, seconds per beat from there).
    tempo: Vec<(f64, f64, f64)>,
    /// (beat, measure number at that beat (0-based), beats per measure from there).
    sigs: Vec<(f64, f64, f64)>,
}

impl Timing {
    pub(crate) fn new(chart: &Parsed) -> Self {
        let res = chart.res as f64;
        // Sorted by tick (stably, so at the same tick the later event wins), 120 BPM until the first.
        let mut tempos: Vec<(u32, f64)> = Vec::new();
        let mut sorted = chart.tempos.clone();
        sorted.sort_by_key(|t| t.0);
        for t in sorted {
            match tempos.last_mut() {
                Some(last) if last.0 == t.0 => *last = t,
                _ => tempos.push(t),
            }
        }
        if tempos.first().is_none_or(|t| t.0 != 0) {
            tempos.insert(0, (0, 500_000.0));
        }
        let mut tempo: Vec<(f64, f64, f64)> = Vec::new();
        for (tick, us) in tempos {
            let beat = tick as f64 / res;
            let secs = tempo
                .last()
                .map_or(0.0, |&(b0, s0, spb0)| s0 + (beat - b0) * spb0);
            tempo.push((beat, secs, us / 1_000_000.0));
        }
        let mut sigs: Vec<(f64, f64, f64)> = Vec::new();
        for (tick, num, den) in time_signatures(&chart.sigs) {
            let beat = tick as f64 / res;
            let bpm = num as f64 * 4.0 / den as f64;
            let measure = sigs
                .last()
                .map_or(0.0, |&(b0, m0, bpm0)| m0 + (beat - b0) / bpm0);
            sigs.push((beat, measure, bpm));
        }
        Self { tempo, sigs }
    }

    pub(crate) fn secs(&self, beat: f64) -> f64 {
        let i = self
            .tempo
            .partition_point(|t| t.0 <= beat)
            .saturating_sub(1);
        let (b0, s0, spb) = self.tempo[i];
        s0 + (beat - b0) * spb
    }

    pub(crate) fn beats(&self, secs: f64) -> f64 {
        let i = self
            .tempo
            .partition_point(|t| t.1 <= secs)
            .saturating_sub(1);
        let (b0, s0, spb) = self.tempo[i];
        b0 + (secs - s0) / spb
    }

    /// Beats per measure at `beat`.
    pub(crate) fn beats_per_measure(&self, beat: f64) -> f64 {
        let i = self.sigs.partition_point(|s| s.0 <= beat).saturating_sub(1);
        self.sigs[i].2
    }

    /// The next time signature change strictly after `beat`.
    pub(crate) fn next_sig(&self, beat: f64) -> f64 {
        let i = self.sigs.partition_point(|s| s.0 <= beat);
        self.sigs.get(i).map_or(f64::INFINITY, |s| s.0)
    }

    /// The beat each measure ends on, up to the first one at or after `until`.
    pub(crate) fn measure_ends(&self, until: f64) -> Vec<f64> {
        let mut out = Vec::new();
        for (k, &(beat, _, bpm)) in self.sigs.iter().enumerate() {
            let stop = self.sigs.get(k + 1).map_or(f64::INFINITY, |s| s.0);
            let mut b = beat + bpm;
            while b <= stop + 1e-9 {
                out.push(b);
                if b >= until {
                    return out;
                }
                b += bpm;
            }
        }
        out
    }

    /// 1-based measure number at `beat`.
    pub(crate) fn measure(&self, beat: f64) -> f64 {
        let i = self.sigs.partition_point(|s| s.0 <= beat).saturating_sub(1);
        let (b0, m0, bpm) = self.sigs[i];
        1.0 + m0 + (beat - b0) / bpm
    }

    /// The beat where `m` measures (0-based, fractional) have gone by.
    pub(crate) fn measure_beat(&self, m: f64) -> f64 {
        let i = self.sigs.partition_point(|s| s.1 <= m).saturating_sub(1);
        let (b0, m0, bpm) = self.sigs[i];
        b0 + (m - m0) * bpm
    }
}

// ─────────────────────────────── Scoring ───────────────────────────────

/// Something that scores: a note (chord) or a group of sustain points at one position.
#[derive(Clone, Copy)]
struct Point {
    beat: f64,
    /// Earliest and latest beat it can be hit (sustain points: exactly at `beat`).
    early: f64,
    late: f64,
    /// Points, already multiplied by the combo multiplier, and before it.
    value: u64,
    base: u64,
    /// Solo and clean-play bonus.
    extra: u64,
    /// The last note of a star power phrase.
    phrase_end: bool,
    note: bool,
    /// Earliest hit time with the full (unsqueezed) window.
    full_early: f64,
}

/// A stretch where whammy fills the bar: continuously over [from, to), then `burst` at `to`.
#[derive(Clone, Copy)]
struct Whammy {
    from: f64,
    to: f64,
    burst: f64,
}

struct Scored {
    points: Vec<Point>,
    whammy: Vec<Whammy>,
    /// Whammy available before each stretch (one more entry than `whammy`: the total).
    whammy_sum: Vec<f64>,
    /// Each star power sustain's own whammy window (note beat, start, end), by note.
    note_whammy: Vec<(f64, f64, f64)>,
    /// The latest end of those windows up to each one (inclusive).
    note_whammy_reach: Vec<f64>,
    no_sp: u64,
    /// The squeeze fraction the hit windows were built with.
    squeeze: f64,
    /// The beat after the last point.
    end: f64,
}

impl Scored {
    fn new(
        track: &TickTrack,
        timing: &Timing,
        res: u32,
        sustain_cutoff: Option<i64>,
        opts: Options,
    ) -> Self {
        let resf = res as f64;
        // Chords: one entry per tick, each lane once (keeping the longer sustain).
        let mut chords: Vec<(u32, Vec<(u8, u32)>)> = Vec::new();
        for &(tick, len, lane) in &track.notes {
            // .mid sustains no longer than the cutoff are just notes; .chart has no cutoff.
            let len = if track.midi {
                let cutoff = sustain_cutoff.unwrap_or(res as i64 / 3 + 1);
                if len as i64 > cutoff { len } else { 0 }
            } else {
                len
            };
            match chords.last_mut() {
                Some((t, gems)) if *t == tick => match gems.iter_mut().find(|g| g.0 == lane) {
                    Some(g) => g.1 = g.1.max(len),
                    None => gems.push((lane, len)),
                },
                _ => chords.push((tick, vec![(lane, len)])),
            }
        }
        // A sustain running into the next note on its lane stops there, and that note carries on
        // for at least the rest of it.
        let mut last_on_lane: HashMap<u8, (usize, usize)> = HashMap::new();
        for c in 0..chords.len() {
            let tick = chords[c].0;
            for g in 0..chords[c].1.len() {
                let lane = chords[c].1[g].0;
                if let Some(&(pc, pg)) = last_on_lane.get(&lane) {
                    let (ptick, plen) = (chords[pc].0, chords[pc].1[pg].1);
                    if ptick + plen > tick {
                        chords[pc].1[pg].1 = tick - ptick;
                        let rest = ptick + plen - tick;
                        chords[c].1[g].1 = chords[c].1[g].1.max(rest);
                    }
                }
                last_on_lane.insert(lane, (c, g));
            }
        }

        let in_ranges =
            |ranges: &[(u32, u32)], tick: u32| ranges.iter().any(|&(s, e)| tick >= s && tick < e);
        // The last note of each phrase.
        let phrase_ends: Vec<u32> = track
            .star
            .iter()
            .filter_map(|&(s, e)| {
                chords
                    .iter()
                    .rev()
                    .find(|c| c.0 >= s && c.0 < e)
                    .map(|c| c.0)
            })
            .collect();

        let window = |beat: f64, frac: f64| {
            let secs = timing.secs(beat);
            (
                timing.beats(secs - WINDOW_SECS * frac),
                timing.beats(secs + WINDOW_SECS * frac),
            )
        };
        let squeeze = opts.squeeze.max(0.001);
        let tick_gap = (res / 25).max(1) as f64;
        let burst_ticks = resf * BURST_BEATS;

        // (beat, order, base value, is note, phrase end, unmultiplied bonus); order puts sustain
        // points that land on a later note's position before that note.
        let mut raw: Vec<(f64, u8, u64, bool, bool, u64)> = Vec::new();
        let mut whammy: Vec<(f64, f64)> = Vec::new();
        let mut note_whammy: Vec<(f64, f64, f64)> = Vec::new();
        for (tick, gems) in &chords {
            let tick = *tick;
            let pos = tick as f64;
            let equal = gems.iter().all(|g| g.1 == gems[0].1);
            // Clone Hero's clean-play bonus and solo bonus aren't multiplied.
            let mut extra = if equal { 2 } else { 2 * gems.len() as u64 };
            if in_ranges(&track.solos, tick) {
                extra += 100;
            }
            raw.push((
                pos / resf,
                1,
                50 * gems.len() as u64,
                true,
                phrase_ends.contains(&tick),
                extra,
            ));
            let mut lens: Vec<u32> = if equal {
                vec![gems[0].1]
            } else {
                gems.iter().map(|g| g.1).collect()
            };
            lens.retain(|&l| l > 0);
            for &len in &lens {
                let count = (len as f64 / tick_gap).ceil() as u64;
                let burst_at = pos + (len as f64 - burst_ticks).max(0.0);
                let mut given = 0;
                let mut at = pos + tick_gap;
                while at < pos + len as f64 - burst_ticks && given < count {
                    raw.push((at / resf, if at == pos { 2 } else { 0 }, 1, false, false, 0));
                    given += 1;
                    at += tick_gap;
                }
                if given < count {
                    raw.push((
                        burst_at / resf,
                        if burst_at == pos { 2 } else { 0 },
                        count - given,
                        false,
                        false,
                        0,
                    ));
                }
            }
            if in_ranges(&track.star, tick) {
                let mut seen: Vec<u32> = Vec::new();
                for &len in &lens {
                    if seen.contains(&len) {
                        continue;
                    }
                    seen.push(len);
                    // Whammy starts within the early window before the note, rounded down to a tick.
                    let start = window(pos / resf, opts.early_whammy).0;
                    let start = (start * resf).floor() / resf;
                    whammy.push((start, (pos + len as f64) / resf));
                    note_whammy.push((pos / resf, start, (pos + len as f64) / resf));
                }
            }
        }
        raw.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

        let mut combo = 0u64;
        let mut no_sp = 0;
        let mut points = Vec::with_capacity(raw.len());
        for (beat, _, base, note, phrase_end, extra) in raw {
            if note {
                combo += 1;
            }
            let mult = (1 + combo / 10).min(4);
            let value = base * mult;
            no_sp += value + extra;
            let (early, late) = if note {
                window(beat, squeeze)
            } else {
                (beat, beat)
            };
            let full_early = if note { window(beat, 1.0).0 } else { beat };
            points.push(Point {
                beat,
                early,
                late,
                value,
                base,
                extra,
                phrase_end,
                note,
                full_early,
            });
        }

        // Overlapping whammy stretches merge; each merged stretch's last quarter beat is paid early.
        whammy.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (a, b) in whammy {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        let whammy: Vec<Whammy> = merged
            .into_iter()
            .map(|(a, b)| {
                let to = (b - BURST_BEATS).max(a);
                Whammy {
                    from: a,
                    to,
                    burst: (b - to) * WHAMMY_PER_BEAT,
                }
            })
            .collect();
        let mut whammy_sum = vec![0.0];
        for s in &whammy {
            whammy_sum
                .push(whammy_sum.last().unwrap() + (s.to - s.from) * WHAMMY_PER_BEAT + s.burst);
        }
        note_whammy.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut note_whammy_reach = Vec::with_capacity(note_whammy.len());
        for &(_, _, end) in &note_whammy {
            note_whammy_reach.push(note_whammy_reach.last().map_or(end, |&r: &f64| r.max(end)));
        }
        let end = points.last().map_or(0.0, |p| p.beat) + 1.0;
        Self {
            end,
            points,
            whammy,
            whammy_sum,
            note_whammy,
            note_whammy_reach,
            no_sp,
            squeeze,
        }
    }
}

// ─────────────────────────────── Star power ───────────────────────────────

enum Run {
    /// The bar emptied at this beat.
    Empty(f64),
    /// It reached the target first.
    Stopped,
    /// The bar at the `until` beat.
    At(f64),
}

impl Scored {
    /// Moves the bar forward from beat `t`. `drain`: an activation is running. `whammy`: all
    /// available whammy is collected. `phrases`: beats at which a phrase adds a quarter bar.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        timing: &Timing,
        mut t: f64,
        mut bar: f64,
        drain: bool,
        whammy: bool,
        phrases: &[f64],
        until: f64,
        target: f64,
    ) -> Run {
        let w: &[Whammy] = if whammy { &self.whammy } else { &[] };
        // Phrases hit before `t` (e.g. just before an activation starts) count straight away.
        let mut ip = 0;
        let mut iw = w.partition_point(|s| s.to < t);
        loop {
            while ip < phrases.len() && phrases[ip] <= t {
                bar = (bar + PHRASE).min(1.0);
                ip += 1;
            }
            while iw < w.len() && w[iw].to <= t {
                if whammy {
                    bar = (bar + w[iw].burst).min(1.0);
                }
                iw += 1;
            }
            if bar >= target {
                return Run::Stopped;
            }
            if t >= until {
                return Run::At(bar);
            }
            let in_whammy = iw < w.len() && w[iw].from <= t;
            let mut next = until;
            if let Some(&p) = phrases.get(ip) {
                next = next.min(p);
            }
            if iw < w.len() {
                next = next.min(if in_whammy { w[iw].to } else { w[iw].from });
            }
            let mut rate = if whammy && in_whammy {
                WHAMMY_PER_BEAT
            } else {
                0.0
            };
            if drain {
                next = next.min(timing.next_sig(t));
                rate -= 1.0 / (MEASURES_PER_BAR * timing.beats_per_measure(t));
            }
            if next.is_infinite() {
                // Nothing else happens: drain to empty (or stay put).
                return if rate < 0.0 {
                    Run::Empty(t + bar / -rate)
                } else {
                    Run::At(bar)
                };
            }
            let dt = next - t;
            if rate < 0.0 && bar + rate * dt <= 0.0 {
                return Run::Empty(t + bar / -rate);
            }
            if rate > 0.0 && bar + rate * dt >= target {
                return Run::Stopped;
            }
            bar = (bar + rate * dt).min(1.0);
            t = next;
        }
    }

    /// All the whammy there is up to beat `t` (counting a burst exactly at `t` if `at_t`).
    /// Outside activations the bar only ever fills, so this is all a bar total needs.
    fn whammy_upto(&self, t: f64, at_t: bool) -> f64 {
        let w = &self.whammy;
        let i = w.partition_point(|s| s.to < t);
        let mut total = self.whammy_sum[i];
        if let Some(s) = w.get(i) {
            if s.from < t {
                total += (t.min(s.to) - s.from) * WHAMMY_PER_BEAT;
            }
            if at_t && s.to == t {
                total += s.burst;
            }
        }
        total
    }

    /// Whammy available from `from` (inclusive) to `until`.
    fn whammy_between(&self, from: f64, until: f64) -> f64 {
        (self.whammy_upto(until, true) - self.whammy_upto(from, false)).max(0.0)
    }

    /// The first beat by which whammy since the start of the song totals `target`.
    fn whammy_reaches(&self, target: f64) -> Option<f64> {
        let w = &self.whammy;
        let i = self.whammy_sum[1..].partition_point(|&s| s < target);
        let s = w.get(i)?;
        let steady = self.whammy_sum[i] + (s.to - s.from) * WHAMMY_PER_BEAT;
        Some(if target <= steady {
            s.from + (target - self.whammy_sum[i]).max(0.0) / WHAMMY_PER_BEAT
        } else {
            s.to
        })
    }

    /// Whammy in [from, until) that comes only from sustains on notes at or after beat `first`
    /// (none of an earlier note's sustains covering it). Before an activation starting at
    /// `first`, its own sustain and later ones can't count yet.
    fn whammy_from_later_notes(&self, first: f64, from: f64, until: f64) -> f64 {
        let w = &self.note_whammy;
        let i = w.partition_point(|n| n.0 < first);
        let Some(&(_, start, _)) = w.get(i) else {
            return 0.0;
        };
        let covered = if i > 0 {
            self.note_whammy_reach[i - 1]
        } else {
            f64::NEG_INFINITY
        };
        (until - start.max(from).max(covered)).max(0.0) * WHAMMY_PER_BEAT
    }

    /// The best case for how long an activation started at `start` with `bar` lasts: every bit
    /// of whammy, with each stretch's last quarter beat held back until the bar needs it, and
    /// phrases inside counted as late as possible, or early if the bar would run out first.
    /// `phrases` are (earliest, latest) hit times.
    fn best_case_end(
        &self,
        timing: &Timing,
        start: f64,
        mut bar: f64,
        phrases: &[(f64, f64)],
    ) -> f64 {
        STATS[1].fetch_add(1, Relaxed);
        let w = &self.whammy;
        let end_of = |s: &Whammy| s.to + s.burst / WHAMMY_PER_BEAT;
        let mut t = start;
        let mut iw = w.partition_point(|s| end_of(s) <= t);
        let mut ip = 0;
        // The held-back end of the current stretch, once it's been reached.
        let mut bank = 0.0;
        let mut banked_for = usize::MAX;
        loop {
            while ip < phrases.len() && phrases[ip].1 <= t {
                bar = (bar + PHRASE).min(1.0);
                ip += 1;
            }
            if let Some(s) = w.get(iw) {
                if s.to <= t && banked_for != iw {
                    bank = s.burst;
                    banked_for = iw;
                }
                if end_of(s) <= t {
                    bar = (bar + bank).min(1.0);
                    bank = 0.0;
                    iw += 1;
                    continue;
                }
            }
            let in_whammy = w.get(iw).is_some_and(|s| s.from <= t && t < s.to);
            let mut next = timing.next_sig(t);
            if let Some(p) = phrases.get(ip) {
                next = next.min(p.1);
            }
            if let Some(s) = w.get(iw) {
                next = next.min(if t < s.from {
                    s.from
                } else if t < s.to {
                    s.to
                } else {
                    end_of(s)
                });
            }
            let rate = if in_whammy { WHAMMY_PER_BEAT } else { 0.0 }
                - 1.0 / (MEASURES_PER_BAR * timing.beats_per_measure(t));
            if rate < 0.0 && (next.is_infinite() || bar + rate * (next - t) < 0.0) {
                let empty = t + bar / -rate;
                // Out of bar: spend the held-back whammy, or hit the next phrase early.
                if bank > 0.0 {
                    bar = bank.min(1.0);
                    bank = 0.0;
                } else if ip < phrases.len() && phrases[ip].0 <= empty {
                    bar = PHRASE;
                    ip += 1;
                } else {
                    return empty;
                }
                t = empty;
                continue;
            }
            if next.is_infinite() {
                return f64::INFINITY;
            }
            bar = (bar + rate * (next - t)).min(1.0);
            t = next;
        }
    }

    /// When an activation started at `start` with `bar` runs out.
    fn activation_end(
        &self,
        timing: &Timing,
        start: f64,
        bar: f64,
        whammy: bool,
        phrases: &[f64],
    ) -> f64 {
        STATS[1].fetch_add(1, Relaxed);
        match self.run(
            timing,
            start,
            bar,
            true,
            whammy,
            phrases,
            f64::INFINITY,
            f64::INFINITY,
        ) {
            Run::Empty(t) => t,
            _ => f64::INFINITY,
        }
    }
}

// ─────────────────────────────── Search ───────────────────────────────

/// A point in the song between activations: the next point that can be scored, the beat from
/// which star power counts again, and whether the bar is known to be full.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct State {
    next: usize,
    resume_bits: u64,
    full: bool,
}

#[derive(Clone, Copy)]
struct Edge {
    first: usize,
    last: usize,
    to: State,
}

struct Search<'a> {
    song: &'a Scored,
    timing: &'a Timing,
    /// Running totals of point values and of phrase ends, for quick sums over ranges.
    value_sum: Vec<u64>,
    phrase_sum: Vec<u32>,
    /// Indices of phrase-ending points, and their beats and earliest and latest hit times.
    phrase_points: Vec<usize>,
    phrase_beat: Vec<f64>,
    phrase_early: Vec<f64>,
    phrase_windows: Vec<(f64, f64)>,
    /// For each point, the first point at or after it that can change the bar after an
    /// activation: a phrase's last note, or a sustain point while whammy is possible.
    next_relevant: Vec<usize>,
    memo: HashMap<State, (u64, Option<Edge>)>,
    /// Progress: states finish from the end of the song backwards, so the earliest finished one
    /// says roughly how much is done.
    progress: &'a (dyn Fn(f32) + Sync),
    earliest_done: usize,
}

impl<'a> Search<'a> {
    fn new(song: &'a Scored, timing: &'a Timing, progress: &'a (dyn Fn(f32) + Sync)) -> Self {
        let mut value_sum = vec![0];
        let mut phrase_sum = vec![0];
        for p in &song.points {
            value_sum.push(value_sum.last().unwrap() + p.value);
            phrase_sum.push(phrase_sum.last().unwrap() + p.phrase_end as u32);
        }
        let phrase_points: Vec<usize> = (0..song.points.len())
            .filter(|&i| song.points[i].phrase_end)
            .collect();
        let at = |f: fn(&Point) -> f64| {
            phrase_points
                .iter()
                .map(|&i| f(&song.points[i]))
                .collect::<Vec<f64>>()
        };
        let (phrase_beat, phrase_early, phrase_late) =
            (at(|p| p.beat), at(|p| p.early), at(|p| p.late));
        let phrase_windows = phrase_early
            .iter()
            .copied()
            .zip(phrase_late.iter().copied())
            .collect();
        let n = song.points.len();
        let mut next_relevant = vec![n; n + 1];
        for i in (0..n).rev() {
            let p = &song.points[i];
            let w = &song.whammy;
            let k = w.partition_point(|s| s.to + s.burst / WHAMMY_PER_BEAT < p.beat);
            let in_whammy = w.get(k).is_some_and(|s| s.from <= p.beat);
            next_relevant[i] = if p.phrase_end || (!p.note && in_whammy) {
                i
            } else {
                next_relevant[i + 1]
            };
        }
        Self {
            song,
            timing,
            value_sum,
            phrase_sum,
            phrase_points,
            phrase_beat,
            phrase_early,
            phrase_windows,
            next_relevant,
            memo: HashMap::new(),
            progress,
            earliest_done: usize::MAX,
        }
    }

    fn run(mut self) -> Outcome {
        let pts = &self.song.points;
        let first = pts[0]
            .early
            .min(self.song.whammy.first().map_or(f64::INFINITY, |w| w.from))
            - 1.0;
        let root = State {
            next: 0,
            resume_bits: first.to_bits(),
            full: false,
        };
        let best = self.solve(root);

        // Follow the best edges to describe the path.
        let mut activations = Vec::new();
        let mut ranges = Vec::new();
        let mut state = root;
        let mut tokens = Vec::new();
        let mut from = 0;
        while let Some(&(_, Some(edge))) = self.memo.get(&state) {
            let before = self.phrase_sum[edge.first] - self.phrase_sum[from];
            let during = self.phrase_sum[edge.last + 1] - self.phrase_sum[edge.first];
            tokens.push(if during > 0 {
                format!("{before}(+{during})")
            } else {
                before.to_string()
            });
            let (a, b) = (&pts[edge.first], &pts[edge.last]);
            activations.push(Activation {
                phrases_before: before,
                phrases_during: during,
                first_measure: self.timing.measure(a.beat),
                last_measure: self.timing.measure(b.beat),
                first_secs: self.timing.secs(a.beat),
                last_secs: self.timing.secs(b.beat),
                bonus: self.value_sum[edge.last + 1] - self.value_sum[edge.first],
                first_beat: a.beat,
                last_beat: b.beat,
            });
            ranges.push((edge.first, edge.last));
            from = edge.last + 1;
            state = edge.to;
        }
        let unused_phrases = self.phrase_sum[pts.len()] - self.phrase_sum[from];
        let mut path = tokens.join("-");
        if unused_phrases > 0 && !activations.is_empty() {
            path.push_str(&format!("-ES{unused_phrases}"));
        }
        if activations.is_empty() {
            path = "None".to_owned();
        }
        let points = pts
            .iter()
            .enumerate()
            .map(|(i, p)| ScorePoint {
                beat: p.beat,
                base: p.base,
                value: p.value,
                extra: p.extra,
                activation: ranges.iter().position(|&(a, b)| (a..=b).contains(&i)),
            })
            .collect();
        let measure_bars = self.measure_bars(&activations);
        Outcome {
            no_sp: self.song.no_sp,
            total: self.song.no_sp + best,
            path,
            activations,
            points,
            measure_bars,
        }
    }

    /// The bar at the end of each measure, playing the path: phrases and all whammy fill it,
    /// and each activation starts at its first point and drains it until empty.
    fn measure_bars(&self, activations: &[Activation]) -> Vec<(f64, f64)> {
        let (song, timing) = (self.song, self.timing);
        let phrases = &self.phrase_beat;
        // Phrases after 	 and up to until.
        let between = |t: f64, until: f64| {
            &phrases[phrases.partition_point(|&p| p <= t)..phrases.partition_point(|&p| p <= until)]
        };
        let mut out = Vec::new();
        let (mut t, mut bar) = (song.points[0].beat.min(0.0), 0.0);
        let mut next = 0; // the next activation
        let mut active = false;
        for end in timing.measure_ends(song.end) {
            loop {
                if !active && next < activations.len() && activations[next].first_beat <= end {
                    let start = activations[next].first_beat;
                    if let Run::At(b) = song.run(timing, t, bar, false, true, between(t, start), start, f64::INFINITY) {
                        bar = b;
                    }
                    (t, active) = (start, true);
                    continue;
                }
                let run = song.run(timing, t, bar, active, true, between(t, end), end, f64::INFINITY);
                match run {
                    Run::Empty(at) if active && at <= end => {
                        (t, bar, active) = (at, 0.0, false);
                        next += 1;
                    }
                    Run::At(b) => {
                        (t, bar) = (end, b.max(0.0));
                        break;
                    }
                    _ => {
                        t = end;
                        break;
                    }
                }
            }
            out.push((end, bar));
        }
        out
    }

    /// Moves a resume time `t` (an activation's end) later while that changes nothing, so
    /// activations ending at slightly different times share a state. Outside a whammy stretch,
    /// nothing fills the bar until the next stretch starts or the next point (index `next`) is
    /// hit, so any time up to the earlier of those is equivalent.
    fn settle(&self, t: f64, next: usize) -> f64 {
        let w = &self.song.whammy;
        let i = w.partition_point(|s| s.to < t);
        if i < w.len() && w[i].from <= t {
            return t; // mid-whammy: the exact time matters
        }
        let gap_end = w.get(i).map_or(f64::INFINITY, |s| s.from);
        let next_early = self
            .song
            .points
            .get(next)
            .map_or(f64::INFINITY, |p| p.early);
        t.max(gap_end.min(next_early))
    }

    /// The index (in phrase order) of the first phrase ending at or after point `i`.
    fn phrase_index(&self, i: usize) -> usize {
        self.phrase_points.partition_point(|&p| p < i)
    }

    /// When the bar first reaches half from `resume`, with whammy and the phrases `from..to`
    /// (in phrase order) completing at their beats.
    fn half_bar_at(&self, resume: f64, from: usize, to: usize) -> Option<f64> {
        let song = self.song;
        let base = song.whammy_upto(resume, false);
        let whammy_time = |need: f64| song.whammy_reaches(base + need).map(|t| t.max(resume));
        // Only used with fewer than two phrases (two make half a bar on their own). With one:: either whammy alone gets there before it, or it's half whammy after.
        match to - from {
            0 => whammy_time(0.5),
            _ => {
                let phrase = self.phrase_beat[from].max(resume);
                match whammy_time(0.5) {
                    Some(t) if t <= phrase => Some(t),
                    _ => whammy_time(0.25).map(|t| t.max(phrase)),
                }
            }
        }
    }

    /// The most extra points reachable from `state`, remembering the best first activation.
    fn solve(&mut self, state: State) -> u64 {
        if let Some(&(v, _)) = self.memo.get(&state) {
            return v;
        }
        let (song, timing) = (self.song, self.timing);
        let pts = &song.points;
        let n = pts.len();
        let resume = f64::from_bits(state.resume_bits);
        let mut best = (0u64, None);
        // For each end point, only the earliest start matters: it scores more and ends sooner.
        // `skip[i]` leads to the first end at or after `i` not yet handled (with path shortening).
        let mut skip: Vec<usize> = (0..=n).collect();
        fn find(skip: &mut [usize], mut i: usize) -> usize {
            let mut root = i;
            while skip[root] != root {
                root = skip[root];
            }
            while skip[i] != root {
                let next = skip[i];
                skip[i] = root;
                i = next;
            }
            root
        }

        let mut cursor = state.next;
        while cursor < n {
            let first = cursor;
            cursor += 1;
            let collected = self.phrase_sum[first] - self.phrase_sum[state.next];
            if !state.full && collected >= 4 {
                // The bar is certainly full here; any later start is covered from that state.
                // When the last activation ended no longer matters (every start from here is
                // after these phrases), so all such states are one.
                let to = State {
                    next: first,
                    resume_bits: f64::NEG_INFINITY.to_bits(),
                    full: true,
                };
                let v = self.solve(to);
                if v > best.0 {
                    best = (v, None);
                    // A zero-weight hop: remember it by pointing at the child's best edge below.
                    best.1 = self.memo.get(&to).and_then(|m| m.1);
                }
                break;
            }
            STATS[0].fetch_add(1, Relaxed);
            let latest = pts[first].late;
            let phrase_bar = if state.full {
                1.0
            } else {
                (collected as f64 * PHRASE).min(1.0)
            };
            // Before an activation the bar only fills, so the most it can hold is a plain sum
            // (whammy counts up to the first point itself, not its late edge).
            let upper = if state.full {
                1.0
            } else {
                (phrase_bar + song.whammy_between(resume, pts[first].beat)).min(1.0)
            };
            if upper < 0.5 {
                // Jump to where whammy could first make half a bar, or to just after the next
                // phrase, whichever comes first.
                let from = self.phrase_index(state.next) + collected as usize;
                let after_phrase = self.phrase_points.get(from).map_or(n, |&i| i + 1);
                let target = self
                    .half_bar_at(resume, from - collected as usize, from)
                    .map_or(n, |t| {
                        let t = timing.beats(timing.secs(t) - WINDOW_SECS * song.squeeze) - 1e-9;
                        pts.partition_point(|p| p.beat < t)
                    });
                cursor = cursor.max(target.min(after_phrase));
                continue;
            }
            // The earliest it can start: once the previous point is hit (as early as possible),
            // and once the bar is half full.
            let prev_early = if first > 0 {
                pts[first - 1].early
            } else {
                f64::NEG_INFINITY
            };
            let (least, earliest) = if phrase_bar >= 0.5 {
                (phrase_bar, prev_early)
            } else {
                let from = self.phrase_index(state.next);
                match self.half_bar_at(resume, from, from + collected as usize) {
                    Some(t) if t <= latest => (0.5, t.max(prev_early)),
                    _ => continue,
                }
            };
            if earliest > latest {
                continue;
            }
            // The most it can have: phrases, whammy until it could start (not from this point's
            // own sustain or later ones), then any whammy up to the point itself.
            let most = if state.full {
                1.0
            } else {
                let before = (song.whammy_between(resume, earliest)
                    - song.whammy_from_later_notes(pts[first].beat, resume, earliest))
                .max(0.0);
                let bar = (phrase_bar + before).min(1.0);
                (bar + song.whammy_between(earliest.max(resume), pts[first].beat)).min(1.0)
            };
            if most < 0.5 {
                continue;
            }

            // Best case: start as late as possible with the most bar and every bit of whammy.
            // Worst case: start as early as possible with the least bar, no whammy, and phrases
            // inside hit early.
            let k = self.phrase_index(first);
            let mut included = 0; // phrase ends from `first` included in the worst case
            let mut worst = song.activation_end(timing, earliest, least, false, &[]);
            // Ends before the worst-case end can't be the last point (the activation would swallow
            // the next one), so start looking just before it.
            let window = WINDOW_SECS * song.squeeze;
            let lo = pts
                .partition_point(|p| p.beat < timing.beats(timing.secs(worst) - window) - 1e-9)
                .max(first + 1)
                - 1;
            let mut last = find(&mut skip, lo.max(first));
            if last >= n {
                continue;
            }
            let reach = song.best_case_end(timing, latest, most, &self.phrase_windows[k..]);
            while last < n {
                let p = &pts[last];
                // The bar may be exactly empty as the last point is hit.
                if p.early > reach {
                    if p.note {
                        break;
                    }
                    last = find(&mut skip, last + 1);
                    continue;
                }
                let need = (self.phrase_sum[last + 1] - self.phrase_sum[first]) as usize;
                if need > included {
                    included = need;
                    worst = song.activation_end(
                        timing,
                        earliest,
                        least,
                        false,
                        &self.phrase_early[k..k + included],
                    );
                }
                STATS[2].fetch_add(1, Relaxed);
                let ends_in_time = last + 1 == n || worst < pts[last + 1].late;
                if ends_in_time {
                    skip[last] = last + 1;
                    let gain = self.value_sum[last + 1] - self.value_sum[first];
                    // Carry on from the next point that can change the bar; whammy counts again
                    // from the worst-case end, or the full-window early edge of the point before
                    // that one if later.
                    let next = self.next_relevant[last + 1];
                    let ended = if next < n {
                        worst.max(pts[next - 1].full_early)
                    } else {
                        worst
                    };
                    let resume = self.settle(ended, next);
                    let to = State {
                        next,
                        resume_bits: resume.to_bits(),
                        full: false,
                    };
                    let v = gain + if last + 1 < n { self.solve(to) } else { 0 };
                    if v > best.0 {
                        best = (v, Some(Edge { first, last, to }));
                    }
                }
                last = find(&mut skip, last + 1);
            }
        }
        self.memo.insert(state, best);
        if state.next < self.earliest_done {
            self.earliest_done = state.next;
            (self.progress)(1.0 - state.next as f32 / n.max(1) as f32);
        }
        if cfg!(test) && self.memo.len() % 2000 == 0 {
            eprintln!("  {} states", self.memo.len());
        }
        best.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compares against CHOpt 1.17.0's results on real charts. Point CHART_FIXTURES at a folder of
    /// chart folders (song.ini + notes.chart/notes.mid) named as below, then run:
    /// cargo test --release optimal_matches_reference -- --ignored --nocapture
    #[test]
    #[ignore]
    fn optimal_matches_reference() {
        let dir =
            std::path::PathBuf::from(std::env::var("CHART_FIXTURES").expect("set CHART_FIXTURES"));
        // (folder, instrument, squeeze/early whammy, no-SP score, total)
        let cases: &[(&str, u8, f64, u64, u64)] = &[
            ("ttfaf-gh3", 0, 1.0, 826401, 1091597),
            ("ttfaf-gh3", 0, 0.0, 826401, 1023665),
            ("soulless4", 0, 1.0, 2097224, 2842776),
            ("soulless4", 0, 0.0, 2097224, 2825684),
            ("soulless4", 3, 1.0, 1998389, 2571513),
            ("soulless4", 3, 0.0, 1998389, 2557537),
            ("hotel-california", 0, 1.0, 342771, 432987),
            ("hotel-california", 0, 0.0, 342771, 430051),
            ("freebird-eurobeat", 0, 1.0, 617108, 738524),
            ("freebird-eurobeat", 0, 0.0, 617108, 735428),
            ("sweet-child-gh2", 0, 1.0, 246062, 308894),
            ("sweet-child-gh2", 0, 0.0, 246062, 307910),
            ("sweet-child-gh2", 3, 1.0, 182472, 201672),
            ("sweet-child-gh2", 3, 0.0, 182472, 201136),
            ("everlong-rb2", 0, 1.0, 390326, 506350),
            ("everlong-rb2", 0, 0.0, 390326, 504238),
            ("everlong-rb2", 3, 1.0, 203942, 250726),
            ("everlong-rb2", 3, 0.0, 203942, 250322),
        ];
        let only = std::env::var("CHART_ONLY").ok();
        let mut ok = 0;
        for &(folder, inst, sq, no_sp, total) in cases {
            if only.as_deref().is_some_and(|o| !folder.contains(o)) {
                continue;
            }
            let path = dir.join(folder);
            let ini = std::fs::read_to_string(path.join("song.ini")).unwrap_or_default();
            let cutoff = ini
                .lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == "sustain_cutoff_threshold")
                .and_then(|(_, v)| v.trim().parse().ok());
            let chart = match std::fs::read(path.join("notes.mid")) {
                Ok(mid) => crate::chart::read_midi_chart(&mid).unwrap(),
                Err(_) => crate::chart::read_chart(
                    &std::fs::read_to_string(path.join("notes.chart")).unwrap(),
                ),
            };
            let started = web_time::Instant::now();
            let out = optimise(
                &chart,
                inst,
                0,
                cutoff,
                Options {
                    squeeze: sq,
                    early_whammy: sq,
                },
            )
            .unwrap();
            let good = out.no_sp == no_sp && out.total == total;
            println!(
                "     points {}, starts {}, sims {}, ends {}",
                0,
                STATS[0].swap(0, Relaxed),
                STATS[1].swap(0, Relaxed),
                STATS[2].swap(0, Relaxed)
            );
            ok += good as u32;
            println!(
                "{} {folder:<18} inst {inst} sq {sq}: no-SP {} (want {no_sp}), total {} (want {total}, off by {}), path {}, {:.2}s",
                if good { "OK  " } else { "FAIL" },
                out.no_sp,
                out.total,
                out.total as i64 - total as i64,
                out.path,
                started.elapsed().as_secs_f64()
            );
            if !good {
                for a in &out.activations {
                    println!(
                        "     {}(+{}): ticks {:.1} to {:.1}, +{}",
                        a.phrases_before,
                        a.phrases_during,
                        a.first_measure,
                        a.last_measure,
                        a.bonus
                    );
                }
            }
        }
        println!("{ok}/{} match", cases.len());
    }
}
