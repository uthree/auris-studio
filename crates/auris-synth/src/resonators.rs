//! Effects built the way the physical instruments are: from the thing itself, set moving.
//!
//! An instrument's body is a resonator waiting for a signal, and nothing in it cares whether the
//! signal is a string's pull at the bridge or a recording. The same goes for every piece of
//! studio hardware whose sound is the sound of something moving. Each effect here models that
//! thing rather than the curve it is usually reduced to:
//!
//! * [`BodyResonance`] puts a track through a box — a guitar's, a violin's or a cello's, a
//!   piano's soundboard, a dulcimer's, or a speaker cabinet — using the same mode banks the
//!   physical instruments sound through.
//! * [`SympatheticStrings`] hangs a bank of undamped strings beside the track, tuned to the
//!   song's chords or key, so that whatever is played in tune sets them ringing the way a sitar's
//!   or a viola d'amore's sympathetic strings answer the bowed ones.
//! * [`PlateReverb`] is a steel sheet in a frame, whose bending waves carry the top of an attack
//!   to the pickups before its body.
//! * [`SpringReverb`] is a tank of coiled springs, which carry it the other way round: every
//!   echo a rising chirp.
//! * [`TapeEcho`] is a loop of tape past three heads, with the transport's wow and flutter and
//!   the oxide's hysteresis loop.
//! * [`TubeStage`] is one triode with the capacitors around it and an output transformer after.
//! * [`RotarySpeaker`] is a turning horn over a turning drum, each with its own inertia.
//!
//! They live here rather than in `auris-dsp` because the first two are made of this crate's
//! bodies and rotors and the rest belong with them; all are ordinary [`auris_core::Effect`]s,
//! registered with the instruments by [`crate::SynthPack`]. Each publishes a
//! [`ResonanceFrame`](auris_core::resonance::ResonanceFrame) for the window to draw while
//! somebody is looking.
//!
//! All work in control ticks of `TICK` frames, counted across blocks rather than per call,
//! so a render is the same whatever block size the host asks for.

mod body;
mod plate;
mod rotary;
mod spring;
mod sympathetic;
mod tape;
mod tube;

pub use body::{BodyKind, BodyResonance};
pub use plate::PlateReverb;
pub use rotary::RotarySpeaker;
pub use spring::SpringReverb;
pub use sympathetic::{SympatheticStrings, Tuning};
pub use tape::TapeEcho;
pub use tube::TubeStage;

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

/// A sample, or silence in place of one that is not a number.
pub(crate) fn finite(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

/// A first-order allpass section, `(a + z⁻¹) / (1 + a·z⁻¹)`: every frequency through at unit
/// gain, those below its corner held back longer than those above when `a` is negative.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Allpass {
    a: f32,
    x1: f32,
    y1: f32,
}

impl Allpass {
    pub(crate) const fn new(a: f32) -> Self {
        Self {
            a,
            x1: 0.0,
            y1: 0.0,
        }
    }

    #[inline]
    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = self.a * x + self.x1 - self.a * self.y1;
        self.x1 = x;
        self.y1 = if y.abs() < 1.0e-30 { 0.0 } else { y };
        self.y1
    }

    pub(crate) fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

/// A cubic through four neighbours, newest first: `points[1]` at `fraction` zero and
/// `points[2]` at one.
#[inline]
pub(crate) fn hermite(points: [f32; 4], fraction: f32) -> f32 {
    let [y0, y1, y2, y3] = points;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * fraction + c2) * fraction + c1) * fraction + y1
}
