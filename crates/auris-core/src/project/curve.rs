//! The curves a clip carries: its pitch bend, and any controller written across it.
//!
//! One file because they are one shape. A curve is a list of [`CurvePoint`]s — a value at an
//! instant, straight lines between them, held flat outside them — and [`ClipCurve`] is the one
//! parameter that says which of a clip's curves is meant. Drawing, dragging, scheduling and
//! writing a MIDI file all read that parameter rather than being written twice, which is what
//! stops them quietly disagreeing.
//!
//! What the number *means* is the curve's business and not a point's: [`BEND_LIMIT`] is
//! semitones either side of the note, [`CONTROLLER_LIMIT`] is how far up a controller goes, and
//! both are read through [`ClipCurve::range`].
//!
//! A **controller is named by its MIDI number** rather than by a variant per kind. The wheel is
//! not special — it is controller 1 — and a clip that can hold 1 can hold 11 and 64 through the
//! same code, which is the whole reason an expression pedal reaches an instrument at all.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::BTreeSet;

use crate::plugin::CC_MODULATION;
use crate::time::Ticks;

/// One point on a curve written across a clip.
///
/// Shared by every curve a clip carries, because they are the same shape and the same rules: a
/// value at an instant, straight lines between the points, held flat outside them. What the number
/// *means* is the curve's business and not this one's.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurvePoint {
    /// Where it sits, measured from the clip's own start.
    pub at: Ticks,
    /// What the curve reads there — semitones on a bend, 0 to 1 on a controller.
    #[serde(
        serialize_with = "serialize_finite_value",
        deserialize_with = "deserialize_finite_value"
    )]
    pub value: f32,
}

fn serialize_finite_value<S>(value: &f32, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if value.is_finite() {
        serializer.serialize_f32(*value)
    } else {
        Err(serde::ser::Error::custom("curve values must be finite"))
    }
}

fn deserialize_finite_value<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    let value = f32::deserialize(deserializer)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(de::Error::custom("curve values must be finite"))
    }
}

/// Which of a clip's curves is meant.
///
/// They are the same shape and are drawn, dragged, scheduled and written to a MIDI file by the
/// same code; this is the one parameter that tells them apart, so that there is exactly one
/// copy of each of those and no chance of them disagreeing.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ClipCurve {
    /// The pitch bend, in semitones either side of the note.
    Bend,
    /// A MIDI controller, from nothing to all the way up.
    ///
    /// The number is the wire's own: 1 is the modulation wheel, 11 expression, 64 the sustain
    /// pedal. Nothing in this crate reads it — what a controller *does* is the instrument's
    /// business, and an instrument answers the handful it knows and ignores the rest.
    Controller(u8),
}

impl ClipCurve {
    /// The modulation wheel: controller 1.
    ///
    /// Spelt out because it is the one controller the application sends on its own — the musical
    /// typing wheel and the audition path both do — and a bare `Controller(1)` at those call
    /// sites would be the same magic number in three crates.
    pub const MODULATION: ClipCurve = ClipCurve::Controller(CC_MODULATION);

    /// The controller number, when this is a controller rather than the bend.
    pub fn controller(self) -> Option<u8> {
        match self {
            ClipCurve::Bend => None,
            ClipCurve::Controller(number) => Some(number),
        }
    }

    /// The furthest from zero a point on this curve may be written.
    pub fn limit(self) -> f32 {
        match self {
            ClipCurve::Bend => BEND_LIMIT,
            ClipCurve::Controller(_) => CONTROLLER_LIMIT,
        }
    }

    /// Whether the curve goes below zero.
    ///
    /// A bend does — the whole point of one is that it goes both ways — and a controller does
    /// not: it is up or it is down, and there is nothing below the bottom of its travel.
    pub fn is_bipolar(self) -> bool {
        matches!(self, ClipCurve::Bend)
    }

    /// The lowest and highest a point may be written at.
    pub fn range(self) -> (f32, f32) {
        match self.is_bipolar() {
            true => (-self.limit(), self.limit()),
            false => (0.0, self.limit()),
        }
    }
}

/// How far a bend may be written, either way.
///
/// An octave. MIDI's own default range is two semitones and a hardware synth has to be told
/// otherwise, but [`NoteEvent::PitchBend`](crate::NoteEvent::PitchBend) carries *semitones* rather
/// than a fourteen-bit fraction, so nothing here has to agree with anything about a range — and a
/// dive of an octave is a sound people want.
pub const BEND_LIMIT: f32 = 12.0;

/// How far a controller goes.
///
/// One, because that is what [`NoteEvent::Controller`](crate::NoteEvent::Controller) carries and
/// what a controller is: all the way up, or somewhere below it. The seven bits MIDI spends on it
/// are a detail of the wire that stops at [`auris_io`](https://docs.rs/auris-io).
pub const CONTROLLER_LIMIT: f32 = 1.0;

