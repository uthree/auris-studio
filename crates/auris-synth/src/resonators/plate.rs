//! A steel sheet hung in a frame, driven at one point and heard at two.

use std::sync::Arc;

use auris_core::param::{ParamUnit, ParamValueCurve};
use auris_core::resonance::{
    RESONANCE_CURVE, RESONANCE_POINTS, RESONANCE_STRINGS, ResonanceCapture, ResonanceFrame,
    ResonanceKind, ResonanceMonitor,
};
use auris_core::{
    AudioBuffer, Effect, ParamDescriptor, ParamId, Parameterized, PluginCategory, PluginDescriptor,
    PrepareContext, ProcessContext,
};
use auris_dsp::{Biquad, BiquadCoefficients, DelayLine};

use super::{Allpass, Level, PUBLISH_TICKS, TICK, finite, sane_rate};
use crate::ParamBank;

/// Paths across the sheet the reverberant field is followed along.
const LINES: usize = 8;
/// Their lengths at size one, in milliseconds: the time a bending wave of a few hundred hertz
/// takes to cross a two-metre sheet and come back off the far edge, at mutually prime lengths
/// so no two paths reinforce each other's echoes.
const LENGTHS_MS: [f32; LINES] = [23.7, 27.1, 31.3, 34.9, 38.3, 42.7, 46.1, 51.9];
/// Largest size, which sets how long the paths are allocated.
const MAX_SIZE: f32 = 2.0;
/// Longest pre-delay, in milliseconds.
const MAX_PREDELAY_MS: f32 = 120.0;

/// Coefficients of the sections a wave passes on its first crossing, from the driver to the
/// pickups. A plate is dispersive: a bending wave's speed grows with the square root of its
/// frequency, so the top of a snare's crack reaches the pickups first and its body follows.
/// Each first-order section holds back what lies below its own corner, and the spread of
/// corners lays the delay out against frequency the way the sheet does.
const SPLASH: [f32; 24] = [
    -0.95, -0.94, -0.93, -0.92, -0.91, -0.9, -0.88, -0.86, -0.84, -0.82, -0.8, -0.77, -0.74, -0.7,
    -0.66, -0.62, -0.58, -0.54, -0.5, -0.45, -0.4, -0.35, -0.3, -0.25,
];

/// The picture's grid: the sheet's width along a string of the frame, its height across them.
const COLUMNS: usize = RESONANCE_POINTS;
const ROWS: usize = RESONANCE_STRINGS;
/// Courant number of the picture's sheet, `κ·k/h²`; the explicit scheme for a thin plate is
/// stable up to a quarter.
const COURANT: f32 = 0.2;
/// Where the driver sits on the sheet, as fractions of its width and height.
const DRIVER: (f32, f32) = (0.3, 0.4);
/// Where the two pickups sit.
const PICKUPS: [(f32, f32); 2] = [(0.72, 0.28), (0.76, 0.7)];

const DECAY: u32 = 0;
const DAMPING: u32 = 1;
const SIZE: u32 = 2;
const PREDELAY: u32 = 3;
const MIX: u32 = 4;
const WIDTH: u32 = 5;

/// Group delay of the first crossing at `hz`, in seconds, at `rate`.
#[cfg(test)]
pub(crate) fn splash_delay(hz: f32, rate: f32) -> f32 {
    let omega = std::f32::consts::TAU * hz / rate;
    let cosine = omega.cos();
    SPLASH
        .iter()
        .map(|a| (1.0 - a * a) / (1.0 + 2.0 * a * cosine + a * a))
        .sum::<f32>()
        / rate
}

/// Sixty-decibel decay time at `hz` of a plate set to `decay` seconds in the bass and
/// `damping` from bright (0) to dark (1), for paths `length` samples long.
pub(crate) fn decay_at(hz: f32, decay: f32, damping: f32, length: f32, rate: f32) -> f32 {
    let (g0, gpi) = loss_gains(decay, damping, length, rate);
    let pole = (g0 - gpi) / (g0 + gpi);
    let omega = std::f32::consts::TAU * hz / rate;
    // |g0 (1 - p) / (1 - p e^{-jω})| once round a path, as a decay rate.
    let magnitude = g0 * (1.0 - pole) / (1.0 - 2.0 * pole * omega.cos() + pole * pole).sqrt();
    let per_pass = -20.0 * magnitude.max(1.0e-12).log10();
    if per_pass <= 0.0 {
        return f32::INFINITY;
    }
    60.0 * length / (per_pass * rate)
}

