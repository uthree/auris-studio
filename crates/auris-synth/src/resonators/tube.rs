//! A triode gain stage and the iron after it.

use std::borrow::Cow;
use std::sync::Arc;

use auris_core::param::db_to_gain;
use auris_core::resonance::{
    RESONANCE_CURVE, ResonanceCapture, ResonanceFrame, ResonanceKind, ResonanceMonitor,
};
use auris_core::{
    AudioBuffer, Effect, ParamDescriptor, ParamId, Parameterized, PluginCategory, PluginDescriptor,
    PrepareContext, ProcessContext,
};

use super::{Level, PUBLISH_TICKS, TICK, finite, sane_rate};
use crate::ParamBank;

/// Supply voltage.
const SUPPLY: f32 = 300.0;
/// Plate load resistor, in ohms.
const PLATE_LOAD: f32 = 100_000.0;
/// Cathode resistor at no bias offset, and its bypass capacitor.
const CATHODE: f32 = 1_500.0;
const CATHODE_CAP: f32 = 22.0e-6;
/// Grid stopper, coupling capacitor, grid leak, and the grid's own resistance once it
/// conducts.
const GRID_STOPPER: f32 = 68_000.0;
const COUPLING_CAP: f32 = 22.0e-9;
const GRID_LEAK: f32 = 1.0e6;
const GRID_CONDUCTING: f32 = 2_000.0;
/// Grid-to-cathode voltages the stage's curve is laid out over.
const GRID_RANGE: (f32, f32) = (-14.0, 4.0);
const TABLE: usize = 2_049;
/// Grid voltage, in volts, that full scale swings at no drive.
const VOLTS_AT_FULL_SCALE: f32 = 0.5;

/// Oversampling factor, and the anti-aliasing filter's length at the high rate.
const FACTOR: usize = 4;
const TAPS: usize = 65;
const PHASE_TAPS: usize = TAPS.div_ceil(FACTOR);
/// The two filters' combined delay, in samples at the base rate.
const LATENCY: usize = (TAPS - 1) / FACTOR;

const VALVE: u32 = 0;
const DRIVE: u32 = 1;
const BIAS: u32 = 2;
const IRON: u32 = 3;
const OUTPUT: u32 = 4;

/// Koren's triode coefficients: amplification factor, exponent, `kg1`, `kp` and `kvb`.
#[derive(Clone, Copy, Debug)]
struct Triode {
    mu: f32,
    ex: f32,
    kg1: f32,
    kp: f32,
    kvb: f32,
}

/// The valves a stage can be built with.
const TRIODES: [Triode; 3] = [
    // 12AX7: high gain, the preamp valve of most guitar amplifiers.
    Triode {
        mu: 100.0,
        ex: 1.4,
        kg1: 1_060.0,
        kp: 600.0,
        kvb: 300.0,
    },
    // 12AT7: medium gain.
    Triode {
        mu: 60.0,
        ex: 1.35,
        kg1: 460.0,
        kp: 300.0,
        kvb: 300.0,
    },
    // 12AU7: low gain, the cleanest and widest swing.
    Triode {
        mu: 21.5,
        ex: 1.3,
        kg1: 1_180.0,
        kp: 84.0,
        kvb: 300.0,
    },
];

const VALVE_CHOICES: [Cow<'static, str>; 3] = [
    Cow::Borrowed("12AX7"),
    Cow::Borrowed("12AT7"),
    Cow::Borrowed("12AU7"),
];

impl Triode {
    /// Plate current, in amperes, at grid voltage `vg` and plate voltage `vp` over the cathode.
    fn current(self, vg: f32, vp: f32) -> f32 {
        let vp = vp.max(0.0);
        let x = self.kp * (1.0 / self.mu + vg / (self.kvb + vp * vp).sqrt());
        let softplus = if x > 30.0 { x } else { x.exp().ln_1p() };
        let e1 = vp / self.kp * softplus;
        if e1 <= 0.0 {
            0.0
        } else {
            2.0 * e1.powf(self.ex) / self.kg1
        }
    }

