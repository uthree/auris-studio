//! Key and chords over the timeline.
//!
//! Harmony is a property of the timeline, exactly as tempo is: it changes as the song goes on,
//! and at any one moment every track obeys the same one. No track owns it and no track may
//! disagree with it. It is therefore built to the same shape as [`TempoMap`](crate::time::TempoMap)
//! — a sorted list of points, each in force until the next — so that a reader who knows one knows
//! the other.
//!
//! # Two timelines, not one
//!
//! A key change and a chord change are different events that rarely coincide. Eight bars of
//! `I V vi IV` do not want the key restated eight times, and a modulation does not want every
//! chord rewritten. The cost is two lookups, and [`Harmony`] exists to do them together.
//!
//! # Numerals, not chords
//!
//! A chord span stores a [`Numeral`] — `IVmaj7`, not `Fmaj7` — read against whatever key is in
//! force where it sits. Transposing the key therefore transposes the progression, which is what a
//! person means by writing `IV`, and a modulation halfway through a section reharmonises the rest
//! of it without a single span being touched.
//!
//! # Ticks, not bars
//!
//! Positions are [`Ticks`], like every other position in the document. A chord chart is written in
//! bars, so [`TimeSignature::position`] is how one is *built* — but what is *stored* is the tick,
//! because a stored bar number silently changes meaning the moment the time signature is edited.
//! Notes would stay where they sound while chords slid somewhere else. Storing ticks means editing
//! the signature moves the bar lines under a fixed harmony, which is the behaviour a person
//! expects.

use serde::{Deserialize, Serialize};

use crate::theory::chart::{Chart, HarmonicEvent};
use crate::theory::chord::Chord;
use crate::theory::key::Key;
use crate::theory::numeral::Numeral;
use crate::theory::pitch::PitchClass;
use crate::theory::scale::ScaleId;
use crate::time::{Ticks, TimeSignature};

/// A key change at a musical position.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPoint {
    /// Where the new key takes effect.
    pub tick: Ticks,
    /// The key from this point onwards.
    pub key: Key,
}

/// Piecewise-constant key over the timeline.
///
/// Invariants, upheld by every constructor and mutator, and the same ones
/// [`TempoMap`](crate::time::TempoMap) keeps: at least one point, sorted by tick, no two points at
/// the same position, and the first point sitting at tick 0. [`Self::key_at`] indexes `points[0]`,
/// so an empty map would panic rather than merely misbehave — which is why deserialization goes
/// through a private repr instead of a plain derive.
///
/// The conversion cannot fail, which is where this parts company with the tempo map: a tempo of
/// zero divides by zero in the render path and has to be refused, whereas a key that will not
/// parse costs one key change and nothing else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "KeyMapRepr")]
pub struct KeyMap {
    points: Vec<KeyPoint>,
}

/// On-disk shape of a [`KeyMap`].
///
/// The key is read as a [`String`] rather than as a [`Key`], which is what makes it possible to
/// drop one bad point instead of failing the whole load. A misspelt key is cosmetic — the project
/// still has all its notes — and refusing to open a song over it would contradict the rule that a
/// missing asset is reported rather than fatal.
#[derive(Deserialize)]
struct KeyMapRepr {
    #[serde(default)]
    points: Vec<KeyPointRepr>,
}

#[derive(Deserialize)]
struct KeyPointRepr {
    tick: Ticks,
    key: String,
}

impl From<KeyMapRepr> for KeyMap {
    fn from(repr: KeyMapRepr) -> Self {
        let points = repr
            .points
            .into_iter()
            .filter_map(|point| {
                Key::parse(&point.key).map(|key| KeyPoint {
                    tick: point.tick,
                    key,
                })
            })
            .collect::<Vec<_>>();
        // `from_points` already answers an empty list with the default; being explicit here says
        // that a file whose every key was unreadable is the same case, not a different one.
        Self::from_points(points)
    }
}

impl Default for KeyMap {
    /// C major for the whole timeline.
    fn default() -> Self {
        Self::constant(Key::new(PitchClass::new(0), ScaleId::Major))
    }
}

impl KeyMap {
    /// A map with one key for the whole timeline.
    pub fn constant(key: Key) -> Self {
        Self {
            points: vec![KeyPoint {
                tick: Ticks::ZERO,
                key,
            }],
        }
    }