/// The loss filter's gain per sample of path at no frequency and at the top, for `length`
/// samples of path.
fn loss_gains(decay: f32, damping: f32, length: f32, rate: f32) -> (f32, f32) {
    let top = decay * (1.0 - 0.9 * damping.clamp(0.0, 1.0));
    let gain = |seconds: f32| 10f32.powf(-3.0 * length / (seconds.max(0.05) * rate));
    (gain(decay), gain(top))
}

/// One path across the sheet with its own losses.
#[derive(Clone, Debug)]
struct Line {
    delay: DelayLine,
    /// Samples of path, at the current size.
    length: f32,
    /// The loss filter: gain at no frequency and its pole.
    gain: f32,
    pole: f32,
    state: f32,
}

/// A plate reverb.
///
/// What is heard is the sheet's reverberant field: eight paths across it, mixed into each other
/// at every reflection off the frame, each losing its top faster than its bottom as a sheet
/// of steel does, with the damping pad drawn closer. Before the field there is the first
/// crossing from the driver to the pickups, which is what makes a plate a plate rather than a
/// room: the sheet carries high frequencies faster than low ones, so every attack arrives as
/// a splash, top first.
///
/// The window shows the same sheet, slowed down to be seen: a coarse grid solving the
/// thin-plate equation, struck at the driver by every attack the track makes, its ripples
/// running out to the frame and dying at the plate's own decay.
pub struct PlateReverb {
    params: ParamBank,
    rate: f32,
    lines: Vec<Line>,
    splash: [Allpass; SPLASH.len()],
    predelay: DelayLine,
    /// Takes the sub-bass a sheet cannot carry out of the field.
    low_cut: [Biquad; 2],
    /// The settings the paths were last laid out for.
    laid: Option<(f32, f32, f32)>,
    mix: f32,
    phase: usize,
    ticks: u32,
    input: Level,
    output: Level,
    /// Energy of the tick before, to strike the picture's sheet on an attack.
    last_energy: f32,
    energy: f32,
    /// The picture's sheet, now and a step ago, with a ring of ghost cells either side.
    sheet: Vec<f32>,
    before: Vec<f32>,
    capture: ResonanceCapture,
}

impl Default for PlateReverb {
    fn default() -> Self {
        Self::new()
    }
}

/// Side of the picture's grid with its ghost ring.
const STRIDE: usize = COLUMNS + 4;
const CELLS: usize = STRIDE * (ROWS + 4);

impl PlateReverb {
    /// Stable plugin identifier stored in projects.
    pub const ID: &str = "auris.fx.plate";

    /// A plate of two and a half seconds. Allocates nothing until `prepare`.
    pub fn new() -> Self {
        let params = ParamBank::new(vec![
            ParamDescriptor::new(DECAY, "decay", "Decay", 0.4, 8.0, 2.5)
                .with_unit(ParamUnit::Seconds)
                .with_curve(ParamValueCurve::Logarithmic),
            ParamDescriptor::percent(DAMPING, "damping", "Damping", 0.45),
            ParamDescriptor::new(SIZE, "size", "Size", 0.5, MAX_SIZE, 1.0)
                .with_unit(ParamUnit::Ratio)
                .with_curve(ParamValueCurve::Logarithmic),
            ParamDescriptor::new(PREDELAY, "predelay", "Pre-Delay", 0.0, MAX_PREDELAY_MS, 8.0)
                .with_unit(ParamUnit::Milliseconds),
            ParamDescriptor::percent(MIX, "mix", "Mix", 0.3),
            ParamDescriptor::percent(WIDTH, "width", "Width", 1.0),
        ]);
        Self {
            params,
            rate: 48_000.0,
            lines: Vec::new(),
            splash: SPLASH.map(Allpass::new),
            predelay: DelayLine::new(0),
            low_cut: Default::default(),
            laid: None,
            mix: 0.3,
            phase: 0,
            ticks: 0,
            input: Level::default(),
            output: Level::default(),
            last_energy: 0.0,
            energy: 0.0,
            sheet: Vec::new(),
            before: Vec::new(),
            capture: ResonanceCapture::default(),
        }
    }

