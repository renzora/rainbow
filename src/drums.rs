//! The best possible Clone Hero score on drums. Drums score differently from guitar: every gem
//! (each drum hit) scores and counts for the combo on its own, cymbals and accents/ghosts are
//! worth more, there's no whammy, each gem has its own hit window, and star power can only be
//! activated at the end of a drum fill, which only shows up a while after the bar reaches half.
//! Results are meant to match CHOpt's defaults (pro drums, 2x kick).
//!
//! Units as in `optimal.rs`: positions in beats, the bar from 0.0 to 1.0.

use std::collections::HashMap;

use crate::chart::{DrumTrack, Parsed};
use crate::optimal::{Activation, MEASURES_PER_BAR, Options, Outcome, PHRASE, ScorePoint, Timing};

/// A fill only shows up this long after the second phrase that half fills the bar is hit.
const APPEAR_SECS: f64 = 2.0;
/// Charts without fills get one at a measure line with a gem this close to it.
const AUTO_FILL_SECS: f64 = 0.25;

/// How the chart's lanes are read.
#[derive(Clone, Copy, PartialEq)]
enum Kit {
    /// Four lanes; yellow, blue and green can be cymbals.
    Pro,
    /// Five lanes, scored as pro drums.
    FiveLane,
    /// Four lanes from a .mid without tom markers: no cymbals.
    Plain,
}

/// Lanes in the order gems at the same tick are sorted.
const RED: u8 = 0;
const YELLOW: u8 = 1;
const BLUE: u8 = 2;
const GREEN: u8 = 3;
const KICK: u8 = 4;
const DOUBLE_KICK: u8 = 5;

#[derive(Clone, Copy)]
struct Gem {
    tick: u32,
    lane: u8,
    cymbal: bool,
    /// Accent or ghost note: worth double.
    dynamic: bool,
}

#[derive(Clone, Copy)]
struct Point {
    beat: f64,
    /// Earliest and latest hit, in beats and in seconds.
    early: f64,
    late: f64,
    early_secs: f64,
    /// Points with the combo multiplier, and before it.
    value: u64,
    base: u64,
    /// Solo bonus.
    extra: u64,
    /// The last gem of a star power phrase.
    phrase_end: bool,
    /// When the fill this gem activates star power for starts (seconds), if it's one.
    fill: Option<f64>,
}

fn in_range(ranges: &[(u32, u32)], tick: u32) -> bool {
    ranges.iter().any(|&(s, e)| tick >= s && tick < e.max(s + 1))
}

/// Works out the optimal score for one difficulty of the drums. None if there are no notes.
pub fn optimise(chart: &Parsed, diff: u8, opts: Options) -> Option<Outcome> {
    let track = chart.drum_track(diff)?;
    let timing = Timing::new(chart);
    let song = Song::new(chart, &track, &timing, opts);
    if song.points.is_empty() {
        return None;
    }
    Some(Search::new(&song, &timing).run())
}

/// Which kind of kit the chart is for, from song.ini and then the chart itself.
fn kit(chart: &Parsed, track: &DrumTrack) -> Kit {
    if chart.pro_drums == Some(true) {
        Kit::Pro
    } else if chart.five_lane_drums == Some(true) {
        Kit::FiveLane
    } else if track.midi {
        if track.toms.iter().any(|t| !t.is_empty()) {
            Kit::Pro
        } else if track.gems.iter().any(|g| g.1 == 5) {
            Kit::FiveLane
        } else {
            Kit::Plain
        }
    } else if !track.cymbals.is_empty() {
        Kit::Pro
    } else if track.gems.iter().any(|g| g.1 == 5) {
        Kit::FiveLane
    } else {
        Kit::Pro
    }
}

