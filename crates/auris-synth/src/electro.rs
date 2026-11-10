//! The electronic drum machines behind `auris.synth.electro_kit`, `auris.synth.edm_kit` and
//! `auris.synth.trap_kit`.
//!
//! One engine, three kits, in the same way [`Analog`](crate::Analog) is one engine with seven
//! patches: every kit is its own instrument in the browser, but they share one parameter set, so
//! the EDM kit's kick can be turned into the trap kit's by its knobs and a project that names one
//! stores nothing the others could not also hold.
//!
//! Nothing here is a recording. Every pad is written down as a formula — the way the analogue
//! drum machines that defined electronic music built their sounds out of a handful of
//! oscillators, a noise source and some filters:
//!
//! ```text
//!  kick, boom, tom   sine whose pitch falls exponentially onto its tune, + click, → soft clip
//!  snare             two damped sines (the heads)            + high-passed noise (the wires)
//!  clap              band-passed noise in four quick bursts, then a short tail
//!  hats, cymbals     six detuned square oscillators + noise → high-pass → band-pass
//!  cowbell           two square oscillators a fifth-ish apart → band-pass
//!  rim               two very short sines + a little noise
//! ```
//!
//! The kick is the point of the kit. Its frequency is
//! `f(t) = tune + (start − tune) · e^(−t/τ)`, so the hit lands as a fast downward sweep — the
//! "punch" — and then rings as a sine at the tune, which is the sub-bass "boom". `drive` puts the
//! sum through `tanh(g·x) / tanh(g)`: at a low gain that is a rounder, louder sine; at a high one
//! it squares the boom off into the odd harmonics that let a sub-bass be heard on speakers too
//! small to reproduce its fundamental, which is most of the reason club and trap kicks are
//! distorted at all.
//!
//! # Keys
//!
//! The pads sit on their General MIDI keys, so a pattern written for any GM kit plays here:
//! 35 is a longer, lower "boom" kick, 36 the kick, 37 rim, 38/40 snare, 39 clap, 42/44 closed hat,
//! 46 open hat, 41–50 six toms tuned by key, 49/52/55/57 crash, 51/53/59 ride, 56 cowbell. A closed
//! hat chokes a ringing open one, as on the machines.
//!
//! # Realtime behaviour
//!
//! Every hit is a fixed set of recursions — multiplied envelopes, phase accumulators, two
//! biquads — set up at note on from values already computed in `refresh`. Note offs do not stop a
//! hit: a drum is a one-shot. Each hit re-seeds its own noise generator from its key, so the same
//! hit always renders the same samples, whatever played before it.

use std::f32::consts::TAU;

use auris_core::motion::{MOTION_MODES, MOTION_POINTS, MotionFrame, MotionGeometry};
use auris_core::param::db_to_gain;
use auris_core::{
    AudioBuffer, DrumMap, DrumRole, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit,
    ParamValueCurve, Parameterized, PluginCategory, PluginDescriptor, PrepareContext,
    ProcessContext,
};
use auris_dsp::adsr::{SEGMENT_TIME_CONSTANTS, SILENCE_LEVEL};
use auris_dsp::{Biquad, BiquadCoefficients};

use crate::observe::{Observer, harmonics, recent_voices};
use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};
use crate::voice::{VoiceAllocator, VoiceState};

/// Voices in the pool: a sixteenth-note hat over a ringing crash and a long boom still fits.
const VOICE_COUNT: usize = 24;

const P_KICK_TUNE: u32 = 0;
const P_KICK_DECAY: u32 = 1;
const P_KICK_PUNCH: u32 = 2;
const P_KICK_CLICK: u32 = 3;
const P_KICK_DRIVE: u32 = 4;
const P_SNARE_TONE: u32 = 5;
const P_SNARE_SNAP: u32 = 6;
const P_SNARE_DECAY: u32 = 7;
const P_CLAP_DECAY: u32 = 8;
const P_HAT_TONE: u32 = 9;
const P_HAT_DECAY: u32 = 10;
const P_METAL: u32 = 11;
const P_CYMBAL_DECAY: u32 = 12;
const P_TOM_DECAY: u32 = 13;
const P_LEVEL: u32 = 14;

/// Octaves the kick's sweep starts above its tune at a punch of 1.
const PUNCH_OCTAVES: f32 = 4.0;

/// The six square oscillators of the metallic voice, in hertz: the inharmonic cluster the
/// classic analogue machines tuned their hats and cymbals from. No two are in a simple ratio,
/// so their sum never settles into a pitch.
const METAL_HZ: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

/// The cowbell's two squares: the same machines used two of the cluster, a little above.
const COWBELL_HZ: [f32; 2] = [587.0, 845.0];

/// Bursts a clap is made of, and the time between them. Several hands never clap together; the
/// smear of four onsets inside 30 ms is what the ear reads as a crowd rather than one slap.
const CLAP_BURSTS: u32 = 4;
const CLAP_SPACING_SECONDS: f32 = 0.0085;
const CLAP_BURST_SECONDS: f32 = 0.006;

/// A closed hat silences a ringing open one over this long, short enough to read as a choke and
/// long enough not to click.
const CHOKE_SECONDS: f32 = 0.006;

/// Attack ramp at the start of every hit. A step from silence is a broadband click; a third of a
/// millisecond is far below what the ear hears as a softened attack.
const ATTACK_SECONDS: f32 = 0.0003;

/// Highest filter frequency as a fraction of the sample rate.
const MAX_FILTER_FRACTION: f32 = 0.45;

/// How much of a noisy pad's output its scope holds: a few milliseconds of hiss or ring.
const SCOPE_SECONDS: f32 = 0.004;

/// Periods of a tuned pad's body its scope holds, so a kick reads as a sine and not a smear.
const SCOPE_PERIODS: f32 = 2.0;

/// One of the electronic kits, each its own instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ElectroStyle {
    /// The analogue drum machine: a round, booming sine kick, snappy two-tone snare, clap and
    /// metallic hats. Electro, house, hip-hop.
    Electro,
    /// A club kit: a short, hard, clicky and distorted kick that cuts through a loud mix, with a
    /// wide clap and crisp hats. Four-on-the-floor EDM.
    Edm,
    /// A trap kit: a long, distorted sub-bass boom of a kick, tight snare and fast hats.
    Trap,
}

