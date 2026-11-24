//! The subtractive synthesiser behind `auris.synth.supersaw` and its nine siblings.
//!
//! One engine, ten patches. Every patch is a separate instrument in the browser — a person
//! looking for "a pad" should find a pad, not a synthesiser and a manual — but they all share
//! one parameter set, so any of them can be turned into any other by its knobs, and a project
//! that names `auris.synth.pad` stores nothing a SuperSaw could not also hold.
//!
//! # The voice
//!
//! ```text
//!  unison stack (1–7 detuned saws, pulses or triangles, panned left and right)
//!  + sub oscillator (a square an octave down)            ─┐
//!  + white noise                                          ├─ soft saturation ─ 24 dB low-pass ─┐
//!  pulse width ← PWM LFO, cutoff ← filter envelope · key ─┘        (per channel)              │
//!                                        cutoff ← tempo-synced filter LFO                     │
//!                                                                                             │
//!  ┌─ drive ─ 12 dB low cut ─ + sine sub (clean, at the note) ─ amp ◄─────────────────────────┘
//! ```
//!
//! Every stage after the filter is off at its knob's zero, and a patch that leaves them there
//! renders exactly what it did before they existed.
//!
//! # Bass house
//!
//! The last three patches are the stages after the filter put to work. A bass house bass is two
//! sounds played as one: a sine an octave below anything else that carries the weight, and a
//! driven saw above it whose low harmonics are cut away so the two never fight. The drive comes
//! *after* the filter, so a stab whose filter has snapped shut still has its grit; the pitch
//! envelope drops each note into place from a few semitones up; and FM with the tempo-synced
//! filter LFO make the saw talk. The patches were set against a club track at 130 BPM, whose bass
//! measured a sine fundamental near 40 Hz, a hollow between 60 and 100 Hz, and a peak of upper
//! harmonics between 200 and 600 Hz that fell away within a sixteenth note.
//!
//! The stack is the SuperSaw's: a centre oscillator at the note and side copies spread over
//! `±detune` semitones with a small, deliberate asymmetry, so the beating between them never
//! settles into a regular pulse. Side copies alternate between left and right — the flattest on
//! one side, the next flattest on the other — so the image is wide without one side sounding
//! sharp. Every copy starts at its own phase, because seven saws starting together strike as one
//! loud edge and then flange apart, which is the opposite of what a wide unison is for.
//!
//! The filter is two topology-preserving state-variable sections in series: the first carries the
//! resonance, the second is a plain Butterworth that turns twelve decibels an octave into
//! twenty-four. The pair stays stable however fast the envelope moves the cutoff, which is what
//! lets the pluck patch sweep four octaves in a quarter of a second.
//!
//! # Realtime behaviour
//!
//! Everything is a fixed array sized in `prepare`. The cutoff is recomputed every sample from
//! its envelope — one `tan` per voice — which is what keeps a fast envelope free of zipper steps.

use std::borrow::Cow;
use std::f32::consts::{PI, SQRT_2, TAU};

use auris_core::motion::{MOTION_MODES, MOTION_POINTS, MotionFrame, MotionGeometry};
use auris_core::param::db_to_gain;
use auris_core::plugin::pitch_to_hz;
use auris_core::{
    AudioBuffer, CC_MODULATION, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit,
    ParamValueCurve, Parameterized, PluginCategory, PluginDescriptor, PrepareContext,
    ProcessContext,
};
use auris_dsp::Adsr;
use auris_dsp::adsr::SEGMENT_TIME_CONSTANTS;

use crate::lfo::Lfo;
use crate::observe::{Observer, harmonics, recent_voices};
use crate::oscillator::{Oscillator, Waveform};
use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, downmix_to_extra_channels, pan_gains, render_segments};
use crate::voice::{VoiceAllocator, VoiceState};

/// Voices in the pool.
const VOICE_COUNT: usize = 8;

/// Most oscillators one note stacks, the classic SuperSaw's seven.
pub const MAX_UNISON: usize = 7;

const P_WAVEFORM: u32 = 0;
const P_UNISON: u32 = 1;
const P_DETUNE: u32 = 2;
const P_MIX: u32 = 3;
const P_WIDTH: u32 = 4;
const P_PULSE_WIDTH: u32 = 5;
const P_PWM: u32 = 6;
const P_PWM_RATE: u32 = 7;
const P_SUB: u32 = 8;
const P_NOISE: u32 = 9;
const P_CUTOFF: u32 = 10;
const P_RESONANCE: u32 = 11;
const P_FILTER_ENV: u32 = 12;
const P_KEY_TRACK: u32 = 13;
const P_FILTER_VELOCITY: u32 = 14;
const P_FILTER_ATTACK: u32 = 15;
const P_FILTER_DECAY: u32 = 16;
const P_FILTER_SUSTAIN: u32 = 17;
const P_FILTER_RELEASE: u32 = 18;
const P_ATTACK: u32 = 19;
const P_DECAY: u32 = 20;
const P_SUSTAIN: u32 = 21;
const P_RELEASE: u32 = 22;
const P_GLIDE: u32 = 23;
const P_VIBRATO_RATE: u32 = 24;
const P_VIBRATO: u32 = 25;
const P_MOD_DEPTH: u32 = 26;
const P_LEVEL: u32 = 27;
const P_DRIVE: u32 = 28;
const P_LOW_CUT: u32 = 29;
const P_SINE_SUB: u32 = 30;
const P_PITCH_ENV: u32 = 31;
const P_PITCH_DECAY: u32 = 32;
const P_FM: u32 = 33;
const P_FM_RATIO: u32 = 34;
const P_FILTER_LFO: u32 = 35;
const P_LFO_RATE: u32 = 36;

/// Gain the drive stage reaches at full drive, in decibels.
const MAX_DRIVE_DB: f32 = 36.0;

/// The peak the drive stage's makeup holds level at: about where a filtered stack sits.
const DRIVE_REFERENCE: f32 = 0.5;

/// The low cut at or below which the high-pass is simply not run, so a patch that does not ask
/// for one renders exactly as it did before the stage existed.
const LOW_CUT_OFF_HZ: f32 = 20.5;

/// Phase deviation of every stack copy at full FM depth, in cycles: an index of 2π, where a
/// saw has turned into a snarl but the note still has a pitch.
const MAX_FM_CYCLES: f32 = 1.0;

/// The filter LFO's tempo divisions as `(label, beats per cycle)`, in parameter order.
const LFO_DIVISIONS: [(&str, f32); 8] = [
    ("1/1", 4.0),
    ("1/2", 2.0),
    ("1/4", 1.0),
    ("1/8", 0.5),
    ("1/16", 0.25),
    ("1/4T", 2.0 / 3.0),
    ("1/8T", 1.0 / 3.0),
    ("1/16T", 1.0 / 6.0),
];

/// Labels for the LFO rate choice, in the order of [`LFO_DIVISIONS`].
const LFO_DIVISION_CHOICES: &[Cow<'static, str>] = &[
    Cow::Borrowed("1/1"),
    Cow::Borrowed("1/2"),
    Cow::Borrowed("1/4"),
    Cow::Borrowed("1/8"),
    Cow::Borrowed("1/16"),
    Cow::Borrowed("1/4T"),
    Cow::Borrowed("1/8T"),
    Cow::Borrowed("1/16T"),
];

/// Tempo the filter LFO follows until the host reports one.
const DEFAULT_BPM: f32 = 120.0;

/// Widest pitch bend followed, in semitones.
const MAX_BEND_SEMITONES: f32 = 24.0;

/// The note key tracking is measured from: at C4 the cutoff is what the knob says.
const KEY_TRACK_CENTRE: f32 = 60.0;

/// Lowest cutoff the filter is driven to, in hertz.
const MIN_CUTOFF_HZ: f32 = 16.0;

/// Highest cutoff as a fraction of the sample rate. `tan` runs away at a half, and the bilinear
/// warp is already steep by here, so the filter is simply open above it.
const MAX_CUTOFF_FRACTION: f32 = 0.45;

/// The lowest and highest cutoff the filter may be asked for at `sample_rate`.
///
/// The ceiling is the one that has to hold, because the filter is unstable above it. At a rate
/// so low that the ceiling falls under [`MIN_CUTOFF_HZ`] the floor comes down to meet it: a
/// range whose low end is above its high end is one `clamp` panics on, and a host may hand any
/// positive rate over.
fn cutoff_bounds(sample_rate: f32) -> (f32, f32) {
    let ceiling = MAX_CUTOFF_FRACTION * sample_rate;
    (MIN_CUTOFF_HZ.min(ceiling), ceiling)
}

/// The resonant section's damping at full resonance: a Q of five, a clear vowel-like peak that
/// stops short of self-oscillation, so no patch can whistle on its own with no note held.
const MIN_DAMPING: f32 = 0.2;

/// How much of the detune each side copy is moved off its even spacing, so the beat between
/// copies is never a regular pulse. Index `i` belongs to copy `i` of the stack.
const DETUNE_SKEW: [f32; MAX_UNISON] = [0.0, 0.05, -0.07, 0.0, 0.06, -0.04, 0.03];

/// Where each copy starts in its cycle at note on, spread by the golden ratio.
const START_PHASES: [f32; MAX_UNISON] = [0.0, 0.618, 0.236, 0.854, 0.472, 0.090, 0.708];

/// The waveforms the stack can play, in parameter order.
const WAVEFORMS: [Waveform; 3] = [Waveform::Saw, Waveform::Square, Waveform::Triangle];

/// Labels for the waveform choice, in the order of [`WAVEFORMS`].
const WAVEFORM_CHOICES: &[Cow<'static, str>] = &[
    Cow::Borrowed("Saw"),
    Cow::Borrowed("Square"),
    Cow::Borrowed("Triangle"),
];