/// The gems as scored, sorted, before flams are added.
fn gems(chart: &Parsed, track: &DrumTrack) -> Vec<Gem> {
    let kit = kit(chart, track);
    let mut out: Vec<Gem> = Vec::with_capacity(track.gems.len());
    for &(tick, raw, vel) in &track.gems {
        let dynamic = if track.midi {
            track.dynamics && (vel == 1 || vel >= 127)
        } else {
            (1..=4).contains(&raw)
                && kit != Kit::Plain
                && (track.accents.contains(&(tick, raw)) || track.ghosts.contains(&(tick, raw)))
        };
        let (lane, cymbal) = match (kit, raw) {
            (_, 0) => (KICK, false),
            (_, 100) => (DOUBLE_KICK, false),
            (Kit::FiveLane, 1) => (RED, false),
            (Kit::FiveLane, 2) => (YELLOW, true),
            (Kit::FiveLane, 3) => (BLUE, false),
            (Kit::FiveLane, 4) => (GREEN, true),
            // The fifth lane's green is a tom; with an orange (green cymbal) beside it, blue.
            (Kit::FiveLane, _) => {
                let orange = track.gems.iter().any(|g| g.0 == tick && g.1 == 4);
                (if orange { BLUE } else { GREEN }, false)
            }
            (Kit::Plain, l) => (l.min(4) - 1, false),
            (Kit::Pro, l) => {
                let lane = l.min(4) - 1;
                let cymbal = lane != RED
                    && if track.midi {
                        !in_range(&track.toms[lane as usize - 1], tick)
                    } else {
                        track
                            .cymbals
                            .iter()
                            .any(|&(at, len, pad)| pad == l.min(4) && tick >= at && tick <= at + len)
                    };
                (lane, cymbal)
            }
        };
        out.push(Gem { tick, lane, cymbal, dynamic });
    }
    // Disco flip: red and yellow swap, red playing as a yellow cymbal and yellow as a red tom.
    if kit == Kit::Pro {
        for g in &mut out {
            if in_range(&track.flips, g.tick) {
                match g.lane {
                    RED => (g.lane, g.cymbal) = (YELLOW, true),
                    YELLOW => (g.lane, g.cymbal) = (RED, false),
                    _ => {}
                }
            }
        }
    }
    out.sort_by_key(|g| (g.tick, g.lane));
    out.dedup_by_key(|g| (g.tick, g.lane));
    out
}

/// The gems as the path chart draws them: (tick, pad lane 0..=3 red to green or None for a
/// kick, cymbal), with five-lane charts shown as pro drums the way they score.
pub(crate) fn drawn(chart: &Parsed, track: &DrumTrack) -> Vec<(u32, Option<u8>, bool)> {
    gems(chart, track)
        .iter()
        .map(|g| (g.tick, (g.lane < KICK).then_some(g.lane), g.cymbal))
        .collect()
}

/// A flam (one pad hit marked as a flam) is played as two hits: the pad and the one beside it.
fn add_flams(gems: &mut Vec<Gem>, flams: &[(u32, u32)]) {
    if flams.is_empty() {
        return;
    }
    let mut extra = Vec::new();
    let mut i = 0;
    while i < gems.len() {
        let tick = gems[i].tick;
        let j = i + gems[i..].iter().take_while(|g| g.tick == tick).count();
        let pads: Vec<&Gem> = gems[i..j].iter().filter(|g| g.lane < KICK).collect();
        if pads.len() == 1 && in_range(flams, tick) {
            let lane = match pads[0].lane {
                RED => YELLOW,
                YELLOW => BLUE,
                BLUE => GREEN,
                _ => BLUE,
            };
            extra.push(Gem { lane, ..*pads[0] });
        }
        i = j;
    }
    gems.extend(extra);
    gems.sort_by_key(|g| (g.tick, g.lane));
}

/// Fills for a chart that has none: at measure lines with a gem near them, at least four
/// measures apart, each from halfway through the measure before (in seconds) to the line.
fn auto_fills(gems: &[Gem], timing: &Timing, res: f64) -> Vec<(u32, u32)> {
    let secs: Vec<f64> = gems.iter().map(|g| timing.secs(g.tick as f64 / res)).collect();
    let Some(&last) = secs.last() else { return Vec::new() };
    let stop = timing.measure(timing.beats(last + AUTO_FILL_SECS)) - 1.0;
    let mut fills = Vec::new();
    let mut m = 1.0;
    while m <= stop + 1e-9 {
        let beat = timing.measure_beat(m);
        let at = timing.secs(beat);
        let i = secs.partition_point(|&t| t < at - AUTO_FILL_SECS - 1e-9);
        if secs.get(i).is_some_and(|&t| t <= at + AUTO_FILL_SECS + 1e-9) {
            let mid = (timing.secs(timing.measure_beat(m - 1.0)) + at) / 2.0;
            let start = (timing.beats(mid) * res + 1e-6) as u32;
            fills.push((start, (beat * res).round() as u32));
            m += 4.0;
        } else {
            m += 1.0;
        }
    }
    fills
}

