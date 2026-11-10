//! The oscillator instruments' view: the built-in synthesisers drawn the way the physical
//! models are.
//!
//! A synthesiser has no body, so what [`super::physical_model`] draws as a string is here what
//! the oscillator plays — one cycle of each sounding voice, published by the instrument as a
//! [`MotionGeometry::Oscillator`](auris_session::MotionGeometry::Oscillator) frame — and what it
//! draws as resonant modes is here the patch itself: for the FM operator pair, the two operators
//! and the sidebands their depth throws out; for the chiptune voice, the one cycle its waveform,
//! duty cycle and bit depth make, and that cycle's harmonics; for the subtractive patches, the
//! unison stack spread across its detune and the filter's response with the note's harmonics
//! under it; for the kick basses, one note's punch falling onto its pitch and the curve its drive
//! bends every sample through; and for the electronic kits, the kick pad as it is struck and the
//! hats' metal oscillators under the filter that makes them hiss.
//!
//! The pictures of the patch are computed here from its parameters rather than read from the
//! instrument, because they have to be there before a note is played — they are what a person
//! looks at while turning the knobs — and because both are short closed forms: a Bessel function
//! and a shape. The numbers are pinned by the tests below.

use super::physical_model::voice_color;
use super::synth_edit::{PlotScale, SynthPlot, stack_across, stack_reach};
use crate::{app::AurisApp, theme::Theme};
use atelier::plot::{LinePlot, Series, SeriesStyle};
use atelier::{Card, Label, SectionHeader, TextColor, TextSize};
use auris_i18n::Key;
use auris_session::{MotionFrame, prelude::*};
use gpui::{AnyElement, Context, div, prelude::*};

/// The FM operator pair.
pub(crate) const FM2: &str = "auris.synth.fm2";
/// The chiptune voice.
pub(crate) const CHIPTUNE: &str = "auris.synth.chiptune";

/// The subtractive synthesiser's patches, every one the same engine with its own defaults.
pub(crate) const ANALOG: [&str; 7] = [
    "auris.synth.supersaw",
    "auris.synth.pad",
    "auris.synth.lead",
    "auris.synth.bass",
    "auris.synth.pwm_strings",
    "auris.synth.brass",
    "auris.synth.pluck",
];

/// Whether `id` is one of the subtractive patches.
pub(crate) fn is_analog(id: &str) -> bool {
    ANALOG.contains(&id)
}

/// The kick basses: one pitched kick formula, played from the keyboard.
pub(crate) const BOOM: [&str; 3] = [
    "auris.synth.boom_bass",
    "auris.synth.kick_bass",
    "auris.synth.sub_bass",
];

/// Whether `id` is one of the kick basses.
pub(crate) fn is_boom(id: &str) -> bool {
    BOOM.contains(&id)
}

/// The electronic drum machines, every pad a formula.
pub(crate) const ELECTRO: [&str; 3] = [
    "auris.synth.electro_kit",
    "auris.synth.edm_kit",
    "auris.synth.trap_kit",
];

/// Whether `id` is one of the electronic drum machines.
pub(crate) fn is_electro_kit(id: &str) -> bool {
    ELECTRO.contains(&id)
}

/// Octaves a kick's punch starts above its pitch at a punch of one, as both instruments have it.
const PUNCH_OCTAVES: f32 = 4.0;

/// The velocity a picture of one note is drawn at: the audition's own.
const PICTURE_VELOCITY: f32 = 0.75;

/// How much of a kick bass's note its punch picture spans, in seconds.
const PUNCH_WINDOW: f32 = 0.25;

/// How much of a kit's kick its picture spans, in seconds.
const KICK_WINDOW: f32 = 0.3;

/// Points a sweep picture is drawn with: enough for the punch's fastest cycles.
const SWEEP_POINTS: usize = 900;

/// Time constants an exponential segment runs for, as `auris_dsp`'s envelope counts them.
const SEGMENT_TIME_CONSTANTS: f32 = 9.4907;

/// The kit's six hat oscillators, in hertz, before the hat tone scales them.
const METAL_HZ: [f32; 6] = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0];

/// Highest frequency the hats' picture reaches, in kilohertz.
const METAL_CEILING_KHZ: f32 = 18.0;

/// The soft clipper both instruments put a kick through: `tanh(g·x) / tanh(g)`, `g = 1 + 9·drive`.
pub(crate) fn soft_clip(x: f32, drive: f32) -> f32 {
    if drive <= 0.0 {
        return x;
    }
    let gain = 1.0 + drive.clamp(0.0, 1.0) * 9.0;
    (x * gain).tanh() / gain.tanh()
}

/// A sine whose pitch follows `hz` over the first `seconds`, under the level `amp`, through the
/// soft clipper: the shape of a struck kick, as `(milliseconds, sample)` points.
pub(crate) fn sweep_wave(
    seconds: f32,
    drive: f32,
    hz: impl Fn(f32) -> f32,
    amp: impl Fn(f32) -> f32,
) -> Vec<(f32, f32)> {
    const SUBSTEPS: usize = 8;
    let step = seconds / (SWEEP_POINTS * SUBSTEPS) as f32;
    let mut phase = 0.0_f32;
    let mut points = Vec::with_capacity(SWEEP_POINTS + 1);
    for point in 0..=SWEEP_POINTS {
        let t = point as f32 * seconds / SWEEP_POINTS as f32;
        let sample = soft_clip((std::f32::consts::TAU * phase).sin(), drive) * amp(t);
        points.push((t * 1_000.0, sample));
        for sub in 0..SUBSTEPS {
            phase += hz(t + sub as f32 * step) * step;
        }
        phase -= phase.floor();
    }
    points
}

/// A kick bass's note: its pitch `2^(sweep · e^(−t/τ))` above `note` hertz.
pub(crate) fn boom_hz(note: f32, punch: f32, punch_time: f32) -> impl Fn(f32) -> f32 {
    let sweep = punch * PUNCH_OCTAVES * (0.8 + 0.2 * PICTURE_VELOCITY);
    let tau = punch_time.max(0.0005);
    move |t| note * (sweep * (-t / tau).exp()).exp2()
}

/// A kit's kick: `tune + (start − tune) · e^(−t/τ)`, the punch falling linearly in hertz.
pub(crate) fn kick_hz(tune: f32, punch: f32) -> impl Fn(f32) -> f32 {
    let start = tune * (punch * PUNCH_OCTAVES * (0.8 + 0.2 * PICTURE_VELOCITY)).exp2();
    let tau = 0.006 + 0.03 * (1.0 - punch);
    move |t| tune + (start - tune) * (-t / tau).exp()
}

/// An exponential fall from one to silence over `seconds`, as the kit's decays are.
fn falling(seconds: f32) -> impl Fn(f32) -> f32 {
    let seconds = seconds.max(1.0e-3);
    move |t| (-t * SEGMENT_TIME_CONSTANTS / seconds).exp()
}

/// An attack, then a fall onto `sustain` over `decay`: the kick bass's envelope while held.
fn held_envelope(attack: f32, decay: f32, sustain: f32) -> impl Fn(f32) -> f32 {
    let (attack, decay) = (attack.max(1.0e-4), decay.max(1.0e-3));
    move |t| {
        if t < attack {
            t / attack
        } else {
            sustain + (1.0 - sustain) * (-(t - attack) * SEGMENT_TIME_CONSTANTS / decay).exp()
        }
    }
}