/// One of the patches the subtractive engine ships with, each its own instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnalogPatch {
    /// Seven detuned saws spread across the stereo field: trance leads and wide chords.
    SuperSaw,
    /// Slow, warm detuned saws behind a filter that opens as the chord swells.
    Pad,
    /// A bright resonant lead with glide, for melodies.
    Lead,
    /// A saw over a sub-octave square with a filter envelope punch.
    Bass,
    /// A pulse stack whose width an LFO keeps moving: the 1980s string machine.
    PwmStrings,
    /// Saws behind a filter that opens slowly as the note swells, like a section breathing in.
    Brass,
    /// A short filter envelope over a percussive amp: plucks and arpeggios.
    Pluck,
    /// The bass house stab: a driven, low-cut saw whose filter snaps shut on every note, over a
    /// clean sine sub.
    HouseBass,
    /// A frequency-modulated, driven saw whose filter talks in eighth notes.
    GrowlBass,
    /// Detuned saws beating against each other, driven, over a clean sine sub.
    ReeseBass,
}

impl AnalogPatch {
    /// Every patch, in the order the browser lists them.
    pub const ALL: [Self; 10] = [
        Self::SuperSaw,
        Self::Pad,
        Self::Lead,
        Self::Bass,
        Self::PwmStrings,
        Self::Brass,
        Self::Pluck,
        Self::HouseBass,
        Self::GrowlBass,
        Self::ReeseBass,
    ];

    /// Whether the patch's default settings put the same signal in both channels.
    pub fn is_mono(self) -> bool {
        self.defaults().width <= 0.0
    }

    /// Stable plugin id stored in a project.
    pub const fn id(self) -> &'static str {
        match self {
            Self::SuperSaw => "auris.synth.supersaw",
            Self::Pad => "auris.synth.pad",
            Self::Lead => "auris.synth.lead",
            Self::Bass => "auris.synth.bass",
            Self::PwmStrings => "auris.synth.pwm_strings",
            Self::Brass => "auris.synth.brass",
            Self::Pluck => "auris.synth.pluck",
            Self::HouseBass => "auris.synth.house_bass",
            Self::GrowlBass => "auris.synth.growl_bass",
            Self::ReeseBass => "auris.synth.reese_bass",
        }
    }

    /// The patch an id names, if it is one of these.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|patch| patch.id() == id)
    }

    fn name(self) -> &'static str {
        match self {
            Self::SuperSaw => "SuperSaw",
            Self::Pad => "Synth Pad",
            Self::Lead => "Synth Lead",
            Self::Bass => "Synth Bass",
            Self::PwmStrings => "PWM Strings",
            Self::Brass => "Synth Brass",
            Self::Pluck => "Synth Pluck",
            Self::HouseBass => "House Bass",
            Self::GrowlBass => "Growl Bass",
            Self::ReeseBass => "Reese Bass",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::SuperSaw => {
                "Seven detuned saws spread across the stereo field: trance leads and wide chords"
            }
            Self::Pad => "Slow detuned saws behind a gently opening filter: warm sustained pads",
            Self::Lead => "Bright resonant saw lead with glide and wheel vibrato",
            Self::Bass => "Saw over a sub-octave square with a punchy filter envelope",
            Self::PwmStrings => "Pulse-width-modulated stack: the lush string machine sound",
            Self::Brass => "Saws behind a slowly opening filter: swelling synth brass",
            Self::Pluck => "Short filter envelope over a percussive amp: plucks and arpeggios",
            Self::HouseBass => {
                "Driven saw stab with a snapping filter over a clean sine sub: bass house"
            }
            Self::GrowlBass => "FM saw through a driven, tempo-synced talking filter: growls",
            Self::ReeseBass => "Detuned beating saws, driven, over a clean sine sub: reese bass",
        }
    }

    /// The patch's own settings, as the parameters' defaults.
    fn defaults(self) -> Defaults {
        let base = Defaults::default();
        match self {
            Self::SuperSaw => Defaults {
                unison: 7.0,
                detune: 0.3,
                mix: 0.8,
                width: 0.9,
                cutoff: 12_000.0,
                resonance: 0.1,
                attack: 0.005,
                decay: 0.4,
                sustain: 0.9,
                release: 0.35,
                level: -18.0,
                ..base
            },
            Self::Pad => Defaults {
                unison: 5.0,
                detune: 0.18,
                mix: 0.7,
                width: 1.0,
                cutoff: 900.0,
                resonance: 0.15,
                filter_env: 24.0,
                key_track: 0.5,
                filter_attack: 1.2,
                filter_decay: 2.0,
                filter_sustain: 0.5,
                filter_release: 1.5,
                attack: 0.8,
                decay: 1.0,
                sustain: 0.85,
                release: 1.6,
                vibrato_rate: 4.5,
                level: -9.0,
                ..base
            },
            Self::Lead => Defaults {
                unison: 2.0,
                detune: 0.06,
                mix: 1.0,
                width: 0.3,
                cutoff: 1_400.0,
                resonance: 0.4,
                filter_env: 30.0,
                key_track: 0.6,
                filter_velocity: 0.4,
                filter_attack: 0.003,
                filter_decay: 0.4,
                filter_sustain: 0.45,
                filter_release: 0.2,
                attack: 0.004,
                decay: 0.2,
                sustain: 0.85,
                release: 0.15,
                glide: 0.05,
                level: -17.0,
                ..base
            },
            Self::Bass => Defaults {
                unison: 1.0,
                width: 0.0,
                sub: 0.6,
                cutoff: 220.0,
                resonance: 0.3,
                filter_env: 36.0,
                key_track: 0.5,
                filter_velocity: 0.5,
                filter_attack: 0.001,
                filter_decay: 0.25,
                filter_sustain: 0.2,
                filter_release: 0.1,
                attack: 0.002,
                decay: 0.3,
                sustain: 0.8,
                release: 0.08,
                level: -14.0,
                ..base
            },
            Self::PwmStrings => Defaults {
                waveform: 1.0,
                unison: 3.0,
                detune: 0.1,
                mix: 0.7,
                width: 0.8,
                pulse_width: 0.5,
                pwm: 0.7,
                pwm_rate: 0.8,
                cutoff: 3_000.0,
                resonance: 0.05,
                filter_env: 6.0,
                key_track: 0.5,
                filter_attack: 0.4,
                filter_decay: 1.0,
                filter_sustain: 0.6,
                filter_release: 0.8,
                attack: 0.35,
                decay: 0.5,
                sustain: 0.9,
                release: 0.8,
                vibrato_rate: 5.0,
                level: -18.5,
                ..base
            },
            Self::Brass => Defaults {
                unison: 3.0,
                detune: 0.08,
                mix: 0.8,
                width: 0.5,
                cutoff: 450.0,
                resonance: 0.15,
                filter_env: 40.0,
                key_track: 0.6,
                filter_velocity: 0.6,
                filter_attack: 0.12,
                filter_decay: 0.6,
                filter_sustain: 0.55,
                filter_release: 0.3,
                attack: 0.05,
                decay: 0.3,
                sustain: 0.9,
                release: 0.25,
                level: -17.0,
                ..base
            },
            Self::Pluck => Defaults {
                unison: 3.0,
                detune: 0.1,
                mix: 0.7,
                width: 0.7,
                cutoff: 350.0,
                resonance: 0.25,
                filter_env: 48.0,
                key_track: 0.7,
                filter_velocity: 0.5,
                filter_attack: 0.001,
                filter_decay: 0.25,
                filter_sustain: 0.0,
                filter_release: 0.2,
                attack: 0.001,
                decay: 0.6,
                sustain: 0.0,
                release: 0.3,
                level: -4.0,
                ..base
            },
            // Set against a bass house drop at 130 BPM, measured between its sixteenth-note hats
            // on stabs of the same note: a spectral centroid of 414 Hz (this patch 427), 1–4 kHz
            // 16.8 dB under 150 Hz–1 kHz (14.4), and the sine's 30–55 Hz 2.6 dB over that
            // (2.5). The low cut is what makes the hollow between the sine and the saw's peak.
            Self::HouseBass => Defaults {
                unison: 2.0,
                detune: 0.08,
                mix: 0.8,
                width: 0.0,
                cutoff: 220.0,
                resonance: 0.45,
                filter_env: 36.0,
                key_track: 0.5,
                filter_velocity: 0.4,
                filter_attack: 0.001,
                filter_decay: 0.25,
                filter_sustain: 0.35,
                filter_release: 0.08,
                attack: 0.001,
                decay: 0.3,
                sustain: 0.75,
                release: 0.05,
                drive: 0.6,
                low_cut: 300.0,
                sine_sub: 0.22,
                pitch_env: 7.0,
                pitch_decay: 0.02,
                level: -5.65,
                ..base
            },
            Self::GrowlBass => Defaults {
                unison: 2.0,
                detune: 0.1,
                mix: 0.8,
                width: 0.0,
                cutoff: 300.0,
                resonance: 0.6,
                filter_env: 12.0,
                key_track: 0.5,
                filter_velocity: 0.3,
                filter_attack: 0.001,
                filter_decay: 0.2,
                filter_sustain: 0.4,
                filter_release: 0.1,
                attack: 0.002,
                decay: 0.3,
                sustain: 0.9,
                release: 0.08,
                drive: 0.7,
                low_cut: 140.0,
                sine_sub: 0.55,
                fm: 0.35,
                filter_lfo: 36.0,
                level: -13.2,
                ..base
            },
            Self::ReeseBass => Defaults {
                unison: 3.0,
                detune: 0.25,
                mix: 1.0,
                width: 0.35,
                cutoff: 900.0,
                resonance: 0.25,
                key_track: 0.4,
                attack: 0.005,
                decay: 0.5,
                sustain: 1.0,
                release: 0.15,
                drive: 0.4,
                low_cut: 80.0,
                sine_sub: 0.3,
                level: -13.1,
                ..base
            },
        }
    }
}