    /// The plate voltage where the load line meets the valve's curve at grid voltage `vg`.
    fn plate(self, vg: f32) -> f32 {
        let (mut low, mut high) = (0.0_f32, SUPPLY);
        for _ in 0..48 {
            let vp = 0.5 * (low + high);
            if vp + PLATE_LOAD * self.current(vg, vp) > SUPPLY {
                high = vp;
            } else {
                low = vp;
            }
        }
        0.5 * (low + high)
    }
}

/// A stage's plate voltage against its grid voltage, read with linear interpolation.
#[derive(Clone, Debug, Default)]
struct Curve {
    plate: Vec<f32>,
}

impl Curve {
    fn of(triode: Triode) -> Self {
        Self {
            plate: (0..TABLE)
                .map(|index| triode.plate(grid_at(index)))
                .collect(),
        }
    }

    fn at(&self, vg: f32) -> f32 {
        let position = (vg - GRID_RANGE.0) / (GRID_RANGE.1 - GRID_RANGE.0) * (TABLE - 1) as f32;
        let position = position.clamp(0.0, (TABLE - 1) as f32 - 1.0e-3);
        let index = position as usize;
        let fraction = position - index as f32;
        self.plate[index] + (self.plate[index + 1] - self.plate[index]) * fraction
    }

    /// The stage's gain, plate volts per grid volt, at `vg`.
    fn slope(&self, vg: f32) -> f32 {
        (self.at(vg + 0.01) - self.at(vg - 0.01)) / 0.02
    }
}

fn grid_at(index: usize) -> f32 {
    GRID_RANGE.0 + (GRID_RANGE.1 - GRID_RANGE.0) * index as f32 / (TABLE - 1) as f32
}

/// The anti-aliasing filter both ways: a Blackman-windowed sinc at the base rate's band.
fn kernel() -> [f32; PHASE_TAPS * FACTOR] {
    let mut taps = [0.0_f32; PHASE_TAPS * FACTOR];
    let centre = (TAPS - 1) as f64 / 2.0;
    let cutoff = 0.45 / FACTOR as f64;
    for (index, tap) in taps.iter_mut().enumerate().take(TAPS) {
        let x = index as f64 - centre;
        let sinc = if x == 0.0 {
            2.0 * cutoff
        } else {
            (std::f64::consts::TAU * cutoff * x).sin() / (std::f64::consts::PI * x)
        };
        let phase = std::f64::consts::TAU * index as f64 / (TAPS - 1) as f64;
        *tap = (sinc * (0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos())) as f32;
    }
    let sum: f32 = taps.iter().sum();
    for tap in &mut taps {
        *tap /= sum;
    }
    taps
}

/// One channel's stage: the filters either side and the circuit's three capacitors.
#[derive(Clone, Copy, Debug)]
struct Stage {
    /// Base-rate input history for the interpolator, newest first.
    up: [f32; PHASE_TAPS],
    /// High-rate output history for the decimator, newest first.
    down: [f32; PHASE_TAPS * FACTOR],
    /// Charge on the coupling capacitor, on the cathode's bypass, and the transformer's flux.
    coupling: f32,
    cathode: f32,
    flux: f32,
    /// The output coupling's memory.
    blocked: f32,
    last: f32,
    /// The grid and plate a moment ago, for the picture.
    grid: f32,
    plate: f32,
    conducting: bool,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            up: [0.0; PHASE_TAPS],
            down: [0.0; PHASE_TAPS * FACTOR],
            coupling: 0.0,
            cathode: 0.0,
            flux: 0.0,
            blocked: 0.0,
            last: 0.0,
            grid: 0.0,
            plate: 0.0,
            conducting: false,
        }
    }
}

/// A stage at rest: cathode volts, plate volts, and its gain in plate volts per grid volt.
type QuietPoint = (f32, f32, f32);

/// The settings a stage is run at, worked out once a block.
#[derive(Clone, Copy, Debug)]
struct Operating {
    volts: f32,
    cathode: f32,
    quiet_plate: f32,
    /// Output per plate volt, so a small signal comes out at the level it went in.
    scale: f32,
    iron: f32,
    dt: f32,
    flux_pole: f32,
    block_pole: f32,
}

