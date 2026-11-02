//! Hammer pulse, bridge-coupled unison strings, dampers and soundboard knock.
//!
//! Every partial of every string is a damped rotating phasor. The bridge absorbs only the motion
//! that the strings of a unison share. Struck together, a unison therefore first decays quickly
//! into the soundboard; the motion the strings do not share, left by slight mistuning and an
//! uneven hammer, then rings on as the aftersound. This is Weinreich's account of the piano's
//! two-stage decay. A single bass string gets the same two stages from a second, weakly coupled
//! polarization. The output is the force the strings exert on the bridge.

use std::f32::consts::{PI, TAU};

use auris_dsp::BiquadCoefficients;

use super::super::{Quality, Settings};

const PARTIALS: usize = 96;
/// Partial-string products one voice may run: three strings of 64 partials.
///
/// [`Quality::High`] keeps this budget. Lifting it to every partial below 0.45 × the sample
/// rate (up to 170 on A0, whose stiff-string 96th partial is already near 7.4 kHz) changed a
/// struck A0 or A1 by -73 dB RMS and anything higher by nothing measurable: the hammer's
/// contact spectrum and the strings' frictional loss have taken those partials first.
const BUDGET: usize = 192;
/// Partial-string products at [`Quality::Draft`]: a triple unison of 21 partials, a pair of 32.
///
/// Measured over a second of a struck note, Draft differs from Standard by -31 dB RMS on A0,
/// -38 dB on A1 and less than -52 dB from A2 up, and 24 held notes cost a little over half.
/// A smaller cut saves less than it looks: Standard's upper partials retire within the first
/// fraction of a second, so most of the sustained cost is the low partials every level keeps.
const DRAFT_BUDGET: usize = 64;
/// Highest key with a damper, F6. Strings above it ring on after the key is released.
const HIGHEST_DAMPER: u8 = 89;
const NEPERS_PER_DB: f32 = 0.115_129_25;
/// Factory controls at which the register curves were measured.
const FACTORY_DECAY: f32 = 11.498_04;
const FACTORY_DAMPING: f32 = 0.009_015;
const FACTORY_HARDNESS: f32 = 0.606_004;

/// Register curves and constants that voice the physical piano.
///
/// Curves hold one value for each C from C1 to C8 and are interpolated geometrically between
/// them. The factory values are fitted to the University of Iowa Steinway recordings described
/// in `docs/grand-piano-register.md`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    feature = "piano-calibration",
    derive(serde::Deserialize),
    serde(deny_unknown_fields)
)]
pub struct PianoVoicing {
    /// Aftersound decay of the fundamental, dB/s: the strings' own loss.
    pub aftersound: [f32; 8],
    /// Additional decay of the motion the strings share, dB/s: what the bridge absorbs.
    pub bridge: [f32; 8],
    /// Exponent of the bridge loss over partial number.
    pub bridge_slope: f32,
    /// Internal friction of partials above the fundamental, dB/s per kHz squared.
    pub friction: f32,
    /// Unison mistuning in cents, and its bound in hertz at the fundamental.
    pub detune: [f32; 2],
    /// Relative difference between the hammer's strikes on the strings of a unison.
    pub unevenness: f32,
    /// A single string's second polarization: bridge weight, hammer share and offset in cents.
    pub polarization: [f32; 3],
    /// Hammer contact duration at velocity 0.65 and factory hardness, in milliseconds.
    pub contact: [f32; 8],
    /// Shape of the contact pulse: a time warp, below one rising quickly and leaving slowly,
    /// and the power of its sine, from a half sine at one to a raised cosine at two.
    pub shape: [f32; 2],
    /// Strike point relative to the Excitation Position control.
    pub strike: [f32; 8],
    /// Exponent by which a faster hammer shortens its pulse.
    pub contact_velocity: f32,
    /// The felt's hard core: its time constant relative to the contact duration, its share of
    /// the force at velocity 0.65 and the exponent by which a faster hammer reaches it.
    pub core: [f32; 3],
    /// Level of the strings, in decibels: what a technician's voicing evens out.
    pub gain: [f32; 8],
    /// Soundboard knock relative to the string drive, in decibels.
    pub knock: [f32; 8],
    /// Knock decay in dB/s, highpass corner and lowpass corner in hertz.
    pub knock_shape: [f32; 3],
    /// Damper time relative to the Release control.
    pub damper: [f32; 8],
    /// Soundboard radiation gains, decibels, at twelve centres from 90 Hz to 10 kHz.
    pub soundboard: [f32; 12],
    /// Corner of the soundboard's low-frequency radiation loss, in hertz.
    pub highpass: f32,
}