/// The defaults a patch writes into the shared parameter set.
#[derive(Clone, Copy, Debug)]
struct Defaults {
    waveform: f32,
    unison: f32,
    detune: f32,
    mix: f32,
    width: f32,
    pulse_width: f32,
    pwm: f32,
    pwm_rate: f32,
    sub: f32,
    noise: f32,
    cutoff: f32,
    resonance: f32,
    filter_env: f32,
    key_track: f32,
    filter_velocity: f32,
    filter_attack: f32,
    filter_decay: f32,
    filter_sustain: f32,
    filter_release: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    glide: f32,
    vibrato_rate: f32,
    vibrato: f32,
    mod_depth: f32,
    level: f32,
    drive: f32,
    low_cut: f32,
    sine_sub: f32,
    pitch_env: f32,
    pitch_decay: f32,
    fm: f32,
    fm_ratio: f32,
    filter_lfo: f32,
    lfo_rate: f32,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            waveform: 0.0,
            unison: 1.0,
            detune: 0.0,
            mix: 0.8,
            width: 0.5,
            pulse_width: 0.5,
            pwm: 0.0,
            pwm_rate: 1.0,
            sub: 0.0,
            noise: 0.0,
            cutoff: 8_000.0,
            resonance: 0.0,
            filter_env: 0.0,
            key_track: 0.0,
            filter_velocity: 0.0,
            filter_attack: 0.01,
            filter_decay: 0.5,
            filter_sustain: 1.0,
            filter_release: 0.3,
            attack: 0.005,
            decay: 0.3,
            sustain: 0.8,
            release: 0.3,
            glide: 0.0,
            vibrato_rate: 5.5,
            vibrato: 0.0,
            mod_depth: crate::chiptune::DEFAULT_MOD_DEPTH,
            level: -8.0,
            drive: 0.0,
            low_cut: 20.0,
            sine_sub: 0.0,
            pitch_env: 0.0,
            pitch_decay: 0.03,
            fm: 0.0,
            fm_ratio: 1.0,
            filter_lfo: 0.0,
            // Eighth notes.
            lfo_rate: 3.0,
        }
    }
}

/// Coefficients of one topology-preserving state-variable section.
#[derive(Clone, Copy, Debug)]
struct SvfCoefficients {
    a1: f32,
    a2: f32,
    a3: f32,
}

impl SvfCoefficients {
    /// A section with prewarped gain `g = tan(π·fc/fs)` and damping `k = 1/Q`.
    fn new(g: f32, k: f32) -> Self {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self { a1, a2, a3: g * a2 }
    }
}

/// The integrator state of one state-variable section.
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// Low-pass output for one input sample (Simper's trapezoidal SVF).
    fn lowpass(&mut self, input: f32, c: SvfCoefficients) -> f32 {
        let v3 = input - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        v2
    }

    /// High-pass output for one input sample, from the same section.
    fn highpass(&mut self, input: f32, c: SvfCoefficients, k: f32) -> f32 {
        let v3 = input - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        input - k * v1 - v2
    }
}

/// The drive stage: `tanh` after a gain of up to [`MAX_DRIVE_DB`], brought back down so that a
/// signal peaking at [`DRIVE_REFERENCE`] still peaks where it did undriven. Turning the knob then
/// changes the tone of a full-level note far more than its level; a quiet one comes up, which is
/// what driving anything into a clipper does.
#[derive(Clone, Copy, Debug)]
struct Drive {
    gain: f32,
    makeup: f32,
}

impl Drive {
    fn new(amount: f32) -> Self {
        let gain = db_to_gain(amount.clamp(0.0, 1.0) * MAX_DRIVE_DB);
        Self {
            gain,
            makeup: DRIVE_REFERENCE.tanh() / (DRIVE_REFERENCE * gain).tanh(),
        }
    }

    fn process(self, x: f32) -> f32 {
        (x * self.gain).tanh() * self.makeup
    }
}

/// The resonant section and the Butterworth behind it, for one channel.
#[derive(Clone, Copy, Debug, Default)]
struct Filter {
    resonant: Svf,
    plain: Svf,
}

impl Filter {
    fn process(&mut self, input: f32, resonant: SvfCoefficients, plain: SvfCoefficients) -> f32 {
        let once = self.resonant.lowpass(input, resonant);
        self.plain.lowpass(once, plain)
    }
}

/// The resonant section's damping for a 0–1 resonance.
fn damping(resonance: f32) -> f32 {
    2.0 - (2.0 - MIN_DAMPING) * resonance.clamp(0.0, 1.0)
}

/// The filter's complex response at `ratio` times its cutoff, as `(real, imaginary)`.
///
/// The analogue prototype rather than the bilinear one: the display wants the shape the knob
/// promises, and the two agree to within a pixel below the twentieth harmonic of any note.
fn response(ratio: f32, resonance: f32) -> (f32, f32) {
    let section = |k: f32| {
        // 1 / (1 − w² + j·k·w)
        let (re, im) = (1.0 - ratio * ratio, k * ratio);
        let norm = re * re + im * im;
        if norm <= f32::MIN_POSITIVE {
            (0.0, -1.0 / k.max(1e-3))
        } else {
            (re / norm, -im / norm)
        }
    };
    let (a, b) = section(damping(resonance));
    let (c, d) = section(SQRT_2);
    (a * c - b * d, a * d + b * c)
}