/// A valve gain stage.
///
/// One triode, wired the way nearly every valve preamplifier wires it: the signal through a
/// coupling capacitor to the grid, the plate fed from a high-voltage supply through a load
/// resistor, the cathode lifted on a bypassed resistor to bias it. The valve is Koren's model
/// of a real one, and where its curve meets the load line is worked out for every grid
/// voltage once, when the stage is prepared.
///
/// The circuit is what makes it more than a curve. Driven hard, the grid swings above the
/// cathode and conducts; the current it draws charges the coupling capacitor and drags the
/// bias down for a moment after, so a loud chord sags and recovers. The cathode capacitor
/// charges with the average current and moves the bias the other way. The plate swings
/// further towards the supply than towards the floor, so the clipping is lopsided and adds
/// even harmonics. After the stage, an output transformer's iron saturates on the bass first.
/// The whole stage runs at four times the rate, so its harmonics fold back as little as they
/// can.
///
/// The window shows the stage's curve, grid volts against plate volts, with the operating
/// point riding along it.
pub struct TubeStage {
    params: ParamBank,
    rate: f32,
    curves: Vec<Curve>,
    kernel: [f32; PHASE_TAPS * FACTOR],
    stages: [Stage; 2],
    /// The valve and bias the quiet point was found for, and the point.
    quiet: Option<((usize, f32), QuietPoint)>,
    phase: usize,
    ticks: u32,
    input: Level,
    output: Level,
    trace: [[f32; 2]; RESONANCE_CURVE],
    trace_at: usize,
    trace_count: u32,
    capture: ResonanceCapture,
}

impl Default for TubeStage {
    fn default() -> Self {
        Self::new()
    }
}

impl TubeStage {
    /// Stable plugin identifier stored in projects.
    pub const ID: &str = "auris.fx.tube";

    /// A 12AX7 stage at a light overdrive. Lays nothing out until `prepare`.
    pub fn new() -> Self {
        let params = ParamBank::new(vec![
            ParamDescriptor::new(VALVE, "valve", "Valve", 0.0, 2.0, 0.0)
                .with_choices(&VALVE_CHOICES),
            ParamDescriptor::decibels(DRIVE, "drive_db", "Drive", 0.0, 36.0, 12.0),
            ParamDescriptor::new(BIAS, "bias", "Bias", -1.0, 1.0, 0.0),
            ParamDescriptor::percent(IRON, "iron", "Iron", 0.3),
            ParamDescriptor::decibels(OUTPUT, "output_db", "Output", -24.0, 12.0, 0.0),
        ]);
        Self {
            params,
            rate: 48_000.0,
            curves: Vec::new(),
            kernel: kernel(),
            stages: [Stage::default(); 2],
            quiet: None,
            phase: 0,
            ticks: 0,
            input: Level::default(),
            output: Level::default(),
            trace: [[0.0; 2]; RESONANCE_CURVE],
            trace_at: 0,
            trace_count: 0,
            capture: ResonanceCapture::default(),
        }
    }

    fn valve(&self) -> usize {
        (self.params.at(VALVE).round().max(0.0) as usize).min(TRIODES.len() - 1)
    }

    /// The cathode resistor: the bias control halves it or doubles it.
    fn cathode_resistor(&self) -> f32 {
        CATHODE * 2f32.powf(self.params.at(BIAS))
    }

    /// The stage at rest: cathode volts, plate volts and the gain there. Allocates nothing.
    fn quiet_point(&mut self) -> QuietPoint {
        let key = (self.valve(), self.params.at(BIAS));
        if let Some((laid, point)) = self.quiet
            && laid == key
        {
            return point;
        }
        let curve = &self.curves[key.0];
        let resistor = self.cathode_resistor();
        // The cathode sits where its own current through its resistor holds it.
        let mut cathode = 1.0_f32;
        for _ in 0..200 {
            let current = (SUPPLY - curve.at(-cathode)) / PLATE_LOAD;
            cathode += 0.3 * (current * resistor - cathode);
        }
        let point = (cathode, curve.at(-cathode), curve.slope(-cathode));
        self.quiet = Some((key, point));
        point
    }