/// How finely a curve is sampled into events.
///
/// A ninety-sixth note, which at 120 bpm is about twenty milliseconds: fine enough that a slide
/// sounds continuous and coarse enough that a bar of it is a few dozen events rather than a few
/// thousand. Only the stretch a curve was actually written over is sampled at all.
pub const CURVE_STEP: Ticks = Ticks(crate::time::TICKS_PER_QUARTER / 24);

/// What a curve reads at `at`: straight lines between its points, flat outside them.
///
/// Flat rather than interpolated towards nothing, because that is what is *heard*: a curve is
/// channel state, so before its first point the instrument is still holding whatever it was last
/// told, and after its last point it holds that. A line drawn only between the ends would show a
/// slide starting somewhere it does not.
pub fn curve_at(points: &[CurvePoint], at: Ticks) -> f32 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return 0.0;
    };
    if at <= first.at {
        return first.value;
    }
    if at >= last.at {
        return last.value;
    }
    // The pair `at` falls between. A handful of points per clip, so a walk is the whole of it.
    let (from, to) = match points.windows(2).find(|pair| at < pair[1].at) {
        Some(pair) => (pair[0], pair[1]),
        None => (*first, *last),
    };
    let span = (to.at - from.at).raw().max(1) as f32;
    let through = (at - from.at).raw() as f32 / span;
    from.value + (to.value - from.value) * through
}