/// The factory piano.
pub(in crate::physical) const VOICING: PianoVoicing = PianoVoicing {
    aftersound: [1.839, 1.345, 1.495, 1.737, 2.614, 5.659, 14.08, 42.86],
    bridge: [1.499, 6.62, 10.05, 16.63, 9.688, 7.19, 9.785, 13.3],
    bridge_slope: -0.1426,
    friction: 0.9172,
    detune: [0.258, 0.09223],
    unevenness: 0.5198,
    polarization: [0.7, 0.743, 0.392],
    contact: [4.273, 2.54, 2.009, 1.889, 1.521, 1.787, 0.9765, 0.4144],
    shape: [1.073, 0.9153],
    strike: [0.9781, 0.9365, 1.063, 0.9961, 1.754, 1.936, 1.924, 1.486],
    contact_velocity: 0.6847,
    core: [0.09207, 0.2054, 2.004],
    gain: [0.5, 1.0, 0.5, 0.0, 3.0, 5.0, 4.0, 5.0],
    knock: [4.042, 4.042, 4.042, 4.266, 5.724, 7.902, 7.824, 12.11],
    knock_shape: [99.71, 40.0, 1285.0],
    damper: [2.5, 2.0, 1.4, 1.0, 0.8, 0.7, 0.6, 0.6],
    soundboard: [
        0.2301, 2.915, 9.332, 9.328, 4.91, 3.511, 7.287, 9.843, 8.214, 5.516, 3.288, 1.908,
    ],
    highpass: 196.6,
};

/// Centre of one of the eight bands over which strike loudness is compared.
fn loudness_band(band: usize) -> f32 {
    60.0 * 50.0_f32.powf(band as f32 / 7.0)
}

/// Power the voicing's soundboard radiates at each loudness band, from its filters' magnitudes.
fn radiation_weights(voicing: &PianoVoicing) -> [f32; 8] {
    const RATE: f64 = 48_000.0;
    std::array::from_fn(|band| {
        let hz = loudness_band(band);
        let highpass = BiquadCoefficients::highpass(RATE, voicing.highpass, 0.707);
        let wet = voicing
            .soundboard
            .iter()
            .enumerate()
            .map(|(index, gain)| {
                let centre = 90.0_f32 * (10_000.0_f32 / 90.0).powf(index as f32 / 11.0);
                BiquadCoefficients::peaking(RATE, centre, 0.9, *gain).magnitude_db(hz, RATE)
            })
            .sum::<f32>()
            + highpass.magnitude_db(hz, RATE);
        // The factory Body Resonance blends 65% of the radiated signal with the dry one.
        let blend = 0.35 + 0.65 * 10.0_f32.powf(wet / 20.0);
        blend * blend
    })
}

/// Geometric interpolation of a C1..C8 curve at a fractional MIDI pitch.
fn register(curve: &[f32; 8], pitch: f32) -> f32 {
    let position = ((pitch - 24.0) / 12.0).clamp(0.0, 7.0);
    let index = (position as usize).min(6);
    let fraction = position - index as f32;
    let (low, high) = (curve[index].max(1e-6), curve[index + 1].max(1e-6));
    low * (high / low).powf(fraction)
}