/// A soft saturator, `tanh`-shaped and bounded at ±2, that the stack goes through before the
/// filter so a resonant peak never meets a stack of aligned edges at full height.
fn saturate(x: f32) -> f32 {
    let x = (x * 0.5).clamp(-3.0, 3.0);
    2.0 * x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

/// One step of a 32-bit xorshift, as a sample between −1 and 1.
fn white(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// A reproducible noise seed for voice `index`; never zero, the one state xorshift cannot leave.
fn noise_seed(index: usize) -> u32 {
    (index as u32).wrapping_mul(0x9e37_79b9) ^ 0x2545_f491 | 1
}

/// One note's oscillators, envelopes and filters.
#[derive(Clone, Debug)]
struct AnalogVoice {
    oscillators: [Oscillator; MAX_UNISON],
    sub: Oscillator,
    noise: u32,
    seed: u32,
    /// The pulse-width wobble, free-running so a held string chord keeps moving.
    pwm: Lfo,
    /// The vibrato, restarted at every note on so a chord wobbles together.
    vibrato: Lfo,
    envelope: Adsr,
    filter_envelope: Adsr,
    filters: [Filter; 2],
    /// The low cut after the drive, one per channel.
    highpass: [Svf; 2],
    /// Semitones the note still sits above itself, falling to nothing after note on.
    pitch_offset: f32,
    /// The FM modulator's and the sine sub's place in their cycles.
    fm_phase: f32,
    sine_phase: f32,
    /// The filter LFO's place in its cycle, restarted at note on so it lands on the beat.
    wobble_phase: f32,
    pitch: f32,
    frequency: f32,
    target: f32,
    velocity: f32,
    /// Where the filter was on the last sample rendered, for the display.
    cutoff: f32,
}

impl AnalogVoice {
    fn new(index: usize, sample_rate: f32) -> Self {
        let oscillators = std::array::from_fn(|_| {
            let mut oscillator = Oscillator::new();
            oscillator.set_sample_rate(sample_rate);
            oscillator
        });
        let mut sub = Oscillator::new();
        sub.set_sample_rate(sample_rate);
        let mut envelope = Adsr::new();
        envelope.set_sample_rate(sample_rate);
        let mut filter_envelope = Adsr::new();
        filter_envelope.set_sample_rate(sample_rate);
        let mut pwm = Lfo::new();
        pwm.set_sample_rate(sample_rate);
        let mut vibrato = Lfo::new();
        vibrato.set_sample_rate(sample_rate);
        let seed = noise_seed(index);
        Self {
            oscillators,
            sub,
            noise: seed,
            seed,
            pwm,
            vibrato,
            envelope,
            filter_envelope,
            filters: [Filter::default(); 2],
            highpass: [Svf::default(); 2],
            pitch_offset: 0.0,
            fm_phase: 0.0,
            sine_phase: 0.0,
            wobble_phase: 0.0,
            pitch: 69.0,
            frequency: 440.0,
            target: 440.0,
            velocity: 0.0,
            cutoff: 0.0,
        }
    }

    /// Back to the state a note starts from on a silent voice.
    fn restart(&mut self) {
        for (oscillator, phase) in self.oscillators.iter_mut().zip(START_PHASES) {
            oscillator.reset();
            oscillator.set_phase(phase);
        }
        self.sub.reset();
        self.noise = self.seed;
        self.vibrato.reset();
        self.filters = [Filter::default(); 2];
        self.highpass = [Svf::default(); 2];
        self.fm_phase = 0.0;
        self.sine_phase = 0.0;
    }
}

/// A polyphonic subtractive synthesiser: a stack of up to seven detuned oscillators with a sub
/// octave and noise, through a resonant 24 dB low-pass with its own envelope.
///
/// Built once per [`AnalogPatch`]; see the module documentation for the signal path.
#[derive(Clone, Debug)]
pub struct Analog {
    patch: AnalogPatch,
    params: ParamBank,
    voices: Vec<AnalogVoice>,
    allocator: VoiceAllocator,
    sample_rate: f32,

    waveform: Waveform,
    unison: usize,
    /// Frequency ratio of each copy in the stack.
    ratios: [f32; MAX_UNISON],
    /// Left and right gains of each copy, panning and the mix level folded together.
    gains: [(f32, f32); MAX_UNISON],
    /// Which way each copy's pulse width swings with the LFO.
    pwm_signs: [f32; MAX_UNISON],
    pulse_width: f32,
    pwm_depth: f32,
    sub_level: f32,
    noise_level: f32,
    cutoff: f32,
    resonance: f32,
    filter_env_semitones: f32,
    key_track: f32,
    filter_velocity: f32,
    glide_coeff: f32,
    bend: f32,
    modulation: f32,
    channel_volume: f32,
    vibrato_depth: f32,
    output_gain: f32,
    /// The drive knob, 0 to 1; at 0 the stage is not run at all.
    drive: f32,
    low_cut: f32,
    sine_sub: f32,
    pitch_env: f32,
    /// What the pitch offset is multiplied by every sample.
    pitch_fall: f32,
    /// FM depth in cycles of phase deviation, and the modulator's ratio to the note.
    fm_cycles: f32,
    fm_ratio: f32,
    /// How far the filter LFO opens the cutoff, in semitones, and its cycle in beats.
    filter_lfo: f32,
    lfo_beats: f32,
    /// The tempo the host last reported.
    bpm: f32,
    last_voice: Option<usize>,
    observer: Observer,
}

impl Analog {
    /// Builds the instrument with `patch`'s settings as its defaults.
    pub fn new(patch: AnalogPatch) -> Self {
        let mut synth = Self {
            patch,
            params: ParamBank::new(descriptors(patch)),
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            sample_rate: 48_000.0,
            waveform: Waveform::Saw,
            unison: 1,
            ratios: [1.0; MAX_UNISON],
            gains: [(1.0, 1.0); MAX_UNISON],
            pwm_signs: [1.0; MAX_UNISON],
            pulse_width: 0.5,
            pwm_depth: 0.0,
            sub_level: 0.0,
            noise_level: 0.0,
            cutoff: 8_000.0,
            resonance: 0.0,
            filter_env_semitones: 0.0,
            key_track: 0.0,
            filter_velocity: 0.0,
            glide_coeff: 1.0,
            bend: 0.0,
            modulation: 0.0,
            channel_volume: 1.0,
            vibrato_depth: 0.0,
            output_gain: 1.0,
            drive: 0.0,
            low_cut: 20.0,
            sine_sub: 0.0,
            pitch_env: 0.0,
            pitch_fall: 0.0,
            fm_cycles: 0.0,
            fm_ratio: 1.0,
            filter_lfo: 0.0,
            lfo_beats: 0.5,
            bpm: DEFAULT_BPM,
            last_voice: None,
            observer: Observer::default(),
        };
        synth.refresh();
        synth
    }

    /// Which patch this instance was built as.
    pub fn patch(&self) -> AnalogPatch {
        self.patch
    }

    fn refresh(&mut self) {
        self.waveform = WAVEFORMS[(self.params.at(P_WAVEFORM).round().max(0.0) as usize).min(2)];
        self.unison = (self
            .params
            .at(P_UNISON)
            .round()
            .clamp(1.0, MAX_UNISON as f32)) as usize;
        let (detune, mix, width) = (
            self.params.at(P_DETUNE),
            self.params.at(P_MIX),
            self.params.at(P_WIDTH),
        );
        let layout = unison_layout(self.unison, detune, mix, width);
        self.ratios = layout.ratios;
        self.gains = layout.gains;
        self.pwm_signs = layout.pwm_signs;
        self.pulse_width = self.params.at(P_PULSE_WIDTH);
        self.pwm_depth = self.params.at(P_PWM);
        self.sub_level = self.params.at(P_SUB);
        self.noise_level = self.params.at(P_NOISE);
        self.cutoff = self.params.at(P_CUTOFF);
        self.resonance = self.params.at(P_RESONANCE);
        self.filter_env_semitones = self.params.at(P_FILTER_ENV);
        self.key_track = self.params.at(P_KEY_TRACK);
        self.filter_velocity = self.params.at(P_FILTER_VELOCITY);
        self.output_gain = db_to_gain(self.params.at(P_LEVEL));
        self.drive = self.params.at(P_DRIVE);
        self.low_cut = self.params.at(P_LOW_CUT);
        self.sine_sub = self.params.at(P_SINE_SUB);
        self.pitch_env = self.params.at(P_PITCH_ENV);
        self.pitch_fall =
            (-1.0 / (self.params.at(P_PITCH_DECAY) * self.sample_rate).max(1.0)).exp();
        self.fm_cycles = self.params.at(P_FM) * MAX_FM_CYCLES;
        self.fm_ratio = self.params.at(P_FM_RATIO);
        self.filter_lfo = self.params.at(P_FILTER_LFO);
        let division =
            (self.params.at(P_LFO_RATE).round().max(0.0) as usize).min(LFO_DIVISIONS.len() - 1);
        self.lfo_beats = LFO_DIVISIONS[division].1;
        self.vibrato_depth =
            self.params.at(P_VIBRATO) + self.params.at(P_MOD_DEPTH) * self.modulation;
        let glide = self.params.at(P_GLIDE);
        self.glide_coeff = if glide <= 0.0 {
            1.0
        } else {
            1.0 - (-SEGMENT_TIME_CONSTANTS / (glide * self.sample_rate).max(1.0)).exp()
        };
        let (vibrato_rate, pwm_rate) = (self.params.at(P_VIBRATO_RATE), self.params.at(P_PWM_RATE));
        let amp = (
            self.params.at(P_ATTACK),
            self.params.at(P_DECAY),
            self.params.at(P_SUSTAIN),
            self.params.at(P_RELEASE),
        );
        let filter = (
            self.params.at(P_FILTER_ATTACK),
            self.params.at(P_FILTER_DECAY),
            self.params.at(P_FILTER_SUSTAIN),
            self.params.at(P_FILTER_RELEASE),
        );
        for voice in &mut self.voices {
            voice.envelope.set_adsr(amp.0, amp.1, amp.2, amp.3);
            voice
                .filter_envelope
                .set_adsr(filter.0, filter.1, filter.2, filter.3);
            voice.vibrato.set_rate(vibrato_rate);
            voice.pwm.set_rate(pwm_rate);
        }
    }

    /// The cutoff a voice's filter sits at for an envelope level `envelope`.
    fn cutoff_for(&self, pitch: f32, velocity: f32, envelope: f32) -> f32 {
        let velocity_scale = 1.0 - self.filter_velocity + self.filter_velocity * velocity;
        let semitones = self.filter_env_semitones * envelope * velocity_scale
            + self.key_track * (pitch - KEY_TRACK_CENTRE);
        let (floor, ceiling) = cutoff_bounds(self.sample_rate);
        (self.cutoff * (semitones / 12.0).exp2()).clamp(floor, ceiling)
    }

    fn note_on(&mut self, key: u8, pitch: f32, velocity: f32) {
        let velocity = finite_or(velocity, 1.0).clamp(0.0, 1.0);
        let pitch = crate::params::sounding_pitch(key, pitch);
        let Some(assignment) = self.allocator.note_on(key, velocity) else {
            return;
        };
        let target = pitch_to_hz(pitch + self.bend);
        let start = if self.glide_coeff >= 1.0 {
            target
        } else {
            self.last_voice
                .and_then(|index| self.voices.get(index))
                .map_or(target, |voice| voice.frequency)
        };
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        if !assignment.stolen {
            voice.restart();
        }
        voice.pitch = pitch;
        voice.velocity = velocity;
        voice.target = target;
        voice.frequency = start;
        voice.pitch_offset = self.pitch_env;
        voice.wobble_phase = 0.0;
        voice.envelope.trigger();
        voice.filter_envelope.trigger();
        self.last_voice = Some(assignment.index);
    }

    fn note_off(&mut self, pitch: u8) {
        for index in self.allocator.note_off(pitch) {
            if let Some(voice) = self.voices.get_mut(index) {
                voice.envelope.release();
                voice.filter_envelope.release();
            }
        }
    }

    /// The newest notes as one cycle each of what leaves their filter.
    fn publish_motion(&mut self) {
        let mut frame = MotionFrame {
            geometry: MotionGeometry::Oscillator,
            active: self.allocator.active_count(),
            expression: self.channel_volume,
            ..MotionFrame::default()
        };
        let voices = &self.voices;
        let selected = recent_voices(&self.allocator, |index| voices[index].envelope.is_active());
        for (observed, index) in frame.voices.iter_mut().zip(selected) {
            let Some(index) = index else {
                break;
            };
            let voice = &self.voices[index];
            let frequency = voice.frequency.max(1.0);
            let mut cycle = [0.0; MOTION_POINTS];
            filtered_cycle(
                self.waveform,
                self.pulse_width,
                voice.cutoff / frequency,
                self.resonance,
                &mut cycle,
            );
            let mut modes = [0.0; MOTION_MODES];
            harmonics(&cycle, &mut modes);
            observed.modes = modes;
            let level = voice.envelope.level() * voice.velocity;
            for (point, sample) in observed.points.iter_mut().zip(cycle) {
                *point = sample * level;
            }
            observed.pitch = voice.pitch + self.bend;
            observed.level = level;
            observed.contact = 0.0;
            observed.excitation = voice.cutoff;
            observed.held = self
                .allocator
                .slot(index)
                .is_some_and(|slot| slot.state == VoiceState::Held);
        }
        self.observer.publish(&frame);
    }
}

/// Ratios, gains and PWM directions of a stack of `count` copies.
struct UnisonLayout {
    ratios: [f32; MAX_UNISON],
    gains: [(f32, f32); MAX_UNISON],
    pwm_signs: [f32; MAX_UNISON],
}

/// Lays out a stack of `count` copies over `±detune` semitones, the centre copy at full level
/// and the side copies at `mix`, alternating left and right across `width`.
///
/// The gains are normalised to the stack's power, not its worst-case amplitude: the copies start
/// at spread phases and drift at different rates, so they never line up all at once, and the
/// saturator ahead of the filter takes care of the moments that come close.
fn unison_layout(count: usize, detune: f32, mix: f32, width: f32) -> UnisonLayout {
    let count = count.clamp(1, MAX_UNISON);
    let mut layout = UnisonLayout {
        ratios: [1.0; MAX_UNISON],
        gains: [(0.0, 0.0); MAX_UNISON],
        pwm_signs: [1.0; MAX_UNISON],
    };
    let mut levels = [0.0f32; MAX_UNISON];
    let mut pans = [0.0f32; MAX_UNISON];
    for slot in 0..count {
        let position = if count > 1 {
            2.0 * slot as f32 / (count - 1) as f32 - 1.0
        } else {
            0.0
        };
        let offset = detune * (position + DETUNE_SKEW[slot] * position.abs().min(1.0));
        layout.ratios[slot] = (offset / 12.0).exp2();
        let centre = position.abs() < 1e-6;
        levels[slot] = if centre || count == 1 {
            1.0
        } else {
            mix.clamp(0.0, 1.0)
        };
        // The flattest copy goes one way, the next flattest the other, and the sharp half
        // mirrors the flat half, so each side holds both flat and sharp copies.
        let mirror = slot.min(count - 1 - slot);
        let side = if mirror % 2 == 0 { 1.0 } else { -1.0 };
        let side = if slot < count - 1 - slot { side } else { -side };
        pans[slot] = width.clamp(0.0, 1.0) * position.abs() * side;
        layout.pwm_signs[slot] = if slot % 2 == 0 { 1.0 } else { -1.0 };
    }
    let power: f32 = levels.iter().map(|level| level * level).sum();
    let norm = if power > 0.0 {
        power.sqrt().recip()
    } else {
        0.0
    };
    for slot in 0..count {
        let (left, right) = pan_gains(pans[slot]);
        let level = levels[slot] * norm;
        layout.gains[slot] = (left * level, right * level);
    }
    layout
}

/// One cycle of `waveform` as the filter leaves it at `ratio` times the note, for the display,
/// scaled so its peak is one.
///
/// The naive cycle's harmonics are put through the filter's response and summed back, which
/// is exactly what the filter does to a held note once its transient has passed.
fn filtered_cycle(
    waveform: Waveform,
    pulse_width: f32,
    ratio: f32,
    resonance: f32,
    out: &mut [f32; MOTION_POINTS],
) {
    let mut naive = [0.0; MOTION_POINTS];
    let mut oscillator = Oscillator::new();
    oscillator.set_pulse_width(pulse_width);
    oscillator.cycle(waveform, &mut naive);
    out.fill(0.0);
    let length = MOTION_POINTS as f32;
    let ratio = ratio.max(1e-3);
    for harmonic in 1..MOTION_POINTS / 2 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (at, sample) in naive.iter().enumerate() {
            let angle = TAU * harmonic as f32 * at as f32 / length;
            re += sample * angle.cos();
            im -= sample * angle.sin();
        }
        let (hr, hi) = response(harmonic as f32 / ratio, resonance);
        let (yr, yi) = (re * hr - im * hi, re * hi + im * hr);
        for (at, sample) in out.iter_mut().enumerate() {
            let angle = TAU * harmonic as f32 * at as f32 / length;
            *sample += 2.0 / length * (yr * angle.cos() - yi * angle.sin());
        }
    }
    // A picture of the shape: how loud it is, is the envelope's to say.
    let loudest = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    if loudest > 1e-6 && loudest.is_finite() {
        out.iter_mut().for_each(|sample| *sample /= loudest);
    } else {
        out.fill(0.0);
    }
}

fn descriptors(patch: AnalogPatch) -> Vec<ParamDescriptor> {
    let d = patch.defaults();
    let seconds = |id, key, name, max: f32, default| {
        ParamDescriptor::new(id, key, name, 0.0, max, default)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0))
    };
    vec![
        ParamDescriptor::new(P_WAVEFORM, "waveform", "Waveform", 0.0, 2.0, d.waveform)
            .with_choices(WAVEFORM_CHOICES),
        ParamDescriptor::new(
            P_UNISON,
            "unison",
            "Unison",
            1.0,
            MAX_UNISON as f32,
            d.unison,
        )
        .with_steps(MAX_UNISON as u32),
        ParamDescriptor::new(P_DETUNE, "detune", "Detune", 0.0, 1.0, d.detune)
            .with_unit(ParamUnit::Semitones)
            .with_curve(ParamValueCurve::Power(2.0)),
        ParamDescriptor::percent(P_MIX, "mix", "Mix", d.mix),
        ParamDescriptor::percent(P_WIDTH, "width", "Stereo Width", d.width),
        ParamDescriptor::new(
            P_PULSE_WIDTH,
            "pulse_width",
            "Pulse Width",
            0.05,
            0.95,
            d.pulse_width,
        )
        .with_unit(ParamUnit::Percent),
        ParamDescriptor::percent(P_PWM, "pwm", "PWM Depth", d.pwm),
        ParamDescriptor::new(P_PWM_RATE, "pwm_rate", "PWM Rate", 0.05, 10.0, d.pwm_rate)
            .with_unit(ParamUnit::Hertz)
            .with_curve(ParamValueCurve::Logarithmic),
        ParamDescriptor::percent(P_SUB, "sub", "Sub Oscillator", d.sub),
        ParamDescriptor::percent(P_NOISE, "noise", "Noise Level", d.noise),
        ParamDescriptor::new(P_CUTOFF, "cutoff", "Cutoff", 20.0, 20_000.0, d.cutoff)
            .with_unit(ParamUnit::Hertz)
            .with_curve(ParamValueCurve::Logarithmic),
        ParamDescriptor::percent(P_RESONANCE, "resonance", "Resonance", d.resonance),
        ParamDescriptor::new(
            P_FILTER_ENV,
            "filter_env",
            "Filter Envelope",
            0.0,
            72.0,
            d.filter_env,
        )
        .with_unit(ParamUnit::Semitones),
        ParamDescriptor::percent(P_KEY_TRACK, "key_track", "Key Tracking", d.key_track),
        ParamDescriptor::percent(
            P_FILTER_VELOCITY,
            "filter_velocity",
            "Velocity to Filter",
            d.filter_velocity,
        ),
        seconds(
            P_FILTER_ATTACK,
            "filter_attack",
            "Filter Attack",
            4.0,
            d.filter_attack,
        ),
        seconds(
            P_FILTER_DECAY,
            "filter_decay",
            "Filter Decay",
            8.0,
            d.filter_decay,
        ),
        ParamDescriptor::percent(
            P_FILTER_SUSTAIN,
            "filter_sustain",
            "Filter Sustain",
            d.filter_sustain,
        ),
        seconds(
            P_FILTER_RELEASE,
            "filter_release",
            "Filter Release",
            8.0,
            d.filter_release,
        ),
        seconds(P_ATTACK, "attack", "Attack", 4.0, d.attack),
        seconds(P_DECAY, "decay", "Decay", 8.0, d.decay),
        ParamDescriptor::percent(P_SUSTAIN, "sustain", "Sustain", d.sustain),
        seconds(P_RELEASE, "release", "Release", 8.0, d.release),
        ParamDescriptor::new(P_GLIDE, "glide", "Glide", 0.0, 1.0, d.glide)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(2.0)),
        ParamDescriptor::new(
            P_VIBRATO_RATE,
            "vibrato_rate",
            "Vibrato Rate",
            0.1,
            12.0,
            d.vibrato_rate,
        )
        .with_unit(ParamUnit::Hertz),
        ParamDescriptor::new(P_VIBRATO, "vibrato", "Vibrato", 0.0, 2.0, d.vibrato)
            .with_unit(ParamUnit::Semitones)
            .with_curve(ParamValueCurve::Power(2.0)),
        ParamDescriptor::new(P_MOD_DEPTH, "mod_depth", "Mod Depth", 0.0, 2.0, d.mod_depth)
            .with_unit(ParamUnit::Semitones)
            .with_curve(ParamValueCurve::Power(2.0)),
        ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, d.level),
        ParamDescriptor::percent(P_DRIVE, "drive", "Drive", d.drive),
        ParamDescriptor::new(P_LOW_CUT, "low_cut", "Low Cut", 20.0, 2_000.0, d.low_cut)
            .with_unit(ParamUnit::Hertz)
            .with_curve(ParamValueCurve::Logarithmic),
        ParamDescriptor::percent(P_SINE_SUB, "sine_sub", "Sine Sub", d.sine_sub),
        ParamDescriptor::new(
            P_PITCH_ENV,
            "pitch_env",
            "Pitch Envelope",
            0.0,
            48.0,
            d.pitch_env,
        )
        .with_unit(ParamUnit::Semitones)
        .with_curve(ParamValueCurve::Power(2.0)),
        ParamDescriptor::new(
            P_PITCH_DECAY,
            "pitch_decay",
            "Pitch Decay",
            0.001,
            1.0,
            d.pitch_decay,
        )
        .with_unit(ParamUnit::Seconds)
        .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::percent(P_FM, "fm", "FM Depth", d.fm),
        ParamDescriptor::new(P_FM_RATIO, "fm_ratio", "FM Ratio", 0.5, 4.0, d.fm_ratio)
            .with_steps(8),
        ParamDescriptor::new(
            P_FILTER_LFO,
            "filter_lfo",
            "Filter LFO",
            0.0,
            72.0,
            d.filter_lfo,
        )
        .with_unit(ParamUnit::Semitones),
        ParamDescriptor::new(
            P_LFO_RATE,
            "lfo_rate",
            "LFO Rate",
            0.0,
            (LFO_DIVISIONS.len() - 1) as f32,
            d.lfo_rate,
        )
        .with_choices(LFO_DIVISION_CHOICES),
    ]
}