/// Clone Hero's drum hit window (either side) from the gaps to the gems before and after.
fn window(prev_gap: f64, next_gap: f64) -> f64 {
    let g = prev_gap.clamp(0.0375, 0.085) + next_gap.clamp(0.0375, 0.085);
    -2.23425815 * g * g + 0.9428571 * g - 0.01
}

struct Song {
    points: Vec<Point>,
    no_sp: u64,
}

impl Song {
    fn new(chart: &Parsed, track: &DrumTrack, timing: &Timing, opts: Options) -> Self {
        let res = chart.res as f64;
        let mut gems = gems(chart, track);
        // Solo bonuses count the chart's own gems, not the second hits of flams.
        let solo_gems: Vec<u32> = gems.iter().filter(|g| in_range(&track.solos, g.tick)).map(|g| g.tick).collect();
        let fills = if track.fills.is_empty() {
            auto_fills(&gems, timing, res)
        } else {
            track.fills.clone()
        };
        if track.midi {
            add_flams(&mut gems, &track.flams);
        }
        let squeeze = opts.squeeze.max(0.001);
        let secs: Vec<f64> = gems.iter().map(|g| timing.secs(g.tick as f64 / res)).collect();
        let mut points = Vec::with_capacity(gems.len());
        let mut combo = 0u64;
        let mut no_sp = 0;
        let mut solo_left = solo_gems;
        solo_left.reverse();
        for (i, g) in gems.iter().enumerate() {
            let prev = if i > 0 { secs[i] - secs[i - 1] } else { f64::INFINITY };
            let next = secs.get(i + 1).map_or(f64::INFINITY, |s| s - secs[i]);
            let w = window(prev, next) * squeeze;
            combo += 1;
            let base = if g.cymbal { 65 } else { 50 } * if g.dynamic { 2 } else { 1 };
            let value = base * (combo / 10 + 1).min(4);
            // Each of the chart's own gems in a solo is worth 100 (flams' second hits aren't).
            let extra = if solo_left.last() == Some(&g.tick) {
                solo_left.pop();
                100
            } else {
                0
            };
            no_sp += value + extra;
            let beat = g.tick as f64 / res;
            points.push(Point {
                beat,
                early: timing.beats(secs[i] - w),
                late: timing.beats(secs[i] + w),
                early_secs: secs[i] - w,
                value,
                base,
                extra,
                phrase_end: false,
                fill: None,
            });
        }
        // A phrase is complete on its last gem (the last one sorted at its last tick).
        for &(s, e) in &track.star {
            if let Some(i) = gems.iter().rposition(|g| g.tick >= s && g.tick < e.max(s + 1)) {
                points[i].phrase_end = true;
            }
        }
        // Each fill activates on the gem nearest its end (the first gem at each tick; on a tie
        // the later one). Where fills share a gem, the last one counts.
        let firsts: Vec<usize> = (0..gems.len()).filter(|&i| i == 0 || gems[i].tick != gems[i - 1].tick).collect();
        for &(start, end) in &fills {
            let at = timing.secs(end as f64 / res);
            let k = firsts.partition_point(|&i| secs[i] < at);
            let pick = match (k.checked_sub(1).map(|k| firsts[k]), firsts.get(k)) {
                (Some(a), Some(&b)) => if at - secs[a] < secs[b] - at { a } else { b },
                (Some(a), None) => a,
                (None, Some(&b)) => b,
                (None, None) => continue,
            };
            points[pick].fill = Some(timing.secs(start as f64 / res));
        }
        Song { points, no_sp }
    }
}

// ─────────────────────────────── Search ───────────────────────────────

#[derive(Clone, Copy)]
struct Edge {
    first: usize,
    last: usize,
    to: (usize, bool),
}