    /// Sets the paths' lengths and losses for the current settings. Allocates nothing.
    fn arrange(&mut self) {
        let wanted = (
            self.params.at(DECAY),
            self.params.at(DAMPING),
            self.params.at(SIZE),
        );
        if self.laid == Some(wanted) {
            return;
        }
        let (decay, damping, size) = wanted;
        for (line, ms) in self.lines.iter_mut().zip(LENGTHS_MS) {
            line.length =
                (ms * 0.001 * size * self.rate).clamp(2.0, line.delay.max_delay_samples() as f32);
            let (g0, gpi) = loss_gains(decay, damping, line.length, self.rate);
            line.gain = g0;
            line.pole = (g0 - gpi) / (g0 + gpi);
        }
        self.laid = Some(wanted);
    }

    /// One step of the picture's sheet: a thin plate, simply supported at the frame.
    fn step_sheet(&mut self, force: f32) {
        let decay = self.params.at(DECAY);
        // The picture steps twice a tick; its ripples die at the plate's own pace.
        let steps_per_second = 2.0 * self.rate / TICK as f32;
        let loss = 6.9 / (decay * steps_per_second);
        let mu2 = COURANT * COURANT;
        let at = |column: usize, row: usize| (row + 2) * STRIDE + column + 2;
        // The frame: the sheet is pinned there, so each edge is still and the ghost beyond it
        // mirrors the sheet inverted.
        let sheet = &mut self.sheet;
        for row in 2..ROWS + 2 {
            let base = row * STRIDE;
            sheet[base + 1] = 0.0;
            sheet[base] = -sheet[base + 2];
            sheet[base + COLUMNS + 2] = 0.0;
            sheet[base + COLUMNS + 3] = -sheet[base + COLUMNS + 1];
        }
        for column in 0..STRIDE {
            sheet[STRIDE + column] = 0.0;
            sheet[column] = -sheet[2 * STRIDE + column];
            sheet[(ROWS + 2) * STRIDE + column] = 0.0;
            sheet[(ROWS + 3) * STRIDE + column] = -sheet[(ROWS + 1) * STRIDE + column];
        }
        let driver = at(
            (DRIVER.0 * (COLUMNS - 1) as f32) as usize,
            (DRIVER.1 * (ROWS - 1) as f32) as usize,
        );
        for row in 0..ROWS {
            for column in 0..COLUMNS {
                let index = at(column, row);
                let u = &self.sheet;
                let near = u[index - 1] + u[index + 1] + u[index - STRIDE] + u[index + STRIDE];
                let diagonal = u[index - STRIDE - 1]
                    + u[index - STRIDE + 1]
                    + u[index + STRIDE - 1]
                    + u[index + STRIDE + 1];
                let far =
                    u[index - 2] + u[index + 2] + u[index - 2 * STRIDE] + u[index + 2 * STRIDE];
                let biharmonic = 20.0 * u[index] - 8.0 * near + 2.0 * diagonal + far;
                let push = if index == driver { force } else { 0.0 };
                let next = (2.0 * u[index] - (1.0 - loss) * self.before[index] - mu2 * biharmonic
                    + push)
                    / (1.0 + loss);
                // The step before is no longer needed once this one is written in its place.
                self.before[index] = if next.is_finite() { next } else { 0.0 };
            }
        }
        std::mem::swap(&mut self.sheet, &mut self.before);
    }

