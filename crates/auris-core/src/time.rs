//! Musical and wall-clock time.
//!
//! Positions in the document model are stored in **ticks** — an integer musical grid with
//! [`TICKS_PER_QUARTER`] ticks per quarter note. Integers avoid the drift that accumulates when
//! edits round-trip through floating point, and they stay meaningful when the tempo changes.
//!
//! Rendering needs samples, so a [`TempoMap`] converts between the two. The map is a list of
//! piecewise-constant tempo segments; a project with a single fixed BPM is just a map with one
//! point, but the same code handles tempo automation added later.

use serde::{Deserialize, Serialize};
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use crate::error::{CoreError, Result};

/// Ticks per quarter note. 960 divides cleanly by 2, 3, 4, 5, 6 and 8, so triplets,
/// quintuplets and 128th notes all land on exact integers.
pub const TICKS_PER_QUARTER: i64 = 960;

/// A position or duration on the musical grid.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Ticks(pub i64);

impl Ticks {
    /// The zero position.
    pub const ZERO: Ticks = Ticks(0);

    /// One quarter note.
    pub const QUARTER: Ticks = Ticks(TICKS_PER_QUARTER);

    /// Builds a tick position from a (possibly fractional) number of quarter notes.
    pub fn from_beats(beats: f64) -> Self {
        Ticks((beats * TICKS_PER_QUARTER as f64).round() as i64)
    }

    /// Builds a tick position from a number of bars in the given time signature.
    pub fn from_bars(bars: f64, signature: TimeSignature) -> Self {
        Ticks::from_beats(bars * signature.beats_per_bar())
    }

    /// This position expressed in quarter notes.
    pub fn as_beats(self) -> f64 {
        self.0 as f64 / TICKS_PER_QUARTER as f64
    }

    /// Raw tick count.
    pub fn raw(self) -> i64 {
        self.0
    }

    /// Clamps negative positions to zero.
    pub fn max_zero(self) -> Ticks {
        Ticks(self.0.max(0))
    }

    /// Rounds down to the nearest multiple of `grid`. A non-positive grid is a no-op.
    pub fn snap_floor(self, grid: Ticks) -> Ticks {
        if grid.0 <= 0 {
            return self;
        }
        Ticks(self.0.div_euclid(grid.0) * grid.0)
    }

    /// Rounds to the nearest multiple of `grid`. A non-positive grid is a no-op.
    pub fn snap_nearest(self, grid: Ticks) -> Ticks {
        if grid.0 <= 0 {
            return self;
        }
        let below = self.snap_floor(grid);
        if self.0 - below.0 >= grid.0 / 2 {
            Ticks(below.0 + grid.0)
        } else {
            below
        }
    }
}

impl Add for Ticks {
    type Output = Ticks;
    fn add(self, rhs: Ticks) -> Ticks {
        Ticks(self.0 + rhs.0)
    }
}

impl Sub for Ticks {
    type Output = Ticks;
    fn sub(self, rhs: Ticks) -> Ticks {
        Ticks(self.0 - rhs.0)
    }
}

impl Mul<i64> for Ticks {
    type Output = Ticks;
    fn mul(self, rhs: i64) -> Ticks {
        Ticks(self.0 * rhs)
    }
}

impl Neg for Ticks {
    type Output = Ticks;
    fn neg(self) -> Ticks {
        Ticks(-self.0)
    }
}

impl AddAssign for Ticks {
    fn add_assign(&mut self, rhs: Ticks) {
        self.0 += rhs.0;
    }
}

impl SubAssign for Ticks {
    fn sub_assign(&mut self, rhs: Ticks) {
        self.0 -= rhs.0;
    }
}

/// A sample-domain position or duration.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Samples(pub u64);

impl Samples {
    /// Converts to seconds at the given sample rate.
    pub fn as_seconds(self, sample_rate: f64) -> Seconds {
        Seconds(self.0 as f64 / sample_rate)
    }

    /// Raw sample count.
    pub fn raw(self) -> u64 {
        self.0
    }
}

impl Add for Samples {
    type Output = Samples;
    fn add(self, rhs: Samples) -> Samples {
        Samples(self.0 + rhs.0)
    }
}

impl Sub for Samples {
    type Output = Samples;
    fn sub(self, rhs: Samples) -> Samples {
        Samples(self.0.saturating_sub(rhs.0))
    }
}