struct Search<'a> {
    song: &'a Song,
    timing: &'a Timing,
    value_sum: Vec<u64>,
    phrase_sum: Vec<u32>,
    phrase_points: Vec<usize>,
    /// Gems that activate a fill, in order.
    fill_points: Vec<usize>,
    memo: HashMap<(usize, bool), (u64, Option<Edge>)>,
}

impl<'a> Search<'a> {
    fn new(song: &'a Song, timing: &'a Timing) -> Self {
        let mut value_sum = vec![0];
        let mut phrase_sum = vec![0];
        for p in &song.points {
            value_sum.push(value_sum.last().unwrap() + p.value);
            phrase_sum.push(phrase_sum.last().unwrap() + p.phrase_end as u32);
        }
        let n = song.points.len();
        Self {
            song,
            timing,
            value_sum,
            phrase_sum,
            phrase_points: (0..n).filter(|&i| song.points[i].phrase_end).collect(),
            fill_points: (0..n).filter(|&i| song.points[i].fill.is_some()).collect(),
            memo: HashMap::new(),
        }
    }

    /// Runs an activation from beat `t` with `bar` until it empties or `until`; phrases are
    /// completed at the given beats. Returns (beat, bar, whether it emptied).
    fn drain(&self, mut t: f64, mut bar: f64, phrases: &[f64], until: f64) -> (f64, f64, bool) {
        let mut ip = 0;
        loop {
            while ip < phrases.len() && phrases[ip] <= t {
                bar = (bar + PHRASE).min(1.0);
                ip += 1;
            }
            if t >= until {
                return (t, bar, false);
            }
            let rate = 1.0 / (MEASURES_PER_BAR * self.timing.beats_per_measure(t));
            let next = self.timing.next_sig(t).min(phrases.get(ip).copied().unwrap_or(f64::INFINITY)).min(until);
            if bar <= rate * (next - t) {
                return (t + bar / rate, 0.0, true);
            }
            bar -= rate * (next - t);
            t = next;
        }
    }

    /// The latest an activation from `t` with `bar` can last: phrases inside hit as late as
    /// possible, or early if the bar would otherwise run out first. `phrases`: (early, late).
    fn reach(&self, mut t: f64, mut bar: f64, phrases: &[(f64, f64)]) -> f64 {
        let mut ip = 0;
        loop {
            while ip < phrases.len() && phrases[ip].1 <= t {
                bar = (bar + PHRASE).min(1.0);
                ip += 1;
            }
            let rate = 1.0 / (MEASURES_PER_BAR * self.timing.beats_per_measure(t));
            let next = self.timing.next_sig(t).min(phrases.get(ip).map_or(f64::INFINITY, |p| p.1));
            if bar < rate * (next - t) {
                let empty = t + bar / rate;
                if ip < phrases.len() && phrases[ip].0 <= empty {
                    (t, bar) = (empty, PHRASE);
                    ip += 1;
                    continue;
                }
                return empty;
            }
            bar -= rate * (next - t);
            t = next;
        }
    }

    /// The first phrase end at or after point `i` (or the end): where a state can start, as
    /// nothing before it can fill the bar.
    fn canonical(&self, i: usize) -> usize {
        let k = self.phrase_points.partition_point(|&p| p < i);
        self.phrase_points.get(k).copied().unwrap_or(self.song.points.len())
    }

    /// The earliest a fill can start for it to show up, counting phrases from point `from`: two
    /// seconds after the second phrase is hit (as early as possible).
    fn appear_after(&self, from: usize) -> Option<f64> {
        let k = self.phrase_points.partition_point(|&p| p < from);
        let p2 = *self.phrase_points.get(k + 1)?;
        Some(self.song.points[p2].early_secs + APPEAR_SECS)
    }

