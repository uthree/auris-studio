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
        let offset = self.0 - below.0;
        let halfway_rounded_up = grid.0 / 2 + grid.0 % 2;
        if offset >= halfway_rounded_up {
            Ticks(below.0.saturating_add(grid.0))
        } else {
            below
        }
    }
}

impl Add for Ticks {
    type Output = Ticks;
    fn add(self, rhs: Ticks) -> Ticks {
        Ticks(self.0.saturating_add(rhs.0))
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
        self.0 = self.0.saturating_add(rhs.0);
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
        // Round once before dividing into fields. Formatting an unrounded seconds remainder
        // independently can produce `60.000` without carrying into the already-computed minute.
        let total_ms = (self.0.max(0.0) * 1_000.0).round() as u64;
        let minutes = total_ms / 60_000;
        let within_minute = total_ms % 60_000;
        let seconds = within_minute / 1_000;
        let millis = within_minute % 1_000;
        format!("{minutes:02}:{seconds:02}.{millis:03}")
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
    /// Beat counts a bar may have.
    ///
    /// Bounded above because [`Self::ticks_per_bar`] is multiplied by a bar number all over the
    /// document, and a numerator in the millions overflows every position derived from it long
    /// before it means anything musically.
    pub const NUMERATORS: std::ops::RangeInclusive<u32> = 1..=32;

    /// Note values that may take the beat: whole through sixteenth.
    pub const DENOMINATORS: [u32; 5] = [1, 2, 4, 8, 16];

    /// The meters offered wherever one is picked from a list rather than typed.
    ///
    /// Simple, compound and the odd ones people actually write in, in the order a musician would
    /// think of them. Not exhaustive — [`FromStr`](std::str::FromStr) takes anything inside the
    /// bounds above — but a menu of every meter in the range would be four hundred rows.
    pub const COMMON: [Self; 8] = [
        Self {
            numerator: 4,
            denominator: 4,
        },
        Self {
            numerator: 3,
            denominator: 4,
        },
        Self {
            numerator: 2,
            denominator: 4,
        },
        Self {
            numerator: 6,
            denominator: 8,
        },
        Self {
            numerator: 12,
            denominator: 8,
        },
        Self {
            numerator: 5,
            denominator: 4,
        },
        Self {
            numerator: 7,
            denominator: 8,
        },
        Self {
            numerator: 9,
            denominator: 8,
        },
    ];

    /// Creates a signature, falling back to 4/4 for degenerate input.
    pub fn new(numerator: u32, denominator: u32) -> Self {
        if !Self::NUMERATORS.contains(&numerator) || !Self::DENOMINATORS.contains(&denominator) {
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
    ///
    /// The note the denominator names: a quarter in 4/4, an eighth in 6/8. This is what a
    /// *position* is counted in, which is why the ruler and [`Self::position`] use it — bar 2
    /// beat 4 of 6/8 means the fourth eighth, and nothing else would be readable.
    ///
    /// It is not what a player *feels*. See [`Self::beat_ticks`].
    pub fn ticks_per_beat(&self) -> Ticks {
        Ticks::from_beats(4.0 / self.denominator as f64)
    }

    /// `true` when the beat divides into three rather than into two.
    ///
    /// 6/8, 9/8 and 12/8 are compound: written in eighths, felt in dotted quarters, two of them to
    /// a bar of 6/8 rather than six. 3/8 is not — one dotted-quarter beat is a bar with no beats
    /// in it, and 3/8 is counted in three the way 3/4 is.
    pub fn is_compound(&self) -> bool {
        self.denominator >= 8 && self.numerator > 3 && self.numerator.is_multiple_of(3)
    }

    /// Length of one *felt* beat in ticks: what a foot taps and a groove is written against.
    ///
    /// The same as [`Self::ticks_per_beat`] everywhere except compound time, where it is three of
    /// them — the dotted quarter of a 6/8. The difference is the whole of why a drum pattern
    /// written for 4/4 cannot simply be counted out in a bar of 6/8: it has four beats and that
    /// bar has two.
    pub fn beat_ticks(&self) -> Ticks {
        match self.is_compound() {
            true => self.ticks_per_beat() * 3,
            false => self.ticks_per_beat(),
        }
    }

    /// How many felt beats are in one bar.
    pub fn felt_beats(&self) -> u32 {
        match self.is_compound() {
            true => self.numerator / 3,
            false => self.numerator,
        }
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

impl std::fmt::Display for TimeSignature {
    /// `4/4`, which is how a signature is written everywhere it is written at all.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

impl std::str::FromStr for TimeSignature {
    type Err = CoreError;

    /// Reads `4/4`, and refuses anything that is not a meter this can count.
    ///
    /// The bounds are the same ones the song specification parser applies, and for the same
    /// reason: a numerator in the millions makes [`Self::ticks_per_bar`] overflow every position
    /// computed from it, and a denominator that is not a note value does not name a beat.
    fn from_str(text: &str) -> Result<Self> {
        let complaint = || CoreError::InvalidTimeSignature(text.to_string());
        let (top, bottom) = text.trim().split_once('/').ok_or_else(complaint)?;
        let numerator: u32 = top.trim().parse().map_err(|_| complaint())?;
        let denominator: u32 = bottom.trim().parse().map_err(|_| complaint())?;
        if !Self::NUMERATORS.contains(&numerator) || !Self::DENOMINATORS.contains(&denominator) {
            return Err(complaint());
        }
        Ok(Self::new(numerator, denominator))
    }
}

/// A signature change at a musical position.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignaturePoint {
    /// Where the new signature takes effect. Always the start of a bar.
    pub tick: Ticks,
    /// The signature from this point onwards.
    pub signature: TimeSignature,
}

/// One stretch of the timeline governed by a single signature.
///
/// What a painter walks. The bar number is carried because it cannot be recovered from the
/// position alone once the meter has changed: bar 9 is 30720 ticks in through four bars of 4/4
/// and four of 3/4, and nothing but the accumulation says so.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SignatureSpan {
    /// Where the stretch begins.
    pub start: Ticks,
    /// Where the next one begins, or `None` for the stretch that runs to the end of time.
    pub end: Option<Ticks>,
    /// The signature in force across it.
    pub signature: TimeSignature,
    /// 1-based number of the bar at [`Self::start`].
    pub first_bar: u32,
}

/// Piecewise-constant time signature over the timeline.
///
/// The third of the document's timelines, built to the same shape as
/// [`TempoMap`] and [`KeyMap`](crate::harmony::KeyMap): a sorted list of points, each in force
/// until the next, anchored at tick 0 so that every position has an answer.
///
/// # Bar lines, and the invariant that makes them mean something
///
/// A meter change carries one thing the other two do not: it moves the bar lines after it. That
/// only works if every change *is* a bar line — a 3/4 beginning half way through a bar of 4/4
/// leaves the bar it interrupts with no length, and the bar numbers after it stop being
/// countable. So the fourth invariant, upheld by every constructor and mutator, is that each
/// point sits a whole number of the previous signature's bars past the point before it.
///
/// Normalisation is what upholds it: a change written anywhere is moved to the nearest bar line,
/// and a change written *before* one already there pushes the later ones onto the new grid. That
/// is a real edit to the later positions, and it is the honest one — the alternative is storing
/// bar numbers, and a stored bar number moves the notes underneath it every time an earlier bar
/// changes length.
///
/// # Nothing here reaches the audio
///
/// A signature is notation. Positions are ticks, the tempo map turns ticks into samples, and
/// neither of them asks how many beats are in a bar. Editing this changes where the bar lines are
/// drawn and nothing about what is heard, which is why the session does not rebuild the graph
/// for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SignatureMapRepr")]
pub struct SignatureMap {
    points: Vec<SignaturePoint>,
}

/// On-disk shape of a [`SignatureMap`].
///
/// Infallible on the way in, like [`KeyMap`](crate::harmony::KeyMap) and unlike [`TempoMap`]: a
/// degenerate signature is answered by [`TimeSignature::new`] with 4/4, and no arithmetic
/// anywhere divides by a numerator. An empty list is the default map rather than an error.
#[derive(Deserialize)]
struct SignatureMapRepr {
    #[serde(default)]
    points: Vec<SignaturePoint>,
}

impl From<SignatureMapRepr> for SignatureMap {
    fn from(repr: SignatureMapRepr) -> Self {
        Self::from_points(repr.points)
    }
}

impl Default for SignatureMap {
    /// 4/4 for the whole timeline.
    fn default() -> Self {
        Self::constant(TimeSignature::default())
    }
}

impl SignatureMap {
    /// A map with one signature for the whole timeline.
    pub fn constant(signature: TimeSignature) -> Self {
        Self {
            points: vec![SignaturePoint {
                tick: Ticks::ZERO,
                signature,
            }],
        }
    }

    /// Builds a map from arbitrary points, normalising order, duplicates and the bar lines.
    pub fn from_points(mut points: Vec<SignaturePoint>) -> Self {
        if points.is_empty() {
            return Self::default();
        }
        for point in &mut points {
            point.tick = point.tick.max_zero();
            // A signature that would not have parsed still has to come out usable, because the
            // file it arrived in is user-editable text.
            point.signature =
                TimeSignature::new(point.signature.numerator, point.signature.denominator);
        }
        points.sort_by_key(|point| point.tick);
        points[0].tick = Ticks::ZERO;
        align_to_bars(&mut points);
        Self { points }
    }

    /// The signature changes, ordered by position.
    pub fn points(&self) -> &[SignaturePoint] {
        &self.points
    }

    /// The signature of the first segment — the "project signature" for a map that never changes.
    pub fn initial(&self) -> TimeSignature {
        self.points[0].signature
    }

    /// Replaces the signature of the first segment.
    ///
    /// The changes after it keep their bar-line invariant, which means the ones that no longer
    /// land on a bar of the new opening meter are moved onto the nearest one.
    pub fn set_initial(&mut self, signature: TimeSignature) {
        self.points[0].signature = signature;
        align_to_bars(&mut self.points);
    }

    /// `true` when one signature holds for the whole song.
    pub fn is_constant(&self) -> bool {
        self.points.len() == 1
    }

    /// Inserts or replaces a signature change, at the bar line `tick` is nearest.
    pub fn set_point(&mut self, tick: Ticks, signature: TimeSignature) {
        let tick = self.snap_bar(tick);
        match self.points.binary_search_by_key(&tick, |point| point.tick) {
            Ok(index) => self.points[index].signature = signature,
            Err(index) => self
                .points
                .insert(index, SignaturePoint { tick, signature }),
        }
        align_to_bars(&mut self.points);
    }

    /// Removes the signature change at `tick`. The anchor at tick 0 cannot be removed.
    pub fn remove_point(&mut self, tick: Ticks) {
        if tick == Ticks::ZERO {
            return;
        }
        if let Ok(index) = self.points.binary_search_by_key(&tick, |point| point.tick) {
            self.points.remove(index);
            align_to_bars(&mut self.points);
        }
    }

    /// Index of the segment containing `tick`.
    fn segment_index(&self, tick: Ticks) -> usize {
        match self.points.binary_search_by_key(&tick, |point| point.tick) {
            Ok(index) => index,
            // `tick` precedes every point only when it is negative; clamp into segment 0.
            Err(index) => index.saturating_sub(1),
        }
    }

    /// The signature in force at `tick`.
    ///
    /// Total, like [`TempoMap::bpm_at`]: there is always a signature, and a negative tick reads
    /// the first segment.
    pub fn signature_at(&self, tick: Ticks) -> TimeSignature {
        self.points[self.segment_index(tick)].signature
    }

    /// Where the signature change in force at `tick` sits.
    ///
    /// Total, for the reason given on [`TempoMap::change_at`]: this is what an editor acts
    /// *through*, and "remove the signature change here" means the one the bar lines are being
    /// drawn from rather than one that happens to start under the pointer.
    pub fn change_at(&self, tick: Ticks) -> Ticks {
        self.points[self.segment_index(tick)].tick
    }

    /// Every stretch of the timeline, each with the bar number it opens on.
    ///
    /// Owned and whole rather than a range query: the list is a handful of points, a paint
    /// closure has to capture `'static`, and the bar numbers only exist by accumulating from the
    /// start anyway.
    pub fn spans(&self) -> Vec<SignatureSpan> {
        let mut spans = Vec::with_capacity(self.points.len());
        let mut bar = 1u32;
        for (index, point) in self.points.iter().enumerate() {
            let end = self.points.get(index + 1).map(|next| next.tick);
            spans.push(SignatureSpan {
                start: point.tick,
                end,
                signature: point.signature,
                first_bar: bar,
            });
            if let Some(end) = end {
                bar = bar.saturating_add(bars_between(point.tick, end, point.signature));
            }
        }
        spans
    }

    /// The 1-based bar containing `tick`. Anything before the timeline starts is bar 1.
    pub fn bar_of(&self, tick: Ticks) -> u32 {
        let tick = tick.max_zero();
        let index = self.segment_index(tick);
        let span = &self.points[index];
        let mut bar = 1u32;
        for (previous, next) in self.points[..=index]
            .iter()
            .zip(self.points[1..=index].iter())
        {
            bar = bar.saturating_add(bars_between(previous.tick, next.tick, previous.signature));
        }
        bar.saturating_add(bars_between(span.tick, tick, span.signature))
    }

    /// Where 1-based `bar` begins. Bar 1 is [`Ticks::ZERO`].
    pub fn bar_start(&self, bar: u32) -> Ticks {
        let wanted = bar.max(1);
        // The last span opening at or before the wanted bar is the one it falls in — every span
        // after it opens later, and the final span runs on forever. There is always one: the
        // anchor's span opens on bar 1, and `wanted` is at least 1.
        let span = self
            .spans()
            .into_iter()
            .take_while(|span| span.first_bar <= wanted)
            .last()
            .expect("the anchor's span opens on bar one");
        span.start + span.signature.ticks_per_bar() * i64::from(wanted - span.first_bar)
    }

    /// Where 1-based `beat` of 1-based `bar` begins.
    ///
    /// The beat is fractional, so that a chord landing on the second half of beat two — `2.5` —
    /// is sayable. This is the constructor that keeps meaningless positions out of the harmony
    /// timeline: nothing hands out a bare tick.
    pub fn position(&self, bar: u32, beat: f64) -> Ticks {
        let start = self.bar_start(bar);
        let per_beat = self.signature_at(start).ticks_per_beat().raw() as f64;
        start + Ticks(((beat.max(1.0) - 1.0) * per_beat).round() as i64)
    }

    /// Start of the bar containing `tick`.
    pub fn bar_floor(&self, tick: Ticks) -> Ticks {
        let tick = tick.max_zero();
        let span = &self.points[self.segment_index(tick)];
        let per_bar = span.signature.ticks_per_bar().raw().max(1);
        span.tick + Ticks((tick.raw() - span.tick.raw()) / per_bar * per_bar)
    }

    /// The bar line `tick` is nearest — where a signature change written there lands.
    ///
    /// Nearest rather than the bar it falls in, because this rounds a *pointer*: aiming just
    /// short of bar nine means bar nine, and answering with bar eight would put the change a
    /// whole bar from where it was asked for.
    pub fn snap_bar(&self, tick: Ticks) -> Ticks {
        let floor = self.bar_floor(tick);
        let per_bar = self.signature_at(floor).ticks_per_bar();
        if tick.raw() - floor.raw() >= per_bar.raw() / 2 {
            floor + per_bar
        } else {
            floor
        }
    }

    /// Bar and beat (both 1-based) plus the tick offset inside that beat.
    pub fn bar_beat_at(&self, tick: Ticks) -> (u32, u32, i64) {
        let tick = tick.max_zero();
        let bar_start = self.bar_floor(tick);
        let signature = self.signature_at(bar_start);
        let per_beat = signature.ticks_per_beat().raw().max(1);
        let in_bar = tick.raw() - bar_start.raw();
        (
            self.bar_of(tick),
            (in_bar / per_beat) as u32 + 1,
            in_bar % per_beat,
        )
    }
}

/// Whole bars of `signature` between two positions, rounded down.
fn bars_between(from: Ticks, to: Ticks, signature: TimeSignature) -> u32 {
    let per_bar = signature.ticks_per_bar().raw().max(1);
    (to.raw().saturating_sub(from.raw()).max(0) / per_bar).min(i64::from(u32::MAX)) as u32
}

/// Moves every change onto a bar line of the stretch before it.
///
/// Forward, so each point is measured against a predecessor that is already correct, and by at
/// least one bar, so the result stays strictly increasing however the points arrived. A change
/// that would land on top of its predecessor is dropped rather than moved: it named the same bar,
/// and the later signature is the one the user asked for last.
fn align_to_bars(points: &mut Vec<SignaturePoint>) {
    // Keep the distances the writer supplied separate from the aligned positions. An earlier
    // point can round forwards past the next point's original tick; measuring that next point
    // from the moved predecessor would then mistake it for a same-bar collision and erase a
    // genuine signature change.
    let original = std::mem::take(points);
    let mut original_previous = original[0].tick;
    points.push(original[0]);
    for mut point in original.into_iter().skip(1) {
        let previous = *points.last().expect("the anchor was inserted above");
        let per_bar = previous.signature.ticks_per_bar().raw().max(1);
        let original_current = point.tick;
        let offset = original_current
            .raw()
            .saturating_sub(original_previous.raw());
        let whole_bars = offset / per_bar;
        let remainder = offset % per_bar;
        let rounded_bars =
            whole_bars.saturating_add(i64::from(remainder >= per_bar.saturating_add(1) / 2));
        // Clamping the final tick itself could leave it between bar lines. Clamp the number of
        // whole bars instead, so even corrupt on-disk ticks retain the map's alignment invariant.
        let representable_bars = i64::MAX.saturating_sub(previous.tick.raw()) / per_bar;
        let bars = rounded_bars.min(representable_bars);
        if bars < 1 {
            // The same bar as the change before it. Two signatures cannot both begin there, and
            // the one written later is the one that was meant.
            points
                .last_mut()
                .expect("the anchor was inserted above")
                .signature = point.signature;
            original_previous = original_current;
            continue;
        }
        point.tick = previous.tick + Ticks(bars * per_bar);
        points.push(point);
        original_previous = original_current;
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
    /// When several points share a tick, the one supplied last wins, as in [`Self::set_point`].
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
        let mut unique: Vec<TempoPoint> = Vec::with_capacity(points.len());
        for point in points {
            match unique.last_mut() {
                Some(previous) if previous.tick == point.tick => *previous = point,
                _ => unique.push(point),
            }
        }
        Ok(Self { points: unique })
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_addition_saturates_at_document_boundaries() {
        assert_eq!(Ticks(i64::MAX) + Ticks(1), Ticks(i64::MAX));
        let mut assigned = Ticks(i64::MAX);
        assigned += Ticks(1);
        assert_eq!(assigned, Ticks(i64::MAX));
    }

    #[test]
    fn an_odd_grid_snaps_to_the_truly_nearest_line() {
        assert_eq!(Ticks(1).snap_nearest(Ticks(3)), Ticks::ZERO);
        assert_eq!(Ticks(2).snap_nearest(Ticks(3)), Ticks(3));
    }

    #[test]
    fn clock_rounding_carries_across_minute_boundaries() {
        assert_eq!(Seconds(59.9994).format_clock(), "00:59.999");
        assert_eq!(Seconds(59.9996).format_clock(), "01:00.000");
        assert_eq!(Seconds(179.9996).format_clock(), "03:00.000");
    }

    #[test]
    fn a_compound_meter_is_felt_in_dotted_beats() {
        let six_eight = TimeSignature::new(6, 8);
        assert!(six_eight.is_compound());
        // Two dotted quarters, not six eighths. The bar is the same length either way; what
        // changes is how many beats are in it, which is what a groove is written against.
        assert_eq!(six_eight.felt_beats(), 2);
        assert_eq!(six_eight.beat_ticks(), Ticks(Ticks::QUARTER.raw() * 3 / 2));
        assert_eq!(six_eight.ticks_per_beat(), Ticks(Ticks::QUARTER.raw() / 2));
        assert_eq!(six_eight.beat_ticks() * 2, six_eight.ticks_per_bar());

        for (numerator, beats) in [(9u32, 3u32), (12, 4)] {
            let signature = TimeSignature::new(numerator, 8);
            assert!(signature.is_compound(), "{numerator}/8");
            assert_eq!(signature.felt_beats(), beats);
            assert_eq!(
                signature.beat_ticks() * i64::from(beats),
                signature.ticks_per_bar()
            );
        }

        // Simple meters feel the note the denominator names, and the two agree.
        for signature in [
            TimeSignature::new(4, 4),
            TimeSignature::new(3, 4),
            TimeSignature::new(5, 4),
            TimeSignature::new(7, 8),
            // Three of anything is counted in three: one dotted beat is a bar with no beats in it.
            TimeSignature::new(3, 8),
        ] {
            assert!(!signature.is_compound(), "{signature:?}");
            assert_eq!(signature.beat_ticks(), signature.ticks_per_beat());
            assert_eq!(signature.felt_beats(), signature.numerator);
        }
    }

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
    fn duplicate_tempo_points_keep_the_last_write() {
        let map = TempoMap::from_points(vec![
            TempoPoint {
                tick: Ticks::ZERO,
                bpm: 90.0,
            },
            TempoPoint {
                tick: Ticks::ZERO,
                bpm: 150.0,
            },
        ])
        .unwrap();

        assert_eq!(map.bpm_at(Ticks::ZERO), 150.0);
    }

    #[test]
    fn bar_beat_counts_from_one() {
        let map = SignatureMap::default();
        assert_eq!(map.bar_beat_at(Ticks::ZERO), (1, 1, 0));
        assert_eq!(map.bar_beat_at(Ticks::from_beats(4.0)), (2, 1, 0));
        assert_eq!(
            map.bar_beat_at(Ticks::from_beats(5.5)),
            (2, 2, TICKS_PER_QUARTER / 2)
        );
    }

    /// Four bars of 4/4, then 3/4 from bar 5.
    fn four_then_three() -> SignatureMap {
        let mut map = SignatureMap::default();
        map.set_point(Ticks::from_beats(16.0), TimeSignature::new(3, 4));
        map
    }

    #[test]
    fn bars_are_counted_through_a_change_of_meter() {
        let map = four_then_three();
        // Bar 5 is where the 3/4 begins: four bars of four quarters.
        assert_eq!(map.bar_of(Ticks::from_beats(16.0)), 5);
        assert_eq!(map.bar_start(5), Ticks::from_beats(16.0));
        // And the bars after it are three quarters long, not four.
        assert_eq!(map.bar_start(6), Ticks::from_beats(19.0));
        assert_eq!(map.bar_of(Ticks::from_beats(19.0)), 6);
        assert_eq!(
            map.bar_of(Ticks::from_beats(21.9)),
            6,
            "still inside bar six"
        );
        assert_eq!(map.bar_of(Ticks::from_beats(22.0)), 7);
        // The beat inside the bar follows the new meter too.
        assert_eq!(map.bar_beat_at(Ticks::from_beats(20.0)), (6, 2, 0));
        // Bar and position invert each other on both sides of the change.
        for bar in 1..12 {
            assert_eq!(map.bar_of(map.bar_start(bar)), bar);
            assert_eq!(map.position(bar, 1.0), map.bar_start(bar));
        }
    }

    #[test]
    fn a_change_lands_on_the_bar_line_it_was_aimed_at() {
        // A pointer never lands on a bar line exactly, and a signature that began half way
        // through a bar would leave that bar with no length and the numbering after it
        // uncountable.
        let mut map = SignatureMap::default();
        map.set_point(Ticks::from_beats(9.0), TimeSignature::new(7, 8));
        assert_eq!(map.points()[1].tick, Ticks::from_beats(8.0));

        // Nearest, not the bar it fell in: aiming just short of bar four means bar four.
        let mut map = SignatureMap::default();
        map.set_point(Ticks::from_beats(11.5), TimeSignature::new(7, 8));
        assert_eq!(map.points()[1].tick, Ticks::from_beats(12.0));
    }

    #[test]
    fn a_change_written_earlier_pushes_the_later_ones_onto_the_new_grid() {
        // The invariant is that every change sits on a bar line of the stretch before it, so
        // shortening the opening bars has to move whatever came after. Bar numbers are derived
        // from the accumulation, and one point off the grid makes every bar after it a fraction.
        let mut map = four_then_three();
        map.set_point(Ticks::ZERO, TimeSignature::new(7, 8));
        let bar = TimeSignature::new(7, 8).ticks_per_bar().raw();
        assert_eq!(
            map.points()[1].tick.raw() % bar,
            0,
            "the 3/4 no longer starts on a bar line: {:?}",
            map.points()
        );
        for bar in 1..8 {
            assert_eq!(map.bar_of(map.bar_start(bar)), bar);
        }
    }

    #[test]
    fn two_signatures_cannot_share_a_bar() {
        // Normalisation moves a change onto the nearest bar line, which can land it on top of
        // one already there. The later signature wins, because it is the one asked for last.
        let map = SignatureMap::from_points(vec![
            SignaturePoint {
                tick: Ticks::ZERO,
                signature: TimeSignature::new(4, 4),
            },
            SignaturePoint {
                tick: Ticks(200),
                signature: TimeSignature::new(3, 4),
            },
        ]);
        assert_eq!(map.points().len(), 1);
        assert_eq!(map.initial(), TimeSignature::new(3, 4));
    }

    #[test]
    fn the_spans_tile_the_timeline_and_number_their_own_bars() {
        let spans = four_then_three().spans();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start, Ticks::ZERO);
        assert_eq!(spans[0].end, Some(Ticks::from_beats(16.0)));
        assert_eq!(spans[0].first_bar, 1);
        assert_eq!(spans[1].start, Ticks::from_beats(16.0));
        assert_eq!(spans[1].end, None, "the last span runs to the end of time");
        assert_eq!(spans[1].first_bar, 5);
    }

    #[test]
    fn removing_a_change_lets_the_meter_before_it_run_through() {
        let mut map = four_then_three();
        map.remove_point(Ticks::from_beats(16.0));
        assert!(map.is_constant());
        assert_eq!(map.signature_at(Ticks::from_beats(100.0)), map.initial());

        // The anchor is not a change, and a song is always in some meter.
        map.remove_point(Ticks::ZERO);
        assert_eq!(map.points().len(), 1);
    }

    #[test]
    fn the_meter_in_force_is_found_from_anywhere_inside_its_stretch() {
        let map = four_then_three();
        assert_eq!(map.change_at(Ticks::ZERO), Ticks::ZERO);
        assert_eq!(map.change_at(Ticks::from_beats(15.0)), Ticks::ZERO);
        assert_eq!(
            map.change_at(Ticks::from_beats(16.0)),
            Ticks::from_beats(16.0)
        );
        assert_eq!(
            map.change_at(Ticks::from_beats(900.0)),
            Ticks::from_beats(16.0)
        );
        assert_eq!(map.change_at(Ticks(-5)), Ticks::ZERO);
    }

    #[test]
    fn signature_points_always_uphold_the_invariant() {
        // The only gate in front of deserialization, so every accepted input has to come out
        // satisfying what the readers assume: ordered, anchored, and every change on a bar line
        // of the stretch before it. Sweep the awkward mixes of negative, duplicate and
        // out-of-order positions against meters of different bar lengths.
        let positions = [-5000i64, -1, 0, 1, 960, 3840, 5000, 20_000];
        let meters = [(4, 4), (3, 4), (7, 8), (12, 8)];
        for a in positions {
            for b in positions {
                for (numerator, denominator) in meters {
                    let map = SignatureMap::from_points(vec![
                        SignaturePoint {
                            tick: Ticks(a),
                            signature: TimeSignature::new(numerator, denominator),
                        },
                        SignaturePoint {
                            tick: Ticks(b),
                            signature: TimeSignature::new(3, 4),
                        },
                    ]);
                    let points = map.points();
                    assert_eq!(points[0].tick, Ticks::ZERO, "{a} {b} -> {points:?}");
                    for pair in points.windows(2) {
                        assert!(pair[0].tick < pair[1].tick, "{a} {b} -> {points:?}");
                        let per_bar = pair[0].signature.ticks_per_bar().raw();
                        assert_eq!(
                            (pair[1].tick.raw() - pair[0].tick.raw()) % per_bar,
                            0,
                            "{a} {b} -> {points:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn aligning_an_earlier_signature_forwards_does_not_swallow_the_next_one() {
        let mut map = SignatureMap::from_points(vec![
            SignaturePoint {
                tick: Ticks::ZERO,
                signature: TimeSignature::new(4, 4),
            },
            SignaturePoint {
                tick: Ticks(7_680),
                signature: TimeSignature::new(3, 4),
            },
            SignaturePoint {
                tick: Ticks(10_560),
                signature: TimeSignature::new(7, 8),
            },
        ]);

        // The new opening bar moves the middle change forwards from 7,680 to 9,600. The final
        // change was only 960 ticks beyond that *new* position, but it was a full 3/4 bar beyond
        // the position the user supplied and must remain a separate change.
        map.set_initial(TimeSignature::new(5, 2));

        assert_eq!(
            map.points(),
            &[
                SignaturePoint {
                    tick: Ticks::ZERO,
                    signature: TimeSignature::new(5, 2),
                },
                SignaturePoint {
                    tick: Ticks(9_600),
                    signature: TimeSignature::new(3, 4),
                },
                SignaturePoint {
                    tick: Ticks(12_480),
                    signature: TimeSignature::new(7, 8),
                },
            ]
        );
    }

    #[test]
    fn an_extreme_deserialized_signature_tick_is_clamped_to_a_bar_line() {
        let map: SignatureMap = serde_json::from_str(
            r#"{"points":[{"tick":0,"signature":{"numerator":4,"denominator":4}},
                         {"tick":9223372036854775807,"signature":{"numerator":3,"denominator":4}}]}"#,
        )
        .unwrap();

        let points = map.points();
        assert_eq!(points.len(), 2);
        let per_bar = points[0].signature.ticks_per_bar().raw();
        assert_eq!(points[1].tick.raw(), i64::MAX / per_bar * per_bar);
        assert_eq!(points[1].tick.raw() % per_bar, 0);
        assert_eq!(map.bar_of(Ticks(i64::MAX)), u32::MAX);
    }

    #[test]
    fn a_signature_map_round_trips_and_survives_a_hand_edit() {
        let map = four_then_three();
        let json = serde_json::to_string(&map).unwrap();
        assert_eq!(serde_json::from_str::<SignatureMap>(&json).unwrap(), map);

        // A file is user-editable text. Nothing here can fail the load: a nonsense meter becomes
        // 4/4 and a position off the bar line is moved onto one.
        let hand_edited: SignatureMap = serde_json::from_str(
            r#"{"points":[{"tick":0,"signature":{"numerator":0,"denominator":0}},
                          {"tick":-40,"signature":{"numerator":3,"denominator":4}},
                          {"tick":7000,"signature":{"numerator":5,"denominator":4}}]}"#,
        )
        .unwrap();
        assert_eq!(hand_edited.points()[0].tick, Ticks::ZERO);
        for pair in hand_edited.points().windows(2) {
            assert!(pair[0].tick < pair[1].tick);
        }
        // And an empty list is the default rather than a document that will not open.
        let empty: SignatureMap = serde_json::from_str(r#"{"points":[]}"#).unwrap();
        assert_eq!(empty, SignatureMap::default());
    }

    #[test]
    fn a_signature_is_read_and_written_the_way_it_is_spoken() {
        assert_eq!(TimeSignature::new(6, 8).to_string(), "6/8");
        assert_eq!(
            "6/8".parse::<TimeSignature>().unwrap(),
            TimeSignature::new(6, 8)
        );
        assert_eq!(
            " 3 / 4 ".parse::<TimeSignature>().unwrap(),
            TimeSignature::new(3, 4)
        );
        // Every common meter reads back as itself.
        for signature in TimeSignature::COMMON {
            assert_eq!(
                signature.to_string().parse::<TimeSignature>().unwrap(),
                signature
            );
        }
        // A bar of four million quarter notes overflows every position derived from it.
        for bad in ["", "4", "4/4/4", "0/4", "4/0", "4000000/4", "4/3", "x/y"] {
            assert!(
                bad.parse::<TimeSignature>().is_err(),
                "`{bad}` parsed as a signature"
            );
        }
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
        // Both negative points clamp onto tick zero, where the later write wins just as it does
        // through `set_point`.
        assert!((map.ticks_to_seconds(Ticks::QUARTER).0 - 60.0 / 100.0).abs() < 1e-9);
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