    /// Builds a map from arbitrary points, normalising order, duplicates and the tick-0 anchor.
    ///
    /// Infallible, unlike [`TempoMap::from_points`](crate::time::TempoMap::from_points): a tempo of
    /// zero divides by zero in the render path, but every key that exists is a usable key. An
    /// empty list gives [`Self::default`] rather than an error, for the same reason.
    pub fn from_points(mut points: Vec<KeyPoint>) -> Self {
        if points.is_empty() {
            return Self::default();
        }
        // Clamp before ordering, exactly as the tempo map does: a point before tick 0 has no
        // meaning, and clamping it afterwards would leave the list out of order for the binary
        // search in `segment_index`, which would then misread silently instead of failing.
        for point in &mut points {
            point.tick = point.tick.max_zero();
        }
        points.sort_by_key(|point| point.tick);
        points[0].tick = Ticks::ZERO;
        points.dedup_by_key(|point| point.tick);
        Self { points }
    }

    /// The key changes, ordered by position.
    pub fn points(&self) -> &[KeyPoint] {
        &self.points
    }

    /// The key of the first segment — the "project key" for a map that never changes.
    pub fn initial(&self) -> Key {
        self.points[0].key
    }

    /// Replaces the key of the first segment.
    pub fn set_initial(&mut self, key: Key) {
        self.points[0].key = key;
    }

    /// `true` when one key holds for the whole song.
    pub fn is_constant(&self) -> bool {
        self.points.len() == 1
    }

    /// Inserts or replaces a key change.
    pub fn set_point(&mut self, tick: Ticks, key: Key) {
        let tick = tick.max_zero();
        match self.points.binary_search_by_key(&tick, |point| point.tick) {
            Ok(index) => self.points[index].key = key,
            Err(index) => self.points.insert(index, KeyPoint { tick, key }),
        }
    }

    /// Removes the key change at `tick`. The anchor at tick 0 cannot be removed.
    pub fn remove_point(&mut self, tick: Ticks) {
        if tick == Ticks::ZERO {
            return;
        }
        if let Ok(index) = self.points.binary_search_by_key(&tick, |point| point.tick) {
            self.points.remove(index);
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

    /// The key in force at `tick`.
    ///
    /// Total, like [`TempoMap::bpm_at`](crate::time::TempoMap::bpm_at): there is always a key, and
    /// a negative tick reads the first segment.
    pub fn key_at(&self, tick: Ticks) -> Key {
        self.points[self.segment_index(tick)].key
    }

    /// The key changes that fall in `from..to`.
    pub fn changes_in(&self, from: Ticks, to: Ticks) -> &[KeyPoint] {
        let (from, to) = ordered(from, to);
        let start = self.points.partition_point(|point| point.tick < from);
        let end = self.points.partition_point(|point| point.tick < to);
        &self.points[start..end]
    }
}

/// A chord change at a musical position.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChordPoint {
    /// Where the chord takes effect.
    pub tick: Ticks,
    /// The chord from here on, or `None` for a stretch with no harmony at all.
    ///
    /// The `None` is what makes clearing the middle of a song mean *cleared* rather than *the
    /// previous chord rings on through it*. A drum break and an unpitched intro are both this.
    #[serde(default)]
    pub chord: Option<Numeral>,
}

/// Chords over the timeline, each in force until the next.
///
/// Unlike [`KeyMap`] this may be empty, and [`Self::numeral_at`] may answer `None`. A project
/// nobody has written chords into has no chords, and inventing a `I` at tick 0 to make the lookup
/// total would be a lie the composer would then go and obey. The other invariants match: sorted,
/// no two points at the same position, none before tick 0.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "ChordMapRepr")]
pub struct ChordMap {
    points: Vec<ChordPoint>,
}

/// On-disk shape of a [`ChordMap`]. See `KeyMapRepr` for why the chord is read as text.
#[derive(Deserialize)]
struct ChordMapRepr {
    #[serde(default)]
    points: Vec<ChordPointRepr>,
}

#[derive(Deserialize)]
struct ChordPointRepr {
    tick: Ticks,
    #[serde(default)]
    chord: Option<String>,
}

impl From<ChordMapRepr> for ChordMap {
    fn from(repr: ChordMapRepr) -> Self {
        let points = repr
            .points
            .into_iter()
            .filter_map(|point| match point.chord {
                // A written chord that does not parse is dropped rather than defaulted: turning
                // `H7` into a silence is honest, turning it into a `I` would be an invention.
                Some(text) => Numeral::parse(&text).map(|chord| ChordPoint {
                    tick: point.tick,
                    chord: Some(chord),
                }),
                None => Some(ChordPoint {
                    tick: point.tick,
                    chord: None,
                }),
            })
            .collect();
        Self::new(points)
    }
}

impl ChordMap {
    /// Builds a timeline from arbitrary points, normalising order, duplicates and negative ticks.
    pub fn new(mut points: Vec<ChordPoint>) -> Self {
        for point in &mut points {
            point.tick = point.tick.max_zero();
        }
        points.sort_by_key(|point| point.tick);
        points.dedup_by_key(|point| point.tick);
        Self { points }
    }