/// One steady cycle of a kick bass at `tone_ratio` times its note: the sine through the drive,
/// then the one-pole tone filter, as the instrument publishes a sounding note.
pub(crate) fn boom_cycle(drive: f32, tone_ratio: f32) -> Vec<f32> {
    let coef = 1.0 - (-std::f32::consts::TAU * tone_ratio.max(0.0) / CYCLE_POINTS as f32).exp();
    let mut state = 0.0_f32;
    let mut cycle = vec![0.0; CYCLE_POINTS];
    // Three cycles first, so the filter has settled.
    for pass in 0..4 {
        for (index, out) in cycle.iter_mut().enumerate() {
            let x = (std::f32::consts::TAU * index as f32 / CYCLE_POINTS as f32).sin();
            state += (soft_clip(x, drive) - state) * coef;
            if pass == 3 {
                *out = state;
            }
        }
    }
    cycle
}

/// The gain of a second-order high-pass at `fc` and quality `q`, at `f`.
fn highpass_gain(f: f32, fc: f32, q: f32) -> f32 {
    let r = f / fc.max(1.0);
    r * r / ((1.0 - r * r).powi(2) + (r / q).powi(2)).sqrt()
}

/// The gain of a second-order band-pass at `fc` and quality `q`, at `f`: one at its centre.
fn bandpass_gain(f: f32, fc: f32, q: f32) -> f32 {
    let r = f / fc.max(1.0);
    (r / q) / ((1.0 - r * r).powi(2) + (r / q).powi(2)).sqrt()
}

/// The hats' filter at `hz`: a high-pass into a band-pass, both moved up by the hat tone.
pub(crate) fn hat_filter(hz: f32, hat_tone: f32) -> f32 {
    let bright = 0.75 + 0.6 * hat_tone;
    highpass_gain(hz, 6_500.0 * bright, 0.7) * bandpass_gain(hz, 9_500.0 * bright, 0.9)
}

/// The odd harmonics of the six hat squares that reach the picture, as `(kHz, level)` stems
/// after the filter, the loudest at `metal`.
pub(crate) fn metal_stems(hat_tone: f32, metal: f32) -> Vec<(f32, f32)> {
    let ceiling = METAL_CEILING_KHZ * 1_000.0;
    let mut stems = Vec::new();
    for base in METAL_HZ {
        let fundamental = base * (0.85 + 0.3 * hat_tone);
        let mut harmonic = 1.0;
        while fundamental * harmonic <= ceiling {
            let hz = fundamental * harmonic;
            stems.push((hz / 1_000.0, hat_filter(hz, hat_tone) / harmonic));
            harmonic += 2.0;
        }
    }
    let loudest = stems.iter().fold(0.0_f32, |a, (_, level)| a.max(*level));
    if loudest > 0.0 {
        for (_, level) in &mut stems {
            *level *= metal / loudest;
        }
    }
    stems
}

/// Highest multiple of the note the sideband and harmonic plots reach.
const HIGHEST_HARMONIC: usize = 16;

/// Harmonics a kick bass's picture shows: as many as its instrument publishes.
const MOTION_MODES_SHOWN: usize = auris_session::MOTION_MODES;

/// Top of the filter plot: unity gain sits halfway, so a resonant peak has room above it.
const FILTER_CEILING: f32 = 2.0;

/// Samples in a drawn cycle, the same as the instrument publishes.
const CYCLE_POINTS: usize = auris_session::MOTION_POINTS;

/// Whether `id` is drawn as oscillators rather than as a body.
pub(crate) fn is_oscillator_model(id: &str) -> bool {
    matches!(id, FM2 | CHIPTUNE) || is_analog(id) || is_boom(id) || is_electro_kit(id)
}

/// The equal-tempered frequency of a MIDI pitch, A4 at 440 Hz.
fn note_hz(pitch: f32) -> f32 {
    440.0 * ((pitch - 69.0) / 12.0).exp2()
}

/// The note key tracking is measured from, as the instrument measures it.
const KEY_TRACK_CENTRE: f32 = 60.0;

/// How far each side copy of the subtractive stack is moved off its even spacing — the
/// instrument's own table, so the picture puts every copy where it sounds.
const DETUNE_SKEW: [f32; 7] = [0.0, 0.05, -0.07, 0.0, 0.06, -0.04, 0.03];

/// The subtractive stack as `(cents from the note, level relative to the centre copy)`.
pub(crate) fn unison_stack(count: f32, detune: f32, mix: f32) -> Vec<(f32, f32)> {
    let count = (count.round() as usize).clamp(1, DETUNE_SKEW.len());
    (0..count)
        .map(|slot| {
            let position = if count > 1 {
                2.0 * slot as f32 / (count - 1) as f32 - 1.0
            } else {
                0.0
            };
            let offset = detune * (position + DETUNE_SKEW[slot] * position.abs());
            let level = if count == 1 || position.abs() < 1e-6 {
                1.0
            } else {
                mix.clamp(0.0, 1.0)
            };
            (offset * 100.0, level)
        })
        .collect()
}

/// The resonant section's damping for a 0–1 resonance, as the instrument sets it.
fn damping(resonance: f32) -> f32 {
    2.0 - 1.8 * resonance.clamp(0.0, 1.0)
}

/// The subtractive filter's complex response at `ratio` times its cutoff: a resonant
/// second-order low-pass followed by a Butterworth one.
fn filter_response(ratio: f32, resonance: f32) -> (f32, f32) {
    let section = |k: f32| {
        let (re, im) = (1.0 - ratio * ratio, k * ratio);
        let norm = (re * re + im * im).max(f32::MIN_POSITIVE);
        (re / norm, -im / norm)
    };
    let (a, b) = section(damping(resonance));
    let (c, d) = section(std::f32::consts::SQRT_2);
    (a * c - b * d, a * d + b * c)
}

/// The filter's gain at `ratio` times its cutoff.
pub(crate) fn filter_gain(ratio: f32, resonance: f32) -> f32 {
    let (re, im) = filter_response(ratio.max(0.0), resonance);
    re.hypot(im)
}

/// One cycle of the subtractive stack's waveform (saw, square, triangle) as the filter leaves
/// it with its cutoff at `ratio` times the note, scaled so its peak is one.
pub(crate) fn analog_cycle(
    waveform: f32,
    pulse_width: f32,
    ratio: f32,
    resonance: f32,
) -> Vec<f32> {
    let width = pulse_width.clamp(0.01, 0.99);
    let naive: Vec<f32> = (0..CYCLE_POINTS)
        .map(|i| {
            let x = i as f32 / CYCLE_POINTS as f32;
            match waveform.round() as i32 {
                1 => {
                    if x < width {
                        1.0
                    } else {
                        -1.0
                    }
                }
                2 => 1.0 - 4.0 * (x - 0.5).abs(),
                _ => 2.0 * x - 1.0,
            }
        })
        .collect();
    let length = CYCLE_POINTS as f32;
    let ratio = ratio.max(1e-3);
    let mut out = vec![0.0; CYCLE_POINTS];
    for harmonic in 1..CYCLE_POINTS / 2 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (at, sample) in naive.iter().enumerate() {
            let angle = std::f32::consts::TAU * harmonic as f32 * at as f32 / length;
            re += sample * angle.cos();
            im -= sample * angle.sin();
        }
        let (hr, hi) = filter_response(harmonic as f32 / ratio, resonance);
        let (yr, yi) = (re * hr - im * hi, re * hi + im * hr);
        for (at, sample) in out.iter_mut().enumerate() {
            let angle = std::f32::consts::TAU * harmonic as f32 * at as f32 / length;
            *sample += 2.0 / length * (yr * angle.cos() - yi * angle.sin());
        }
    }
    let loudest = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    if loudest > 1e-6 {
        out.iter_mut().for_each(|sample| *sample /= loudest);
    }
    out
}