/// Linear interpolation of a C1..C8 curve held in decibels.
fn register_db(curve: &[f32; 8], pitch: f32) -> f32 {
    let position = ((pitch - 24.0) / 12.0).clamp(0.0, 7.0);
    let index = (position as usize).min(6);
    let fraction = position - index as f32;
    curve[index] + (curve[index + 1] - curve[index]) * fraction
}

fn one_pole(rate: f32, hz: f32) -> f32 {
    1.0 - (-TAU * hz.min(rate * 0.45) / rate).exp()
}

/// The contact pulse's shape on a unit time axis, tabulated once per voicing so that striking a
/// note needs neither powers nor sines.
#[derive(Clone, Copy, Debug)]
struct PulseShape {
    table: [f32; 65],
}

impl PulseShape {
    fn new(shape: [f32; 2]) -> Self {
        let [warp, power] = [shape[0].clamp(0.2, 5.0), shape[1].clamp(0.5, 4.0)];
        Self {
            table: std::array::from_fn(|point| {
                let phase = point as f32 / 64.0;
                (PI * phase.powf(warp)).sin().max(0.0).powf(power)
            }),
        }
    }

    /// Unnormalized force at a phase of the contact, 0..1.
    fn at(&self, phase: f32) -> f32 {
        let position = phase.clamp(0.0, 1.0) * 64.0;
        let index = (position as usize).min(63);
        let fraction = position - index as f32;
        self.table[index] + fraction * (self.table[index + 1] - self.table[index])
    }

    /// Spectrum of the unit-area pulse lasting `seconds`, as a complex value at `hz`.
    fn spectrum(&self, hz: f32, seconds: f32) -> (f32, f32) {
        let step = TAU * hz * seconds / 64.0;
        let (sine, cosine) = step.sin_cos();
        // Midpoint of each of the 64 segments, rotated by a recurrence rather than a sine each.
        let (half_sine, half_cosine) = (step * 0.5).sin_cos();
        let mut rotation = (half_cosine, -half_sine);
        let (mut real, mut imag, mut area) = (0.0_f32, 0.0_f32, 0.0_f32);
        for pair in self.table.windows(2) {
            let force = 0.5 * (pair[0] + pair[1]);
            real += force * rotation.0;
            imag += force * rotation.1;
            area += force;
            rotation = (
                rotation.0 * cosine + rotation.1 * sine,
                rotation.1 * cosine - rotation.0 * sine,
            );
        }
        let area = area.max(1e-9);
        (real / area, imag / area)
    }
}

/// One hammer blow: how long the felt stays on the strings, and how much of the force comes
/// from its hard core, over what time constant.
struct Strike {
    seconds: f32,
    core: f32,
    core_seconds: f32,
}

impl Strike {
    fn new(voicing: &PianoVoicing, pitch: f32, velocity: f32, hardness: f32) -> Self {
        // The felt stays on the strings longer for a soft hammer and a soft strike; in the
        // treble, for longer than a period, which is what keeps those notes nearly pure.
        // Felt stiffens as it compresses: a short pulse from its hard core keeps the upper
        // partials of a loud strike, and grows with velocity.
        let relative = (velocity / 0.65).max(0.45);
        let softness = 2.0_f32.powf(2.0 * (FACTORY_HARDNESS - hardness));
        let seconds = register(&voicing.contact, pitch)
            * 1e-3
            * relative.powf(-voicing.contact_velocity)
            * softness;
        let [length, share, growth] = voicing.core;
        Self {
            seconds,
            core: (share * relative.powf(growth) / softness).clamp(0.0, 1.0),
            core_seconds: seconds * length,
        }
    }

    /// Power the pulse gives the harmonics nearest eight frequencies from 60 Hz to 3 kHz,
    /// through the strike point's comb and the soundboard's radiation at each.
    fn power(
        &self,
        shape: &PulseShape,
        fundamental: f32,
        strike: f32,
        radiation: &[f32; 8],
    ) -> f32 {
        radiation
            .iter()
            .enumerate()
            .map(|(band, weight)| {
                let n = (loudness_band(band) / fundamental).round().max(1.0);
                weight * (self.response(shape, fundamental * n) * (PI * n * strike).sin()).powi(2)
            })
            .sum::<f32>()
    }