impl ElectroStyle {
    /// Every kit, in the order the browser lists them.
    pub const ALL: [Self; 3] = [Self::Electro, Self::Edm, Self::Trap];

    /// Stable plugin id stored in a project.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Electro => "auris.synth.electro_kit",
            Self::Edm => "auris.synth.edm_kit",
            Self::Trap => "auris.synth.trap_kit",
        }
    }

    /// The kit an id names, if it is one of these.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.id() == id)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Electro => "Electro Kit",
            Self::Edm => "EDM Kit",
            Self::Trap => "Trap Kit",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Electro => {
                "Analogue drum machine from formulas: booming sine kick, snappy snare, clap, metallic hats"
            }
            Self::Edm => "Club kit: hard clicky distorted kick, wide clap and crisp hats for EDM",
            Self::Trap => "Trap kit: long distorted sub-bass boom kick, tight snare, fast hats",
        }
    }

    /// The kit's own settings, as the parameters' defaults.
    fn defaults(self) -> [f32; 15] {
        match self {
            //              tune  decay punch click drive  snare snap  sdec  clap  htone hdec metal cym   tom   level
            Self::Electro => [
                50.0, 0.65, 0.35, 0.12, 0.12, 180.0, 0.55, 0.18, 0.2, 0.45, 0.45, 0.8, 1.8, 0.45,
                -6.0,
            ],
            Self::Edm => [
                52.0, 0.38, 0.6, 0.55, 0.5, 200.0, 0.8, 0.2, 0.28, 0.65, 0.3, 0.45, 2.2, 0.35, -6.0,
            ],
            Self::Trap => [
                43.0, 1.6, 0.42, 0.22, 0.45, 220.0, 0.85, 0.14, 0.2, 0.6, 0.32, 0.6, 1.6, 0.6, -6.0,
            ],
        }
    }
}