/// The Bessel function of the first kind, `J_n(x)`, from Bessel's own integral.
///
/// `J_n(x) = 1/π ∫₀^π cos(nτ − x sin τ) dτ`, by the trapezoid rule. The integrand is smooth
/// and periodic, so the rule converges far faster than its order suggests: 256 panels hold every
/// order and depth the FM instrument can reach to well inside a pixel.
pub(crate) fn bessel_j(order: i32, x: f64) -> f64 {
    const PANELS: usize = 256;
    let n = f64::from(order);
    let step = std::f64::consts::PI / PANELS as f64;
    let integrand = |tau: f64| (n * tau - x * tau.sin()).cos();
    let ends = (integrand(0.0) + integrand(std::f64::consts::PI)) / 2.0;
    let inner: f64 = (1..PANELS).map(|i| integrand(i as f64 * step)).sum();
    (ends + inner) * step / std::f64::consts::PI
}

/// The spectrum of the operator pair: `(frequency as a multiple of the note, amplitude)`.
///
/// `sin(ωt + β sin(rωt))` is the sum over every `k` of `J_k(β) sin((1 + k·r)ωt)`. A sideband
/// below zero hertz folds back to the positive frequency with its sign flipped, and lands on
/// whatever is already there — which is how an integer ratio makes a harmonic spectrum out of
/// what is, in general, not one. Components closer than a thousandth of the note are one.
pub(crate) fn fm_sidebands(ratio: f32, index: f32, highest: f32) -> Vec<(f32, f32)> {
    let ratio = f64::from(ratio.max(1e-3));
    let index = f64::from(index.max(0.0));
    let reach = (highest as f64 + 1.0) / ratio + index + 4.0;
    let orders = reach.ceil().min(256.0) as i32;
    let mut lines: Vec<(f64, f64)> = Vec::new();
    for k in -orders..=orders {
        let frequency = 1.0 + f64::from(k) * ratio;
        let amplitude = bessel_j(k, index);
        let (frequency, amplitude) = if frequency < 0.0 {
            (-frequency, -amplitude)
        } else {
            (frequency, amplitude)
        };
        if frequency > f64::from(highest) + 1e-3 {
            continue;
        }
        match lines
            .iter_mut()
            .find(|(at, _)| (*at - frequency).abs() < 1e-3)
        {
            Some((_, sum)) => *sum += amplitude,
            None => lines.push((frequency, amplitude)),
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    lines
        .into_iter()
        .map(|(frequency, amplitude)| (frequency as f32, amplitude.abs() as f32))
        .filter(|(_, amplitude)| *amplitude > 0.004)
        .collect()
}

/// One carrier period of the operator pair at `index` radians, both starting at phase zero.
pub(crate) fn fm_cycle(ratio: f32, index: f32) -> Vec<f32> {
    use std::f32::consts::TAU;
    (0..CYCLE_POINTS)
        .map(|i| {
            let x = i as f32 / CYCLE_POINTS as f32;
            (TAU * x + index * (TAU * ratio * x).sin()).sin()
        })
        .collect()
}

/// One cycle of the chiptune voice's waveform as its parameters set it: the shape in the
/// parameter's order (sine, square, saw, triangle, noise), the square at its duty cycle, every
/// sample in the bit depth's steps.
///
/// Noise is the NES register from its first state, shifted 32 times a cycle as the instrument
/// shifts it: a picture of the grain rather than of any particular moment of it.
pub(crate) fn chip_cycle(waveform: f32, pulse_width: f32, bits: f32) -> Vec<f32> {
    let width = pulse_width.clamp(0.01, 0.99);
    let mut register: u16 = 1;
    let mut cycle: Vec<f32> = (0..CYCLE_POINTS)
        .map(|i| {
            let x = i as f32 / CYCLE_POINTS as f32;
            match waveform.round() as i32 {
                0 => (std::f32::consts::TAU * x).sin(),
                2 => 2.0 * x - 1.0,
                3 => 1.0 - 4.0 * (x - 0.5).abs(),
                4 => {
                    let level = if register & 1 == 0 { 1.0 } else { -1.0 };
                    if i % 2 == 1 {
                        let feedback = (register ^ (register >> 1)) & 1;
                        register = (register >> 1) | (feedback << 14);
                    }
                    level
                }
                _ => {
                    if x < width {
                        1.0
                    } else {
                        -1.0
                    }
                }
            }
        })
        .collect();
    let bits = bits.round().clamp(1.0, 16.0);
    if bits < 16.0 {
        let levels = (bits - 1.0).exp2();
        for sample in &mut cycle {
            *sample = (*sample * levels).round() / levels;
        }
    }
    cycle
}

/// The magnitudes of a cycle's first `count` harmonics, relative to the strongest.
pub(crate) fn cycle_harmonics(cycle: &[f32], count: usize) -> Vec<f32> {
    let length = cycle.len().max(1) as f32;
    let mut magnitudes: Vec<f32> = (1..=count)
        .map(|harmonic| {
            let (mut real, mut imaginary) = (0.0_f32, 0.0_f32);
            for (at, sample) in cycle.iter().enumerate() {
                let angle = std::f32::consts::TAU * harmonic as f32 * at as f32 / length;
                real += sample * angle.cos();
                imaginary += sample * angle.sin();
            }
            real.hypot(imaginary)
        })
        .collect();
    let loudest = magnitudes.iter().fold(0.0_f32, |a, b| a.max(*b));
    if loudest > 1e-6 {
        magnitudes.iter_mut().for_each(|value| *value /= loudest);
    } else {
        magnitudes.fill(0.0);
    }
    magnitudes
}

/// The voice a picture of the patch follows: the loudest one sounding.
fn loudest_voice(frame: &MotionFrame) -> Option<&auris_session::MotionVoice> {
    frame
        .voices
        .iter()
        .filter(|voice| voice.level > 0.0 && voice.pitch.is_finite())
        .max_by(|a, b| a.level.total_cmp(&b.level))
}

impl AurisApp {
    /// One of an instrument's parameters by its stable key, as it stands now.
    pub(crate) fn instrument_param(&mut self, track: TrackId, key: &str) -> Option<f32> {
        self.session
            .instrument_descriptors(track)
            .iter()
            .find(|descriptor| descriptor.key == key)
            .map(|descriptor| {
                self.session.param_value(
                    ParamTarget::Instrument {
                        track,
                        param: descriptor.id,
                    },
                    descriptor,
                )
            })
    }

    /// The patch's own cycle, drawn faintly on the first lane while nothing sounds: what a
    /// note would look like before one is played.
    pub(crate) fn oscillator_resting_cycle(&mut self, track: TrackId, id: &str) -> Vec<f32> {
        let pitch = f32::from(self.physical_view.pitch);
        let mut param =
            |key: &str, default: f32| self.instrument_param(track, key).unwrap_or(default);
        match id {
            FM2 => {
                let ratio = param("ratio", 2.0);
                fm_cycle(ratio, param("index", 5.0))
            }
            id if is_boom(id) => {
                let tone = param("tone", 3_000.0);
                boom_cycle(param("drive", 0.0), tone / note_hz(pitch))
            }
            id if is_electro_kit(id) => {
                // Two periods of the kick's ring, as a struck pad's scope holds them.
                let drive = param("kick_drive", 0.2);
                (0..CYCLE_POINTS)
                    .map(|index| {
                        let x = std::f32::consts::TAU * 2.0 * index as f32 / CYCLE_POINTS as f32;
                        soft_clip(x.sin(), drive)
                    })
                    .collect()
            }
            id if is_analog(id) => {
                let cutoff = param("cutoff", 8_000.0)
                    * (param("key_track", 0.0) * (pitch - KEY_TRACK_CENTRE) / 12.0).exp2();
                analog_cycle(
                    param("waveform", 0.0),
                    param("pulse_width", 0.5),
                    cutoff / note_hz(pitch),
                    param("resonance", 0.0),
                )
            }
            _ => {
                let waveform = param("waveform", 1.0);
                let pulse_width = param("pulse_width", 0.5);
                chip_cycle(waveform, pulse_width, param("bits", 16.0))
            }
        }
    }

    /// The patch as a picture: the operators and their sidebands, or the cycle and its harmonics.
    pub(crate) fn oscillator_analysis(
        &mut self,
        track: TrackId,
        id: &str,
        frame: &MotionFrame,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spacing = theme.atelier.spacing;
        let content = if id == FM2 {
            self.fm_operators(track, frame, theme, cx)
        } else if is_analog(id) {
            self.analog_patch(track, frame, theme, cx)
        } else if is_boom(id) {
            self.boom_patch(track, frame, theme, cx)
        } else if is_electro_kit(id) {
            self.electro_kit(track, frame, theme, cx)
        } else {
            self.chip_shape(track, theme, cx)
        };
        Card::new()
            .id("physical-analysis")
            .debug_selector(|| "physical-analysis".into())
            .w_full()
            .flex_shrink_0()
            .p(spacing.lg)
            .gap(spacing.md)
            .child(content)
            .into_any_element()
    }

    /// The two operators, the depth between them, and the spectrum that depth makes.
    fn fm_operators(
        &mut self,
        track: TrackId,
        frame: &MotionFrame,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let ratio = self.instrument_param(track, "ratio").unwrap_or(2.0);
        let index = self.instrument_param(track, "index").unwrap_or(5.0);
        let decay = self.instrument_param(track, "mod_decay").unwrap_or(0.35);
        let live = loudest_voice(frame);
        let now = live.map(|voice| voice.excitation);
        let operator = |name: Key, value: String, accent: bool| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(spacing.xxs)
                .px(spacing.md)
                .py(spacing.sm)
                .min_w(gpui::rems(7.))
                .rounded(tokens.shape.radius_md)
                .bg(tokens.colors.sunken)
                .border_1()
                .border_color(if accent {
                    tokens.colors.accent
                } else {
                    tokens.colors.border
                })
                .child(Label::new(self.t(name)).size(TextSize::Small))
                .child(Label::new(value).mono().color(TextColor::Muted))
        };
        let link = |caption: Option<String>| {
            div()
                .flex_1()
                .min_w(gpui::rems(3.))
                .flex()
                .flex_col()
                .items_center()
                .gap(spacing.xxs)
                .child(
                    Label::new(caption.unwrap_or_default())
                        .mono()
                        .size(TextSize::XSmall)
                        .color(TextColor::Muted),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .h(gpui::px(1.5))
                                .bg(tokens.colors.border_strong),
                        )
                        .child(Label::new("▸").color(TextColor::Faint)),
                )
        };
        let depth = match now {
            Some(now) => format!("{} {now:.1} / {index:.1}", self.t(Key::SynthDepth)),
            None => format!("{} {index:.1}", self.t(Key::SynthDepth)),
        };
        let diagram = div()
            .id("synth-operators")
            .debug_selector(|| "synth-operators".into())
            .w_full()
            .flex()
            .items_center()
            .gap(spacing.sm)
            .child(operator(
                Key::SynthModulator,
                format!("× {ratio:.2}"),
                false,
            ))
            .child(link(Some(depth)))
            .child(operator(Key::SynthCarrier, "× 1.00".to_owned(), true))
            .child(link(None))
            .child(Label::new(self.t(Key::SynthOutput)).size(TextSize::Small));

        let highest = HIGHEST_HARMONIC as f32;
        let mut plot = LinePlot::new()
            .x_range(0., highest)
            .y_range(0., 1.)
            .legend(true)
            .series(
                Series::stems(fm_sidebands(ratio, index, highest))
                    .name(self.t(Key::SynthAttack))
                    .color(tokens.colors.text_faint)
                    .faint(),
            );
        if let (Some(voice), Some(now)) = (live, now) {
            plot = plot.series(
                Series::stems(fm_sidebands(ratio, now, highest))
                    .style(SeriesStyle::Stems)
                    .name(self.t(Key::SynthNow))
                    .color(voice_color(theme, voice.pitch)),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(spacing.md)
            .child(
                SectionHeader::new(self.t(Key::SynthOperators))
                    .description(self.t(Key::SynthOperatorsHint)),
            )
            .child(diagram)
            .child(
                Label::new(format!("{} · {decay:.2} s", self.t(Key::SynthDepthDecay)))
                    .size(TextSize::XSmall)
                    .color(TextColor::Faint),
            )
            .child(
                SectionHeader::new(self.t(Key::SynthSidebands))
                    .description(self.t(Key::SynthSidebandsHint)),
            )
            .child(self.synth_plot_well(
                "synth-sidebands",
                track,
                SynthPlot::Sidebands,
                PlotScale {
                    across: highest,
                    reach: highest,
                },
                plot,
                cx,
            ))
            .child(edit_hint(self.t(Key::SynthSidebandsEdit)))
            .into_any_element()
    }

    /// The subtractive stack across its detune beside the filter that shapes it.
    fn analog_patch(
        &mut self,
        track: TrackId,
        frame: &MotionFrame,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let resting_pitch = f32::from(self.physical_view.pitch);
        let mut param =
            |key: &str, default: f32| self.instrument_param(track, key).unwrap_or(default);
        let unison = param("unison", 1.0);
        let detune = param("detune", 0.0);
        let mix = param("mix", 0.8);
        let width = param("width", 0.5);
        let waveform = param("waveform", 0.0);
        let pulse_width = param("pulse_width", 0.5);
        let cutoff = param("cutoff", 8_000.0);
        let resonance = param("resonance", 0.0);
        let key_track = param("key_track", 0.0);
        let live = loudest_voice(frame);
        let pitch = live.map_or(resting_pitch, |voice| voice.pitch);
        let note = note_hz(pitch).max(1.0);
        let tracked = (key_track * (pitch - KEY_TRACK_CENTRE) / 12.0).exp2();
        let resting = cutoff * tracked / note;
        let now = live
            .map(|voice| voice.excitation / note)
            .filter(|ratio| ratio.is_finite() && *ratio > 0.0);

        let stack = unison_stack(unison, detune, mix);
        let reach = self
            .held_reach(track, SynthPlot::Stack)
            .unwrap_or_else(|| stack_reach(detune));
        let color = tokens.colors.series[0];
        let highest = HIGHEST_HARMONIC as f32;
        let response = |ratio: f32| -> Vec<(f32, f32)> {
            (0..=160)
                .map(|step| {
                    let at = step as f32 / 10.0;
                    (at, filter_gain(at / ratio, resonance).min(FILTER_CEILING))
                })
                .collect()
        };
        let harmonics = |ratio: f32| -> Vec<(f32, f32)> {
            let shape = analog_cycle(waveform, pulse_width, 1.0e3, 0.0);
            cycle_harmonics(&shape, HIGHEST_HARMONIC)
                .into_iter()
                .enumerate()
                .map(|(n, value)| {
                    let harmonic = n as f32 + 1.0;
                    (
                        harmonic,
                        (value * filter_gain(harmonic / ratio, resonance)).min(FILTER_CEILING),
                    )
                })
                .collect()
        };
        let mut filter = LinePlot::new()
            .x_range(0., highest)
            .y_range(0., FILTER_CEILING)
            .legend(true)
            .series(
                Series::line(response(resting))
                    .name(self.t(Key::SynthCutoffSet))
                    .color(tokens.colors.text_faint)
                    .faint(),
            );
        let (lit, lit_color) = match (live, now) {
            (Some(voice), Some(now)) => (now, voice_color(theme, voice.pitch)),
            _ => (resting, color),
        };
        if let (Some(voice), Some(now)) = (live, now) {
            filter = filter.series(
                Series::line(response(now))
                    .name(self.t(Key::SynthNow))
                    .color(voice_color(theme, voice.pitch)),
            );
        }
        filter = filter.series(
            Series::stems(harmonics(lit))
                .style(SeriesStyle::Stems)
                .color(lit_color),
        );
        // The knee a drag moves, marked where the set cutoff meets its curve.
        if resting <= highest {
            filter = filter.series(
                Series::line(vec![(
                    resting,
                    filter_gain(1.0, resonance).min(FILTER_CEILING),
                )])
                .style(SeriesStyle::LineMarkers)
                .color(color),
            );
        }
        let stack_well = self.synth_plot_well(
            "synth-stack",
            track,
            SynthPlot::Stack,
            PlotScale {
                across: stack_across(reach),
                reach,
            },
            LinePlot::new()
                .x_range(-reach, reach)
                .y_range(0., 1.)
                .series(Series::stems(stack).color(color)),
            cx,
        );
        let filter_well = self.synth_plot_well(
            "synth-filter",
            track,
            SynthPlot::Filter,
            PlotScale {
                across: highest * note / tracked.max(f32::MIN_POSITIVE),
                reach: highest,
            },
            filter,
            cx,
        );
        let cutoff_now = live.map_or(cutoff, |voice| voice.excitation);
        div()
            .flex()
            .flex_col()
            .gap(spacing.md)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_wrap()
                    .gap(spacing.lg)
                    .child(plot_column(
                        theme,
                        SectionHeader::new(self.t(Key::SynthStack))
                            .description(self.t(Key::SynthStackHint)),
                        stack_well,
                        Some(self.t(Key::SynthStackEdit)),
                    ))
                    .child(plot_column(
                        theme,
                        SectionHeader::new(self.t(Key::SynthFilter))
                            .description(self.t(Key::SynthFilterHint)),
                        filter_well,
                        Some(self.t(Key::SynthFilterEdit)),
                    )),
            )
            .child(
                Label::new(format!(
                    "{} {:.0} · ±{:.0} cents · {} {:.0}% · {} {:.0} Hz",
                    self.t(Key::SynthUnison),
                    unison.round(),
                    detune * 100.0,
                    self.t(Key::SynthWidth),
                    width * 100.0,
                    self.t(Key::SynthCutoff),
                    cutoff_now,
                ))
                .size(TextSize::XSmall)
                .color(TextColor::Faint),
            )
            .into_any_element()
    }

    /// A kick bass: one note's punch, the drive curve every sample goes through, and the
    /// harmonics the two leave on the note.
    fn boom_patch(
        &mut self,
        track: TrackId,
        frame: &MotionFrame,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let resting_pitch = f32::from(self.physical_view.pitch);
        let mut param =
            |key: &str, default: f32| self.instrument_param(track, key).unwrap_or(default);
        let punch = param("punch", 0.3);
        let punch_time = param("punch_time", 0.025);
        let drive = param("drive", 0.4);
        let tone = param("tone", 3_200.0);
        let attack = param("attack", 0.001);
        let decay = param("decay", 1.0);
        let sustain = param("sustain", 0.3);
        let live = loudest_voice(frame);
        let pitch = live.map_or(resting_pitch, |voice| voice.pitch);
        let note = note_hz(pitch).max(1.0);
        let color = tokens.colors.series[0];
        let lit = live.map_or(color, |voice| voice_color(theme, voice.pitch));

        let envelope = held_envelope(attack, decay, sustain);
        let wave = sweep_wave(
            PUNCH_WINDOW,
            drive,
            boom_hz(note, punch, punch_time),
            &envelope,
        );
        let window_ms = PUNCH_WINDOW * 1_000.0;
        let outline: Vec<(f32, f32)> = (0..=100)
            .map(|step| {
                let t = PUNCH_WINDOW * step as f32 / 100.0;
                (t * 1_000.0, envelope(t))
            })
            .collect();
        let punch_well = self.synth_plot_well(
            "synth-punch",
            track,
            SynthPlot::Punch,
            PlotScale {
                across: 0.1,
                reach: window_ms,
            },
            LinePlot::new()
                .x_range(0., window_ms)
                .y_range(-1., 1.)
                .series(
                    Series::line(outline)
                        .color(tokens.colors.text_faint)
                        .faint(),
                )
                .series(Series::line(wave).color(lit)),
            cx,
        );
        let curve: Vec<(f32, f32)> = (0..=100)
            .map(|step| {
                let x = step as f32 / 50.0 - 1.0;
                (x, soft_clip(x, drive))
            })
            .collect();
        let drive_well = self.synth_plot_well(
            "synth-drive",
            track,
            SynthPlot::Drive,
            PlotScale {
                across: 1.0,
                reach: 1.0,
            },
            LinePlot::new()
                .x_range(-1., 1.)
                .y_range(-1., 1.)
                .series(
                    Series::line(vec![(-1., -1.), (1., 1.)])
                        .color(tokens.colors.text_faint)
                        .faint(),
                )
                .series(Series::line(curve).color(color)),
            cx,
        );
        let harmonics: Vec<f32> = match live {
            Some(voice) if voice.modes.iter().any(|mode| *mode > 0.0) => voice.modes.to_vec(),
            _ => cycle_harmonics(&boom_cycle(drive, tone / note), MOTION_MODES_SHOWN),
        };
        let harmonics_well = div()
            .id("synth-harmonics")
            .debug_selector(|| "synth-harmonics".into())
            .h(gpui::rems(8.))
            .rounded(tokens.shape.radius_sm)
            .bg(tokens.colors.sunken)
            .overflow_hidden()
            .p(spacing.xs)
            .child(
                LinePlot::new()
                    .x_range(0., MOTION_MODES_SHOWN as f32 + 1.0)
                    .y_range(0., 1.)
                    .series(
                        Series::stems(
                            harmonics
                                .iter()
                                .enumerate()
                                .map(|(n, value)| (n as f32 + 1., *value))
                                .collect(),
                        )
                        .color(lit),
                    ),
            );
        let now = live.map_or(note, |voice| voice.excitation);
        div()
            .flex()
            .flex_col()
            .gap(spacing.md)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_wrap()
                    .gap(spacing.lg)
                    .child(plot_column(
                        theme,
                        SectionHeader::new(self.t(Key::SynthPunch))
                            .description(self.t(Key::SynthPunchHint)),
                        punch_well,
                        Some(self.t(Key::SynthPunchEdit)),
                    ))
                    .child(plot_column(
                        theme,
                        SectionHeader::new(self.t(Key::SynthDrive))
                            .description(self.t(Key::SynthDriveHint)),
                        drive_well,
                        Some(self.t(Key::SynthDriveEdit)),
                    )),
            )
            .child(
                div().w_full().flex().child(plot_column(
                    theme,
                    SectionHeader::new(self.t(Key::SynthHarmonics))
                        .description(self.t(Key::SynthHarmonicsHint)),
                    harmonics_well,
                    None,
                )),
            )
            .child(
                Label::new(format!(
                    "{} {:.0}% · {:.0} ms · {} {:.0}% · {} {now:.0} Hz",
                    self.t(Key::SynthPunchAmount),
                    punch * 100.0,
                    punch_time * 1_000.0,
                    self.t(Key::SynthDrive),
                    drive * 100.0,
                    self.t(Key::SynthNow),
                ))
                .size(TextSize::XSmall)
                .color(TextColor::Faint),
            )
            .into_any_element()
    }

    /// An electronic kit: its kick as struck, and its hats' metal under their filter.
    fn electro_kit(
        &mut self,
        track: TrackId,
        frame: &MotionFrame,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let mut param =
            |key: &str, default: f32| self.instrument_param(track, key).unwrap_or(default);
        let tune = param("kick_tune", 50.0);
        let decay = param("kick_decay", 0.5);
        let punch = param("kick_punch", 0.4);
        let drive = param("kick_drive", 0.2);
        let hat_tone = param("hat_tone", 0.5);
        let metal = param("metal", 0.6);
        let color = tokens.colors.series[0];
        // Lit in the colour of a kick still sounding, if one is.
        let kick = frame
            .voices
            .iter()
            .find(|voice| voice.level > 0.0 && matches!(voice.pitch.round() as i32, 35 | 36));
        let lit = kick.map_or(color, |voice| voice_color(theme, voice.pitch));

        let envelope = falling(decay);
        let wave = sweep_wave(KICK_WINDOW, drive, kick_hz(tune, punch), &envelope);
        let window_ms = KICK_WINDOW * 1_000.0;
        let outline: Vec<(f32, f32)> = (0..=100)
            .map(|step| {
                let t = KICK_WINDOW * step as f32 / 100.0;
                (t * 1_000.0, envelope(t))
            })
            .collect();
        let kick_well = self.synth_plot_well(
            "synth-kick",
            track,
            SynthPlot::Kick,
            PlotScale {
                across: 1.0,
                reach: window_ms,
            },
            LinePlot::new()
                .x_range(0., window_ms)
                .y_range(-1., 1.)
                .series(
                    Series::line(outline)
                        .color(tokens.colors.text_faint)
                        .faint(),
                )
                .series(Series::line(wave).color(lit)),
            cx,
        );
        let response: Vec<(f32, f32)> = (1..=180)
            .map(|step| {
                let khz = METAL_CEILING_KHZ * step as f32 / 180.0;
                (khz, hat_filter(khz * 1_000.0, hat_tone).min(1.0))
            })
            .collect();
        let metal_well = self.synth_plot_well(
            "synth-metal",
            track,
            SynthPlot::Metal,
            PlotScale {
                across: 1.0,
                reach: METAL_CEILING_KHZ,
            },
            LinePlot::new()
                .x_range(0., METAL_CEILING_KHZ)
                .y_range(0., 1.)
                .series(
                    Series::line(response)
                        .color(tokens.colors.text_faint)
                        .faint(),
                )
                .series(
                    Series::stems(metal_stems(hat_tone, metal))
                        .style(SeriesStyle::Stems)
                        .color(color),
                ),
            cx,
        );
        div()
            .flex()
            .flex_col()
            .gap(spacing.md)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_wrap()
                    .gap(spacing.lg)
                    .child(plot_column(
                        theme,
                        SectionHeader::new(self.t(Key::SynthKick))
                            .description(self.t(Key::SynthKickHint)),
                        kick_well,
                        Some(self.t(Key::SynthKickEdit)),
                    ))
                    .child(plot_column(
                        theme,
                        SectionHeader::new(self.t(Key::SynthMetal))
                            .description(self.t(Key::SynthMetalHint)),
                        metal_well,
                        Some(self.t(Key::SynthMetalEdit)),
                    )),
            )
            .child(
                Label::new(format!(
                    "{} {tune:.0} Hz · {} {:.0}% · {} {:.0}% · {} {:.0}%",
                    self.t(Key::SynthKickAmount),
                    self.t(Key::SynthPunchAmount),
                    punch * 100.0,
                    self.t(Key::SynthDrive),
                    drive * 100.0,
                    self.t(Key::SynthMetalAmount),
                    metal * 100.0,
                ))
                .size(TextSize::XSmall)
                .color(TextColor::Faint),
            )
            .into_any_element()
    }

    /// The chiptune voice's cycle beside the harmonics it has, and its unison stack.
    fn chip_shape(&mut self, track: TrackId, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let cycle = self.oscillator_resting_cycle(track, CHIPTUNE);
        let harmonics = cycle_harmonics(&cycle, HIGHEST_HARMONIC);
        let unison = self
            .instrument_param(track, "unison")
            .unwrap_or(1.0)
            .round();
        let spread = self.instrument_param(track, "spread").unwrap_or(0.0);
        let square = self
            .instrument_param(track, "waveform")
            .is_some_and(|waveform| waveform.round() as i32 == CHIP_SQUARE);
        let color = tokens.colors.series[0];
        let last = cycle.last().copied().unwrap_or_default();
        let shape: Vec<(f32, f32)> = cycle
            .iter()
            .enumerate()
            .map(|(i, sample)| (i as f32 / CYCLE_POINTS as f32, *sample))
            .chain(std::iter::once((1.0, last)))
            .collect();
        let reach = self
            .held_reach(track, SynthPlot::ChipStack)
            .unwrap_or_else(|| stack_reach(spread));
        let cycle_well = self.synth_plot_well(
            "synth-cycle",
            track,
            SynthPlot::Cycle,
            PlotScale {
                // Only a square has a duty cycle to move; the other shapes take the bit depth.
                across: if square { 1.0 } else { 0.0 },
                reach: 1.0,
            },
            LinePlot::new()
                .x_range(0., 1.)
                .y_range(-1., 1.)
                .series(Series::line(shape).style(SeriesStyle::Steps).color(color)),
            cx,
        );
        let stack_well = self.synth_plot_well(
            "synth-chip-stack",
            track,
            SynthPlot::ChipStack,
            PlotScale {
                across: stack_across(reach),
                reach,
            },
            LinePlot::new()
                .x_range(-reach, reach)
                .y_range(0., 1.)
                .series(Series::stems(chip_stack(unison, spread)).color(color)),
            cx,
        );
        let harmonics_well = div()
            .id("synth-harmonics")
            .debug_selector(|| "synth-harmonics".into())
            .h(gpui::rems(8.))
            .rounded(tokens.shape.radius_sm)
            .bg(tokens.colors.sunken)
            .overflow_hidden()
            .p(spacing.xs)
            .child(
                LinePlot::new()
                    .x_range(0., HIGHEST_HARMONIC as f32)
                    .y_range(0., 1.)
                    .series(
                        Series::stems(
                            harmonics
                                .iter()
                                .enumerate()
                                .map(|(n, value)| (n as f32 + 1., *value))
                                .collect(),
                        )
                        .color(color),
                    ),
            );
        div()
            .flex()
            .flex_col()
            .gap(spacing.md)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_wrap()
                    .gap(spacing.lg)
                    .child(plot_column(
                        theme,
                        SectionHeader::new(self.t(Key::SynthCycle))
                            .description(self.t(Key::SynthCycleHint)),
                        cycle_well,
                        Some(self.t(if square {
                            Key::SynthCycleEdit
                        } else {
                            Key::SynthCycleEditBits
                        })),
                    ))
                    .child(plot_column(
                        theme,
                        SectionHeader::new(self.t(Key::SynthHarmonics))
                            .description(self.t(Key::SynthHarmonicsHint)),
                        harmonics_well,
                        None,
                    )),
            )
            .child(
                div().w_full().flex().child(plot_column(
                    theme,
                    SectionHeader::new(self.t(Key::SynthStack))
                        .description(self.t(Key::SynthChipStackHint)),
                    stack_well,
                    Some(self.t(Key::SynthChipStackEdit)),
                )),
            )
            .child(
                Label::new(format!(
                    "{} {unison:.0} · ±{:.0} cents",
                    self.t(Key::SynthUnison),
                    spread * 100.0,
                ))
                .size(TextSize::XSmall)
                .color(TextColor::Faint),
            )
            .into_any_element()
    }
}

