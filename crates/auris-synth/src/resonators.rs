//! Effects made of the physical instruments' bodies: something to put any track through.
//!
//! An instrument's body is a resonator waiting for a signal, and nothing in it cares whether the
//! signal is a string's pull at the bridge or a recording. Two of them are offered as effects:
//!
//! * [`BodyResonance`] puts a track through a box — a guitar's, a violin's or a cello's, a
//!   piano's soundboard, a dulcimer's, or a speaker cabinet — using the same mode banks the
//!   physical instruments sound through.
//! * [`SympatheticStrings`] hangs a bank of undamped strings beside the track, tuned to the
//!   song's chords or key, so that whatever is played in tune sets them ringing the way a sitar's
//!   or a viola d'amore's sympathetic strings answer the bowed ones.
//!
//! Both live here rather than in `auris-dsp` because they are made of this crate's bodies and
//! rotors; both are ordinary [`auris_core::Effect`]s and are registered with the instruments by
//! [`crate::SynthPack`]. Each publishes a [`ResonanceFrame`](auris_core::resonance::ResonanceFrame)
//! for the window to draw while somebody is looking.
//!
//! Both work in control ticks of `TICK` frames, counted across blocks rather than per call,
//! so a render is the same whatever block size the host asks for.

mod body;
mod sympathetic;

pub use body::{BodyKind, BodyResonance};
pub use sympathetic::{SympatheticStrings, Tuning};

/// Frames per control tick.
pub(crate) const TICK: usize = 64;

/// Frames between published frames: a little over sixty a second at 48 kHz.
pub(crate) const PUBLISH_TICKS: u32 = 12;

/// A peak level that falls away at a display's pace.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Level(f32);

impl Level {
    /// Takes in a block's peak; returns the held level.
    pub(crate) fn feed(&mut self, peak: f32) -> f32 {
        let peak = if peak.is_finite() { peak } else { 0.0 };
        self.0 = peak.max(self.0 * 0.92);
        self.0
    }

    pub(crate) fn get(self) -> f32 {
        self.0
    }

    pub(crate) fn reset(&mut self) {
        self.0 = 0.0;
    }
}

/// A sample rate a resonator can lay itself out for.
pub(crate) fn sane_rate(rate: f64) -> f32 {
    if rate.is_finite() && rate > 0.0 {
        rate.clamp(8_000.0, 384_000.0) as f32
    } else {
        48_000.0
    }
}