/// The map a drum track playing one of these kits starts with: the generation roles on the keys
/// the kit plays them from, and a named lane for every pad.
pub fn electro_drum_map() -> DrumMap {
    let mut map = DrumMap::from_voices([
        (DrumRole::Kick, 36),
        (DrumRole::Snare, 38),
        (DrumRole::ClosedHat, 42),
        (DrumRole::OpenHat, 46),
        (DrumRole::Crash, 49),
        (DrumRole::Tom, 47),
    ]);
    for (key, name) in [
        (35, "Boom Kick"),
        (37, "Rim"),
        (39, "Clap"),
        (41, "Low Tom"),
        (45, "Mid Tom"),
        (50, "High Tom"),
        (51, "Ride"),
        (56, "Cowbell"),
    ] {
        map.add_lane(key, name);
    }
    map
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pad {
    Boom,
    Kick,
    Rim,
    Snare,
    Clap,
    ClosedHat,
    OpenHat,
    Tom,
    Crash,
    Ride,
    Cowbell,
}

impl Pad {
    fn at_key(key: u8) -> Option<Self> {
        Some(match key {
            35 => Self::Boom,
            36 => Self::Kick,
            37 => Self::Rim,
            38 | 40 => Self::Snare,
            39 => Self::Clap,
            42 | 44 => Self::ClosedHat,
            46 => Self::OpenHat,
            41 | 43 | 45 | 47 | 48 | 50 => Self::Tom,
            49 | 52 | 55 | 57 => Self::Crash,
            51 | 53 | 59 => Self::Ride,
            56 => Self::Cowbell,
            _ => return None,
        })
    }

    fn is_hat(self) -> bool {
        matches!(self, Self::ClosedHat | Self::OpenHat)
    }
}

/// An exponentially decaying level: `level *= coef` every sample.
#[derive(Clone, Copy, Debug, Default)]
struct Decay {
    level: f32,
    coef: f32,
}

impl Decay {
    /// Full level, falling to -80 dB in `seconds`, the same meaning a decay time has on the
    /// [`Adsr`](auris_dsp::Adsr) every other built-in voice uses.
    fn new(seconds: f32, sample_rate: f32) -> Self {
        Self {
            level: 1.0,
            coef: decay_coef(seconds, sample_rate),
        }
    }

    fn off() -> Self {
        Self::default()
    }

    #[inline]
    fn next(&mut self) -> f32 {
        let out = self.level;
        self.level *= self.coef;
        out
    }

    fn audible(self) -> bool {
        self.level > SILENCE_LEVEL
    }
}

fn decay_coef(seconds: f32, sample_rate: f32) -> f32 {
    let samples = (seconds.max(0.0005) * sample_rate).max(1.0);
    (-SEGMENT_TIME_CONSTANTS / samples).exp()
}

/// A sine whose frequency falls exponentially from `start` onto `end`.
#[derive(Clone, Copy, Debug, Default)]
struct Sweep {
    phase: f32,
    end: f32,
    extra: f32,
    pitch_coef: f32,
    gain: f32,
    amp: Decay,
}

impl Sweep {
    fn new(start: f32, end: f32, pitch_seconds: f32, decay: f32, gain: f32, rate: f32) -> Self {
        Self {
            phase: 0.0,
            end,
            extra: (start - end).max(0.0),
            // A time constant, not a time to silence: `τ` is how quickly the punch lands.
            pitch_coef: (-1.0 / (pitch_seconds.max(0.0002) * rate)).exp(),
            gain,
            amp: if gain > 0.0 {
                Decay::new(decay, rate)
            } else {
                Decay::off()
            },
        }
    }

    #[inline]
    fn next(&mut self, inv_rate: f32) -> f32 {
        if !self.amp.audible() {
            return 0.0;
        }
        let hz = self.end + self.extra;
        self.extra *= self.pitch_coef;
        let out = (TAU * self.phase).sin() * self.amp.next() * self.gain;
        self.phase += hz * inv_rate;
        self.phase -= self.phase.floor();
        out
    }
}

/// The values the knobs give every pad, computed once per parameter change rather than per hit.
#[derive(Clone, Copy, Debug)]
struct Settings {
    kick_tune: f32,
    kick_decay: f32,
    kick_punch: f32,
    kick_click: f32,
    kick_drive: f32,
    snare_tone: f32,
    snare_snap: f32,
    snare_decay: f32,
    clap_decay: f32,
    hat_tone: f32,
    hat_decay: f32,
    metal: f32,
    cymbal_decay: f32,
    tom_decay: f32,
}

/// One hit.
#[derive(Clone, Debug)]
struct Hit {
    pad: Pad,
    velocity: f32,
    /// Samples since the hit started, for the attack ramp and the clap's bursts.
    age: u32,
    attack_step: f32,
    attack: f32,
    body: Sweep,
    second: Sweep,
    /// White noise and the metal cluster share one filtered path.
    noise_gain: f32,
    noise: Decay,
    metal_gain: f32,
    metal: Decay,
    metal_hz: [f32; 6],
    metal_phase: [f32; 6],
    metal_count: usize,
    first: Biquad,
    second_filter: Biquad,
    /// The click is filtered separately, so it stays a click under a kick's drive.
    click_gain: f32,
    click: Decay,
    click_filter: Biquad,
    drive: f32,
    drive_norm: f32,
    /// Clap: samples between bursts, and the burst envelope's own decay.
    burst_spacing: u32,
    burst_coef: f32,
    burst: f32,
    /// Fades the whole hit out once a closed hat has choked it.
    choke: f32,
    choke_coef: f32,
    rng: u32,
    /// The hit's own output, decimated into a ring the display draws, oldest first from `scope_at`.
    scope: [f32; MOTION_POINTS],
    scope_at: usize,
    scope_stride: u32,
    scope_wait: u32,
}

impl Hit {
    fn new() -> Self {
        Self {
            pad: Pad::Kick,
            velocity: 0.0,
            age: 0,
            attack_step: 1.0,
            attack: 1.0,
            body: Sweep::default(),
            second: Sweep::default(),
            noise_gain: 0.0,
            noise: Decay::off(),
            metal_gain: 0.0,
            metal: Decay::off(),
            metal_hz: [0.0; 6],
            metal_phase: [0.0; 6],
            metal_count: 0,
            first: Biquad::new(BiquadCoefficients::identity()),
            second_filter: Biquad::new(BiquadCoefficients::identity()),
            click_gain: 0.0,
            click: Decay::off(),
            click_filter: Biquad::new(BiquadCoefficients::identity()),
            drive: 0.0,
            drive_norm: 1.0,
            burst_spacing: 0,
            burst_coef: 0.0,
            burst: 0.0,
            choke: 1.0,
            choke_coef: 1.0,
            rng: 1,
            scope: [0.0; MOTION_POINTS],
            scope_at: 0,
            scope_stride: 1,
            scope_wait: 0,
        }
    }

    /// Keeps every `scope_stride`-th sample of the hit's output for the display.
    #[inline]
    fn record(&mut self, sample: f32) {
        if self.scope_wait == 0 {
            self.scope[self.scope_at] = sample;
            self.scope_at = (self.scope_at + 1) % MOTION_POINTS;
            self.scope_wait = self.scope_stride;
        }
        self.scope_wait -= 1;
    }

    /// The scope's ring unrolled, oldest sample first.
    fn scope_points(&self, out: &mut [f32; MOTION_POINTS]) {
        for (index, point) in out.iter_mut().enumerate() {
            *point = self.scope[(self.scope_at + index) % MOTION_POINTS];
        }
    }

    /// The body's pitch right now, in hertz, or zero for a pad with no tuned body.
    fn body_hz(&self) -> f32 {
        if self.body.amp.audible() {
            self.body.end + self.body.extra
        } else {
            0.0
        }
    }

    fn is_active(&self) -> bool {
        self.choke > SILENCE_LEVEL
            && (self.body.amp.audible()
                || self.second.amp.audible()
                || self.noise.audible()
                || self.metal.audible()
                || self.click.audible())
    }

    fn level(&self) -> f32 {
        let loudest = (self.body.amp.level * self.body.gain)
            .max(self.second.amp.level * self.second.gain)
            .max(self.noise.level * self.noise_gain)
            .max(self.metal.level * self.metal_gain)
            .max(self.click.level * self.click_gain);
        loudest * self.choke * self.velocity
    }

    fn silence(&mut self) {
        self.body.amp = Decay::off();
        self.second.amp = Decay::off();
        self.noise = Decay::off();
        self.metal = Decay::off();
        self.click = Decay::off();
        self.choke = 0.0;
    }

    /// Sets the hit up for `pad` struck on `key`.
    fn strike(&mut self, pad: Pad, key: u8, velocity: f32, s: &Settings, rate: f32) {
        let filter = |f: fn(f64, f32, f32) -> BiquadCoefficients, hz: f32, q: f32| {
            f(f64::from(rate), hz.min(rate * MAX_FILTER_FRACTION), q)
        };
        self.pad = pad;
        self.velocity = velocity;
        self.age = 0;
        self.attack_step = 1.0 / (ATTACK_SECONDS * rate).max(1.0);
        self.attack = 0.0;
        self.choke = 1.0;
        self.choke_coef = 1.0;
        self.body = Sweep::default();
        self.second = Sweep::default();
        self.noise_gain = 0.0;
        self.noise = Decay::off();
        self.metal_gain = 0.0;
        self.metal = Decay::off();
        self.metal_count = 0;
        self.metal_phase = [0.0, 0.31, 0.57, 0.13, 0.79, 0.42];
        self.click_gain = 0.0;
        self.click = Decay::off();
        self.drive = 0.0;
        self.burst_spacing = 0;
        self.burst = 1.0;
        self.first.reset();
        self.second_filter.reset();
        self.click_filter.reset();
        self.first.set_coefficients(BiquadCoefficients::identity());
        self.second_filter
            .set_coefficients(BiquadCoefficients::identity());
        // Every hit is its own: the same key struck the same way is the same samples.
        self.rng = 0x9E37_79B9 ^ (u32::from(key) << 16) ^ 0x5bd1;
        // A harder hit punches higher, as a harder strike tightens a real head.
        let accent = 0.8 + 0.2 * velocity;

        match pad {
            Pad::Kick | Pad::Boom => {
                let boom = pad == Pad::Boom;
                let tune = if boom {
                    s.kick_tune * 0.84
                } else {
                    s.kick_tune
                };
                let decay = if boom {
                    (s.kick_decay * 2.0).min(4.0)
                } else {
                    s.kick_decay
                };
                let punch = if boom {
                    s.kick_punch * 0.85
                } else {
                    s.kick_punch
                };
                let start = tune * (punch * PUNCH_OCTAVES * accent).exp2();
                // More punch is also a faster drop: a big sweep that lingers is a laser, not a kick.
                let tau = 0.006 + 0.03 * (1.0 - punch);
                self.body = Sweep::new(start, tune, tau, decay, 1.0, rate);
                self.click_gain = s.kick_click * 0.9;
                self.click = Decay::new(0.012, rate);
                self.click_filter.set_coefficients(filter(
                    BiquadCoefficients::bandpass,
                    3_800.0,
                    0.7,
                ));
                self.drive = (s.kick_drive + if boom { 0.15 } else { 0.0 }).min(1.0);
            }
            Pad::Tom => {
                let tune = 130.0 * ((f32::from(key) - 47.0) / 12.0).exp2();
                let decay = s.tom_decay * (1.15 - (f32::from(key) - 41.0) / 30.0);
                self.body = Sweep::new(tune * 1.5, tune, 0.03, decay, 0.9, rate);
                self.noise_gain = 0.08;
                self.noise = Decay::new(0.05, rate);
                self.first
                    .set_coefficients(filter(BiquadCoefficients::bandpass, tune * 6.0, 0.8));
                self.drive = s.kick_drive * 0.3;
            }
            Pad::Snare => {
                let tone = s.snare_tone;
                let snap = s.snare_snap;
                self.body = Sweep::new(
                    tone * 1.35,
                    tone,
                    0.008,
                    s.snare_decay * 0.7,
                    0.75 * (1.0 - 0.4 * snap),
                    rate,
                );
                self.second = Sweep::new(
                    tone * 1.75 * 1.2,
                    tone * 1.75,
                    0.006,
                    s.snare_decay * 0.45,
                    0.45 * (1.0 - 0.4 * snap),
                    rate,
                );
                self.noise_gain = 0.25 + 0.75 * snap;
                self.noise = Decay::new(s.snare_decay, rate);
                self.first.set_coefficients(filter(
                    BiquadCoefficients::highpass,
                    1_100.0,
                    std::f32::consts::FRAC_1_SQRT_2,
                ));
                self.second_filter.set_coefficients(filter(
                    BiquadCoefficients::lowpass,
                    9_500.0,
                    std::f32::consts::FRAC_1_SQRT_2,
                ));
                self.drive = s.kick_drive * 0.25;
            }
            Pad::Clap => {
                self.noise_gain = 2.6;
                self.noise = Decay::new(s.clap_decay, rate);
                self.burst_spacing = (CLAP_SPACING_SECONDS * rate).max(1.0) as u32;
                self.burst_coef = decay_coef(CLAP_BURST_SECONDS, rate);
                self.first
                    .set_coefficients(filter(BiquadCoefficients::bandpass, 1_150.0, 1.4));
                self.second_filter.set_coefficients(filter(
                    BiquadCoefficients::highpass,
                    650.0,
                    0.7,
                ));
            }
            Pad::ClosedHat | Pad::OpenHat => {
                let bright = 0.75 + 0.6 * s.hat_tone;
                let decay = if pad == Pad::ClosedHat {
                    (s.hat_decay * 0.16).max(0.02)
                } else {
                    s.hat_decay
                };
                self.metal_count = METAL_HZ.len();
                for (hz, base) in self.metal_hz.iter_mut().zip(METAL_HZ) {
                    *hz = base * (0.85 + 0.3 * s.hat_tone);
                }
                self.metal_gain = 2.4 * s.metal;
                self.metal = Decay::new(decay, rate);
                self.noise_gain = 2.1 * (1.0 - s.metal);
                self.noise = Decay::new(decay, rate);
                self.first.set_coefficients(filter(
                    BiquadCoefficients::highpass,
                    6_500.0 * bright,
                    0.7,
                ));
                self.second_filter.set_coefficients(filter(
                    BiquadCoefficients::bandpass,
                    9_500.0 * bright,
                    0.9,
                ));
            }
            Pad::Crash | Pad::Ride => {
                let ride = pad == Pad::Ride;
                self.metal_count = METAL_HZ.len();
                for (hz, base) in self.metal_hz.iter_mut().zip(METAL_HZ) {
                    *hz = base * if ride { 1.6 } else { 1.1 };
                }
                let decay = if ride {
                    s.cymbal_decay * 0.65
                } else {
                    s.cymbal_decay
                };
                self.metal_gain = if ride { 1.3 } else { 0.9 } * (0.3 + 0.7 * s.metal);
                self.metal = Decay::new(decay, rate);
                self.noise_gain = if ride { 0.25 } else { 1.0 } * (1.2 - 0.7 * s.metal);
                self.noise = Decay::new(decay * if ride { 0.4 } else { 0.9 }, rate);
                self.first.set_coefficients(filter(
                    BiquadCoefficients::highpass,
                    if ride { 2_800.0 } else { 3_600.0 },
                    0.7,
                ));
                self.second_filter.set_coefficients(if ride {
                    filter(BiquadCoefficients::bandpass, 5_500.0, 0.7)
                } else {
                    filter(BiquadCoefficients::lowpass, 14_000.0, 0.7)
                });
            }
            Pad::Cowbell => {
                self.metal_count = COWBELL_HZ.len();
                self.metal_hz[..2].copy_from_slice(&COWBELL_HZ);
                self.metal_gain = 0.55;
                self.metal = Decay::new(0.35, rate);
                self.first
                    .set_coefficients(filter(BiquadCoefficients::bandpass, 2_600.0, 1.2));
                self.second_filter.set_coefficients(filter(
                    BiquadCoefficients::highpass,
                    450.0,
                    0.7,
                ));
            }
            Pad::Rim => {
                self.body = Sweep::new(1_750.0, 1_700.0, 0.002, 0.035, 0.7, rate);
                self.second = Sweep::new(480.0, 460.0, 0.002, 0.03, 0.5, rate);
                self.noise_gain = 0.4;
                self.noise = Decay::new(0.012, rate);
                self.first
                    .set_coefficients(filter(BiquadCoefficients::bandpass, 4_500.0, 0.8));
            }
        }
        let gain = 1.0 + self.drive * 9.0;
        self.drive_norm = 1.0 / gain.tanh();
        // A tuned pad shows a couple of periods of its body; the rest a few milliseconds.
        let tuned = matches!(pad, Pad::Kick | Pad::Boom | Pad::Tom | Pad::Snare);
        let window = if tuned && self.body.end > 0.0 {
            SCOPE_PERIODS / self.body.end
        } else {
            SCOPE_SECONDS
        };
        self.scope_stride = ((window * rate / MOTION_POINTS as f32).round() as u32).max(1);
        self.scope = [0.0; MOTION_POINTS];
        self.scope_at = 0;
        self.scope_wait = 0;
    }

    #[inline]
    fn white(&mut self) -> f32 {
        // xorshift32: a full-period generator in three shifts, seeded per hit.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// One sample of the hit, before velocity and the output gain.
    #[inline]
    fn next(&mut self, inv_rate: f32) -> f32 {
        let mut tone = self.body.next(inv_rate) + self.second.next(inv_rate);

        let mut bright = 0.0;
        if self.noise_gain > 0.0 && self.noise.audible() {
            let mut env = self.noise.next();
            if self.burst_spacing > 0 {
                // The clap: during the bursts each onset is its own fast envelope, and the tail
                // only takes over after the last.
                if self.age < self.burst_spacing * CLAP_BURSTS {
                    if self.age.is_multiple_of(self.burst_spacing) {
                        self.burst = 1.0;
                    }
                    env = self.burst;
                    self.burst *= self.burst_coef;
                    // The tail's own clock waits for the bursts to finish.
                    self.noise.level = 1.0;
                }
            }
            bright += self.white() * env * self.noise_gain;
        }
        if self.metal_gain > 0.0 && self.metal.audible() {
            let mut sum = 0.0;
            for (phase, hz) in self
                .metal_phase
                .iter_mut()
                .zip(self.metal_hz)
                .take(self.metal_count)
            {
                sum += if *phase < 0.5 { 1.0 } else { -1.0 };
                *phase += hz * inv_rate;
                *phase -= phase.floor();
            }
            bright += sum / self.metal_count.max(1) as f32 * self.metal.next() * self.metal_gain;
        }
        if bright != 0.0 {
            tone += self
                .second_filter
                .process_sample(self.first.process_sample(bright));
        }

        if self.click_gain > 0.0 && self.click.audible() {
            let noise = self.white();
            tone +=
                self.click_filter.process_sample(noise) * self.click.next() * self.click_gain * 2.0;
        }

        if self.drive > 0.0 {
            tone = (tone * (1.0 + self.drive * 9.0)).tanh() * self.drive_norm;
        }

        if self.attack < 1.0 {
            self.attack = (self.attack + self.attack_step).min(1.0);
        }
        self.age = self.age.saturating_add(1);
        let out = tone * self.attack * self.choke;
        self.choke *= self.choke_coef;
        out
    }
}

/// An electronic drum machine: every pad synthesised from oscillators, noise and filters.
#[derive(Clone, Debug)]
pub struct ElectroKit {
    style: ElectroStyle,
    params: ParamBank,
    hits: Vec<Hit>,
    allocator: VoiceAllocator,
    sample_rate: f32,
    settings: Settings,
    output_gain: f32,
    channel_volume: f32,
    observer: Observer,
}

impl ElectroKit {
    /// Builds `style` with its own settings.
    pub fn new(style: ElectroStyle) -> Self {
        let mut kit = Self {
            style,
            params: ParamBank::new(descriptors(style)),
            hits: Vec::new(),
            allocator: VoiceAllocator::new(),
            sample_rate: 48_000.0,
            settings: Settings {
                kick_tune: 50.0,
                kick_decay: 0.5,
                kick_punch: 0.4,
                kick_click: 0.2,
                kick_drive: 0.2,
                snare_tone: 180.0,
                snare_snap: 0.6,
                snare_decay: 0.18,
                clap_decay: 0.2,
                hat_tone: 0.5,
                hat_decay: 0.4,
                metal: 0.6,
                cymbal_decay: 1.8,
                tom_decay: 0.45,
            },
            output_gain: 1.0,
            channel_volume: 1.0,
            observer: Observer::default(),
        };
        kit.refresh();
        kit
    }

    /// Which kit this is.
    pub fn style(&self) -> ElectroStyle {
        self.style
    }

    fn refresh(&mut self) {
        let p = |index| self.params.at(index);
        self.settings = Settings {
            kick_tune: p(P_KICK_TUNE),
            kick_decay: p(P_KICK_DECAY),
            kick_punch: p(P_KICK_PUNCH),
            kick_click: p(P_KICK_CLICK),
            kick_drive: p(P_KICK_DRIVE),
            snare_tone: p(P_SNARE_TONE),
            snare_snap: p(P_SNARE_SNAP),
            snare_decay: p(P_SNARE_DECAY),
            clap_decay: p(P_CLAP_DECAY),
            hat_tone: p(P_HAT_TONE),
            hat_decay: p(P_HAT_DECAY),
            metal: p(P_METAL),
            cymbal_decay: p(P_CYMBAL_DECAY),
            tom_decay: p(P_TOM_DECAY),
        };
        self.output_gain = db_to_gain(p(P_LEVEL));
    }

    fn note_on(&mut self, key: u8, velocity: f32) {
        let velocity = finite_or(velocity, 1.0).clamp(0.0, 1.0);
        let Some(pad) = Pad::at_key(key) else {
            return;
        };
        if pad.is_hat() {
            // Any hat closes the open one: they are one cymbal pair on one stand.
            let choke = decay_coef(CHOKE_SECONDS, self.sample_rate);
            for (slot, hit) in self.hits.iter_mut().enumerate() {
                let sounding = self
                    .allocator
                    .slot(slot)
                    .is_some_and(|s| s.state != VoiceState::Free);
                if hit.pad == Pad::OpenHat && sounding {
                    hit.choke_coef = choke;
                }
            }
        }
        let Some(assignment) = self.allocator.note_on(key, velocity) else {
            return;
        };
        let rate = self.sample_rate;
        let settings = self.settings;
        if let Some(hit) = self.hits.get_mut(assignment.index) {
            hit.strike(pad, key, velocity, &settings, rate);
        }
    }

    /// The newest hits, each as the last moments of what it sounds.
    fn publish_motion(&mut self) {
        let mut frame = MotionFrame {
            geometry: MotionGeometry::Oscillator,
            active: self.allocator.active_count(),
            expression: self.channel_volume,
            ..MotionFrame::default()
        };
        let hits = &self.hits;
        let selected = recent_voices(&self.allocator, |index| {
            hits.get(index).is_some_and(Hit::is_active)
        });
        let gain = self.output_gain * self.channel_volume;
        for (observed, index) in frame.voices.iter_mut().zip(selected) {
            let Some(index) = index else {
                break;
            };
            let hit = &self.hits[index];
            let mut points = [0.0; MOTION_POINTS];
            hit.scope_points(&mut points);
            for point in &mut points {
                *point *= gain;
            }
            let mut modes = [0.0; MOTION_MODES];
            harmonics(&points, &mut modes);
            observed.points = points;
            observed.modes = modes;
            observed.pitch = self
                .allocator
                .slot(index)
                .map_or(0.0, |slot| f32::from(slot.pitch));
            observed.level = hit.level();
            observed.contact = 0.0;
            observed.excitation = hit.body_hz();
            observed.held = self
                .allocator
                .slot(index)
                .is_some_and(|slot| slot.state == VoiceState::Held);
        }
        self.observer.publish(&frame);
    }
}

fn descriptors(style: ElectroStyle) -> Vec<ParamDescriptor> {
    let d = style.defaults();
    let seconds = |id, key: &'static str, name: &'static str, min, max, default| {
        ParamDescriptor::new(id, key, name, min, max, default)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0))
    };
    vec![
        ParamDescriptor::hertz(P_KICK_TUNE, "kick_tune", "Kick Tune", 30.0, 120.0, d[0]),
        seconds(P_KICK_DECAY, "kick_decay", "Kick Decay", 0.05, 3.0, d[1]),
        ParamDescriptor::percent(P_KICK_PUNCH, "kick_punch", "Kick Punch", d[2]),
        ParamDescriptor::percent(P_KICK_CLICK, "kick_click", "Kick Click", d[3]),
        ParamDescriptor::percent(P_KICK_DRIVE, "kick_drive", "Kick Drive", d[4]),
        ParamDescriptor::hertz(P_SNARE_TONE, "snare_tone", "Snare Tone", 120.0, 400.0, d[5]),
        ParamDescriptor::percent(P_SNARE_SNAP, "snare_snap", "Snare Snap", d[6]),
        seconds(P_SNARE_DECAY, "snare_decay", "Snare Decay", 0.04, 0.8, d[7]),
        seconds(P_CLAP_DECAY, "clap_decay", "Clap Decay", 0.05, 0.8, d[8]),
        ParamDescriptor::percent(P_HAT_TONE, "hat_tone", "Hat Tone", d[9]),
        seconds(P_HAT_DECAY, "hat_decay", "Hat Decay", 0.1, 1.5, d[10]),
        ParamDescriptor::percent(P_METAL, "metal", "Metal", d[11]),
        seconds(
            P_CYMBAL_DECAY,
            "cymbal_decay",
            "Cymbal Decay",
            0.3,
            4.0,
            d[12],
        ),
        seconds(P_TOM_DECAY, "tom_decay", "Tom Decay", 0.1, 1.5, d[13]),
        ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, d[14]),
    ]
}