    /// The most extra points from a state: the next point that can matter and whether the bar
    /// is known to be full (four phrases with no activation; then every fill shows up).
    fn solve(&mut self, state: (usize, bool)) -> u64 {
        if let Some(&(v, _)) = self.memo.get(&state) {
            return v;
        }
        let pts = &self.song.points;
        let n = pts.len();
        let (next, full) = state;
        let mut best: (u64, Option<Edge>) = (0, None);
        let bound = if full { Some(f64::NEG_INFINITY) } else { self.appear_after(next) };
        // Once four phrases are in, the bar is full; later fills are all handled by that state.
        let k0 = self.phrase_points.partition_point(|&p| p < next);
        let fourth = if full { None } else { self.phrase_points.get(k0 + 3).copied() };
        if let Some(bound) = bound {
            let from = self.fill_points.partition_point(|&i| i < next);
            for f in from..self.fill_points.len() {
                let first = self.fill_points[f];
                if fourth.is_some_and(|p| first > p) {
                    let to = (fourth.unwrap() + 1, true);
                    let v = self.solve(to);
                    if v > best.0 {
                        best = (v, self.memo.get(&to).and_then(|m| m.1));
                    }
                    break;
                }
                let collected = self.phrase_sum[first] - self.phrase_sum[next];
                if !full && (collected < 2 || pts[first].fill.unwrap() < bound) {
                    continue;
                }
                let bar = if full { 1.0 } else { (collected as f64 * PHRASE).min(1.0) };
                let k = self.phrase_points.partition_point(|&p| p < first);
                let windows: Vec<(f64, f64)> = self.phrase_points[k..].iter().map(|&i| (pts[i].early, pts[i].late)).collect();
                // Best case: start as late as possible; worst: as early as possible, with
                // phrases inside hit early.
                let reach = self.reach(pts[first].late, bar, &windows);
                let mut included = 0;
                let mut worst = self.drain(pts[first].early, bar, &[], f64::INFINITY).0;
                for last in first..n {
                    let p = &pts[last];
                    if p.early > reach {
                        break;
                    }
                    let need = (self.phrase_sum[last + 1] - self.phrase_sum[first]) as usize;
                    if need > included {
                        included = need;
                        let early: Vec<f64> = windows[..need].iter().map(|w| w.0).collect();
                        worst = self.drain(pts[first].early, bar, &early, f64::INFINITY).0;
                    }
                    // The activation has to be able to end before the next gem.
                    if last + 1 < n && worst >= pts[last + 1].late {
                        continue;
                    }
                    let gain = self.value_sum[last + 1] - self.value_sum[first];
                    let to = (self.canonical(last + 1), false);
                    let v = gain + if to.0 < n { self.solve(to) } else { 0 };
                    if v > best.0 {
                        best = (v, Some(Edge { first, last, to }));
                    }
                }
            }
        }
        self.memo.insert(state, best);
        best.0
    }

    /// The path token for an activation at `first` after the previous one ended before point
    /// `from`: how many fills that show up are skipped, with CHOpt's E/L hints where whether a
    /// fill shows up depends on hitting the second phrase early or late.
    fn token(&self, from: usize, first: usize) -> String {
        let pts = &self.song.points;
        let k = self.phrase_points.partition_point(|&p| p < from);
        let Some(&p2) = self.phrase_points.get(k + 1) else {
            return "0".to_owned();
        };
        let early = pts[p2].early_secs + APPEAR_SECS;
        let late = 2.0 * self.timing.secs(pts[p2].beat) - pts[p2].early_secs + APPEAR_SECS;
        let between: Vec<usize> = self.fill_points.iter().copied().filter(|&i| i > p2 && i < first).collect();
        let skipped = between.iter().filter(|&&i| pts[i].fill.unwrap() >= early).count();
        let start = pts[first].fill.unwrap_or(0.0);
        if skipped == 0 && late > start {
            return "0(E)".to_owned();
        }
        if skipped > 0
            && let Some(&i) = self.fill_points.iter().find(|&&i| i > p2)
        {
            let s = pts[i].fill.unwrap();
            if s > early && s < late {
                return format!("{}(L)", skipped - 1);
            }
        }
        skipped.to_string()
    }