    fn publish(&mut self) {
        let mut frame = ResonanceFrame {
            kind: ResonanceKind::Plate,
            input: self.input.get(),
            output: self.output.get(),
            string_count: ROWS,
            ..ResonanceFrame::default()
        };
        for (row, string) in frame.strings.iter_mut().enumerate().take(ROWS) {
            for (column, point) in string.points.iter_mut().enumerate() {
                *point = self.sheet[(row + 2) * STRIDE + column + 2];
            }
        }
        let (decay, damping) = (self.params.at(DECAY), self.params.at(DAMPING));
        let length = self.lines.iter().map(|line| line.length).sum::<f32>() / LINES as f32;
        for (index, point) in frame.curve.iter_mut().enumerate() {
            let t = index as f32 / (RESONANCE_CURVE - 1) as f32;
            let hz = 50.0 * (16_000.0_f32 / 50.0).powf(t);
            *point = [hz.log10(), decay_at(hz, decay, damping, length, self.rate)];
        }
        frame.curve_len = RESONANCE_CURVE;
        frame.values[0] = decay;
        frame.values[1] = decay_at(8_000.0, decay, damping, length, self.rate);
        frame.values[2] = DRIVER.0;
        frame.values[3] = DRIVER.1;
        frame.values[4] = PICKUPS[0].0;
        frame.values[5] = PICKUPS[0].1;
        frame.values[6] = PICKUPS[1].0;
        frame.values[7] = PICKUPS[1].1;
        self.capture.publish(&frame);
    }

    fn tick(&mut self) {
        let energy = (self.energy / TICK as f32).sqrt();
        self.energy = 0.0;
        // Struck by what is new in the tick: an attack, not a held note.
        let force = (energy - self.last_energy).max(0.0) * 0.5;
        self.last_energy = energy;
        if self.capture.is_watched() {
            self.step_sheet(force);
            self.step_sheet(0.0);
        }
        self.ticks = self.ticks.wrapping_add(1);
        if self.capture.is_watched() && self.ticks.is_multiple_of(PUBLISH_TICKS) {
            self.publish();
        }
    }
}

impl Parameterized for PlateReverb {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        self.params.set(id, value);
    }
}

/// Signs each path is heard with at each pickup: the two see the field from different places.
const TAPS: [[f32; LINES]; 2] = [
    [1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, -1.0],
    [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0],
];