    /// Magnitude of the unit-area force pulse's spectrum at a frequency.
    fn response(&self, shape: &PulseShape, hz: f32) -> f32 {
        let omega = TAU * hz;
        let main = shape.spectrum(hz, self.seconds);
        // A gamma pulse of three equal stages transforms to (1 + i w tau)^-3.
        let wt = omega * self.core_seconds;
        let stage = (1.0 / (1.0 + wt * wt), -wt / (1.0 + wt * wt));
        let mut core = (1.0_f32, 0.0_f32);
        for _ in 0..3 {
            core = (
                core.0 * stage.0 - core.1 * stage.1,
                core.0 * stage.1 + core.1 * stage.0,
            );
        }
        let blend = (
            main.0 + self.core * (core.0 - main.0),
            main.1 + self.core * (core.1 - main.1),
        );
        blend.0.hypot(blend.1)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct StringMode {
    real: f32,
    imag: f32,
    sine: f32,
    cosine: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Partial {
    strings: [StringMode; 3],
    /// Per-sample decay of each string's own motion.
    radius: f32,
    /// Share of the bridge-coupled motion absorbed per sample.
    bridge: f32,
    /// Modal response to a unit hammer force.
    drive: f32,
    /// Signed bridge-force weight: the mode's slope at the bridge.
    radiation: f32,
    hz: f32,
}

/// The soundboard's response to the hammer and key: band-limited noise under a decay.
#[derive(Clone, Copy, Debug, Default)]
struct Knock {
    gain: f32,
    envelope: f32,
    decay: f32,
    onset: f32,
    rise: f32,
    seed: u32,
    highpass: f32,
    lowpass: f32,
    stages: [f32; 4],
}

impl Knock {
    fn next(&mut self) -> f32 {
        // At -80 dB the knock is far below every string it accompanies.
        if self.envelope < 1e-4 {
            return 0.0;
        }
        // A fixed seed per strike keeps rendering deterministic and block-size independent.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        let noise = self.seed as f32 * (2.0 / u32::MAX as f32) - 1.0;
        let [low, lower, slow, slower] = &mut self.stages;
        *low += self.lowpass * (noise - *low);
        *lower += self.lowpass * (*low - *lower);
        *slow += self.highpass * (*lower - *slow);
        let once = *lower - *slow;
        *slower += self.highpass * (once - *slower);
        let band = once - *slower;
        self.onset += self.rise * (1.0 - self.onset);
        self.envelope *= self.decay;
        self.gain * self.envelope * self.onset * band
    }
}

#[derive(Clone, Debug)]
pub(super) struct Piano {
    partials: [Partial; PARTIALS],
    active: usize,
    strings: usize,
    weights: [f32; 3],
    shares: [f32; 3],
    norm: f32,
    output: f32,
    contact_frames: usize,
    pulse_frames: f32,
    pulse_area: f32,
    radiation: [f32; 8],
    shape: PulseShape,
    core_pole: f32,
    core: f32,
    core_pulse: [f32; 3],
    age: usize,
    pitch: f32,
    fundamental: f32,
    damped: bool,
    knock: Knock,
    thump: f32,
    voicing: PianoVoicing,
}

impl Default for Piano {
    fn default() -> Self {
        Self {
            partials: [Partial::default(); PARTIALS],
            active: 0,
            strings: 1,
            weights: [1.0, 0.0, 0.0],
            shares: [1.0, 0.0, 0.0],
            norm: 1.0,
            output: 1.0,
            contact_frames: 2,
            pulse_frames: 1.0,
            pulse_area: 1.0,
            radiation: radiation_weights(&VOICING),
            shape: PulseShape::new(VOICING.shape),
            core_pole: 0.0,
            core: 0.0,
            core_pulse: [0.0; 3],
            age: 0,
            pitch: 60.0,
            fundamental: 261.6,
            damped: false,
            knock: Knock::default(),
            thump: 0.0,
            voicing: VOICING,
        }
    }
}

impl Piano {
    pub(super) fn set_voicing(&mut self, voicing: PianoVoicing) {
        self.voicing = voicing;
        self.radiation = radiation_weights(&voicing);
        self.shape = PulseShape::new(voicing.shape);
    }

    pub(super) fn motion(&self, points: &mut [f32], modes: &mut [f32]) {
        let last = points.len().saturating_sub(1).max(1) as f32;
        let strings = self.strings.min(3);
        for (index, point) in points.iter_mut().enumerate() {
            let x = index as f32 / last;
            *point = if index == 0 || x == 1.0 {
                0.0
            } else {
                self.partials[..self.active.min(16)]
                    .iter()
                    .enumerate()
                    .map(|(n, partial)| {
                        partial.strings[..strings]
                            .iter()
                            .map(|s| s.real)
                            .sum::<f32>()
                            / strings as f32
                            * (PI * (n + 1) as f32 * x).sin()
                    })
                    .sum()
            };
        }
        for (partial, magnitude) in self.partials.iter().zip(modes) {
            *magnitude = partial.strings[..strings]
                .iter()
                .map(|s| s.real.hypot(s.imag))
                .sum::<f32>()
                / strings as f32;
        }
    }

    pub(super) fn contact(&self) -> f32 {
        if self.age < self.contact_frames {
            1.0 - self.age as f32 / self.contact_frames as f32
        } else {
            0.0
        }
    }

    pub(super) fn excite(
        &mut self,
        frequency: f32,
        velocity: f32,
        rate: f32,
        quality: Quality,
        settings: Settings,
    ) {
        let voicing = self.voicing;
        self.fundamental = frequency;
        self.pitch = 69.0 + 12.0 * (frequency / 440.0).log2();
        self.damped = false;
        // Single strings below C2, pairs below F#3 and triple unisons above. The single string
        // carries its second polarization as a weakly coupled companion.
        let unison = if frequency < 65.0 {
            1
        } else if frequency < 180.0 {
            2
        } else {
            3
        };
        let [cents, bound] = voicing.detune;
        let detune = cents.min(1200.0 * (1.0 + bound / frequency).log2());
        let uneven = voicing.unevenness;
        let [weight, share, offset] = voicing.polarization;
        let (strings, cents, weights, shares) = match unison {
            1 => (2, [0.0, offset, 0.0], [1.0, weight, 0.0], [1.0, share, 0.0]),
            2 => (
                2,
                [-detune, detune, 0.0],
                [1.0, 1.0, 0.0],
                [1.0 + uneven, 1.0 - uneven, 0.0],
            ),
            _ => (
                3,
                [-detune, 0.0, detune],
                [1.0; 3],
                [1.0 + uneven, 1.0, 1.0 - uneven],
            ),
        };
        self.strings = strings;
        self.weights = weights;
        self.shares = shares;
        self.norm = 1.0 / weights.iter().map(|w| w * w).sum::<f32>();
        self.output = 1.0 / weights.iter().sum::<f32>();
        let strike = Strike::new(&voicing, self.pitch, velocity, settings.hardness);
        let samples = (strike.seconds * rate).max(1.0);
        let core = (strike.core_seconds * rate).max(0.25);
        self.pulse_frames = samples.ceil();
        self.pulse_area = (0..self.pulse_frames as usize)
            .map(|frame| self.contact_shape(frame))
            .sum::<f32>()
            .max(1e-6);
        self.core_pole = (-1.0 / core).exp();
        self.core = strike.core;
        self.core_pulse = [0.0; 3];
        self.contact_frames = (self.pulse_frames as usize).max((14.0 * core) as usize) + 1;
        self.age = 0;
        // Velocity changes what the hammer excites, while loudness keeps the instrument's
        // velocity law: compare the radiated excitation with the same hammer struck at mf.
        // Hardness is voicing, and a harder felt is louder as well as brighter.
        let point = (settings.position * register(&voicing.strike, self.pitch)).clamp(0.01, 0.5);
        let reference = Strike::new(&voicing, self.pitch, 0.65, settings.hardness);
        let loudness = (reference.power(&self.shape, frequency, point, &self.radiation)
            / strike
                .power(&self.shape, frequency, point, &self.radiation)
                .max(1e-12))
        .sqrt()
        .clamp(0.1, 10.0);
        let amplitude = velocity.powf(1.25)
            * loudness
            * 10.0_f32.powf(register_db(&voicing.gain, self.pitch) / 20.0);
        let budget = if quality == Quality::Draft {
            DRAFT_BUDGET
        } else {
            BUDGET
        };
        let limit = (budget / strings).min(PARTIALS);
        self.active = 0;
        for (index, partial) in self.partials.iter_mut().enumerate() {
            *partial = Partial::default();
            let n = (index + 1) as f32;
            let ratio =
                n * ((1.0 + settings.stiffness * n * n) / (1.0 + settings.stiffness)).sqrt();
            if index >= limit || frequency * ratio * 1.002 >= rate * 0.45 {
                continue;
            }
            self.active = index + 1;
            partial.hz = frequency * ratio;
            // Hammer force excites modal velocity; the bridge feels each mode's slope.
            partial.drive = (PI * n * point).sin() / ratio * amplitude;
            partial.radiation = if index % 2 == 0 { n } else { -n };
            for (string, cents) in partial.strings.iter_mut().zip(cents).take(strings) {
                let angle = TAU * partial.hz * (cents / 1200.0_f32).exp2() / rate;
                (string.sine, string.cosine) = angle.sin_cos();
            }
        }
        let [decay, highpass, lowpass] = voicing.knock_shape;
        self.knock = Knock {
            gain: 10.0_f32.powf(register_db(&voicing.knock, self.pitch) / 20.0)
                * velocity.powf(1.25),
            envelope: 1.0,
            decay: (-decay * NEPERS_PER_DB / rate).exp(),
            onset: 0.0,
            rise: 1.0 - (-4.0 / self.pulse_frames).exp(),
            seed: 0x9E37_79B9 ^ (u32::from(self.pitch.round() as u8) << 16) ^ velocity.to_bits(),
            highpass: one_pole(rate, highpass),
            lowpass: one_pole(rate, lowpass),
            stages: [0.0; 4],
        };
        self.update_loss(rate, settings);
    }

    /// Unnormalized contact force at a frame: a power of a sine on a warped time axis.
    fn contact_shape(&self, frame: usize) -> f32 {
        let phase = (frame as f32 + 0.5) / self.pulse_frames;
        if phase >= 1.0 {
            return 0.0;
        }
        self.shape.at(phase)
    }

    pub(super) fn update_loss(&mut self, rate: f32, settings: Settings) {
        let voicing = &self.voicing;
        let scale = FACTORY_DECAY / settings.decay.max(0.01) * (1.0 + 6.0 * settings.damping)
            / (1.0 + 6.0 * FACTORY_DAMPING);
        let after = register(&voicing.aftersound, self.pitch);
        let bridge = register(&voicing.bridge, self.pitch);
        let damper = if self.damped {
            // Like the shared envelope, a damper takes Release seconds to remove 80 dB.
            80.0 / (settings.release.max(0.005) * register(&voicing.damper, self.pitch))
        } else {
            0.0
        };
        let fundamental = self.fundamental * 1e-3;
        for (index, partial) in self.partials[..self.active].iter_mut().enumerate() {
            if partial.hz == 0.0 {
                continue;
            }
            let khz = partial.hz * 1e-3;
            let own = (after + voicing.friction * (khz * khz - fundamental * fundamental).max(0.0))
                * scale
                + damper;
            partial.radius = (-own * NEPERS_PER_DB / rate).exp();
            let shared = bridge * ((index + 1) as f32).powf(voicing.bridge_slope);
            partial.bridge = 1.0 - (-shared * NEPERS_PER_DB / rate).exp();
        }
    }

    /// Lowers the damper onto a released key's strings; `false` above the dampers.
    pub(super) fn damp(&mut self, rate: f32, settings: Settings) -> bool {
        if self.pitch.round() > f32::from(HIGHEST_DAMPER) {
            return false;
        }
        self.damped = true;
        self.update_loss(rate, settings);
        true
    }

    /// Lifts the damper again, as the sustain pedal does for a decaying note.
    pub(super) fn lift(&mut self, rate: f32, settings: Settings) {
        if self.damped {
            self.damped = false;
            self.update_loss(rate, settings);
        }
    }

    /// The soundboard knock that accompanied the last string sample.
    pub(super) fn thump(&self) -> f32 {
        self.thump
    }

    pub(super) fn is_damped(&self) -> bool {
        self.damped
    }

    /// Frames until the slowest undamped motion has fallen by 90 dB.
    pub(super) fn lifetime(&self, rate: f32, settings: Settings) -> usize {
        let scale = FACTORY_DECAY / settings.decay.max(0.01) * (1.0 + 6.0 * settings.damping)
            / (1.0 + 6.0 * FACTORY_DAMPING);
        let after = register(&self.voicing.aftersound, self.pitch) * scale;
        (90.0 / after.max(0.5) * rate) as usize
    }

    pub(super) fn next(&mut self) -> f32 {
        let force = if self.age < self.contact_frames {
            let main = self.contact_shape(self.age) / self.pulse_area;
            // The core is an impulse through three equal one-pole stages: a gamma pulse.
            let mut core = if self.age == 0 { 1.0 } else { 0.0 };
            let gain = 1.0 - self.core_pole;
            for stage in &mut self.core_pulse {
                *stage += gain * (core - *stage);
                core = *stage;
            }
            main + self.core * (core - main)
        } else {
            0.0
        };
        self.age = self.age.saturating_add(1);
        let strings = self.strings;
        let weights = self.weights;
        let shares = self.shares;
        let mut output = 0.0;
        for partial in &mut self.partials[..self.active] {
            if partial.radius == 0.0 {
                continue;
            }
            let push = partial.drive * force;
            let mut shared_real = 0.0;
            let mut shared_imag = 0.0;
            let mut strength = 0.0;
            for ((string, weight), share) in partial.strings[..strings]
                .iter_mut()
                .zip(weights)
                .zip(shares)
            {
                string.imag += push * share;
                let real =
                    partial.radius * (string.cosine * string.real - string.sine * string.imag);
                string.imag =
                    partial.radius * (string.sine * string.real + string.cosine * string.imag);
                string.real = real;
                shared_real += weight * real;
                shared_imag += weight * string.imag;
                strength += real.abs() + string.imag.abs();
            }
            // Retire modes below -240 dB before they enter denormal arithmetic.
            if self.age > self.contact_frames && strength < 1e-12 {
                for string in &mut partial.strings[..strings] {
                    string.real = 0.0;
                    string.imag = 0.0;
                }
                partial.radius = 0.0;
                continue;
            }
            // The bridge takes a share of the coupled motion only; motion orthogonal to the
            // coupling keeps nothing but the strings' own loss. Passive for any share in 0..=1.
            let absorbed = partial.bridge * self.norm;
            for (string, weight) in partial.strings[..strings].iter_mut().zip(weights) {
                string.real -= absorbed * weight * shared_real;
                string.imag -= absorbed * weight * shared_imag;
            }
            output += partial.radiation * shared_real;
        }
        self.thump = self.knock.next();
        output * self.output
    }

    pub(super) fn energy(&self) -> f32 {
        self.partials[..self.active]
            .iter()
            .flat_map(|partial| &partial.strings[..self.strings])
            .map(|string| string.real.abs() + string.imag.abs())
            .sum::<f32>()
            + if self.knock.envelope < 1e-4 {
                0.0
            } else {
                self.knock.envelope * self.knock.gain
            }
    }

    pub(super) fn retune(&mut self, ratio: f32) {
        for string in self.partials[..self.active]
            .iter_mut()
            .flat_map(|partial| &mut partial.strings[..self.strings])
        {
            let angle = string.sine.atan2(string.cosine) * ratio;
            if angle >= PI * 0.9 {
                string.real = 0.0;
                string.imag = 0.0;
            }
            (string.sine, string.cosine) = angle.sin_cos();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            hardness: 0.5,
            position: 0.14,
            decay: 4.0,
            damping: 0.12,
            stiffness: 0.0003,
            release: 0.2,
            pickup: 0.0,
            electric: false,
            pickup_position: 0.14,
            tone: 12_000.0,
            pickup_q: 0.707,
            decay_ratio: 1.0,
            bow_speed: 0.0,
            bow_response: 0.012,
        }
    }

    #[test]
    fn draft_halves_the_partial_budget_and_high_keeps_the_standard_one() {
        let mut piano = Piano::default();
        let mut products = |hz, quality| {
            piano.excite(hz, 0.8, 48_000.0, quality, settings());
            piano.active * piano.strings
        };
        // A bass pair fills either budget; a treble triple runs out of band first.
        assert_eq!(products(55.0, Quality::Standard), BUDGET);
        assert_eq!(products(55.0, Quality::High), BUDGET);
        assert_eq!(products(55.0, Quality::Draft), DRAFT_BUDGET);
        let treble = products(1_760.0, Quality::Standard);
        assert!(treble < DRAFT_BUDGET);
        assert_eq!(products(1_760.0, Quality::Draft), treble);
    }

    #[test]
    fn contact_duration_and_string_count_follow_the_performance() {
        let mut piano = Piano::default();
        for (hz, strings) in [(55.0, 2), (110.0, 2), (440.0, 3)] {
            piano.excite(hz, 0.3, 48_000.0, Quality::Standard, settings());
            assert_eq!(piano.strings, strings);
            let soft = piano.contact_frames;
            piano.excite(
                hz,
                0.9,
                48_000.0,
                Quality::Standard,
                Settings {
                    hardness: 1.0,
                    ..settings()
                },
            );
            assert!(piano.contact_frames < soft);
        }
        piano.excite(65.4, 0.8, 48_000.0, Quality::Standard, settings());
        assert!(piano.active > 32);
    }

    #[test]
    fn unison_rotations_are_detuned_and_bridge_coupling_is_passive() {
        let mut piano = Piano::default();
        piano.excite(440.0, 0.8, 48_000.0, Quality::Standard, settings());
        let first = &piano.partials[0];
        assert!(first.strings[0].sine < first.strings[1].sine);
        assert!(first.strings[1].sine < first.strings[2].sine);
        for _ in 0..piano.contact_frames {
            piano.next();
        }
        let energy = |piano: &Piano| {
            piano.partials[..piano.active]
                .iter()
                .flat_map(|partial| &partial.strings[..piano.strings])
                .map(|string| string.real * string.real + string.imag * string.imag)
                .sum::<f32>()
        };
        let mut previous = energy(&piano);
        for _ in 0..24_000 {
            piano.next();
            let current = energy(&piano);
            assert!(current <= previous + 1e-6);
            previous = current;
        }
    }

    #[test]
    fn inaudible_partials_retire_before_entering_denormal_range() {
        let mut piano = Piano::default();
        piano.excite(
            130.8,
            0.8,
            48_000.0,
            Quality::Standard,
            Settings {
                decay: 0.3,
                ..settings()
            },
        );
        for _ in 0..96_000 {
            piano.next();
        }
        assert!(
            piano.partials[..piano.active]
                .iter()
                .any(|partial| partial.radius == 0.0)
        );
        assert!(piano.partials[0].radius > 0.0);
        for partial in &piano.partials[..piano.active] {
            for string in &partial.strings {
                assert!(string.real == 0.0 || string.real.abs() > f32::MIN_POSITIVE);
                assert!(string.imag == 0.0 || string.imag.abs() > f32::MIN_POSITIVE);
            }
        }
    }
}