/// A duration in seconds.
#[derive(Copy, Clone, Debug, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seconds(pub f64);

impl Seconds {
    /// Converts to samples at the given sample rate, rounding to nearest.
    pub fn as_samples(self, sample_rate: f64) -> Samples {
        Samples((self.0 * sample_rate).round().max(0.0) as u64)
    }

    /// Formats as `mm:ss.mmm`.
    pub fn format_clock(self) -> String {
        let total = self.0.max(0.0);
        let minutes = (total / 60.0).floor() as u64;
        let seconds = total - minutes as f64 * 60.0;
        format!("{minutes:02}:{seconds:06.3}")
    }
}

/// A duration in quarter notes.
#[derive(Copy, Clone, Debug, Default, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Beats(pub f64);

impl Beats {
    /// Converts to the integer tick grid.
    pub fn as_ticks(self) -> Ticks {
        Ticks::from_beats(self.0)
    }
}

/// A musical time signature.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSignature {
    /// Beats per bar (the upper number).
    pub numerator: u32,
    /// Note value that gets one beat (the lower number): 4 = quarter, 8 = eighth.
    pub denominator: u32,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

impl TimeSignature {
    /// Creates a signature, falling back to 4/4 for degenerate input.
    pub fn new(numerator: u32, denominator: u32) -> Self {
        if numerator == 0 || denominator == 0 {
            Self::default()
        } else {
            Self {
                numerator,
                denominator,
            }
        }
    }

    /// Length of one bar in quarter notes.
    pub fn beats_per_bar(&self) -> f64 {
        self.numerator as f64 * 4.0 / self.denominator as f64
    }

    /// Length of one bar in ticks.
    pub fn ticks_per_bar(&self) -> Ticks {
        Ticks::from_beats(self.beats_per_bar())
    }

    /// Length of one notated beat in ticks.
    pub fn ticks_per_beat(&self) -> Ticks {
        Ticks::from_beats(4.0 / self.denominator as f64)
    }

    /// Where 1-based `bar` begins. Bar 1 is [`Ticks::ZERO`].
    ///
    /// Bars are a function of the signature alone — the tempo map decides how long a bar *lasts*,
    /// never where it *is*.
    pub fn bar_start(&self, bar: u32) -> Ticks {
        self.ticks_per_bar() * i64::from(bar.max(1) - 1)
    }

    /// Where 1-based `beat` of 1-based `bar` begins.
    ///
    /// The beat is fractional so that a chord landing on the second half of beat two — `2.5` — is
    /// sayable. This is the constructor that keeps meaningless positions out of the harmony
    /// timeline: nothing hands out a bare tick, so nothing can put a chord at tick 12345.
    pub fn position(&self, bar: u32, beat: f64) -> Ticks {
        let offset = (beat.max(1.0) - 1.0) * self.ticks_per_beat().raw() as f64;
        self.bar_start(bar) + Ticks(offset.round() as i64)
    }

    /// The 1-based bar containing `tick`. Anything before the timeline starts is bar 1.
    pub fn bar_of(&self, tick: Ticks) -> u32 {
        let per_bar = self.ticks_per_bar().raw().max(1);
        (tick.raw().max(0) / per_bar) as u32 + 1
    }
}

/// A tempo change at a musical position.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TempoPoint {
    /// Where the new tempo takes effect.
    pub tick: Ticks,
    /// Quarter notes per minute from this point onwards.
    pub bpm: f64,
}

/// Piecewise-constant tempo over the timeline.
///
/// Invariants, upheld by every constructor and mutator: at least one point, sorted by tick,
/// the first point sits at tick 0, and every BPM is finite and positive.
///
/// Deserialization is routed through [`TryFrom`] rather than a plain derive: a hand-edited or
/// corrupted project file would otherwise construct a map that violates those invariants, and
/// the readers all index `points[0]` or divide by a BPM without re-checking.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "TempoMapRepr")]
pub struct TempoMap {
    points: Vec<TempoPoint>,
}

/// On-disk shape of a [`TempoMap`]. It mirrors what `Serialize` emits, so files written by
/// older builds keep loading; only the validation on the way in is new.
#[derive(Deserialize)]
struct TempoMapRepr {
    points: Vec<TempoPoint>,
}

impl TryFrom<TempoMapRepr> for TempoMap {
    type Error = CoreError;

    fn try_from(repr: TempoMapRepr) -> Result<Self> {
        Self::from_points(repr.points)
    }
}