    /// The chord changes, ordered by position.
    pub fn points(&self) -> &[ChordPoint] {
        &self.points
    }

    /// `true` when no chord has been written anywhere.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Where the last chord change sits, or [`Ticks::ZERO`] when nothing has been written.
    pub fn end(&self) -> Ticks {
        self.points.last().map_or(Ticks::ZERO, |point| point.tick)
    }

    /// Inserts or replaces a change. `None` ends the harmony from here.
    pub fn set_point(&mut self, tick: Ticks, chord: Option<Numeral>) {
        let tick = tick.max_zero();
        match self.points.binary_search_by_key(&tick, |point| point.tick) {
            Ok(index) => self.points[index].chord = chord,
            Err(index) => self.points.insert(index, ChordPoint { tick, chord }),
        }
    }

    /// Removes the change at `tick`, letting whatever preceded it run through.
    pub fn remove_point(&mut self, tick: Ticks) {
        if let Ok(index) = self.points.binary_search_by_key(&tick, |point| point.tick) {
            self.points.remove(index);
        }
    }

    /// The chord written for `tick`.
    ///
    /// `None` has three causes, and they are not distinguished because nothing needs to: no chord
    /// has been written at all, `tick` precedes the first one, or a cleared stretch is in force.
    pub fn numeral_at(&self, tick: Ticks) -> Option<Numeral> {
        let index = self
            .points
            .binary_search_by_key(&tick, |point| point.tick)
            .unwrap_or_else(|index| index.wrapping_sub(1));
        self.points.get(index).and_then(|point| point.chord)
    }

    /// Empties `from..to`, leaving what sounded at `to` still sounding there.
    ///
    /// A cleared stretch is a real thing rather than an absence, so this writes a `None` at `from`
    /// and restores the interrupted chord at `to`. Clearing the middle of a progression must not
    /// let the chord before the gap ring on through it.
    pub fn clear_range(&mut self, from: Ticks, to: Ticks) {
        let (from, to) = ordered(from.max_zero(), to.max_zero());
        if from == to {
            return;
        }
        let resumes = self.numeral_at(to);
        self.points
            .retain(|point| point.tick < from || point.tick >= to);
        // Only mark the gap when something would otherwise have sounded through it.
        if self.numeral_at(from).is_some() {
            self.set_point(from, None);
        }
        if resumes.is_some() && self.numeral_at(to) != resumes {
            self.set_point(to, resumes);
        }
    }
}

/// The key and the chords a song is written in, over time.
///
/// Held by the [`Project`](crate::Project) beside the tempo map, because it is the same kind of
/// thing: a property of the timeline that every track reads and no track owns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Harmony {
    /// The key over the timeline. Answers at every tick.
    ///
    /// `default` on the field as well as on [`Project::harmony`](crate::Project::harmony), because
    /// the outer default only covers a *missing* harmony and not a *partial* one.
    #[serde(default)]
    pub keys: KeyMap,
    /// The chords over the timeline. May be empty, and may have gaps.
    #[serde(default)]
    pub chords: ChordMap,
}

impl Harmony {
    /// A harmony in one key with nothing written in it yet.
    pub fn in_key(key: Key) -> Self {
        Self {
            keys: KeyMap::constant(key),
            chords: ChordMap::default(),
        }
    }

    /// The key in force at `tick`.
    pub fn key_at(&self, tick: Ticks) -> Key {
        self.keys.key_at(tick)
    }

    /// The numeral written at `tick`, if any.
    pub fn numeral_at(&self, tick: Ticks) -> Option<Numeral> {
        self.chords.numeral_at(tick)
    }

