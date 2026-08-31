//! The pitch ornaments a note carries: a scoop into it, a fall off it, a vibrato across it.
//!
//! Each one is a handful of numbers a person set, stored on the [`Note`](super::Note) so the
//! gesture travels, saves and re-renders with it — the same reasoning that put the lyric there.
//! What the numbers *mean* — the exact curve each ornament adds to the note's pitch — is not
//! decided here: [`auris_vocal`](https://docs.rs/auris-vocal)'s `ornament_offset` is the one
//! implementation of that shape, read by the frames a voice model is fed and by the editor
//! drawing and grabbing the same curve, because two implementations of one contour would drift.
//!
//! The defaults are taken from measurement rather than taste: singers' vibrato clusters around
//! 4–6.7 Hz at a few tenths of a semitone of sway, and a scoop is typically about a semitone
//! over roughly a tenth of a second. Each `Default` is the ornament a menu drops onto a note,
//! already sounding plausible before any handle is touched.

use serde::{Deserialize, Serialize};

/// A rise into the note from below — しゃくり.
///
/// The pitch starts `depth` semitones under the note and settles onto it over the first
/// `seconds` of the note, easing at both ends.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scoop {
    /// Semitones below the note the rise starts from.
    pub depth: f32,
    /// Seconds the rise takes, from the note's start.
    pub seconds: f64,
}

impl Default for Scoop {
    fn default() -> Self {
        Self {
            depth: 1.0,
            seconds: 0.10,
        }
    }
}

/// A drop away at the note's end — フォール.
///
/// The pitch leaves the note over its last `seconds` and lands `depth` semitones under it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fall {
    /// Semitones below the note the drop lands at.
    pub depth: f32,
    /// Seconds the drop takes, ending where the note does.
    pub seconds: f64,
}

impl Default for Fall {
    fn default() -> Self {
        Self {
            depth: 2.0,
            seconds: 0.15,
        }
    }
}

/// A periodic sway around the note's pitch once it has settled.
///
/// A sinusoid at `rate`, `depth` semitones either way, held off for `delay` seconds from the
/// note's start and growing to full sway over `fade_in` more — the constant-frequency,
/// amplitude-faded model the synthesis literature has used since the nineties, because it is
/// perceptually indistinguishable from the measured thing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vibrato {
    /// Semitones of sway either side of the note.
    pub depth: f32,
    /// Cycles per second.
    pub rate: f32,
    /// Seconds after the note's start before the sway begins.
    pub delay: f64,
    /// Seconds the sway takes to reach full depth once begun.
    pub fade_in: f64,
}

impl Default for Vibrato {
    fn default() -> Self {
        Self {
            depth: 0.35,
            rate: 5.8,
            delay: 0.3,
            fade_in: 0.3,
        }
    }
}
