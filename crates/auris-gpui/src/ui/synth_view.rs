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
//! under it.
//!
//! The pictures of the patch are computed here from its parameters rather than read from the
//! instrument, because they have to be there before a note is played — they are what a person
//! looks at while turning the knobs — and because both are short closed forms: a Bessel function
//! and a shape. The numbers are pinned by the tests below.

use super::physical_model::voice_color;
use crate::{app::AurisApp, theme::Theme};
use atelier::plot::{LinePlot, Series, SeriesStyle};
use atelier::{Card, Label, SectionHeader, TextColor, TextSize};
use auris_i18n::Key;
use auris_session::{MotionFrame, prelude::*};
use gpui::{AnyElement, div, prelude::*};

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

/// Highest multiple of the note the sideband and harmonic plots reach.
const HIGHEST_HARMONIC: usize = 16;

/// Top of the filter plot: unity gain sits halfway, so a resonant peak has room above it.
const FILTER_CEILING: f32 = 2.0;

/// Samples in a drawn cycle, the same as the instrument publishes.
const CYCLE_POINTS: usize = auris_session::MOTION_POINTS;

/// Whether `id` is drawn as oscillators rather than as a body.
pub(crate) fn is_oscillator_model(id: &str) -> bool {
    matches!(id, FM2 | CHIPTUNE) || is_analog(id)
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
    fn instrument_param(&mut self, track: TrackId, key: &str) -> Option<f32> {
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
    ) -> AnyElement {
        let spacing = theme.atelier.spacing;
        let content = if id == FM2 {
            self.fm_operators(track, frame, theme)
        } else if is_analog(id) {
            self.analog_patch(track, frame, theme)
        } else {
            self.chip_shape(track, theme)
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
    fn fm_operators(&mut self, track: TrackId, frame: &MotionFrame, theme: &Theme) -> AnyElement {
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
            .child(
                div()
                    .id("synth-sidebands")
                    .debug_selector(|| "synth-sidebands".into())
                    .h(gpui::rems(9.))
                    .rounded(tokens.shape.radius_sm)
                    .bg(tokens.colors.sunken)
                    .overflow_hidden()
                    .p(spacing.xs)
                    .child(plot),
            )
            .into_any_element()
    }

    /// The subtractive stack across its detune beside the filter that shapes it.
    fn analog_patch(&mut self, track: TrackId, frame: &MotionFrame, theme: &Theme) -> AnyElement {
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
        let resting = cutoff * (key_track * (pitch - KEY_TRACK_CENTRE) / 12.0).exp2() / note;
        let now = live
            .map(|voice| voice.excitation / note)
            .filter(|ratio| ratio.is_finite() && *ratio > 0.0);

        let stack = unison_stack(unison, detune, mix);
        let reach = (detune * 110.0).max(25.0);
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
        let well = |id: &'static str, plot: LinePlot| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .h(gpui::rems(8.))
                .rounded(tokens.shape.radius_sm)
                .bg(tokens.colors.sunken)
                .overflow_hidden()
                .p(spacing.xs)
                .child(plot)
        };
        let column = |header: SectionHeader, body: gpui::Stateful<gpui::Div>| {
            div()
                .flex_1()
                .min_w(gpui::rems(14.))
                .flex()
                .flex_col()
                .gap(spacing.sm)
                .child(header)
                .child(body)
        };
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
                    .child(column(
                        SectionHeader::new(self.t(Key::SynthStack))
                            .description(self.t(Key::SynthStackHint)),
                        well(
                            "synth-stack",
                            LinePlot::new()
                                .x_range(-reach, reach)
                                .y_range(0., 1.)
                                .series(Series::stems(stack).color(color)),
                        ),
                    ))
                    .child(column(
                        SectionHeader::new(self.t(Key::SynthFilter))
                            .description(self.t(Key::SynthFilterHint)),
                        well("synth-filter", filter),
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

    /// The chiptune voice's cycle beside the harmonics it has.
    fn chip_shape(&mut self, track: TrackId, theme: &Theme) -> AnyElement {
        let tokens = &theme.atelier;
        let spacing = tokens.spacing;
        let cycle = self.oscillator_resting_cycle(track, CHIPTUNE);
        let harmonics = cycle_harmonics(&cycle, HIGHEST_HARMONIC);
        let unison = self
            .instrument_param(track, "unison")
            .unwrap_or(1.0)
            .round();
        let spread = self.instrument_param(track, "spread").unwrap_or(0.0);
        let color = tokens.colors.series[0];
        let last = cycle.last().copied().unwrap_or_default();
        let shape: Vec<(f32, f32)> = cycle
            .iter()
            .enumerate()
            .map(|(i, sample)| (i as f32 / CYCLE_POINTS as f32, *sample))
            .chain(std::iter::once((1.0, last)))
            .collect();
        let well = |id: &'static str, plot: LinePlot| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .h(gpui::rems(8.))
                .rounded(tokens.shape.radius_sm)
                .bg(tokens.colors.sunken)
                .overflow_hidden()
                .p(spacing.xs)
                .child(plot)
        };
        let column = |header: SectionHeader, body: gpui::Stateful<gpui::Div>| {
            div()
                .flex_1()
                .min_w(gpui::rems(14.))
                .flex()
                .flex_col()
                .gap(spacing.sm)
                .child(header)
                .child(body)
        };
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
                    .child(column(
                        SectionHeader::new(self.t(Key::SynthCycle))
                            .description(self.t(Key::SynthCycleHint)),
                        well(
                            "synth-cycle",
                            LinePlot::new()
                                .x_range(0., 1.)
                                .y_range(-1., 1.)
                                .series(Series::line(shape).style(SeriesStyle::Steps).color(color)),
                        ),
                    ))
                    .child(column(
                        SectionHeader::new(self.t(Key::SynthHarmonics)),
                        well(
                            "synth-harmonics",
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
                        ),
                    )),
            )
            .children((unison > 1.).then(|| {
                Label::new(format!(
                    "{} {unison:.0} · ±{spread:.2} st",
                    self.t(Key::SynthUnison)
                ))
                .size(TextSize::XSmall)
                .color(TextColor::Faint)
            }))
            .into_any_element()
    }
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
}