/// A curve sampled into the `(tick, value)` pairs an instrument reads, from the clip's own start.
///
/// Every `step` across the stretch the curve was written over, plus the points themselves so a
/// corner lands exactly where it was drawn. An empty curve produces nothing at all, which is what
/// keeps this off every project that has never used one.
///
/// It **ends at zero** whenever the curve does not. A curve is channel state that an instrument
/// holds until it is told otherwise, so a clip that finishes two semitones sharp — or with the
/// wheel still up — would carry that into everything after it, including a clip somebody else
/// wrote on the far side of a gap, with nothing on screen to say why.
pub fn curve_events(points: &[CurvePoint], length: Ticks, step: Ticks) -> Vec<(Ticks, f32)> {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Vec::new();
    };
    let step = Ticks(step.raw().max(1));
    let mut ticks = BTreeSet::new();
    if first.at > Ticks::ZERO {
        // `curve_at` holds the first value backwards. Schedule that same state at the clip's
        // beginning rather than leaving the instrument at a previous clip's value until the
        // first drawn point arrives.
        ticks.insert(Ticks::ZERO);
    }
    let mut at = first.at.max_zero();
    while at < last.at && at <= length {
        ticks.insert(at);
        at += step;
    }
    for point in points {
        if point.at >= Ticks::ZERO && point.at <= length {
            ticks.insert(point.at);
        }
    }
    let value_at_end = curve_at(points, length);
    let mut out: Vec<(Ticks, f32)> = ticks
        .into_iter()
        .map(|at| (at, curve_at(points, at)))
        .collect();
    if value_at_end != 0.0 {
        out.push((length, 0.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::CONTROLLER_MAX;
    use crate::project::{ClipId, MidiClip};

    fn bent(points: &[(i64, f32)], length: i64) -> MidiClip {
        MidiClip {
            bend: points
                .iter()
                .map(|(at, semitones)| CurvePoint {
                    at: Ticks(*at),
                    value: *semitones,
                })
                .collect(),
            ..MidiClip::new(ClipId(1), "bent", Ticks::ZERO, Ticks(length))
        }
    }

    #[test]
    fn a_curve_point_cannot_cross_the_file_boundary_with_a_non_finite_value() {
        let point = CurvePoint {
            at: Ticks::ZERO,
            value: f32::NAN,
        };
        assert!(serde_json::to_string(&point).is_err());

        let infinite = r#"{"at":0,"value":1e40}"#;
        assert!(serde_json::from_str::<CurvePoint>(infinite).is_err());
    }

    #[test]
    fn a_bend_runs_in_straight_lines_and_holds_flat_outside_itself() {
        // The rule an automation lane already follows, and for the same reason: a curve makes a
        // claim about the stretch it was written over and none at all about the rest.
        let clip = bent(&[(480, 0.0), (960, 2.0)], 1920);
        assert_eq!(
            clip.curve_at(ClipCurve::Bend, Ticks(0)),
            0.0,
            "flat before the first point"
        );
        assert_eq!(clip.curve_at(ClipCurve::Bend, Ticks(480)), 0.0);
        assert!(
            (clip.curve_at(ClipCurve::Bend, Ticks(720)) - 1.0).abs() < 0.001,
            "halfway up"
        );
        assert_eq!(clip.curve_at(ClipCurve::Bend, Ticks(960)), 2.0);
        assert_eq!(
            clip.curve_at(ClipCurve::Bend, Ticks(1900)),
            2.0,
            "and flat after the last"
        );

        // A clip nobody has bent is not bent, which is what keeps this off every project that has
        // never used one.
        assert_eq!(bent(&[], 1920).curve_at(ClipCurve::Bend, Ticks(500)), 0.0);
        assert!(
            bent(&[], 1920)
                .curve_events(ClipCurve::Bend, CURVE_STEP)
                .is_empty()
        );
    }

    #[test]
    fn a_bend_comes_back_to_nothing_before_the_clip_ends() {
        // A bend is channel state an instrument holds until it is told otherwise. A clip that
        // finished two semitones sharp would detune everything after it — including a clip
        // somebody else wrote, on the far side of a gap, with nothing on screen to say why.
        let events = bent(&[(0, 0.0), (960, 2.0)], 1920).curve_events(ClipCurve::Bend, CURVE_STEP);
        let (last_at, last_value) = *events.last().expect("it was bent");
        assert_eq!(last_value, 0.0, "the bend was left hanging");
        assert_eq!(last_at, Ticks(1920), "and it is released at the clip's end");

        // A curve that already ends at nothing needs no such release.
        let settled = bent(&[(0, 2.0), (960, 0.0)], 1920).curve_events(ClipCurve::Bend, CURVE_STEP);
        assert_eq!(settled.last().map(|(at, _)| *at), Some(Ticks(960)));

        // Nothing is scheduled past the window the notes were cut to.
        let past = bent(&[(0, 1.0), (4000, 2.0)], 960).curve_events(ClipCurve::Bend, CURVE_STEP);
        assert!(
            past.iter().all(|(at, _)| *at <= Ticks(960)),
            "a point ran past the clip: {past:?}"
        );

        // The raw last point is zero, but at the truncated boundary the line has not reached it.
        let truncated = bent(&[(0, 0.0), (100, 1.0), (2000, 0.0)], 500)
            .curve_events(ClipCurve::Bend, CURVE_STEP);
        assert_eq!(truncated.last(), Some(&(Ticks(500), 0.0)));
    }

    #[test]
    fn a_curve_whose_first_point_is_late_holds_that_value_from_the_clip_start() {
        let events = bent(&[(480, 2.0)], 1_920).curve_events(ClipCurve::Bend, CURVE_STEP);
        assert_eq!(
            events,
            [(Ticks::ZERO, 2.0), (Ticks(480), 2.0), (Ticks(1_920), 0.0)]
        );
    }

    #[test]
    fn a_bend_is_sampled_finely_enough_to_hear_as_a_slide() {
        // Every step across the stretch the curve covers, so a rise sounds continuous rather than
        // as a staircase — and the corners land exactly where they were drawn.
        let events = bent(&[(0, 0.0), (960, 2.0)], 1920).curve_events(ClipCurve::Bend, CURVE_STEP);
        let across = events.iter().filter(|(at, _)| *at <= Ticks(960)).count();
        assert!(across >= 20, "half a bar gave only {across} events");
        assert!(
            events
                .iter()
                .any(|(at, value)| *at == Ticks(960) && *value == 2.0)
        );
        // In time order, which is what the scheduler sorts against.
        for pair in events.windows(2) {
            assert!(pair[0].0 <= pair[1].0, "{events:?} goes back in time");
        }
    }

    #[test]
    fn every_drawn_corner_is_emitted_even_when_it_is_off_the_sampling_grid() {
        let events = bent(&[(0, 0.0), (505, 3.0), (1000, 0.0)], 1000)
            .curve_events(ClipCurve::Bend, CURVE_STEP);

        assert!(
            events.contains(&(Ticks(505), 3.0)),
            "the drawn peak was rounded away: {events:?}"
        );
    }

    #[test]
    fn a_controller_is_named_by_its_number_and_the_wheel_is_one_of_them() {
        // The wheel is not a kind of its own: it is controller 1, and the constant exists so that
        // the crates which send it do not each spell the number out.
        assert_eq!(ClipCurve::MODULATION, ClipCurve::Controller(1));
        assert_eq!(ClipCurve::MODULATION.controller(), Some(CC_MODULATION));
        assert_eq!(ClipCurve::Bend.controller(), None);

        // Every controller is read the same way, whichever it is: up from nothing, never below.
        for number in [CC_MODULATION, 11, 64, CONTROLLER_MAX] {
            let curve = ClipCurve::Controller(number);
            assert!(!curve.is_bipolar(), "controller {number} went bipolar");
            assert_eq!(curve.range(), (0.0, CONTROLLER_LIMIT));
        }
        // A bend goes both ways, which is the whole point of one.
        assert_eq!(ClipCurve::Bend.range(), (-BEND_LIMIT, BEND_LIMIT));
    }
}