    fn publish(&mut self) {
        let valve = self.valve();
        let (cathode, plate, _) = self.quiet_point();
        let mut frame = ResonanceFrame {
            kind: ResonanceKind::Tube,
            input: self.input.get(),
            output: self.output.get(),
            ..ResonanceFrame::default()
        };
        let curve = &self.curves[valve];
        for (index, point) in frame.curve.iter_mut().enumerate() {
            let vg = -8.0 + 10.0 * index as f32 / (RESONANCE_CURVE - 1) as f32;
            *point = [vg, curve.at(vg)];
        }
        frame.curve_len = RESONANCE_CURVE;
        for index in 0..RESONANCE_CURVE {
            frame.trace[index] = self.trace[(self.trace_at + index) % RESONANCE_CURVE];
        }
        frame.trace_len = RESONANCE_CURVE;
        let stage = &self.stages[0];
        frame.values = [
            -cathode,
            plate,
            stage.cathode,
            stage.coupling,
            SUPPLY,
            1_000.0 * (SUPPLY - stage.plate) / PLATE_LOAD,
            f32::from(u8::from(stage.conducting)),
            self.params.at(DRIVE),
        ];
        self.capture.publish(&frame);
    }

    fn operating(&mut self) -> Operating {
        let (cathode, quiet_plate, gain) = self.quiet_point();
        let volts = VOLTS_AT_FULL_SCALE * db_to_gain(self.params.at(DRIVE));
        let high = self.rate * FACTOR as f32;
        Operating {
            volts,
            cathode,
            quiet_plate,
            scale: 1.0 / (gain.abs().max(1.0) * volts),
            iron: self.params.at(IRON),
            dt: 1.0 / high,
            flux_pole: (-std::f32::consts::TAU * 40.0 / high).exp(),
            block_pole: (-std::f32::consts::TAU * 8.0 / high).exp(),
        }
    }
}

impl Stage {
    /// One sample at the high rate through the circuit.
    fn circuit(&mut self, x: f32, curve: &Curve, at: Operating, resistor: f32) -> f32 {
        let input = x * at.volts;
        // The coupling capacitor's far side, and the grid over the cathode.
        let open = input - self.coupling - self.cathode;
        let (grid, grid_current) = if open > 0.0 {
            let grid = open * GRID_CONDUCTING / (GRID_STOPPER + GRID_CONDUCTING);
            (grid, (open - grid) / GRID_STOPPER)
        } else {
            (open, 0.0)
        };
        self.conducting = grid_current > 0.0;
        let plate = curve.at(grid);
        let plate_current = (SUPPLY - plate) / PLATE_LOAD;
        // The coupling capacitor charges with what the grid draws and through the leak.
        let leak = (grid + self.cathode) / GRID_LEAK;
        self.coupling += (grid_current + leak) * at.dt / COUPLING_CAP;
        // The cathode's bypass with the valve's current and the grid's.
        self.cathode +=
            (plate_current + grid_current - self.cathode / resistor) * at.dt / CATHODE_CAP;
        if !(self.coupling.is_finite() && self.cathode.is_finite()) {
            self.coupling = 0.0;
            self.cathode = at.cathode;
        }
        self.grid = grid;
        self.plate = plate;
        // The output coupling: only the swing passes, inverted back.
        let swing = -(plate - at.quiet_plate) * at.scale;
        self.blocked = at.block_pole * (self.blocked + swing - self.last);
        self.last = swing;
        let signal = self.blocked;
        // The output transformer saturates on the bass first: its flux is the signal's
        // integral, and it is the flux that runs out.
        self.flux = at.flux_pole * self.flux + (1.0 - at.flux_pole) * signal * 3.0;
        let excess = self.flux - self.flux.tanh();
        signal - at.iron * excess
    }