    /// The chord sounding at `tick`: the numeral there, read against the key there.
    pub fn chord_at(&self, tick: Ticks) -> Option<Chord> {
        self.chords
            .numeral_at(tick)
            .map(|numeral| numeral.chord_in(self.keys.key_at(tick)))
    }

    /// `true` when no chord has been written anywhere.
    pub fn is_empty(&self) -> bool {
        self.chords.is_empty()
    }

    /// Every chord sounding across `from..to`, clipped to that window.
    ///
    /// Cleared stretches produce no event, so the result tiles only the parts of the window that
    /// have harmony. Each event carries the key in force where it starts; a key change inside one
    /// chord splits it, because the second half is a different chord.
    ///
    /// Owned rather than an iterator because both callers need it that way: a paint closure has to
    /// capture `'static`, and the composer indexes.
    pub fn events_in(
        &self,
        from: Ticks,
        to: Ticks,
        signature: TimeSignature,
    ) -> Vec<HarmonicEvent> {
        let (from, to) = ordered(from.max_zero(), to.max_zero());
        if from == to {
            return Vec::new();
        }

        // Every position where either timeline changes, plus the window's own edges.
        let mut edges = vec![from, to];
        edges.extend(
            self.chords
                .points()
                .iter()
                .map(|point| point.tick)
                .filter(|tick| *tick > from && *tick < to),
        );
        edges.extend(
            self.keys
                .changes_in(from, to)
                .iter()
                .map(|point| point.tick)
                .filter(|tick| *tick > from),
        );
        edges.sort();
        edges.dedup();

        let mut events = Vec::new();
        for pair in edges.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            let Some(numeral) = self.chords.numeral_at(start) else {
                continue;
            };
            let key = self.keys.key_at(start);
            events.push(HarmonicEvent {
                numeral,
                chord: numeral.chord_in(key),
                key,
                start,
                length: end - start,
                bar: (signature.bar_of(start) - 1) as usize,
            });
        }
        events
    }

    /// Writes `chart`, repeated or truncated to `bars` bars, starting at `from`.
    ///
    /// Returns how many chords were written. Deliberately does not snap: a chart divides a bar
    /// musically, and a three-chord bar in 4/4 is three lots of 1280 ticks — not a multiple of any
    /// sensible editing grid, and rounding it would lose the bar line. A stamp is a division of a
    /// bar; a drag is an edit on the grid.
    ///
    /// Whatever sounded at the end of the stamped range keeps sounding there, so laying a
    /// progression over the first eight bars of a song does not silence the ninth.
    pub fn stamp(
        &mut self,
        chart: &Chart,
        from: Ticks,
        bars: usize,
        signature: TimeSignature,
    ) -> usize {
        let from = from.max_zero();
        let bar_ticks = signature.ticks_per_bar();
        let fitted = chart.fit_to(bars);
        let slots = fitted.positions(bar_ticks);
        let until = from + bar_ticks * bars as i64;

        self.chords.clear_range(from, until);
        for slot in &slots {
            self.chords.set_point(from + slot.start, Some(slot.numeral));
        }
        slots.len()
    }

    /// Empties the chords in `from..to`. The key timeline is untouched.
    pub fn clear(&mut self, from: Ticks, to: Ticks) {
        self.chords.clear_range(from, to);
    }
}