impl TryFrom<Vec<TempoPoint>> for TempoMap {
    type Error = CoreError;

    fn try_from(points: Vec<TempoPoint>) -> Result<Self> {
        Self::from_points(points)
    }
}

impl Default for TempoMap {
    fn default() -> Self {
        Self::constant(120.0)
    }
}

impl TempoMap {
    /// Lowest tempo the map accepts.
    pub const MIN_BPM: f64 = 10.0;
    /// Highest tempo the map accepts.
    pub const MAX_BPM: f64 = 999.0;

    /// A map with one tempo for the whole timeline.
    pub fn constant(bpm: f64) -> Self {
        Self {
            points: vec![TempoPoint {
                tick: Ticks::ZERO,
                bpm: Self::clamp_bpm(bpm),
            }],
        }
    }

    /// Builds a map from arbitrary points, normalising order and the tick-0 anchor.
    pub fn from_points(mut points: Vec<TempoPoint>) -> Result<Self> {
        if points.is_empty() {
            return Err(CoreError::InvalidTempoMap(
                "at least one tempo point is required".into(),
            ));
        }
        for point in &points {
            if !point.bpm.is_finite() || point.bpm <= 0.0 {
                return Err(CoreError::InvalidTempoMap(format!(
                    "tempo {} at tick {} is not a positive finite value",
                    point.bpm,
                    point.tick.raw()
                )));
            }
        }
        // Clamp positions *before* ordering. A point before tick 0 has no meaning on the
        // timeline, and clamping it later would not help: the sort would keep it in front, the
        // anchor below would overwrite only the first entry, and the rest of the list would
        // stay out of order. `segment_index` binary-searches and `ticks_to_seconds` walks
        // forwards, so an unordered list misreads silently instead of failing.
        for point in &mut points {
            point.bpm = Self::clamp_bpm(point.bpm);
            point.tick = point.tick.max_zero();
        }
        points.sort_by_key(|p| p.tick);
        // The first segment must start at zero or positions before it are undefined.
        points[0].tick = Ticks::ZERO;
        points.dedup_by_key(|p| p.tick);
        Ok(Self { points })
    }

    fn clamp_bpm(bpm: f64) -> f64 {
        if bpm.is_finite() {
            bpm.clamp(Self::MIN_BPM, Self::MAX_BPM)
        } else {
            120.0
        }
    }

    /// The tempo points, ordered by position.
    pub fn points(&self) -> &[TempoPoint] {
        &self.points
    }

    /// Tempo of the first segment — the "project BPM" for a constant map.
    pub fn initial_bpm(&self) -> f64 {
        self.points[0].bpm
    }

    /// Replaces the tempo of the first segment.
    pub fn set_initial_bpm(&mut self, bpm: f64) {
        self.points[0].bpm = Self::clamp_bpm(bpm);
    }

    /// Inserts or replaces a tempo change.
    pub fn set_point(&mut self, tick: Ticks, bpm: f64) {
        let bpm = Self::clamp_bpm(bpm);
        let tick = tick.max_zero();
        match self.points.binary_search_by_key(&tick, |p| p.tick) {
            Ok(index) => self.points[index].bpm = bpm,
            Err(index) => self.points.insert(index, TempoPoint { tick, bpm }),
        }
    }

    /// Removes the tempo change at `tick`. The anchor at tick 0 cannot be removed.
    pub fn remove_point(&mut self, tick: Ticks) {
        if tick == Ticks::ZERO {
            return;
        }
        if let Ok(index) = self.points.binary_search_by_key(&tick, |p| p.tick) {
            self.points.remove(index);
        }
    }

    /// Index of the segment containing `tick`.
    fn segment_index(&self, tick: Ticks) -> usize {
        match self.points.binary_search_by_key(&tick, |p| p.tick) {
            Ok(index) => index,
            // `tick` precedes every point only when it is negative; clamp into segment 0.
            Err(index) => index.saturating_sub(1),
        }
    }

    /// Tempo in effect at `tick`.
    pub fn bpm_at(&self, tick: Ticks) -> f64 {
        self.points[self.segment_index(tick)].bpm
    }