impl Effect for PlateReverb {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            Self::ID,
            "Plate Reverb",
            "A steel sheet in a frame: a splash of bending waves, top first, then a bright field",
            PluginCategory::Reverb,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = sane_rate(ctx.sample_rate);
        self.lines = LENGTHS_MS
            .iter()
            .map(|ms| Line {
                delay: DelayLine::new((ms * 0.001 * MAX_SIZE * self.rate).ceil() as usize + 2),
                length: 2.0,
                gain: 0.0,
                pole: 0.0,
                state: 0.0,
            })
            .collect();
        self.predelay = DelayLine::new((MAX_PREDELAY_MS * 0.001 * self.rate).ceil() as usize + 2);
        let cut = BiquadCoefficients::highpass(f64::from(self.rate), 70.0, 0.7);
        for filter in &mut self.low_cut {
            filter.set_coefficients(cut);
        }
        self.sheet = vec![0.0; CELLS];
        self.before = vec![0.0; CELLS];
        self.laid = None;
        self.reset();
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.delay.reset();
            line.state = 0.0;
        }
        self.predelay.reset();
        for section in &mut self.splash {
            section.reset();
        }
        for filter in &mut self.low_cut {
            filter.reset();
        }
        self.sheet.fill(0.0);
        self.before.fill(0.0);
        self.mix = self.params.at(MIX);
        self.phase = 0;
        self.ticks = 0;
        self.input.reset();
        self.output.reset();
        self.energy = 0.0;
        self.last_energy = 0.0;
    }

    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext) {
        if self.lines.is_empty() {
            return;
        }
        self.arrange();
        let frames = buffer.frame_count().min(ctx.block_frames);
        let stereo = buffer.channel_count() >= 2;
        let predelay = (self.params.at(PREDELAY) * 0.001 * self.rate).max(1.0);
        let width = self.params.at(WIDTH);
        let mix_target = self.params.at(MIX);
        // A field whose paths keep nearly everything they are given grows loud; this keeps a
        // long plate and a short one at about the same level.
        let mean_gain = self.lines.iter().map(|line| line.gain).sum::<f32>() / LINES as f32;
        let level = 0.9 * (1.0 - mean_gain * mean_gain).max(1.0e-4).sqrt() * (LINES as f32).sqrt();
        let (mut heard, mut out) = (0.0_f32, 0.0_f32);
        let channels = buffer.channels_mut();
        for frame in 0..frames {
            let left = finite(channels[0][frame]);
            let right = if stereo {
                finite(channels[1][frame])
            } else {
                left
            };
            let drive = 0.5 * (left + right);
            heard = heard.max(left.abs()).max(right.abs());
            self.energy += drive * drive;
            self.predelay.write(drive);
            let crossed = self
                .splash
                .iter_mut()
                .fold(self.predelay.read(predelay), |sample, section| {
                    section.process(sample)
                });
            // What each path delivers to the frame, after its losses.
            let mut arriving = [0.0_f32; LINES];
            for (value, line) in arriving.iter_mut().zip(&mut self.lines) {
                let raw = line.delay.read(line.length);
                line.state = line.gain * (1.0 - line.pole) * raw + line.pole * line.state;
                *value = line.state;
            }
            // A reflection off the frame sends every path into every other: a Householder
            // matrix, lossless, and mixing as evenly as eight paths can.
            let total: f32 = arriving.iter().sum::<f32>() * (2.0 / LINES as f32);
            for (index, line) in self.lines.iter_mut().enumerate() {
                let sign = if index % 2 == 0 { 1.0 } else { -1.0 };
                line.delay
                    .write(arriving[index] - total + sign * crossed * (1.0 / LINES as f32));
            }
            let mut wet = [0.0_f32; 2];
            for (side, taps) in wet.iter_mut().zip(&TAPS) {
                *side = arriving.iter().zip(taps).map(|(a, t)| a * t).sum::<f32>() * level;
            }
            let wet = [
                self.low_cut[0].process_sample(wet[0]),
                self.low_cut[1].process_sample(wet[1]),
            ];
            let (mid, side) = (0.5 * (wet[0] + wet[1]), 0.5 * (wet[1] - wet[0]) * width);
            let ramp = (frame + 1) as f32 / frames as f32;
            let mix = self.mix + (mix_target - self.mix) * ramp;
            let (out_left, out_right) = (
                (1.0 - mix) * left + mix * (mid - side),
                (1.0 - mix) * right + mix * (mid + side),
            );
            out = out.max(out_left.abs()).max(out_right.abs());
            if stereo {
                channels[0][frame] = out_left;
                channels[1][frame] = out_right;
            } else {
                channels[0][frame] = 0.5 * (out_left + out_right);
            }
            self.phase += 1;
            if self.phase == TICK {
                self.phase = 0;
                self.input.feed(heard);
                self.output.feed(out);
                (heard, out) = (0.0, 0.0);
                self.tick();
            }
        }
        self.mix = mix_target;
    }

    fn tail_frames(&self) -> usize {
        (self.params.at(DECAY) * 1.2 * self.rate) as usize
            + (MAX_PREDELAY_MS * 0.001 * self.rate) as usize
    }

    fn resonance_monitor(&self) -> Option<Arc<ResonanceMonitor>> {
        Some(self.capture.monitor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::rms;

    const RATE: f64 = 48_000.0;

    fn prepared() -> PlateReverb {
        let mut effect = PlateReverb::new();
        effect.prepare(&PrepareContext::new(RATE, 512, 2));
        effect
    }

    fn run(effect: &mut PlateReverb, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (Vec::new(), Vec::new());
        for (block, chunk) in input.chunks(480).enumerate() {
            let mut buffer = AudioBuffer::stereo(chunk.len(), RATE);
            buffer.channel_mut(0).copy_from_slice(chunk);
            buffer.channel_mut(1).copy_from_slice(chunk);
            effect.process(
                &mut buffer,
                &ProcessContext::realtime(RATE, chunk.len(), (block * 480) as u64, 120.0, true),
            );
            left.extend_from_slice(buffer.channel(0));
            right.extend_from_slice(buffer.channel(1));
        }
        (left, right)
    }

    fn impulse(frames: usize) -> Vec<f32> {
        let mut signal = vec![0.0; frames];
        signal[0] = 1.0;
        signal
    }

    #[test]
    fn the_first_crossing_carries_the_top_before_the_bottom() {
        let rate = RATE as f32;
        let early = splash_delay(5_000.0, rate);
        let late = splash_delay(200.0, rate);
        assert!(late - early > 0.004, "200 Hz {late} s, 5 kHz {early} s");
        assert!(splash_delay(100.0, rate) >= late);
    }

    #[test]
    fn the_tail_lasts_about_as_long_as_the_decay_says() {
        for decay in [1.0, 4.0] {
            let mut effect = prepared();
            effect.set_param(ParamId(MIX), 1.0);
            effect.set_param(ParamId(DECAY), decay);
            effect.set_param(ParamId(DAMPING), 0.0);
            effect.set_param(ParamId(PREDELAY), 0.0);
            let frames = (decay * 1.5) as usize * 48_000 + 48_000;
            let (left, _) = run(&mut effect, &impulse(frames));
            let window = 4_800;
            let early = rms(&left[4_800..4_800 + window]);
            let at = 4_800 + (decay * 0.5 * 48_000.0) as usize;
            let later = rms(&left[at..at + window]);
            // Half the decay time on, the field is about thirty decibels down.
            let drop = 20.0 * (early / later).log10();
            assert!((20.0..42.0).contains(&drop), "decay {decay}: {drop} dB");
        }
    }

    #[test]
    fn damping_shortens_the_top_and_leaves_the_bottom() {
        let rate = RATE as f32;
        let length = 0.038 * rate;
        assert!((decay_at(60.0, 3.0, 0.8, length, rate) - 3.0).abs() < 0.2);
        assert!(decay_at(8_000.0, 3.0, 0.8, length, rate) < 2.0);
        assert!(decay_at(20_000.0, 3.0, 0.8, length, rate) < 1.0);
        assert!(decay_at(8_000.0, 3.0, 0.0, length, rate) > 2.8);
    }

    #[test]
    fn long_and_short_plates_come_back_at_a_similar_level() {
        let noise: Vec<f32> = (0..48_000)
            .map(|index| crate::physical::body::seeded(7, index) - 0.5)
            .collect();
        let mut levels = Vec::new();
        for decay in [0.8, 6.0] {
            let mut effect = prepared();
            effect.set_param(ParamId(MIX), 1.0);
            effect.set_param(ParamId(DECAY), decay);
            let (left, _) = run(&mut effect, &noise);
            levels.push(20.0 * (rms(&left[24_000..]) / rms(&noise)).log10());
        }
        for level in &levels {
            assert!((-12.0..6.0).contains(level), "{levels:?}");
        }
        assert!((levels[0] - levels[1]).abs() < 8.0, "{levels:?}");
    }

    #[test]
    fn the_pickups_hear_the_field_differently() {
        let mut effect = prepared();
        effect.set_param(ParamId(MIX), 1.0);
        let (left, right) = run(&mut effect, &impulse(24_000));
        let difference: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
        assert!(rms(&difference) > 0.3 * rms(&left));
    }

    #[test]
    fn a_watched_plate_ripples_after_an_attack_and_publishes_its_decay() {
        let mut effect = prepared();
        let monitor = effect.resonance_monitor().unwrap();
        monitor.watch(true);
        let mut burst = vec![0.0; 9_600];
        for (index, sample) in burst.iter_mut().enumerate().take(2_400) {
            *sample = (crate::physical::body::seeded(3, index as u32) - 0.5) * 1.6;
        }
        let _ = run(&mut effect, &burst);
        let frame = monitor.read().unwrap();
        assert_eq!(frame.kind, ResonanceKind::Plate);
        assert_eq!(frame.string_count, ROWS);
        let moving = frame.strings[..ROWS]
            .iter()
            .flat_map(|row| row.points)
            .map(f32::abs)
            .fold(0.0, f32::max);
        assert!(moving > 1.0e-4, "{moving}");
        assert!(moving.is_finite());
        assert_eq!(frame.curve_len, RESONANCE_CURVE);
        assert!(frame.curve[0][1] > frame.curve[RESONANCE_CURVE - 1][1]);
    }
}