/// A range with its ends the right way round.
fn ordered(from: Ticks, to: Ticks) -> (Ticks, Ticks) {
    if from <= to { (from, to) } else { (to, from) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::TICKS_PER_QUARTER;

    fn key(text: &str) -> Key {
        Key::parse(text).unwrap()
    }

    fn numeral(text: &str) -> Numeral {
        Numeral::parse(text).unwrap()
    }

    /// 4/4, so one bar is 3840 ticks.
    fn four_four() -> TimeSignature {
        TimeSignature::new(4, 4)
    }

    fn bar(n: i64) -> Ticks {
        Ticks(3840 * n)
    }

    #[test]
    fn a_key_map_is_never_empty_and_always_starts_at_zero() {
        // The same sweep the tempo map gets: whatever order and whatever positions go in, the
        // invariants the binary search depends on come out.
        let positions = [-5000i64, -960, -1, 0, 1, 960, 5000];
        for a in positions {
            for b in positions {
                for c in positions {
                    let map = KeyMap::from_points(vec![
                        KeyPoint {
                            tick: Ticks(a),
                            key: key("C major"),
                        },
                        KeyPoint {
                            tick: Ticks(b),
                            key: key("D minor"),
                        },
                        KeyPoint {
                            tick: Ticks(c),
                            key: key("E dorian"),
                        },
                    ]);
                    let points = map.points();
                    assert!(!points.is_empty(), "{a} {b} {c}");
                    assert_eq!(points[0].tick, Ticks::ZERO, "{a} {b} {c}");
                    for pair in points.windows(2) {
                        assert!(pair[0].tick < pair[1].tick, "{a} {b} {c} not ascending");
                    }
                }
            }
        }
        assert_eq!(KeyMap::from_points(Vec::new()), KeyMap::default());
    }

    #[test]
    fn the_same_invariants_survive_deserialization() {
        let json = r#"{"points":[
            {"tick":5000,"key":"E dorian"},
            {"tick":-99,"key":"C major"},
            {"tick":960,"key":"D minor"}
        ]}"#;
        let map: KeyMap = serde_json::from_str(json).unwrap();
        assert_eq!(map.points()[0].tick, Ticks::ZERO);
        assert_eq!(map.points()[0].key, key("C major"));
        for pair in map.points().windows(2) {
            assert!(pair[0].tick < pair[1].tick);
        }
    }

    #[test]
    fn an_empty_key_map_on_disk_reads_as_the_default_rather_than_panicking() {
        // `key_at` indexes points[0]; a hand-edited file must not be able to make that panic.
        let map: KeyMap = serde_json::from_str(r#"{"points":[]}"#).unwrap();
        assert_eq!(map.key_at(Ticks::ZERO), key("C major"));
        assert_eq!(map.key_at(Ticks(-1)), key("C major"));

        let all_bad: KeyMap = serde_json::from_str(r#"{"points":[{"tick":0,"key":"H"}]}"#).unwrap();
        assert_eq!(all_bad.key_at(bar(4)), key("C major"));
    }

    #[test]
    fn a_key_change_takes_effect_where_it_is_written_and_not_before() {
        let mut keys = KeyMap::constant(key("C major"));
        keys.set_point(bar(8), key("Eb major"));
        assert_eq!(keys.key_at(bar(8) - Ticks(1)), key("C major"));
        assert_eq!(keys.key_at(bar(8)), key("Eb major"));
        assert_eq!(keys.key_at(bar(100)), key("Eb major"));
        assert!(!keys.is_constant());
    }

    #[test]
    fn the_key_at_tick_zero_cannot_be_removed() {
        let mut keys = KeyMap::constant(key("A minor"));
        keys.set_point(bar(4), key("C major"));
        keys.remove_point(Ticks::ZERO);
        assert_eq!(keys.key_at(Ticks::ZERO), key("A minor"), "the anchor stays");
        keys.remove_point(bar(4));
        assert!(keys.is_constant());
        assert_eq!(keys.key_at(bar(4)), key("A minor"));
    }

    #[test]
    fn a_chord_timeline_may_be_empty_and_says_so() {
        let mut chords = ChordMap::default();
        assert!(chords.is_empty());
        assert_eq!(chords.numeral_at(Ticks::ZERO), None);

        chords.set_point(bar(2), Some(numeral("IV")));
        assert!(!chords.is_empty());
        assert_eq!(chords.numeral_at(bar(1)), None, "before the first chord");
        assert_eq!(chords.numeral_at(bar(2)), Some(numeral("IV")));
        assert_eq!(chords.numeral_at(bar(9)), Some(numeral("IV")), "runs on");

        chords.set_point(bar(4), None);
        assert_eq!(chords.numeral_at(bar(9)), None, "a cleared stretch is real");
    }

    #[test]
    fn clearing_a_range_leaves_what_sounded_after_it_sounding() {
        let mut harmony = Harmony::in_key(key("C major"));
        let chart = Chart::parse("| I | V | vi | IV |").unwrap();
        harmony.stamp(&chart, Ticks::ZERO, 16, four_four());
        assert_eq!(harmony.numeral_at(bar(12)), Some(numeral("I")));

        // Bars 9..13 counted from one are ticks 8..12 counted from zero.
        harmony.clear(bar(8), bar(12));
        assert_eq!(harmony.numeral_at(bar(7)), Some(numeral("IV")), "before");
        assert_eq!(harmony.numeral_at(bar(8)), None, "the gap is a gap");
        assert_eq!(harmony.numeral_at(bar(11)), None, "all the way through");
        assert_eq!(
            harmony.numeral_at(bar(12)),
            Some(numeral("I")),
            "and the song resumes on the far side"
        );
    }

    #[test]
    fn every_chord_of_a_range_is_covered_once_with_no_gap_and_no_overlap() {
        let mut harmony = Harmony::in_key(key("C major"));
        let chart = Chart::parse("| I | V | vi | IV |").unwrap();
        harmony.stamp(&chart, Ticks::ZERO, 8, four_four());

        let events = harmony.events_in(Ticks::ZERO, bar(8), four_four());
        assert_eq!(events.len(), 8);
        let mut cursor = Ticks::ZERO;
        for event in &events {
            assert_eq!(event.start, cursor, "a gap or an overlap");
            cursor = event.end();
        }
        assert_eq!(cursor, bar(8), "the last chord ends on the bar line");
    }

    #[test]
    fn a_range_is_clipped_to_the_window_it_was_asked_for() {
        let mut harmony = Harmony::in_key(key("C major"));
        harmony.stamp(
            &Chart::parse("| I | V |").unwrap(),
            Ticks::ZERO,
            8,
            four_four(),
        );

        // Half of bar 3 to half of bar 5, counting bars from one.
        let from = bar(2) + Ticks(1920);
        let to = bar(4) + Ticks(1920);
        let events = harmony.events_in(from, to, four_four());
        assert_eq!(events.first().unwrap().start, from);
        assert_eq!(events.last().unwrap().end(), to);
    }

    #[test]
    fn a_chord_is_the_numeral_read_against_the_key_in_force_where_it_sits() {
        let mut harmony = Harmony::in_key(key("C major"));
        harmony.stamp(
            &Chart::parse("| I | IV |").unwrap(),
            Ticks::ZERO,
            8,
            four_four(),
        );
        assert_eq!(harmony.chord_at(Ticks::ZERO).unwrap().to_string(), "C");
        assert_eq!(harmony.chord_at(bar(1)).unwrap().to_string(), "F");

        // A modulation halfway through reharmonises the rest without touching a single chord.
        // `Chord` always spells with sharps, so Eb major's tonic reads back as D#; that is the
        // chord's own display convention and not something the harmony decides.
        harmony.keys.set_point(bar(4), key("Eb major"));
        assert_eq!(harmony.chord_at(bar(4)).unwrap().to_string(), "D#");
        assert_eq!(harmony.chord_at(bar(5)).unwrap().to_string(), "G#");
        assert_eq!(
            harmony.numeral_at(bar(5)),
            harmony.numeral_at(bar(1)),
            "the progression itself is unchanged"
        );
    }

    #[test]
    fn a_key_change_inside_a_chord_splits_it() {
        let mut harmony = Harmony::in_key(key("C major"));
        harmony.chords.set_point(Ticks::ZERO, Some(numeral("I")));
        harmony.keys.set_point(bar(1), key("D major"));

        let events = harmony.events_in(Ticks::ZERO, bar(2), four_four());
        assert_eq!(events.len(), 2, "one chord, two keys, two events");
        assert_eq!(events[0].chord.to_string(), "C");
        assert_eq!(events[1].chord.to_string(), "D");
        assert_eq!(events[0].end(), events[1].start, "and they meet exactly");
    }

    #[test]
    fn a_stamped_progression_ends_exactly_on_a_bar_line() {
        let mut harmony = Harmony::in_key(key("C major"));
        // Three chords in one bar of 4/4: 3840 does not divide by three into a grid position, and
        // must not be rounded to one.
        let written = harmony.stamp(
            &Chart::parse("| I V vi |").unwrap(),
            Ticks::ZERO,
            1,
            four_four(),
        );
        assert_eq!(written, 3);

        let events = harmony.events_in(Ticks::ZERO, bar(1), four_four());
        let total: i64 = events.iter().map(|event| event.length.raw()).sum();
        assert_eq!(total, 3840, "the bar adds up exactly");
        assert_eq!(events[0].length.raw(), 1280);
        assert_eq!(events[2].end(), bar(1));
    }

    #[test]
    fn stamping_leaves_the_music_after_the_stamp_alone() {
        let mut harmony = Harmony::in_key(key("C major"));
        harmony.chords.set_point(bar(8), Some(numeral("bVII")));
        harmony.stamp(
            &Chart::parse("| I | V |").unwrap(),
            Ticks::ZERO,
            8,
            four_four(),
        );
        assert_eq!(harmony.numeral_at(bar(8)), Some(numeral("bVII")));
        assert_eq!(harmony.numeral_at(bar(7)), Some(numeral("V")));
    }

    #[test]
    fn transposing_the_key_transposes_the_whole_progression() {
        let mut harmony = Harmony::in_key(key("C major"));
        harmony.stamp(
            &Chart::parse("| I | V | vi | IV |").unwrap(),
            Ticks::ZERO,
            4,
            four_four(),
        );
        let before: Vec<String> = harmony
            .events_in(Ticks::ZERO, bar(4), four_four())
            .iter()
            .map(|event| event.chord.to_string())
            .collect();
        assert_eq!(before, ["C", "G", "Am", "F"]);

        harmony
            .keys
            .set_initial(harmony.keys.initial().transposed(2));
        let after: Vec<String> = harmony
            .events_in(Ticks::ZERO, bar(4), four_four())
            .iter()
            .map(|event| event.chord.to_string())
            .collect();
        assert_eq!(after, ["D", "A", "Bm", "G"]);
    }

    #[test]
    fn a_position_is_built_from_a_bar_and_a_beat_rather_than_from_a_tick() {
        let signature = four_four();
        assert_eq!(signature.position(1, 1.0), Ticks::ZERO);
        assert_eq!(signature.position(2, 1.0), bar(1));
        assert_eq!(signature.position(2, 3.0), bar(1) + Ticks(2 * 960));
        // Several chords in one bar is completely normal, and the off-beat has to be sayable.
        assert_eq!(signature.position(1, 2.5), Ticks(1440));
        assert_eq!(signature.bar_of(Ticks(3839)), 1);
        assert_eq!(signature.bar_of(bar(1)), 2);
        assert_eq!(signature.bar_start(3), bar(2));

        let three_four = TimeSignature::new(3, 4);
        assert_eq!(three_four.position(2, 1.0), Ticks(3 * TICKS_PER_QUARTER));
    }

    #[test]
    fn the_harmony_wire_shape_does_not_drift() {
        let mut harmony = Harmony::in_key(key("Bb minor"));
        harmony
            .chords
            .set_point(Ticks::ZERO, Some(numeral("IVmaj7")));
        harmony.chords.set_point(bar(1), Some(numeral("III7")));
        harmony.chords.set_point(bar(2), None);

        let json = serde_json::to_string(&harmony).unwrap();
        assert_eq!(
            json,
            r#"{"keys":{"points":[{"tick":0,"key":"Bb minor"}]},"chords":{"points":[{"tick":0,"chord":"IVmaj7"},{"tick":3840,"chord":"III7"},{"tick":7680,"chord":null}]}}"#
        );
        assert_eq!(serde_json::from_str::<Harmony>(&json).unwrap(), harmony);
    }

    #[test]
    fn a_partial_harmony_object_still_opens() {
        // `serde(default)` on the field covers a missing `harmony`; it does not cover a present
        // one that is missing half its contents.
        let only_chords: Harmony =
            serde_json::from_str(r#"{"chords":{"points":[{"tick":0,"chord":"I"}]}}"#).unwrap();
        assert_eq!(only_chords.key_at(Ticks::ZERO), key("C major"));
        assert_eq!(only_chords.numeral_at(Ticks::ZERO), Some(numeral("I")));

        let neither: Harmony = serde_json::from_str("{}").unwrap();
        assert_eq!(neither, Harmony::default());
        assert!(neither.is_empty());
    }

    #[test]
    fn a_corrupt_chord_costs_one_chord_and_not_the_project() {
        let json = r#"{"keys":{"points":[{"tick":0,"key":"C major"}]},
                       "chords":{"points":[
                          {"tick":0,"chord":"I"},
                          {"tick":3840,"chord":"H7"},
                          {"tick":7680,"chord":"V"}]}}"#;
        let harmony: Harmony = serde_json::from_str(json).unwrap();
        assert_eq!(harmony.chords.points().len(), 2, "the bad one is dropped");
        assert_eq!(harmony.numeral_at(Ticks::ZERO), Some(numeral("I")));
        assert_eq!(
            harmony.numeral_at(bar(1)),
            Some(numeral("I")),
            "and the chord before it simply runs on"
        );
        assert_eq!(harmony.numeral_at(bar(2)), Some(numeral("V")));
    }
}