    /// Where the tempo change in force at `tick` sits.
    ///
    /// Total, like [`Self::bpm_at`]: the anchor at tick 0 is always in force somewhere, so a
    /// `tick` before every change answers [`Ticks::ZERO`]. This is what an editor acts
    /// *through* — "remove the tempo change here" means the one currently driving the clock,
    /// not one that happens to start under the pixel the pointer landed on.
    pub fn change_at(&self, tick: Ticks) -> Ticks {
        self.points[self.segment_index(tick)].tick
    }

    /// Seconds per tick within a segment at `bpm`.
    fn seconds_per_tick(bpm: f64) -> f64 {
        60.0 / (bpm * TICKS_PER_QUARTER as f64)
    }

    /// Wall-clock offset of `tick` from the start of the timeline.
    ///
    /// Negative ticks extrapolate backwards using the first segment's tempo.
    pub fn ticks_to_seconds(&self, tick: Ticks) -> Seconds {
        if tick.0 <= 0 {
            return Seconds(tick.0 as f64 * Self::seconds_per_tick(self.points[0].bpm));
        }
        let mut seconds = 0.0;
        for (index, point) in self.points.iter().enumerate() {
            let segment_end = self
                .points
                .get(index + 1)
                .map_or(tick, |next| next.tick.min(tick));
            if segment_end <= point.tick {
                break;
            }
            seconds += (segment_end.0 - point.tick.0) as f64 * Self::seconds_per_tick(point.bpm);
            if segment_end == tick {
                break;
            }
        }
        Seconds(seconds)
    }

    /// Inverse of [`Self::ticks_to_seconds`].
    pub fn seconds_to_ticks(&self, seconds: Seconds) -> Ticks {
        if seconds.0 <= 0.0 {
            return Ticks((seconds.0 / Self::seconds_per_tick(self.points[0].bpm)).round() as i64);
        }
        let mut elapsed = 0.0;
        for (index, point) in self.points.iter().enumerate() {
            let seconds_per_tick = Self::seconds_per_tick(point.bpm);
            match self.points.get(index + 1) {
                Some(next) => {
                    let segment_seconds = (next.tick.0 - point.tick.0) as f64 * seconds_per_tick;
                    if elapsed + segment_seconds > seconds.0 {
                        let into_segment = (seconds.0 - elapsed) / seconds_per_tick;
                        return Ticks(point.tick.0 + into_segment.round() as i64);
                    }
                    elapsed += segment_seconds;
                }
                None => {
                    let into_segment = (seconds.0 - elapsed) / seconds_per_tick;
                    return Ticks(point.tick.0 + into_segment.round() as i64);
                }
            }
        }
        Ticks::ZERO
    }

    /// Converts a musical position to a sample position.
    pub fn ticks_to_samples(&self, tick: Ticks, sample_rate: f64) -> Samples {
        self.ticks_to_seconds(tick).as_samples(sample_rate)
    }

    /// Converts a sample position to a musical position.
    pub fn samples_to_ticks(&self, samples: Samples, sample_rate: f64) -> Ticks {
        self.seconds_to_ticks(samples.as_seconds(sample_rate))
    }

