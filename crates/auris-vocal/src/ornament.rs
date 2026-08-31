//! The one implementation of what a note's pitch ornaments sound like.
//!
//! A [`Scoop`], [`Fall`] or [`Vibrato`] is a handful of numbers stored on the note; this module
//! turns them into semitones at a moment in time. [`ornament_offset`] is public and pure for
//! the same reason [`phoneme_layout`](crate::phoneme_layout) is: the frames a voice model is
//! fed, the curve an editor draws and the handle a hand grabs must all read the same contour,
//! and two implementations of it would drift.
//!
//! The shapes themselves:
//!
//! * **Scoop** rises from `depth` semitones under the note onto it over the first `seconds`,
//!   along a half-cosine — flat at both ends, so the contour leaves the scoop with no corner
//!   for a voice to catch on. The S-curve UTAU defaults to, for the same reason.
//! * **Fall** is the mirror: the last `seconds` leave the note along a half-cosine and land
//!   `depth` under it.
//! * **Vibrato** is a sinusoid at `rate`, `depth` either way, silent for `delay` seconds and
//!   growing linearly to full sway over `fade_in` more. A constant-frequency sinusoid with a
//!   faded onset is the standard model in the synthesis literature because listeners cannot
//!   tell it from the measured thing.
//!
//! A scoop or fall asking for more than half the note is capped at half — the same cap the
//! consonant rule wears, and what keeps the two from ever overlapping on a short note.

use std::f64::consts::PI;

use auris_core::{Fall, Scoop, Vibrato};

/// Semitones the ornaments move the pitch, `t` seconds into a note `length` seconds long.
///
/// Zero outside the note, zero where no ornament reaches, and exactly zero with none set —
/// which is what keeps every un-ornamented note's frames identical to what they were before
/// ornaments existed. Degenerate numbers (a non-positive or non-finite span or rate) switch
/// that ornament off rather than propagating.
pub fn ornament_offset(
    scoop: Option<&Scoop>,
    fall: Option<&Fall>,
    vibrato: Option<&Vibrato>,
    t: f64,
    length: f64,
) -> f32 {
    if !t.is_finite() || !length.is_finite() || length <= 0.0 || t < 0.0 || t >= length {
        return 0.0;
    }
    let mut offset = 0.0f64;

    if let Some(scoop) = scoop {
        let seconds = ornament_reach(scoop.seconds, length);
        if t < seconds {
            let ease = (1.0 + (PI * t / seconds).cos()) / 2.0;
            offset -= f64::from(scoop.depth) * ease;
        }
    }

    if let Some(fall) = fall {
        let seconds = ornament_reach(fall.seconds, length);
        let from = length - seconds;
        if seconds > 0.0 && t >= from {
            let ease = (1.0 - (PI * (t - from) / seconds).cos()) / 2.0;
            offset -= f64::from(fall.depth) * ease;
        }
    }

    if let Some(vibrato) = vibrato {
        let delay = vibrato.delay.max(0.0);
        if t >= delay && vibrato.rate.is_finite() && vibrato.rate > 0.0 {
            let grown = match vibrato.fade_in.is_finite() && vibrato.fade_in > 0.0 {
                true => ((t - delay) / vibrato.fade_in).clamp(0.0, 1.0),
                false => 1.0,
            };
            let phase = 2.0 * PI * f64::from(vibrato.rate) * (t - delay);
            offset += f64::from(vibrato.depth) * grown * phase.sin();
        }
    }

    offset as f32
}