impl Parameterized for Analog {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        if self.params.set(id, value) {
            self.refresh();
        }
    }
}

impl SegmentRenderer for Analog {
    fn handle_event(&mut self, event: &NoteEvent) {
        match *event {
            NoteEvent::NoteOn {
                key,
                pitch,
                velocity,
                ..
            } => self.note_on(key, pitch, velocity),
            NoteEvent::NoteOff { key, .. } => self.note_off(key),
            NoteEvent::AllNotesOff { .. } => {
                for index in self.allocator.release_all() {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.envelope.release();
                        voice.filter_envelope.release();
                    }
                }
            }
            NoteEvent::AllSoundOff { .. } => {
                for index in self.allocator.release_all() {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.envelope.kill();
                        voice.filter_envelope.kill();
                    }
                }
                self.last_voice = None;
            }
            NoteEvent::PitchBend { semitones, .. } => {
                self.bend =
                    finite_or(semitones, 0.0).clamp(-MAX_BEND_SEMITONES, MAX_BEND_SEMITONES);
            }
            NoteEvent::Controller { number, value, .. } => {
                if number == 7 {
                    self.channel_volume = finite_or(value, 1.0).clamp(0.0, 1.0).powi(2);
                }
                if number == CC_MODULATION {
                    self.modulation = finite_or(value, 0.0).clamp(0.0, 1.0);
                    self.refresh();
                }
            }
        }
    }

    fn render_segment(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        let Some((first, rest)) = out.channels_mut().split_first_mut() else {
            return;
        };
        let Some(left) = first.get_mut(start..end) else {
            return;
        };
        let mut right = rest
            .first_mut()
            .and_then(|channel| channel.get_mut(start..end));
        left.fill(0.0);
        if let Some(right) = right.as_deref_mut() {
            right.fill(0.0);
        }

        let waveform = self.waveform;
        let pulse = waveform == Waveform::Square;
        let unison = self.unison;
        let (ratios, gains, signs) = (self.ratios, self.gains, self.pwm_signs);
        let (pulse_width, pwm_depth) = (self.pulse_width, self.pwm_depth * 0.45);
        let (sub_level, noise_level) = (self.sub_level, self.noise_level);
        let resonant_damping = damping(self.resonance);
        let rate = self.sample_rate;
        let bend = self.bend;
        let vibrato_depth = self.vibrato_depth;
        let glide_coeff = self.glide_coeff;
        let drive = (self.drive > 0.0).then(|| Drive::new(self.drive));
        let (floor, ceiling) = cutoff_bounds(rate);
        let low_cut = (self.low_cut > LOW_CUT_OFF_HZ).then(|| {
            let g = (PI * self.low_cut.clamp(floor, ceiling) / rate).tan();
            SvfCoefficients::new(g, SQRT_2)
        });
        let sine_sub = self.sine_sub;
        let pitch_fall = self.pitch_fall;
        let (fm_cycles, fm_ratio) = (self.fm_cycles, self.fm_ratio);
        let lfo_semitones = self.filter_lfo / 12.0;
        let lfo_step = self.bpm / 60.0 / self.lfo_beats / rate;

        for index in 0..self.voices.len() {
            if !self.voices[index].envelope.is_active() {
                continue;
            }
            let (pitch, velocity) = (self.voices[index].pitch, self.voices[index].velocity);
            // Everything the cutoff depends on but the envelope, so the per-sample work is one
            // multiply by a power of two.
            let fixed = self.cutoff_for(pitch, velocity, 0.0);
            let velocity_scale = 1.0 - self.filter_velocity + self.filter_velocity * velocity;
            let sweep = self.filter_env_semitones * velocity_scale / 12.0;
            let voice = &mut self.voices[index];
            voice.target = pitch_to_hz(pitch + bend);

            for frame in 0..left.len() {
                voice.frequency += (voice.target - voice.frequency) * glide_coeff;
                let swing = match vibrato_depth > 0.0 {
                    true => (voice.vibrato.next() * vibrato_depth / 12.0).exp2(),
                    false => {
                        voice.vibrato.next();
                        1.0
                    }
                };
                let mut frequency = voice.frequency * swing;
                if voice.pitch_offset != 0.0 {
                    frequency *= (voice.pitch_offset / 12.0).exp2();
                    voice.pitch_offset *= pitch_fall;
                    if voice.pitch_offset.abs() < 1e-4 {
                        voice.pitch_offset = 0.0;
                    }
                }
                let modulation = if fm_cycles > 0.0 {
                    let deviation = (voice.fm_phase * TAU).sin() * fm_cycles;
                    voice.fm_phase += frequency * fm_ratio / rate;
                    voice.fm_phase -= voice.fm_phase.floor();
                    deviation
                } else {
                    0.0
                };
                let wobble = voice.pwm.next() * pwm_depth;
                let (mut l, mut r) = (0.0f32, 0.0f32);
                for slot in 0..unison {
                    let oscillator = &mut voice.oscillators[slot];
                    oscillator.set_frequency(frequency * ratios[slot]);
                    if pulse {
                        oscillator.set_pulse_width(pulse_width + wobble * signs[slot]);
                    }
                    let sample = if fm_cycles > 0.0 {
                        oscillator.next_modulated(waveform, modulation)
                    } else {
                        oscillator.next(waveform)
                    };
                    l += sample * gains[slot].0;
                    r += sample * gains[slot].1;
                }
                let mut centre = 0.0;
                if sub_level > 0.0 {
                    voice.sub.set_frequency(frequency * 0.5);
                    centre += voice.sub.next(Waveform::Square) * sub_level;
                }
                if noise_level > 0.0 {
                    centre += white(&mut voice.noise) * noise_level;
                }

                let envelope = voice.filter_envelope.process();
                let mut opening = sweep * envelope;
                if lfo_semitones > 0.0 {
                    // From closed, up to the depth and back: a note's first wobble starts where
                    // the cutoff knob is, on the beat the note was struck.
                    opening += lfo_semitones * 0.5 * (1.0 - (voice.wobble_phase * TAU).cos());
                    voice.wobble_phase += lfo_step;
                    voice.wobble_phase -= voice.wobble_phase.floor();
                }
                let cutoff = (fixed * opening.exp2()).clamp(floor, ceiling);
                voice.cutoff = cutoff;
                let g = (PI * cutoff / rate).tan();
                let resonant = SvfCoefficients::new(g, resonant_damping);
                let plain = SvfCoefficients::new(g, SQRT_2);
                let amp = voice.envelope.process() * velocity;
                let mut l = voice.filters[0].process(saturate(l + centre), resonant, plain);
                let mut r = voice.filters[1].process(saturate(r + centre), resonant, plain);
                if let Some(drive) = drive {
                    l = drive.process(l);
                    r = drive.process(r);
                }
                if let Some(low_cut) = low_cut {
                    l = voice.highpass[0].highpass(l, low_cut, SQRT_2);
                    r = voice.highpass[1].highpass(r, low_cut, SQRT_2);
                }
                if sine_sub > 0.0 {
                    let sine = (voice.sine_phase * TAU).sin() * sine_sub;
                    voice.sine_phase += frequency / rate;
                    voice.sine_phase -= voice.sine_phase.floor();
                    l += sine;
                    r += sine;
                }
                let (l, r) = (l * amp, r * amp);
                match right.as_deref_mut() {
                    Some(right) => {
                        left[frame] += l;
                        right[frame] += r;
                    }
                    None => left[frame] += 0.5 * (l + r),
                }
            }

            let level = voice.envelope.level() * velocity;
            let finished = voice.envelope.is_finished();
            self.allocator.set_level(index, level);
            if finished {
                self.allocator.retire(index);
            }
        }

        let output_gain = self.output_gain * self.channel_volume;
        for sample in left.iter_mut() {
            *sample *= output_gain;
        }
        if let Some(right) = right {
            for sample in right.iter_mut() {
                *sample *= output_gain;
            }
        }
    }
}