    fn process(
        &mut self,
        x: f32,
        kernel: &[f32],
        curve: &Curve,
        at: Operating,
        resistor: f32,
    ) -> f32 {
        self.up.copy_within(0..PHASE_TAPS - 1, 1);
        self.up[0] = x;
        for phase in 0..FACTOR {
            let mut sample = 0.0;
            for (tap, value) in self.up.iter().enumerate() {
                sample += kernel[tap * FACTOR + phase] * value;
            }
            let shaped = self.circuit(sample * FACTOR as f32, curve, at, resistor);
            self.down.copy_within(0..PHASE_TAPS * FACTOR - 1, 1);
            self.down[0] = shaped;
        }
        self.down
            .iter()
            .zip(kernel)
            .map(|(value, tap)| value * tap)
            .sum()
    }
}

impl Parameterized for TubeStage {
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

impl Effect for TubeStage {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            Self::ID,
            "Tube Stage",
            "One triode with its coupling and cathode capacitors, and an output transformer's iron",
            PluginCategory::Distortion,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = sane_rate(ctx.sample_rate);
        if self.curves.is_empty() {
            self.curves = TRIODES.iter().map(|triode| Curve::of(*triode)).collect();
        }
        self.quiet = None;
        self.reset();
    }

    fn reset(&mut self) {
        let (cathode, _, _) = if self.curves.is_empty() {
            (0.0, 0.0, 0.0)
        } else {
            self.quiet_point()
        };
        for stage in &mut self.stages {
            *stage = Stage {
                cathode,
                ..Stage::default()
            };
        }
        self.phase = 0;
        self.ticks = 0;
        self.input.reset();
        self.output.reset();
        self.trace = [[0.0; 2]; RESONANCE_CURVE];
    }

    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext) {
        if self.curves.is_empty() {
            return;
        }
        let at = self.operating();
        let resistor = self.cathode_resistor();
        let gain = db_to_gain(self.params.at(OUTPUT));
        let frames = buffer.frame_count().min(ctx.block_frames);
        let channels = buffer.channel_count().min(2);
        let valve = self.valve();
        let (mut heard, mut out) = (0.0_f32, 0.0_f32);
        for frame in 0..frames {
            let curve = &self.curves[valve];
            for (channel, stage) in self.stages.iter_mut().enumerate().take(channels) {
                let sample = finite(buffer.channel(channel)[frame]);
                heard = heard.max(sample.abs());
                let shaped = stage.process(sample, &self.kernel, curve, at, resistor) * gain;
                out = out.max(shaped.abs());
                buffer.channel_mut(channel)[frame] = shaped;
            }
            self.trace_count = self.trace_count.wrapping_add(1);
            if self.trace_count.is_multiple_of(6) {
                self.trace[self.trace_at] = [self.stages[0].grid, self.stages[0].plate];
                self.trace_at = (self.trace_at + 1) % RESONANCE_CURVE;
            }
            self.phase += 1;
            if self.phase == TICK {
                self.phase = 0;
                self.input.feed(heard);
                self.output.feed(out);
                (heard, out) = (0.0, 0.0);
                self.ticks = self.ticks.wrapping_add(1);
                if self.capture.is_watched() && self.ticks.is_multiple_of(PUBLISH_TICKS) {
                    self.publish();
                }
            }
        }
    }

    fn latency_frames(&self) -> usize {
        LATENCY
    }

    fn tail_frames(&self) -> usize {
        LATENCY + (self.rate * 0.05) as usize
    }

    fn resonance_monitor(&self) -> Option<Arc<ResonanceMonitor>> {
        Some(self.capture.monitor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::goertzel;

    const RATE: f64 = 48_000.0;

    fn prepared() -> TubeStage {
        let mut effect = TubeStage::new();
        effect.prepare(&PrepareContext::new(RATE, 512, 2));
        effect
    }

    fn run(effect: &mut TubeStage, input: &[f32]) -> Vec<f32> {
        let mut left = Vec::new();
        for (block, chunk) in input.chunks(512).enumerate() {
            let mut buffer = AudioBuffer::stereo(chunk.len(), RATE);
            buffer.channel_mut(0).copy_from_slice(chunk);
            buffer.channel_mut(1).copy_from_slice(chunk);
            effect.process(
                &mut buffer,
                &ProcessContext::realtime(RATE, chunk.len(), (block * 512) as u64, 120.0, true),
            );
            left.extend_from_slice(buffer.channel(0));
        }
        left
    }

    fn sine(hz: f64, level: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|index| level * (std::f64::consts::TAU * hz * index as f64 / RATE).sin() as f32)
            .collect()
    }

    #[test]
    fn the_twelve_ax_seven_rests_where_its_data_sheet_puts_it() {
        let mut effect = prepared();
        let (cathode, plate, gain) = effect.quiet_point();
        // A 12AX7 on 300 V, 100k and 1k5 idles near a volt and a half and two hundred volts,
        // with a voltage gain in the sixties.
        assert!((0.8..2.2).contains(&cathode), "{cathode} V");
        assert!((150.0..240.0).contains(&plate), "{plate} V");
        assert!((-80.0..-45.0).contains(&gain), "{gain}");
    }

    #[test]
    fn a_quiet_signal_passes_at_its_own_level_and_a_loud_one_grows_harmonics() {
        let mut effect = prepared();
        effect.set_param(ParamId(DRIVE), 0.0);
        effect.set_param(ParamId(IRON), 0.0);
        effect.reset();
        let quiet = run(&mut effect, &sine(440.0, 0.05, 24_000));
        let level = goertzel(&quiet[12_000..], RATE, 440.0);
        assert!((0.035..0.065).contains(&level), "{level}");

        let mut driven = prepared();
        driven.set_param(ParamId(DRIVE), 30.0);
        driven.reset();
        let loud = run(&mut driven, &sine(440.0, 0.5, 24_000));
        let tail = &loud[12_000..];
        let fundamental = goertzel(tail, RATE, 440.0);
        let second = goertzel(tail, RATE, 880.0);
        let third = goertzel(tail, RATE, 1_320.0);
        assert!(
            second > 0.05 * fundamental,
            "second {second} of {fundamental}"
        );
        assert!(third > 0.02 * fundamental, "third {third} of {fundamental}");
        assert!(
            loud.iter()
                .all(|sample| sample.is_finite() && sample.abs() < 4.0)
        );
    }

    #[test]
    fn a_hard_hit_drags_the_bias_down_through_the_grid() {
        let mut effect = prepared();
        effect.set_param(ParamId(DRIVE), 30.0);
        effect.reset();
        let _ = run(&mut effect, &sine(110.0, 0.8, 4_800));
        // The coupling capacitor has charged from the grid's current.
        assert!(
            effect.stages[0].coupling > 0.5,
            "{}",
            effect.stages[0].coupling
        );
    }

    #[test]
    fn oversampling_keeps_folded_harmonics_down() {
        let mut effect = prepared();
        effect.set_param(ParamId(DRIVE), 36.0);
        effect.reset();
        // A loud 5 kHz tone's fifth harmonic lands at 25 kHz, which would fold to 23 kHz,
        // and its seventh at 35 kHz, which would fold to 13 kHz.
        let out = run(&mut effect, &sine(5_000.0, 0.7, 24_000));
        let tail = &out[12_000..];
        let fundamental = goertzel(tail, RATE, 5_000.0);
        let folded = goertzel(tail, RATE, 13_000.0);
        assert!(folded < 0.01 * fundamental, "{folded} of {fundamental}");
    }

    #[test]
    fn a_watched_stage_draws_its_curve_and_where_it_is_on_it() {
        let mut effect = prepared();
        let monitor = effect.resonance_monitor().unwrap();
        monitor.watch(true);
        let _ = run(&mut effect, &sine(220.0, 0.5, 9_600));
        let frame = monitor.read().unwrap();
        assert_eq!(frame.kind, ResonanceKind::Tube);
        // Plate volts fall as the grid rises.
        assert!(frame.curve[0][1] > frame.curve[RESONANCE_CURVE - 1][1]);
        let (low, high) = frame
            .trace
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, h), [g, _]| {
                (l.min(*g), h.max(*g))
            });
        assert!(high - low > 1.0, "the grid swung {low}..{high} V");
        assert_eq!(effect.latency_frames(), LATENCY);
    }
}