    /// Bar and beat (both 1-based) plus the tick offset inside that beat.
    pub fn bar_beat_at(&self, tick: Ticks, signature: TimeSignature) -> (u32, u32, i64) {
        let ticks_per_bar = signature.ticks_per_bar().0.max(1);
        let ticks_per_beat = signature.ticks_per_beat().0.max(1);
        let position = tick.0.max(0);
        let bar = position / ticks_per_bar;
        let in_bar = position % ticks_per_bar;
        let beat = in_bar / ticks_per_beat;
        (bar as u32 + 1, beat as u32 + 1, in_bar % ticks_per_beat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_tempo_converts_both_ways() {
        let map = TempoMap::constant(120.0);
        // At 120 BPM one quarter note is exactly 0.5 s.
        let seconds = map.ticks_to_seconds(Ticks::QUARTER);
        assert!((seconds.0 - 0.5).abs() < 1e-12);
        assert_eq!(map.seconds_to_ticks(seconds), Ticks::QUARTER);
    }

    #[test]
    fn tempo_change_is_piecewise() {
        // 1 bar of 4/4 at 120 BPM (2 s), then 120 BPM doubled to 240 (1 s per bar).
        let bar = Ticks::from_beats(4.0);
        let map = TempoMap::from_points(vec![
            TempoPoint {
                tick: Ticks::ZERO,
                bpm: 120.0,
            },
            TempoPoint {
                tick: bar,
                bpm: 240.0,
            },
        ])
        .unwrap();
        assert!((map.ticks_to_seconds(bar).0 - 2.0).abs() < 1e-9);
        assert!((map.ticks_to_seconds(bar * 2).0 - 3.0).abs() < 1e-9);
        assert_eq!(map.seconds_to_ticks(Seconds(3.0)), bar * 2);
        assert_eq!(map.bpm_at(bar), 240.0);
        assert_eq!(map.bpm_at(bar - Ticks(1)), 120.0);
    }

    #[test]
    fn the_change_in_force_is_found_from_anywhere_inside_its_stretch() {
        let bar = Ticks::from_beats(4.0);
        let map = TempoMap::from_points(vec![
            TempoPoint {
                tick: Ticks::ZERO,
                bpm: 120.0,
            },
            TempoPoint {
                tick: bar,
                bpm: 240.0,
            },
        ])
        .unwrap();
        assert_eq!(map.change_at(Ticks::ZERO), Ticks::ZERO);
        assert_eq!(map.change_at(bar - Ticks(1)), Ticks::ZERO);
        assert_eq!(map.change_at(bar), bar);
        assert_eq!(map.change_at(bar * 3), bar);
        // Before the timeline starts, the anchor is what is in force.
        assert_eq!(map.change_at(Ticks(-5)), Ticks::ZERO);
    }

    #[test]
    fn from_points_normalises_order_and_anchor() {
        let map = TempoMap::from_points(vec![
            TempoPoint {
                tick: Ticks(5000),
                bpm: 90.0,
            },
            TempoPoint {
                tick: Ticks(100),
                bpm: 150.0,
            },
        ])
        .unwrap();
        assert_eq!(map.points()[0].tick, Ticks::ZERO);
        assert_eq!(map.points()[0].bpm, 150.0);
        assert_eq!(map.points()[1].tick, Ticks(5000));
    }

    #[test]
    fn bar_beat_counts_from_one() {
        let map = TempoMap::constant(120.0);
        let signature = TimeSignature::default();
        assert_eq!(map.bar_beat_at(Ticks::ZERO, signature), (1, 1, 0));
        assert_eq!(
            map.bar_beat_at(Ticks::from_beats(4.0), signature),
            (2, 1, 0)
        );
        assert_eq!(
            map.bar_beat_at(Ticks::from_beats(5.5), signature),
            (2, 2, TICKS_PER_QUARTER / 2)
        );
    }

    #[test]
    fn deserializing_an_empty_point_list_is_an_error() {
        // `initial_bpm` and `ticks_to_seconds` index `points[0]`, so an empty map panics on use.
        let result = serde_json::from_str::<TempoMap>(r#"{"points":[]}"#);
        assert!(result.is_err(), "empty tempo map must not deserialize");
    }

    #[test]
    fn deserializing_a_non_positive_bpm_is_an_error() {
        for bad in ["0.0", "-120.0"] {
            let json = format!(r#"{{"points":[{{"tick":0,"bpm":{bad}}}]}}"#);
            let result = serde_json::from_str::<TempoMap>(&json);
            assert!(result.is_err(), "bpm {bad} must not deserialize");
        }
    }

    #[test]
    fn deserialized_tempo_stays_inside_the_supported_range() {
        // A surviving out-of-range BPM makes `ticks_to_seconds` non-finite, and
        // `Seconds::as_samples` then saturates to `u64::MAX` frames.
        let map: TempoMap =
            serde_json::from_str(r#"{"points":[{"tick":0,"bpm":0.0000001}]}"#).unwrap();
        assert_eq!(map.initial_bpm(), TempoMap::MIN_BPM);
        let seconds = map.ticks_to_seconds(Ticks::QUARTER);
        assert!(seconds.0.is_finite());
        assert!(seconds.as_samples(48_000.0).raw() < u64::MAX);

        let map: TempoMap = serde_json::from_str(r#"{"points":[{"tick":0,"bpm":1e9}]}"#).unwrap();
        assert_eq!(map.initial_bpm(), TempoMap::MAX_BPM);
    }

    #[test]
    fn deserializing_normalises_order_and_anchor() {
        let map: TempoMap = serde_json::from_str(
            r#"{"points":[{"tick":5000,"bpm":90.0},{"tick":100,"bpm":150.0}]}"#,
        )
        .unwrap();
        assert_eq!(map.points().len(), 2);
        assert_eq!(map.points()[0].tick, Ticks::ZERO);
        assert_eq!(map.points()[0].bpm, 150.0);
        assert_eq!(map.points()[1].tick, Ticks(5000));
        assert_eq!(map.points()[1].bpm, 90.0);
    }

    #[test]
    fn from_points_always_upholds_the_invariant() {
        // `from_points` is the only gate in front of deserialization, so every accepted input
        // has to come out satisfying what the readers assume. Cover the awkward mixes of
        // negative, duplicate and out-of-order positions in one sweep.
        let candidates = [-5000i64, -960, -1, 0, 1, 960, 5000];
        for a in candidates {
            for b in candidates {
                for c in candidates {
                    let map = TempoMap::from_points(
                        [(a, 120.0), (b, 90.0), (c, 60.0)]
                            .into_iter()
                            .map(|(tick, bpm)| TempoPoint {
                                tick: Ticks(tick),
                                bpm,
                            })
                            .collect(),
                    )
                    .unwrap();
                    let points = map.points();
                    assert_eq!(points[0].tick, Ticks::ZERO, "{a} {b} {c} -> {points:?}");
                    assert!(
                        points.windows(2).all(|pair| pair[0].tick < pair[1].tick),
                        "{a} {b} {c} -> {points:?}"
                    );
                    assert!(
                        map.ticks_to_seconds(Ticks(4000)).0 > 0.0,
                        "{a} {b} {c} -> {points:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn deserializing_folds_positions_before_the_anchor() {
        // Rejecting bad BPM is not enough: a corrupted file can also hold points before tick 0,
        // and those leave the list unordered once the tick-0 anchor is forced. Readers trust the
        // ordering, so the map has to come back sorted rather than merely non-empty.
        let map: TempoMap = serde_json::from_str(
            r#"{"points":[{"tick":-100,"bpm":90.0},{"tick":-50,"bpm":100.0},{"tick":960,"bpm":120.0}]}"#,
        )
        .unwrap();
        assert!(
            map.points().iter().all(|p| p.tick >= Ticks::ZERO),
            "positions must be clamped onto the timeline: {:?}",
            map.points()
        );
        assert!(
            map.points()
                .windows(2)
                .all(|pair| pair[0].tick < pair[1].tick),
            "points must stay strictly ordered: {:?}",
            map.points()
        );
        assert_eq!(map.points()[0].tick, Ticks::ZERO);
        // With the list out of order the first segment ran backwards, so `ticks_to_seconds`
        // broke out immediately and reported 0 s for every position on the timeline.
        assert!((map.ticks_to_seconds(Ticks::QUARTER).0 - 60.0 / 90.0).abs() < 1e-9);
    }

    #[test]
    fn tempo_map_json_round_trips_unchanged() {
        let map = TempoMap::from_points(vec![
            TempoPoint {
                tick: Ticks::ZERO,
                bpm: 120.0,
            },
            TempoPoint {
                tick: Ticks(5000),
                bpm: 90.0,
            },
        ])
        .unwrap();
        let json = serde_json::to_string(&map).unwrap();
        // The wire shape must not drift, or projects saved by older builds stop loading.
        assert_eq!(
            json,
            r#"{"points":[{"tick":0,"bpm":120.0},{"tick":5000,"bpm":90.0}]}"#
        );
        assert_eq!(serde_json::from_str::<TempoMap>(&json).unwrap(), map);
    }

    #[test]
    fn project_round_trips_with_its_tempo_map() {
        let mut project = crate::project::Project::new("Round trip", 48_000.0);
        project.tempo_map.set_point(Ticks(5000), 90.0);
        let json = serde_json::to_string(&project).unwrap();
        let loaded: crate::project::Project = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, project);
        assert_eq!(loaded.bpm(), 120.0);
    }

    #[test]
    fn snapping_rounds_to_the_grid() {
        let grid = Ticks(TICKS_PER_QUARTER / 4);
        assert_eq!(grid, Ticks(240));
        assert_eq!(Ticks(250).snap_floor(grid), Ticks(240));
        assert_eq!(Ticks(250).snap_nearest(grid), Ticks(240));
        // 350 is 110 past the lower gridline but 130 short of the upper one.
        assert_eq!(Ticks(350).snap_nearest(grid), Ticks(240));
        assert_eq!(Ticks(370).snap_nearest(grid), Ticks(480));
        assert_eq!(Ticks(-10).snap_floor(grid), Ticks(-240));
    }
}