    fn run(mut self) -> Outcome {
        let pts = &self.song.points;
        let root = (self.canonical(0), false);
        let best = if root.0 < pts.len() { self.solve(root) } else { 0 };

        let mut activations = Vec::new();
        let mut ranges = Vec::new();
        let mut tokens = Vec::new();
        let mut state = root;
        let mut from = 0;
        while let Some(&(_, Some(edge))) = self.memo.get(&state) {
            tokens.push(self.token(from, edge.first));
            let before = self.phrase_sum[edge.first] - self.phrase_sum[from];
            let during = self.phrase_sum[edge.last + 1] - self.phrase_sum[edge.first];
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
        let path = if tokens.is_empty() { "None".to_owned() } else { tokens.join("-") };
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

    /// The bar at the end of each measure, playing the path.
    fn measure_bars(&self, activations: &[Activation]) -> Vec<(f64, f64)> {
        let pts = &self.song.points;
        let phrases: Vec<f64> = self.phrase_points.iter().map(|&i| pts[i].beat).collect();
        let between = |t: f64, until: f64| &phrases[phrases.partition_point(|&p| p < t)..phrases.partition_point(|&p| p <= until)];
        let mut out = Vec::new();
        let (mut t, mut bar, mut active, mut next) = (pts[0].beat.min(0.0), 0.0, false, 0);
        for end in self.timing.measure_ends(pts.last().unwrap().beat) {
            loop {
                if !active && next < activations.len() && activations[next].first_beat <= end {
                    let start = activations[next].first_beat;
                    // Phrases before the activation note fill the bar first.
                    let before = &phrases[phrases.partition_point(|&p| p < t)..phrases.partition_point(|&p| p < start)];
                    bar = (bar + before.len() as f64 * PHRASE).min(1.0);
                    (t, active) = (start, true);
                    continue;
                }
                if active {
                    let (at, b, emptied) = self.drain(t, bar, between(t, end), end);
                    if emptied && at <= end {
                        (t, bar, active) = (at, 0.0, false);
                        next += 1;
                        continue;
                    }
                    bar = b;
                } else {
                    bar = (bar + between(t, end).len() as f64 * PHRASE).min(1.0);
                }
                t = end + 1e-9;
                break;
            }
            out.push((end, bar));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    /// Checks against CHOpt's results for drum charts. Run with:
    /// DRUM_FIXTURES=<folder of chart folders> cargo test --release drums_match_reference -- --ignored --nocapture
    /// Each chart folder holds notes.chart or notes.mid, song.ini, and expected.txt with
    /// "<no-SP score> <total> <path>" for default squeeze then for no squeeze, one per line.
    #[test]
    #[ignore]
    fn drums_match_reference() {
        let Ok(root) = std::env::var("DRUM_FIXTURES") else { return };
        let mut bad = 0;
        for dir in std::fs::read_dir(root).unwrap().flatten() {
            let dir = dir.path();
            let Ok(expected) = std::fs::read_to_string(dir.join("expected.txt")) else { continue };
            let ini = std::fs::read_to_string(dir.join("song.ini")).unwrap_or_default();
            let flag = |key: &str| {
                ini.lines()
                    .filter_map(|l| l.split_once('='))
                    .find(|(k, _)| k.trim().eq_ignore_ascii_case(key))
                    .map(|(_, v)| matches!(v.trim().to_lowercase().as_str(), "true" | "1"))
            };
            let mut chart = if let Ok(text) = std::fs::read_to_string(dir.join("notes.chart")) {
                crate::chart::read_chart(&text)
            } else {
                crate::chart::read_midi_chart(&std::fs::read(dir.join("notes.mid")).unwrap()).unwrap()
            };
            chart.pro_drums = flag("pro_drums");
            chart.five_lane_drums = flag("five_lane_drums");
            for (line, squeeze) in expected.lines().zip([1.0, 0.0]) {
                let want: Vec<&str> = line.split_whitespace().collect();
                let opts = crate::optimal::Options { squeeze, early_whammy: squeeze };
                let got = super::optimise(&chart, 0, opts).unwrap();
                let ok = want == [got.no_sp.to_string().as_str(), &got.total.to_string(), &got.path];
                bad += !ok as u32;
                println!(
                    "{} {:<28} sqz {squeeze}: no-SP {} total {} path {}   want {line}",
                    if ok { "ok  " } else { "FAIL" },
                    dir.file_name().unwrap().to_string_lossy(),
                    got.no_sp,
                    got.total,
                    got.path,
                );
                for a in &got.activations {
                    println!("      act beat {:.3}..{:.3} bonus {}", a.first_beat, a.last_beat, a.bonus);
                }
            }
        }
        assert_eq!(bad, 0, "{bad} mismatches");
    }
}