/// The chiptune parameter's index for the square, the one shape with a duty cycle.
const CHIP_SQUARE: i32 = 1;

/// The chiptune stack as `(cents from the note, level)`: the copies spread evenly over
/// ±`spread` semitones, every one at the same level, as the instrument spreads them.
pub(crate) fn chip_stack(count: f32, spread: f32) -> Vec<(f32, f32)> {
    let count = count.round().max(1.0) as usize;
    (0..count)
        .map(|slot| {
            let position = if count > 1 {
                2.0 * slot as f32 / (count - 1) as f32 - 1.0
            } else {
                0.0
            };
            (position * spread * 100.0, 1.0)
        })
        .collect()
}

/// One picture beside the others: its header, its well, and a faint line saying what dragging
/// it does.
fn plot_column(
    theme: &Theme,
    header: SectionHeader,
    body: gpui::Stateful<gpui::Div>,
    hint: Option<&'static str>,
) -> gpui::Div {
    let spacing = theme.atelier.spacing;
    div()
        .flex_1()
        .min_w(gpui::rems(14.))
        .flex()
        .flex_col()
        .gap(spacing.sm)
        .child(header)
        .child(body)
        .children(hint.map(edit_hint))
}

/// What dragging a picture does, under it.
fn edit_hint(text: &'static str) -> Label {
    Label::new(text)
        .size(TextSize::XSmall)
        .color(TextColor::Faint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_supersaw_stack_is_centred_and_reaches_its_detune() {
        let stack = unison_stack(7.0, 0.3, 0.8);
        assert_eq!(stack.len(), 7);
        assert_eq!(stack[3], (0.0, 1.0));
        assert!((stack[0].0 + 30.0).abs() < 3.0 && (stack[6].0 - 30.0).abs() < 3.0);
        assert!(stack.iter().filter(|(_, level)| *level == 0.8).count() == 6);
        assert_eq!(unison_stack(1.0, 0.5, 0.2), vec![(0.0, 1.0)]);
    }

    #[test]
    fn the_filter_passes_below_its_cutoff_and_falls_24_db_an_octave_above() {
        assert!((filter_gain(0.01, 0.0) - 1.0).abs() < 1e-3);
        let octave = filter_gain(2.0, 0.0);
        let two = filter_gain(4.0, 0.0);
        // Far above the corner each octave costs four powers of two.
        assert!((octave / two - 16.0).abs() < 3.0, "{octave} → {two}");
        // Resonance lifts the corner itself.
        assert!(filter_gain(1.0, 1.0) > filter_gain(1.0, 0.0) * 4.0);
    }

    #[test]
    fn a_closed_filter_rounds_the_saw_into_a_sine() {
        let open = cycle_harmonics(&analog_cycle(0.0, 0.5, 40.0, 0.0), 8);
        assert!((open[1] - 0.5).abs() < 0.05, "{open:?}");
        let closed = cycle_harmonics(&analog_cycle(0.0, 0.5, 1.0, 0.0), 8);
        assert!(closed[3] < 0.02, "{closed:?}");
        let cycle = analog_cycle(1.0, 0.5, 3.0, 0.3);
        assert_eq!(cycle.len(), CYCLE_POINTS);
        assert!((cycle.iter().fold(0.0f32, |a, b| a.max(b.abs())) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn bessel_functions_match_their_tables() {
        // Abramowitz & Stegun, table 9.1.
        assert!((bessel_j(0, 1.0) - 0.765_197_686_6).abs() < 1e-9);
        assert!((bessel_j(1, 1.0) - 0.440_050_585_7).abs() < 1e-9);
        assert!((bessel_j(2, 5.0) - 0.046_565_116_3).abs() < 1e-9);
        // The first zero of J₀, where a carrier vanishes from its own spectrum.
        assert!(bessel_j(0, 2.404_825_557_7).abs() < 1e-9);
        // Negative orders mirror with the order's parity.
        assert!((bessel_j(-3, 4.0) + bessel_j(3, 4.0)).abs() < 1e-12);
        // The deepest index the instrument reaches, at an order its power series loses in f64.
        assert!((bessel_j(20, 24.0) - 0.161_912_651_7).abs() < 1e-9);
    }

    #[test]
    fn a_silent_modulator_leaves_the_carrier_alone() {
        assert_eq!(fm_sidebands(2.0, 0.0, 16.0), vec![(1.0, 1.0)]);
    }

    #[test]
    fn an_integer_ratio_puts_every_sideband_on_a_harmonic() {
        let lines = fm_sidebands(2.0, 1.0, 16.0);
        // 1 ± 2k: the odd harmonics only, the hollow tone a 2:1 pair is known for.
        assert!(lines.iter().all(|(at, _)| (at.round() - at).abs() < 1e-4));
        assert!(lines.iter().all(|(at, _)| at.round() as i32 % 2 == 1));
        // The fundamental is J₀ and the folded J₋₁ together: 0.7652 + 0.4401.
        let fundamental = lines.iter().find(|(at, _)| *at == 1.0).unwrap().1;
        assert!((fundamental - 1.205_248).abs() < 1e-4, "{fundamental}");
        // The third is J₁ and the folded J₋₂, which arrives inverted: 0.4401 − 0.1149.
        let third = lines.iter().find(|(at, _)| *at == 3.0).unwrap().1;
        assert!((third - 0.325_148).abs() < 1e-4, "{third}");
    }

    #[test]
    fn a_bell_ratio_lands_between_the_harmonics_and_depth_widens_the_spread() {
        let shallow = fm_sidebands(3.5, 0.5, 16.0);
        let deep = fm_sidebands(3.5, 8.0, 16.0);
        assert!(shallow.iter().any(|(at, _)| (*at - 4.5).abs() < 1e-4));
        assert!(shallow.iter().any(|(at, _)| (*at - 2.5).abs() < 1e-4));
        let widest = |lines: &[(f32, f32)]| {
            lines
                .iter()
                .filter(|(_, amplitude)| *amplitude > 0.05)
                .fold(0.0_f32, |a, (at, _)| a.max(*at))
        };
        assert!(widest(&deep) > widest(&shallow) + 5.0);
        assert!(deep.iter().all(|(at, _)| *at <= 16.0));
    }

    #[test]
    fn the_chip_cycle_follows_duty_cycle_and_bit_depth() {
        let pulse = chip_cycle(1.0, 0.125, 16.0);
        assert_eq!(pulse.len(), 64);
        assert_eq!(pulse.iter().filter(|s| **s > 0.0).count(), 8);
        let triangle = chip_cycle(3.0, 0.5, 2.0);
        // Two bits are two steps either side of zero.
        let mut levels: Vec<i32> = triangle.iter().map(|s| (s * 2.0) as i32).collect();
        levels.sort_unstable();
        levels.dedup();
        assert_eq!(levels, vec![-2, -1, 0, 1, 2]);
        let noise = chip_cycle(4.0, 0.5, 16.0);
        assert!(noise.iter().all(|s| s.abs() == 1.0));
        assert!(noise.iter().any(|s| *s > 0.0) && noise.iter().any(|s| *s < 0.0));
    }

    #[test]
    fn a_square_has_only_odd_harmonics_and_a_saw_has_them_all() {
        let square = cycle_harmonics(&chip_cycle(1.0, 0.5, 16.0), 16);
        assert_eq!(square[0], 1.0);
        assert!(square[1] < 1e-4 && square[3] < 1e-4);
        assert!((square[2] - 1.0 / 3.0).abs() < 0.01);
        let saw = cycle_harmonics(&chip_cycle(2.0, 0.5, 16.0), 16);
        assert!((saw[1] - 0.5).abs() < 0.01 && (saw[3] - 0.25).abs() < 0.02);
        assert_eq!(cycle_harmonics(&[0.0; 64], 4), vec![0.0; 4]);
    }

    #[test]
    fn the_fm_cycle_is_a_sine_until_the_modulator_moves_it() {
        let plain = fm_cycle(2.0, 0.0);
        assert!((plain[16] - 1.0).abs() < 1e-5);
        let bright = cycle_harmonics(&fm_cycle(1.0, 3.0), 8);
        assert!(bright[2..].iter().any(|value| *value > 0.3), "{bright:?}");
    }

    #[test]
    fn the_soft_clipper_keeps_full_scale_and_squares_off_with_drive() {
        assert_eq!(soft_clip(0.5, 0.0), 0.5);
        for drive in [0.1, 0.5, 1.0] {
            assert!((soft_clip(1.0, drive) - 1.0).abs() < 1e-6);
            assert!((soft_clip(-1.0, drive) + 1.0).abs() < 1e-6);
        }
        assert!(soft_clip(0.3, 1.0) > 0.99);
        assert!(soft_clip(0.3, 0.1) < 0.6);
    }

    #[test]
    fn a_kick_falls_from_its_punch_onto_its_tune() {
        let kick = kick_hz(50.0, 0.5);
        let start = 50.0 * (0.5 * PUNCH_OCTAVES * (0.8 + 0.2 * PICTURE_VELOCITY)).exp2();
        assert!((kick(0.0) - start).abs() < 1e-3);
        assert!((kick(0.3) - 50.0).abs() < 0.01);
        let boom = boom_hz(41.2, 0.3, 0.025);
        assert!(boom(0.0) > 41.2 * 2.0);
        assert!((boom(0.25) - 41.2).abs() < 0.05);
        // No punch is the note from the first sample.
        assert!((boom_hz(41.2, 0.0, 0.025)(0.0) - 41.2).abs() < 1e-4);
    }

    #[test]
    fn a_kick_picture_rings_at_its_tune_once_the_punch_is_over() {
        let wave = sweep_wave(KICK_WINDOW, 0.0, kick_hz(50.0, 0.4), |_| 1.0);
        assert_eq!(wave.len(), SWEEP_POINTS + 1);
        assert_eq!(wave.last().unwrap().0, KICK_WINDOW * 1_000.0);
        // From 100 ms to 300 ms a 50 Hz ring rises through zero ten times.
        let rising = wave
            .windows(2)
            .filter(|pair| pair[0].0 >= 100.0 && pair[0].1 < 0.0 && pair[1].1 >= 0.0)
            .count();
        assert!((9..=11).contains(&rising), "{rising}");
        let quiet = sweep_wave(KICK_WINDOW, 0.0, kick_hz(50.0, 0.4), falling(0.1));
        assert!(quiet.last().unwrap().1.abs() < 1e-3);
    }

    #[test]
    fn the_kick_bass_cycle_is_a_sine_until_driven() {
        let clean = cycle_harmonics(&boom_cycle(0.0, 100.0), 4);
        assert!(clean[1] < 1e-3 && clean[2] < 1e-3, "{clean:?}");
        let driven = cycle_harmonics(&boom_cycle(0.9, 100.0), 4);
        assert!(driven[2] > 0.2, "{driven:?}");
        assert!(driven[1] < 1e-3, "a symmetric clip makes no even harmonics");
        // A tone below the note rounds the drive back off.
        let dark = cycle_harmonics(&boom_cycle(0.9, 0.8), 4);
        assert!(dark[2] < driven[2] * 0.5, "{dark:?}");
    }

    #[test]
    fn the_hats_pass_their_band_and_the_tone_moves_it() {
        let dull = hat_filter(9_500.0 * 0.75, 0.0);
        assert!(dull > 0.5, "{dull}");
        assert!(hat_filter(1_000.0, 0.5) < 0.01);
        assert!(hat_filter(12_000.0, 1.0) > hat_filter(12_000.0, 0.0));
        let stems = metal_stems(0.5, 0.6);
        let loudest = stems.iter().fold(0.0_f32, |a, (_, level)| a.max(*level));
        assert!((loudest - 0.6).abs() < 1e-5);
        assert!(stems.iter().all(|(khz, _)| *khz <= METAL_CEILING_KHZ));
        assert!(metal_stems(0.5, 0.0).iter().all(|(_, level)| *level == 0.0));
    }
}