impl Instrument for Analog {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            self.patch.id(),
            self.patch.name(),
            self.patch.description(),
            PluginCategory::Synth,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = crate::sample_rate_f32(ctx.sample_rate);
        self.voices.clear();
        self.voices.reserve(VOICE_COUNT);
        for index in 0..VOICE_COUNT {
            self.voices.push(AnalogVoice::new(index, self.sample_rate));
        }
        self.allocator.prepare(VOICE_COUNT);
        self.bend = 0.0;
        self.modulation = 0.0;
        self.channel_volume = 1.0;
        self.last_voice = None;
        self.refresh();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.envelope.silence();
            voice.filter_envelope.silence();
            voice.restart();
            voice.pwm.reset();
        }
        self.allocator.clear();
        self.bend = 0.0;
        self.modulation = 0.0;
        self.channel_volume = 1.0;
        self.last_voice = None;
        self.refresh();
        self.observer.reset(MotionGeometry::Oscillator);
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = ctx.block_frames.min(out.frame_count());
        let bpm = ctx.bpm as f32;
        self.bpm = if bpm.is_finite() && bpm > 0.0 {
            bpm.min(1_000.0)
        } else {
            DEFAULT_BPM
        };
        render_segments(self, events, out, frames);
        downmix_to_extra_channels(out, 0, frames);
        if self.observer.due(frames, self.sample_rate) {
            self.publish_motion();
        }
    }

    fn motion_monitor(&self) -> Option<std::sync::Arc<auris_core::motion::MotionMonitor>> {
        Some(self.observer.monitor())
    }

    fn active_voices(&self) -> usize {
        self.allocator.active_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Rig, goertzel, peak, peak_db, rms, zero_crossing_hz};

    const RATE: f64 = 48_000.0;

    fn on(frame: u32, pitch: u8) -> NoteEvent {
        NoteEvent::note_on(frame, pitch, 1.0)
    }

    fn off(frame: u32, pitch: u8) -> NoteEvent {
        NoteEvent::NoteOff { frame, key: pitch }
    }

    fn rig(patch: AnalogPatch) -> Rig {
        Rig::new(Box::new(Analog::new(patch)), RATE, 512, 2)
    }

    /// A plain patch whose measurements are not coloured by its own settings: one sine-like
    /// triangle, the filter wide open, a flat envelope.
    fn plain() -> Rig {
        let mut rig = rig(AnalogPatch::SuperSaw);
        for (key, value) in [
            ("waveform", 2.0),
            ("unison", 1.0),
            ("cutoff", 20_000.0),
            ("resonance", 0.0),
            ("attack", 0.0),
            ("decay", 0.0),
            ("sustain", 1.0),
            ("release", 0.02),
            ("level", 0.0),
        ] {
            rig.set_param(key, value);
        }
        rig
    }

    #[test]
    fn every_patch_is_its_own_instrument_with_the_same_controls() {
        let reference: Vec<_> = Analog::new(AnalogPatch::SuperSaw)
            .parameters()
            .iter()
            .map(|p| p.key.to_string())
            .collect();
        assert_eq!(reference.len(), 37);
        for patch in AnalogPatch::ALL {
            let synth = Analog::new(patch);
            assert_eq!(synth.descriptor().id, patch.id());
            assert_eq!(AnalogPatch::from_id(patch.id()), Some(patch));
            assert_eq!(synth.descriptor().category, PluginCategory::Synth);
            let keys: Vec<_> = synth
                .parameters()
                .iter()
                .map(|p| p.key.to_string())
                .collect();
            assert_eq!(keys, reference, "{patch:?} has a different control set");
            for (index, descriptor) in synth.parameters().iter().enumerate() {
                assert_eq!(descriptor.id.index(), index, "id of `{}`", descriptor.key);
                assert!(
                    (descriptor.min..=descriptor.max).contains(&descriptor.default),
                    "{patch:?} default for {} is out of range",
                    descriptor.key
                );
            }
        }
    }

    #[test]
    fn a_single_oscillator_sounds_at_the_note() {
        let mut rig = plain();
        let rendered = rig.render(24_000, &[on(0, 69)]);
        let measured = zero_crossing_hz(&rendered[2_048..], RATE);
        assert!((measured / 440.0 - 1.0).abs() < 0.01, "{measured:.2} Hz");
    }

    #[test]
    fn the_supersaw_stack_spreads_its_copies_across_the_detune() {
        let layout = unison_layout(7, 0.3, 0.8, 1.0);
        let cents: Vec<f32> = layout.ratios.iter().map(|r| 1200.0 * r.log2()).collect();
        // Centre exactly on the note, outermost copies near ±30 cents, all distinct.
        assert!(cents[3].abs() < 1e-3, "{cents:?}");
        assert!((cents[0] + 30.0).abs() < 3.0 && (cents[6] - 30.0).abs() < 3.0);
        let mut sorted = cents.clone();
        sorted.sort_by(f32::total_cmp);
        assert!(sorted.windows(2).all(|w| w[1] - w[0] > 3.0), "{cents:?}");
        // Spacing is uneven, so the beat between neighbours never becomes one regular pulse.
        let steps: Vec<f32> = sorted.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            steps.windows(2).any(|w| (w[0] - w[1]).abs() > 0.5),
            "{steps:?}"
        );
        // Each side holds as much as the other, so the downmix is the centred stack.
        let left: f32 = layout.gains.iter().map(|g| g.0).sum();
        let right: f32 = layout.gains.iter().map(|g| g.1).sum();
        assert!((left - right).abs() < 1e-4, "{left} vs {right}");
        // And the whole stack carries the power of one oscillator.
        let power: f32 = layout
            .gains
            .iter()
            .map(|(l, r)| ((l + r) / 2.0).powi(2))
            .sum();
        assert!((power - 1.0).abs() < 1e-4, "{power}");
    }

    #[test]
    fn the_supersaw_is_wide_and_a_single_oscillator_is_not() {
        let mut wide = rig(AnalogPatch::SuperSaw);
        let (l, r) = wide.render_stereo(24_000, &[on(0, 60)]);
        let side: Vec<f32> = l.iter().zip(&r).map(|(l, r)| (l - r) * 0.5).collect();
        let mid: Vec<f32> = l.iter().zip(&r).map(|(l, r)| (l + r) * 0.5).collect();
        let ratio = rms(&side[4_800..]) / rms(&mid[4_800..]);
        assert!(ratio > 0.3, "side/mid {ratio:.3}");

        let mut narrow = rig(AnalogPatch::Bass);
        let (l, r) = narrow.render_stereo(4_096, &[on(0, 36)]);
        assert_eq!(l, r, "a zero-width patch should be mono");
    }

    #[test]
    fn detuned_copies_beat_and_a_unison_of_one_does_not() {
        // The level of a held note over 50 ms windows: a detuned stack swells and falls as its
        // copies drift in and out of phase, a single oscillator holds still.
        fn swing(unison: f32, detune: f32) -> f32 {
            let mut rig = plain();
            rig.set_param("waveform", 0.0);
            rig.set_param("unison", unison);
            rig.set_param("detune", detune);
            rig.set_param("mix", 1.0);
            rig.set_param("width", 0.0);
            let rendered = rig.render(96_000, &[on(0, 57)]);
            let levels: Vec<f32> = rendered[4_800..].chunks(2_400).map(rms).collect();
            let (low, high) = levels
                .iter()
                .fold((f32::MAX, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            high / low
        }
        assert!(swing(1.0, 0.0) < 1.02);
        assert!(swing(3.0, 0.3) > 1.3, "{}", swing(3.0, 0.3));
    }

    #[test]
    fn the_cutoff_takes_the_top_off_a_saw() {
        fn third_harmonic(cutoff: f32) -> f64 {
            let mut rig = plain();
            rig.set_param("waveform", 0.0);
            rig.set_param("cutoff", cutoff);
            let rendered = rig.render(24_000, &[on(0, 57)]);
            goertzel(&rendered[4_096..], RATE, 660.0)
        }
        let open = third_harmonic(20_000.0);
        let closed = third_harmonic(300.0);
        // Two octaves above a 24 dB/octave cutoff the third harmonic is down by about 48 dB;
        // allow plenty for the bilinear warp and the second-order transition.
        assert!(open > 0.05, "open {open}");
        assert!(closed < open * 0.05, "closed {closed} against open {open}");
    }

    #[test]
    fn resonance_lifts_the_harmonic_at_the_cutoff() {
        fn at_cutoff(resonance: f32) -> f64 {
            let mut rig = plain();
            rig.set_param("waveform", 0.0);
            rig.set_param("cutoff", 880.0);
            rig.set_param("resonance", resonance);
            let rendered = rig.render(24_000, &[on(0, 57)]);
            goertzel(&rendered[4_096..], RATE, 880.0)
        }
        let flat = at_cutoff(0.0);
        let peaked = at_cutoff(1.0);
        assert!(peaked > flat * 2.5, "{peaked} against {flat}");
    }

    #[test]
    fn the_filter_envelope_opens_and_closes_the_pluck() {
        let mut rig = rig(AnalogPatch::Pluck);
        rig.set_param("sustain", 1.0);
        rig.set_param("unison", 1.0);
        let rendered = rig.render(48_000, &[on(0, 57)]);
        let bright = goertzel(&rendered[240..2_640], RATE, 1_760.0);
        let dark = goertzel(&rendered[36_000..], RATE, 1_760.0);
        assert!(dark < bright * 0.1, "attack {bright}, tail {dark}");
    }

    #[test]
    fn key_tracking_moves_the_cutoff_with_the_note() {
        let mut synth = Analog::new(AnalogPatch::Bass);
        synth.set_param_by_key("key_track", 1.0);
        synth.prepare(&PrepareContext::new(RATE, 512, 2));
        let c4 = synth.cutoff_for(60.0, 1.0, 0.0);
        let c5 = synth.cutoff_for(72.0, 1.0, 0.0);
        assert!((c5 / c4 - 2.0).abs() < 1e-3, "{c4} → {c5}");
    }

    #[test]
    fn velocity_reaches_the_filter_only_as_far_as_asked() {
        let mut synth = Analog::new(AnalogPatch::Brass);
        synth.prepare(&PrepareContext::new(RATE, 512, 2));
        synth.set_param_by_key("filter_velocity", 0.0);
        assert_eq!(
            synth.cutoff_for(60.0, 0.2, 1.0),
            synth.cutoff_for(60.0, 1.0, 1.0)
        );
        synth.set_param_by_key("filter_velocity", 1.0);
        assert!(synth.cutoff_for(60.0, 0.2, 1.0) < synth.cutoff_for(60.0, 1.0, 1.0) / 4.0);
    }

    #[test]
    fn the_sub_oscillator_sounds_an_octave_down() {
        let mut rig = plain();
        rig.set_param("sub", 1.0);
        let rendered = rig.render(24_000, &[on(0, 69)]);
        assert!(goertzel(&rendered[4_096..], RATE, 220.0) > 0.2);
        let mut without = plain();
        let rendered = without.render(24_000, &[on(0, 69)]);
        assert!(goertzel(&rendered[4_096..], RATE, 220.0) < 0.01);
    }

    #[test]
    fn pulse_width_modulation_moves_the_second_harmonic() {
        // A square has no even harmonics; a moving width keeps bringing them in and out.
        let mut rig = plain();
        rig.set_param("waveform", 1.0);
        rig.set_param("pulse_width", 0.5);
        rig.set_param("pwm", 0.3);
        rig.set_param("pwm_rate", 0.5);
        let rendered = rig.render(96_000, &[on(0, 57)]);
        let seconds: Vec<f64> = rendered
            .chunks(1_200)
            .map(|chunk| goertzel(chunk, RATE, 440.0))
            .collect();
        let high = seconds.iter().cloned().fold(0.0, f64::max);
        let low = seconds.iter().cloned().fold(f64::MAX, f64::min);
        assert!(high > 0.1 && low < high * 0.2, "{seconds:?}");
    }

    #[test]
    fn glide_starts_the_next_note_at_the_last_one() {
        let mut rig = plain();
        rig.set_param("glide", 0.2);
        rig.render(4_800, &[on(0, 57)]);
        let next = rig.render(1_200, &[off(0, 57), on(0, 69)]);
        let measured = zero_crossing_hz(&next[..1_200], RATE);
        assert!(measured < 330.0, "glided note started at {measured:.1} Hz");
    }

    #[test]
    fn every_patch_sounds_at_a_sensible_level_and_falls_silent() {
        for patch in AnalogPatch::ALL {
            let mut rig = rig(patch);
            let pitch = if patch == AnalogPatch::Bass { 36 } else { 60 };
            let rendered = rig.render(96_000 * 2, &[on(0, pitch), off(48_000, pitch)]);
            let held = &rendered[..48_000];
            let level = gain_to_db_f(rms(held));
            assert!(
                (-36.0..-6.0).contains(&level),
                "{patch:?} holds at {level:.1} dB RMS"
            );
            assert!(peak(held) < 1.0, "{patch:?} peaked at {}", peak(held));
            let tail = &rendered[rendered.len() - 4_800..];
            assert!(peak_db(tail) < -60.0, "{patch:?} still rings");
            assert_eq!(rig.instrument.active_voices(), 0, "{patch:?}");
        }
    }

    fn gain_to_db_f(value: f32) -> f32 {
        auris_core::param::gain_to_db(value)
    }

    #[test]
    fn a_chord_at_full_tilt_stays_finite_and_bounded() {
        let descriptors = descriptors(AnalogPatch::SuperSaw);
        for extreme in [0.0f32, 0.5, 1.0] {
            let mut synth = Analog::new(AnalogPatch::SuperSaw);
            for descriptor in &descriptors {
                synth.set_param(descriptor.id, descriptor.denormalize(extreme));
            }
            let mut rig = Rig::new(Box::new(synth), RATE, 512, 2);
            let mut events: Vec<NoteEvent> = (0..12).map(|i| on(i, 24 + i as u8 * 9)).collect();
            events.push(NoteEvent::PitchBend {
                frame: 20,
                semitones: 240.0,
            });
            events.push(off(400, 24));
            let rendered = rig.render(16_384, &events);
            assert!(
                rendered.iter().all(|s| s.is_finite()),
                "extreme {extreme} went non-finite"
            );
            assert!(
                peak(&rendered) < 64.0,
                "extreme {extreme}: {}",
                peak(&rendered)
            );
        }
    }

    #[test]
    fn drive_adds_harmonics_without_moving_the_level_far() {
        // A triangle's third harmonic is a ninth of its fundamental; clipping squares it off.
        fn third_over_first(drive: f32) -> (f64, f32) {
            let mut rig = plain();
            rig.set_param("drive", drive);
            let rendered = rig.render(24_000, &[on(0, 57)]);
            let tail = &rendered[4_096..];
            let ratio = goertzel(tail, RATE, 660.0) / goertzel(tail, RATE, 220.0);
            (ratio, gain_to_db_f(rms(tail)))
        }
        let (clean, clean_level) = third_over_first(0.0);
        let (driven, driven_level) = third_over_first(0.8);
        assert!(clean < 0.13, "clean third/first {clean}");
        assert!(
            driven > clean * 2.5,
            "driven {driven} against clean {clean}"
        );
        assert!(
            (driven_level - clean_level).abs() < 6.0,
            "{clean_level:.1} dB → {driven_level:.1} dB"
        );
    }

    #[test]
    fn the_low_cut_hollows_the_stack_and_the_sine_sub_fills_the_bottom() {
        fn at(low_cut: f32, sine_sub: f32, hz: f64) -> f64 {
            let mut rig = plain();
            rig.set_param("waveform", 0.0);
            rig.set_param("low_cut", low_cut);
            rig.set_param("sine_sub", sine_sub);
            let rendered = rig.render(48_000, &[on(0, 33)]);
            goertzel(&rendered[8_192..], RATE, hz)
        }
        let a1 = pitch_to_hz(33.0) as f64;
        let open = at(20.0, 0.0, a1);
        let cut = at(300.0, 0.0, a1);
        // Two and a half octaves under a 12 dB/octave corner: about 29 dB down.
        assert!(cut < open * 0.05, "cut {cut} against open {open}");
        // The eighth harmonic, at the corner, is barely touched.
        assert!(at(300.0, 0.0, 8.0 * a1) > at(20.0, 0.0, 8.0 * a1) * 0.5);
        // The sine comes after the low cut, so it is the whole of the fundamental again.
        let filled = at(300.0, 0.5, a1);
        assert!(filled > cut * 20.0, "filled {filled}");
        assert!(at(300.0, 0.5, 2.0 * a1) < filled * 0.1, "the sine is clean");
    }

    #[test]
    fn the_pitch_envelope_drops_the_note_into_place() {
        let mut rig = plain();
        rig.set_param("pitch_env", 12.0);
        rig.set_param("pitch_decay", 0.02);
        let rendered = rig.render(24_000, &[on(0, 57)]);
        let early = zero_crossing_hz(&rendered[..480], RATE);
        let settled = zero_crossing_hz(&rendered[9_600..], RATE);
        assert!(early > 330.0, "the first 10 ms sit at {early:.1} Hz");
        assert!((settled / 220.0 - 1.0).abs() < 0.01, "{settled:.2} Hz");
    }

    #[test]
    fn fm_spreads_a_triangle_into_sidebands() {
        fn upper(fm: f32) -> f64 {
            let mut rig = plain();
            rig.set_param("fm", fm);
            let rendered = rig.render(24_000, &[on(0, 57)]);
            let tail = &rendered[4_096..];
            // A triangle has no even harmonics; FM at the note's own frequency brings them in.
            goertzel(tail, RATE, 440.0) / goertzel(tail, RATE, 220.0).max(1e-9)
        }
        assert!(upper(0.0) < 0.01, "{}", upper(0.0));
        assert!(upper(0.5) > 0.2, "{}", upper(0.5));
    }

    #[test]
    fn the_filter_lfo_follows_the_tempo_from_the_note_on() {
        // At 120 BPM a quarter-note LFO is 2 Hz: closed on every half second from the note on,
        // open a quarter of a second after each.
        let mut rig = plain();
        rig.set_param("waveform", 0.0);
        rig.set_param("cutoff", 200.0);
        rig.set_param("filter_lfo", 48.0);
        rig.set_param("lfo_rate", 2.0);
        let rendered = rig.render(72_000, &[on(0, 45)]);
        // Windowed, so the loud low harmonics do not leak into the one being read.
        let bright = |centre: usize| {
            let span = 2_400;
            let windowed: Vec<f32> = rendered[centre - span / 2..centre + span / 2]
                .iter()
                .enumerate()
                .map(|(at, sample)| sample * (PI * at as f32 / span as f32).sin().powi(2))
                .collect();
            goertzel(&windowed, RATE, 1_760.0)
        };
        for cycle in 1..=2 {
            let at = cycle * 24_000;
            let (closed, open) = (bright(at), bright(at + 12_000));
            assert!(
                open > closed * 10.0,
                "cycle {cycle}: open {open}, closed {closed}"
            );
        }
    }

    #[test]
    fn the_house_bass_is_a_sine_under_a_hollowed_peak() {
        // The shape the patch was set to: the sine carries the fundamental, the second harmonic
        // is cut away, and the saw's harmonics come back a few hundred hertz up.
        let mut rig = rig(AnalogPatch::HouseBass);
        let rendered = rig.render(9_600, &[on(0, 27)]);
        let held = &rendered[2_400..9_600];
        let f0 = pitch_to_hz(27.0) as f64;
        let level = |harmonic: f64| goertzel(held, RATE, harmonic * f0);
        // Power in whole harmonics, so the beating of the two saws averages out.
        let power = |harmonics: std::ops::RangeInclusive<u32>| -> f64 {
            harmonics.map(|h| level(f64::from(h)).powi(2)).sum()
        };
        assert!(
            level(1.0) > level(2.0) * 4.0,
            "{} vs {}",
            level(1.0),
            level(2.0)
        );
        // 60–120 Hz against 250–500 Hz: the hollow measured in the reference is wider than this,
        // but the shape is the one the patch is for.
        let (hollow, peak) = (power(2..=3), power(7..=12));
        assert!(peak > hollow * 4.0, "peak {peak}, hollow {hollow}");
    }

    #[test]
    fn reset_silences_and_rewinds() {
        let mut rig = rig(AnalogPatch::Pad);
        rig.render(4_096, &[on(0, 60)]);
        rig.instrument.reset();
        assert!(rig.render(512, &[]).iter().all(|s| *s == 0.0));
        assert_eq!(rig.instrument.active_voices(), 0);
    }

    #[test]
    fn the_displayed_cycle_follows_the_filter() {
        let mut open = [0.0; MOTION_POINTS];
        filtered_cycle(Waveform::Saw, 0.5, 40.0, 0.0, &mut open);
        let mut closed = [0.0; MOTION_POINTS];
        filtered_cycle(Waveform::Saw, 0.5, 1.0, 0.0, &mut closed);
        let mut open_modes = [0.0; MOTION_MODES];
        let mut closed_modes = [0.0; MOTION_MODES];
        harmonics(&open, &mut open_modes);
        harmonics(&closed, &mut closed_modes);
        // Open, the saw keeps its 1/n series; closed at the fundamental, the fourth is gone.
        assert!((open_modes[1] - 0.5).abs() < 0.05, "{open_modes:?}");
        assert!(closed_modes[3] < 0.02, "{closed_modes:?}");
    }
}