impl Parameterized for ElectroKit {
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

impl SegmentRenderer for ElectroKit {
    fn handle_event(&mut self, event: &NoteEvent) {
        match *event {
            NoteEvent::NoteOn {
                pitch, velocity, ..
            } => self.note_on(pitch, velocity),
            NoteEvent::NoteOff { pitch, .. } => {
                self.allocator.note_off(pitch);
            }
            NoteEvent::AllNotesOff { .. } => {
                self.allocator.release_all();
            }
            NoteEvent::AllSoundOff { .. } => {
                let choke = decay_coef(CHOKE_SECONDS, self.sample_rate);
                for index in self.allocator.release_all() {
                    if let Some(hit) = self.hits.get_mut(index) {
                        hit.choke_coef = choke;
                    }
                }
            }
            NoteEvent::Controller { number, value, .. } => {
                if number == 7 {
                    self.channel_volume = finite_or(value, 1.0).clamp(0.0, 1.0).powi(2);
                }
            }
            // A kit's pads keep their tuning; a bend would detune the whole machine at once.
            NoteEvent::PitchBend { .. } => {}
        }
    }

    fn render_segment(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        let Some((mono, _)) = out.channels_mut().split_first_mut() else {
            return;
        };
        let Some(dst) = mono.get_mut(start..end) else {
            return;
        };
        dst.fill(0.0);
        let inv_rate = 1.0 / self.sample_rate;
        for (slot, hit) in self.hits.iter_mut().enumerate() {
            if self
                .allocator
                .slot(slot)
                .is_none_or(|s| s.state == VoiceState::Free)
            {
                continue;
            }
            let gain = hit.velocity * (0.35 + 0.65 * hit.velocity);
            for sample in dst.iter_mut() {
                let value = hit.next(inv_rate) * gain;
                hit.record(value);
                *sample += value;
            }
            if !hit.first.coefficients().is_finite() || !(hit.body.phase.is_finite()) {
                hit.silence();
            }
            self.allocator.set_level(slot, hit.level());
            if !hit.is_active() {
                hit.silence();
                self.allocator.retire(slot);
            }
        }
        let gain = self.output_gain * self.channel_volume;
        for sample in dst.iter_mut() {
            *sample *= gain;
        }
    }
}

impl Instrument for ElectroKit {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            self.style.id(),
            self.style.name(),
            self.style.description(),
            PluginCategory::Drum,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sample_rate = crate::sample_rate_f32(ctx.sample_rate);
        self.hits.clear();
        self.hits.resize_with(VOICE_COUNT, Hit::new);
        self.allocator.prepare(VOICE_COUNT);
        self.channel_volume = 1.0;
        self.refresh();
    }