/// A scoop or fall's span, capped at half the note and switched off when degenerate.
///
/// Public for the editor's sake: a handle drawn on the gesture has to sit where the gesture
/// audibly reaches, not where an over-long span asked to, and a second copy of this cap
/// would drift from the one the frames obey.
pub fn ornament_reach(seconds: f64, length: f64) -> f64 {
    match seconds.is_finite() && seconds > 0.0 {
        true => seconds.min(length / 2.0),
        false => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scooped(depth: f32, seconds: f64) -> Option<Scoop> {
        Some(Scoop { depth, seconds })
    }

    fn falling(depth: f32, seconds: f64) -> Option<Fall> {
        Some(Fall { depth, seconds })
    }

    #[test]
    fn no_ornament_is_exactly_no_offset() {
        assert_eq!(ornament_offset(None, None, None, 0.5, 1.0), 0.0);
    }

    #[test]
    fn a_scoop_starts_a_depth_under_and_settles_onto_the_note() {
        let scoop = scooped(1.0, 0.2);
        // The very start is the full depth under.
        assert!((ornament_offset(scoop.as_ref(), None, None, 0.0, 1.0) + 1.0).abs() < 1e-6);
        // Halfway up the rise is halfway home — the cosine's midpoint.
        assert!((ornament_offset(scoop.as_ref(), None, None, 0.1, 1.0) + 0.5).abs() < 1e-6);
        // Past the rise the note is itself.
        assert_eq!(ornament_offset(scoop.as_ref(), None, None, 0.2, 1.0), 0.0);
        assert_eq!(ornament_offset(scoop.as_ref(), None, None, 0.7, 1.0), 0.0);
    }

    #[test]
    fn a_fall_leaves_the_note_and_lands_a_depth_under() {
        let fall = falling(2.0, 0.2);
        // Before the drop the note is itself.
        assert_eq!(ornament_offset(None, fall.as_ref(), None, 0.5, 1.0), 0.0);
        // Halfway down is half the depth.
        assert!((ornament_offset(None, fall.as_ref(), None, 0.9, 1.0) + 1.0).abs() < 1e-6);
        // The last sampled instants sit at the floor of the drop.
        assert!((ornament_offset(None, fall.as_ref(), None, 0.9999, 1.0) + 2.0).abs() < 1e-3);
    }

    #[test]
    fn a_span_past_half_the_note_is_capped_at_half() {
        // A 2-second scoop on a 1-second note reaches the note's pitch at 0.5, not never.
        let scoop = scooped(1.0, 2.0);
        assert_eq!(ornament_offset(scoop.as_ref(), None, None, 0.5, 1.0), 0.0);
        assert!(ornament_offset(scoop.as_ref(), None, None, 0.25, 1.0) < 0.0);
        // Which is also what keeps a capped scoop and fall from ever overlapping.
        let fall = falling(1.0, 2.0);
        let both = ornament_offset(scoop.as_ref(), fall.as_ref(), None, 0.25, 1.0);
        assert_eq!(
            both,
            ornament_offset(scoop.as_ref(), None, None, 0.25, 1.0),
            "the first half is the scoop's alone"
        );
    }

    #[test]
    fn a_vibrato_sways_at_its_rate_once_grown() {
        let vibrato = Some(Vibrato {
            depth: 0.5,
            rate: 5.0,
            delay: 0.0,
            fade_in: 0.0,
        });
        // A quarter period into a 5 Hz cycle is the crest.
        assert!((ornament_offset(None, None, vibrato.as_ref(), 0.05, 2.0) - 0.5).abs() < 1e-6);
        // Half a period is the zero crossing.
        assert!(ornament_offset(None, None, vibrato.as_ref(), 0.1, 2.0).abs() < 1e-6);
        // Three quarters is the trough.
        assert!((ornament_offset(None, None, vibrato.as_ref(), 0.15, 2.0) + 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_vibrato_waits_out_its_delay_and_grows_through_its_fade() {
        let vibrato = Some(Vibrato {
            depth: 0.5,
            rate: 5.0,
            delay: 0.4,
            fade_in: 0.2,
        });
        // Before the delay: nothing.
        assert_eq!(ornament_offset(None, None, vibrato.as_ref(), 0.3, 2.0), 0.0);
        // A quarter period past the delay the sway is at its crest, but the fade is only
        // a quarter grown: 0.5 × 0.25.
        assert!((ornament_offset(None, None, vibrato.as_ref(), 0.45, 2.0) - 0.125).abs() < 1e-6);
        // Well past the fade the crest is the full depth.
        assert!((ornament_offset(None, None, vibrato.as_ref(), 1.45, 2.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn degenerate_numbers_switch_an_ornament_off() {
        assert_eq!(
            ornament_offset(scooped(1.0, f64::NAN).as_ref(), None, None, 0.0, 1.0),
            0.0
        );
        assert_eq!(
            ornament_offset(scooped(1.0, -1.0).as_ref(), None, None, 0.0, 1.0),
            0.0
        );
        let still = Some(Vibrato {
            depth: 0.5,
            rate: 0.0,
            delay: 0.0,
            fade_in: 0.0,
        });
        assert_eq!(ornament_offset(None, None, still.as_ref(), 0.25, 1.0), 0.0);
    }

    #[test]
    fn outside_the_note_there_is_nothing() {
        let scoop = scooped(1.0, 0.2);
        assert_eq!(ornament_offset(scoop.as_ref(), None, None, -0.1, 1.0), 0.0);
        assert_eq!(ornament_offset(scoop.as_ref(), None, None, 1.0, 1.0), 0.0);
        assert_eq!(ornament_offset(scoop.as_ref(), None, None, 0.0, 0.0), 0.0);
    }
}