    fn reset(&mut self) {
        for hit in &mut self.hits {
            hit.silence();
        }
        self.allocator.clear();
        self.channel_volume = 1.0;
        self.observer.reset(MotionGeometry::Oscillator);
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = ctx.block_frames.min(out.frame_count());
        render_segments(self, events, out, frames);
        spread_to_all_channels(out, frames);
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
    use crate::test_support::{
        Rig, band_amplitude, count_allocations, peak, peak_db, rms, zero_crossing_hz,
    };

    const RATE: f64 = 48_000.0;

    fn rig(style: ElectroStyle) -> Rig {
        let mut kit = ElectroKit::new(style);
        kit.set_param_by_key("level", 0.0);
        Rig::new(Box::new(kit), RATE, 512, 2)
    }

    fn hit(frame: u32, pitch: u8) -> NoteEvent {
        NoteEvent::NoteOn {
            frame,
            pitch,
            velocity: 1.0,
        }
    }

    fn ms(milliseconds: usize) -> usize {
        milliseconds * 48
    }

    #[test]
    fn every_kit_shares_one_parameter_layout_with_its_own_defaults() {
        let keys = |kit: &ElectroKit| {
            kit.parameters()
                .iter()
                .map(|p| p.key.to_string())
                .collect::<Vec<_>>()
        };
        let reference = ElectroKit::new(ElectroStyle::Electro);
        for style in ElectroStyle::ALL {
            let kit = ElectroKit::new(style);
            assert_eq!(kit.descriptor().id, style.id());
            assert_eq!(ElectroStyle::from_id(style.id()), Some(style));
            assert_eq!(kit.descriptor().category, PluginCategory::Drum);
            assert_eq!(keys(&kit), keys(&reference));
            for (index, descriptor) in kit.parameters().iter().enumerate() {
                assert_eq!(descriptor.id.index(), index, "id of `{}`", descriptor.key);
                assert!(
                    (descriptor.min..=descriptor.max).contains(&descriptor.default),
                    "{:?} {} default out of range",
                    style,
                    descriptor.key
                );
            }
        }
        assert_ne!(ElectroStyle::Edm.defaults(), ElectroStyle::Trap.defaults());
        assert_eq!(ElectroStyle::from_id("auris.synth.drumkit"), None);
    }

    #[test]
    fn the_kick_lands_high_and_rings_at_its_tune() {
        for style in ElectroStyle::ALL {
            let tune = style.defaults()[0];
            let rendered = rig(style).render(ms(300), &[hit(0, 36)]);
            let punch = zero_crossing_hz(&rendered[..ms(25)], RATE);
            let boom = zero_crossing_hz(&rendered[ms(100)..ms(200)], RATE);
            assert!(
                (boom - f64::from(tune)).abs() < f64::from(tune) * 0.06,
                "{style:?}: tail at {boom:.1} Hz, tune {tune}"
            );
            assert!(
                punch > boom * 1.4,
                "{style:?}: the punch ({punch:.0} Hz) does not start well above the boom"
            );
            assert!(peak(&rendered) > 0.9 && peak(&rendered) <= 1.0001);
        }
    }

    #[test]
    fn the_trap_kick_booms_far_longer_than_the_club_kick() {
        let window = ms(500)..ms(900);
        let trap = rig(ElectroStyle::Trap).render(ms(900), &[hit(0, 36)]);
        let edm = rig(ElectroStyle::Edm).render(ms(900), &[hit(0, 36)]);
        let (trap, edm) = (rms(&trap[window.clone()]), rms(&edm[window]));
        assert!(trap > 0.05, "trap tail {trap}");
        assert!(trap > edm * 30.0, "trap {trap} vs edm {edm}");
    }

    #[test]
    fn the_boom_key_is_lower_and_longer_than_the_kick() {
        let kick = rig(ElectroStyle::Electro).render(ms(1_000), &[hit(0, 36)]);
        let boom = rig(ElectroStyle::Electro).render(ms(1_000), &[hit(0, 35)]);
        let kick_hz = zero_crossing_hz(&kick[ms(100)..ms(200)], RATE);
        let boom_hz = zero_crossing_hz(&boom[ms(100)..ms(200)], RATE);
        assert!(boom_hz < kick_hz * 0.9, "{boom_hz} vs {kick_hz}");
        assert!(rms(&boom[ms(500)..]) > rms(&kick[ms(500)..]) * 5.0);
    }

    #[test]
    fn drive_adds_the_harmonics_that_carry_a_sub_on_small_speakers() {
        let third = |drive: f32| {
            let mut rig = rig(ElectroStyle::Electro);
            rig.set_param("kick_drive", drive);
            rig.set_param("kick_click", 0.0);
            // Long enough that the tail is still loud where it is measured: drive is a curve
            // over level, and a quiet tail passes through its straight middle.
            rig.set_param("kick_decay", 3.0);
            let tune = f64::from(rig.instrument.param(ParamId(P_KICK_TUNE)));
            let rendered = rig.render(ms(300), &[hit(0, 36)]);
            let tail = &rendered[ms(100)..ms(300)];
            band_amplitude(tail, RATE, tune * 3.0) / band_amplitude(tail, RATE, tune)
        };
        let (clean, driven) = (third(0.0), third(1.0));
        assert!(
            clean < 0.05,
            "a clean kick already has a third harmonic of {clean}"
        );
        assert!(driven > 0.15, "full drive only gave {driven}");
    }

    #[test]
    fn hats_are_bright_and_kicks_are_dark() {
        let rendered = rig(ElectroStyle::Edm).render(ms(100), &[hit(0, 46)]);
        let high = band_amplitude(&rendered, RATE, 10_000.0);
        let low = band_amplitude(&rendered, RATE, 200.0);
        assert!(high > low * 10.0, "hat: {high} at 10 kHz, {low} at 200 Hz");

        let kick = rig(ElectroStyle::Edm).render(ms(100), &[hit(0, 36)]);
        assert!(band_amplitude(&kick, RATE, 60.0) > band_amplitude(&kick, RATE, 5_000.0) * 10.0);
    }

    #[test]
    fn a_closed_hat_chokes_the_open_one() {
        let open = rig(ElectroStyle::Electro).render(ms(300), &[hit(0, 46)]);
        let choked =
            rig(ElectroStyle::Electro).render(ms(300), &[hit(0, 46), hit(ms(60) as u32, 44)]);
        // The pedal hat itself is over inside 80 ms; anything left after that is the open hat.
        let after = ms(145)..ms(220);
        assert!(
            rms(&open[after.clone()]) > 0.001,
            "the open hat had already died"
        );
        assert!(
            peak_db(&choked[after]) < -70.0,
            "the open hat rang on through a closed one"
        );
    }

    #[test]
    fn the_clap_is_several_hands_not_one() {
        // Four onsets inside 30 ms: the level must rise again after each burst has decayed.
        let rendered = rig(ElectroStyle::Electro).render(ms(60), &[hit(0, 39)]);
        let spacing = (CLAP_SPACING_SECONDS * 48_000.0) as usize;
        let mut onsets = 0;
        for burst in 0..CLAP_BURSTS as usize {
            let start = burst * spacing;
            let head = rms(&rendered[start..start + 48]);
            let tail = rms(&rendered[start + spacing - 48..start + spacing]);
            if head > tail * 2.0 {
                onsets += 1;
            }
        }
        assert_eq!(onsets, CLAP_BURSTS);
    }

    #[test]
    fn every_pad_decays_to_silence_and_frees_its_voice() {
        for style in ElectroStyle::ALL {
            for key in [35, 36, 37, 38, 39, 41, 42, 46, 49, 51, 56] {
                let mut rig = rig(style);
                let rendered = rig.render(48_000 * 9, &[hit(0, key)]);
                assert!(peak(&rendered) > 0.1, "{style:?} key {key} too quiet");
                assert!(peak(&rendered) < 1.5, "{style:?} key {key} too loud");
                assert!(rendered.iter().all(|s| s.is_finite()));
                assert!(
                    peak_db(&rendered[48_000 * 8..]) < -80.0,
                    "{style:?} key {key} never finished"
                );
                assert_eq!(rig.instrument.active_voices(), 0, "{style:?} key {key}");
            }
        }
    }

    #[test]
    fn a_key_with_no_pad_is_silent() {
        let mut rig = rig(ElectroStyle::Edm);
        let rendered = rig.render(512, &[hit(0, 60), hit(0, 20)]);
        assert!(rendered.iter().all(|s| *s == 0.0));
        assert_eq!(rig.instrument.active_voices(), 0);
    }

    #[test]
    fn the_same_hit_renders_the_same_samples_whatever_came_before() {
        let alone = rig(ElectroStyle::Trap).render(ms(400), &[hit(0, 38)]);
        let mut busy = rig(ElectroStyle::Trap);
        busy.render(
            ms(400),
            &[hit(0, 42), hit(ms(50) as u32, 39), hit(ms(90) as u32, 46)],
        );
        busy.render(48_000 * 3, &[]);
        let again = busy.render(ms(400), &[hit(0, 38)]);
        assert_eq!(alone, again);
    }

    #[test]
    fn velocity_shapes_the_level() {
        let at = |velocity| {
            let rendered = rig(ElectroStyle::Electro).render(
                ms(200),
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 38,
                    velocity,
                }],
            );
            peak(&rendered)
        };
        assert!(at(1.0) > at(0.5) * 2.0);
        assert!(at(0.2) > 0.0);
    }

    #[test]
    fn the_display_shows_each_hit_as_it_sounds() {
        let mut rig = rig(ElectroStyle::Edm);
        let monitor = rig.instrument.motion_monitor().unwrap();
        monitor.watch(true);
        rig.render(ms(200), &[hit(0, 36), hit(ms(150) as u32, 42)]);
        let frame = monitor.read().unwrap();
        assert_eq!(frame.geometry, MotionGeometry::Oscillator);
        assert_eq!(frame.active, 2);
        // Newest first: the hat, then the kick.
        let (hat, kick) = (frame.voices[0], frame.voices[1]);
        assert_eq!((hat.pitch, kick.pitch), (42.0, 36.0));
        // The kick's punch is over and its body rings at its tune; a hat has no tuned body.
        let tune = rig.instrument.param_by_key("kick_tune").unwrap();
        assert!((kick.excitation - tune).abs() < 1.0, "{}", kick.excitation);
        assert_eq!(hat.excitation, 0.0);
        for voice in [hat, kick] {
            assert!(voice.points.iter().any(|point| point.abs() > 1e-3));
            assert!(voice.points.iter().all(|point| point.is_finite()));
            assert!(voice.level > 0.0);
        }
        // Two periods of the kick: its scope rises through two cycles' worth of zero crossings.
        let crossings = kick
            .points
            .windows(2)
            .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
            .count();
        assert!((1..=3).contains(&crossings), "{crossings}");
        rig.instrument.reset();
        assert_eq!(monitor.read().unwrap().active, 0);
    }

    #[test]
    fn watching_never_changes_the_sound() {
        let mut watched = rig(ElectroStyle::Trap);
        let mut plain = rig(ElectroStyle::Trap);
        watched.instrument.motion_monitor().unwrap().watch(true);
        let events = [hit(0, 36), hit(ms(50) as u32, 39), hit(ms(80) as u32, 46)];
        assert_eq!(
            watched.render(ms(400), &events),
            plain.render(ms(400), &events)
        );
    }

    #[test]
    fn processing_allocates_nothing() {
        let mut kit = ElectroKit::new(ElectroStyle::Edm);
        kit.prepare(&PrepareContext::new(RATE, 512, 2));
        let mut output = AudioBuffer::stereo(512, RATE);
        let context = ProcessContext::realtime(RATE, 512, 0, 120.0, true);
        // More hits than voices, so stealing is exercised too.
        let events: [NoteEvent; 60] =
            std::array::from_fn(|index| hit(index as u32 * 8, [36, 39, 42, 46, 49, 56][index % 6]));
        let count = count_allocations(|| {
            kit.process(&events, &mut output, &context);
            for _ in 0..4 {
                kit.process(&[], &mut output, &context);
            }
            kit.set_param_by_key("kick_decay", 1.0);
            kit.process(
                &[NoteEvent::AllSoundOff { frame: 0 }],
                &mut output,
                &context,
            );
        });
        assert_eq!(count, 0);
    }
}
