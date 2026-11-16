//! Coiled springs in a tank, shaken at one end and heard at the other.

use std::sync::Arc;

use auris_core::param::{ParamUnit, ParamValueCurve, db_to_gain};
use auris_core::resonance::{
    RESONANCE_CURVE, RESONANCE_POINTS, ResonanceCapture, ResonanceFrame, ResonanceKind,
    ResonanceMonitor,
};
use auris_core::{
    AudioBuffer, Effect, ParamDescriptor, ParamId, Parameterized, PluginCategory, PluginDescriptor,
    PrepareContext, ProcessContext,
};
use auris_dsp::{Biquad, BiquadCoefficients, DelayLine};

use super::{Level, PUBLISH_TICKS, TICK, finite, sane_rate};
use crate::ParamBank;

/// Springs in the tank.
pub(crate) const SPRINGS: usize = 3;
/// Each spring's one-way transit time at length one, in milliseconds. Three springs of
/// slightly different lengths, so their echoes never fall together.
const TRANSIT_MS: [f32; SPRINGS] = [33.0, 39.0, 45.5];
/// Longest length, which sets what is allocated.
const MAX_LENGTH: f32 = 1.6;
/// Dispersive sections a wave passes on one crossing.
const SECTIONS: usize = 80;
/// Where a helical spring stops carrying the waves that chirp, in hertz. Below it the coil
/// bends, and a higher frequency takes longer to get along it; above it the spring barely
/// carries anything.
const TRANSITION_HZ: f32 = 4_400.0;
/// Longest stretch a section is built for: the transition at the highest rate.
const MAX_STRETCH: usize = 48;

const DECAY: u32 = 0;
const LENGTH: u32 = 1;
const DISPERSION: u32 = 2;
const DRIVE: u32 = 3;
const TONE: u32 = 4;
const MIX: u32 = 5;
const WIDTH: u32 = 6;

/// [`SECTIONS`] first-order allpasses stretched over `stretch` samples, each
/// `(a + z⁻ᴷ) / (1 + a·z⁻ᴷ)`. A section's delay rises from almost nothing to its most `stretch`
/// times between rest and the top, which laid end to end eighty times over is a spring's chirp.
///
/// Every section is stretched alike and they advance together, so a sample only ever meets
/// the state that one phase of the stretch left behind: the state is kept a row of all the
/// sections per phase, and a sample walks along one row. The three springs share the
/// stretch too, so they walk their rows side by side, which lets a processor work on all three
/// at once instead of waiting on each section in turn.
#[derive(Clone, Debug, Default)]
struct Cascade {
    x: Vec<[f32; SPRINGS]>,
    y: Vec<[f32; SPRINGS]>,
    at: usize,
}

impl Cascade {
    fn new() -> Self {
        Self {
            x: vec![[0.0; SPRINGS]; SECTIONS * MAX_STRETCH],
            y: vec![[0.0; SPRINGS]; SECTIONS * MAX_STRETCH],
            at: 0,
        }
    }

    #[inline]
    fn process(&mut self, input: [f32; SPRINGS], a: f32, stretch: usize) -> [f32; SPRINGS] {
        let row = self.at * SECTIONS;
        let xs = &mut self.x[row..row + SECTIONS];
        let ys = &mut self.y[row..row + SECTIONS];
        let mut value = input;
        for (x, y) in xs.iter_mut().zip(ys.iter_mut()) {
            for lane in 0..SPRINGS {
                let output = a * (value[lane] - y[lane]) + x[lane];
                x[lane] = value[lane];
                y[lane] = output;
                value[lane] = output;
            }
        }
        if value.iter().all(|v| v.abs() < 1.0e-25) {
            // Silence: put the whole row to rest before it can reach the denormal range.
            xs.fill([0.0; SPRINGS]);
            ys.fill([0.0; SPRINGS]);
            value = [0.0; SPRINGS];
        }
        self.at += 1;
        if self.at >= stretch {
            self.at = 0;
        }
        value
    }

    fn reset(&mut self) {
        self.x.fill([0.0; SPRINGS]);
        self.y.fill([0.0; SPRINGS]);
        self.at = 0;
    }
}

/// Group delay at `hz` of one crossing's sections, in seconds, without the transit itself.
pub(crate) fn chirp_delay(hz: f32, a: f32, stretch: usize, rate: f32) -> f32 {
    let k = stretch as f32;
    let omega = std::f32::consts::TAU * hz / rate;
    SECTIONS as f32 * k * (1.0 - a * a) / (1.0 + 2.0 * a * (k * omega).cos() + a * a) / rate
}

/// One spring: the wave going out along it and the wave coming back.
#[derive(Clone, Debug)]
struct Spring {
    out: DelayLine,
    back: DelayLine,
    /// The loss of a round trip, one-pole: gain and pole.
    gain: f32,
    pole: f32,
    state: f32,
    /// Samples of one crossing, at the current length.
    transit: f32,
    /// What the far end heard last, which is also what sets off back along the coil.
    arrived: f32,
    level: Level,
}

/// A spring reverb.
///
/// The tank is three coiled springs side by side, each shaken at one end by a transducer and
/// heard at the other by a pickup. A helical spring is dispersive the other way round from a
/// plate: below its transition the coil itself bends, and higher frequencies crawl along it
/// more slowly than low ones. Every echo therefore arrives as a rising chirp, and each trip
/// back and forth along the coil draws it out further — the drip of a surf guitar's reverb.
/// The transducer is a small, easily overdriven coil, and *Drive* is how hard it is pushed.
///
/// The window shows the springs as they are now: the wave on its way out along each coil and
/// the wave on its way back, added where they meet, and the delay of one crossing against
/// frequency.
pub struct SpringReverb {
    params: ParamBank,
    rate: f32,
    springs: Vec<Spring>,
    /// Every spring's sections on the way out and on the way back.
    outward: Cascade,
    returning: Cascade,
    /// The stretch each section was built for at this rate.
    stretch: usize,
    /// The transducer's limited band at its input and the tank's dull top at its output.
    low_cut: Biquad,
    tone: [Biquad; 2],
    laid: Option<(f32, f32)>,
    mix: f32,
    phase: usize,
    ticks: u32,
    input: Level,
    output: Level,
    capture: ResonanceCapture,
}

impl Default for SpringReverb {
    fn default() -> Self {
        Self::new()
    }
}

impl SpringReverb {
    /// Stable plugin identifier stored in projects.
    pub const ID: &str = "auris.fx.spring";

    /// A three-spring tank. Allocates nothing until `prepare`.
    pub fn new() -> Self {
        let params = ParamBank::new(vec![
            ParamDescriptor::new(DECAY, "decay", "Decay", 0.5, 6.0, 2.2)
                .with_unit(ParamUnit::Seconds)
                .with_curve(ParamValueCurve::Logarithmic),
            ParamDescriptor::new(LENGTH, "length", "Length", 0.6, MAX_LENGTH, 1.0)
                .with_unit(ParamUnit::Ratio)
                .with_curve(ParamValueCurve::Logarithmic),
            ParamDescriptor::percent(DISPERSION, "dispersion", "Dispersion", 0.6),
            ParamDescriptor::decibels(DRIVE, "drive_db", "Drive", 0.0, 24.0, 3.0),
            ParamDescriptor::hertz(TONE, "tone", "Tone", 1_500.0, 8_000.0, 4_000.0),
            ParamDescriptor::percent(MIX, "mix", "Mix", 0.3),
            ParamDescriptor::percent(WIDTH, "width", "Width", 0.8),
        ]);
        Self {
            params,
            rate: 48_000.0,
            springs: Vec::new(),
            outward: Cascade::default(),
            returning: Cascade::default(),
            stretch: 5,
            low_cut: Biquad::default(),
            tone: Default::default(),
            laid: None,
            mix: 0.3,
            phase: 0,
            ticks: 0,
            input: Level::default(),
            output: Level::default(),
            capture: ResonanceCapture::default(),
        }
    }

    /// The sections' coefficient, by how dispersive the springs are set.
    fn coefficient(&self) -> f32 {
        0.3 + 0.45 * self.params.at(DISPERSION)
    }

    fn arrange(&mut self) {
        let wanted = (self.params.at(DECAY), self.params.at(LENGTH));
        if self.laid != Some(wanted) {
            let (decay, length) = wanted;
            let rate = self.rate;
            for (spring, ms) in self.springs.iter_mut().zip(TRANSIT_MS) {
                spring.transit =
                    (ms * 0.001 * length * rate).clamp(2.0, spring.out.max_delay_samples() as f32);
                let round_trip = 2.0 * spring.transit;
                let g0 = 10f32.powf(-3.0 * round_trip / (decay * rate));
                let gpi = 10f32.powf(-3.0 * round_trip / (0.35 * decay * rate));
                spring.gain = g0;
                spring.pole = (g0 - gpi) / (g0 + gpi);
            }
            self.laid = Some(wanted);
        }
        let tone = BiquadCoefficients::lowpass(
            f64::from(self.rate),
            self.params.at(TONE).min(self.rate * 0.45),
            0.6,
        );
        for filter in &mut self.tone {
            filter.set_coefficients(tone);
        }
    }

    fn publish(&mut self) {
        let mut frame = ResonanceFrame {
            kind: ResonanceKind::Spring,
            input: self.input.get(),
            output: self.output.get(),
            string_count: SPRINGS,
            ..ResonanceFrame::default()
        };
        for (string, spring) in frame.strings.iter_mut().zip(&self.springs) {
            string.pitch = spring.transit / self.rate;
            string.level = spring.level.get();
            for (index, point) in string.points.iter_mut().enumerate() {
                let along = index as f32 / (RESONANCE_POINTS - 1) as f32;
                // The outgoing wave has gone `along` of the way; the returning one has come
                // back the rest.
                *point = spring.out.read((along * spring.transit).max(1.0))
                    + spring.back.read(((1.0 - along) * spring.transit).max(1.0));
            }
        }
        let a = self.coefficient();
        let top = TRANSITION_HZ.min(self.rate * 0.45);
        for (index, point) in frame.curve.iter_mut().enumerate() {
            let hz = 40.0 + (top - 40.0) * index as f32 / (RESONANCE_CURVE - 1) as f32;
            let transit = self.springs.first().map_or(0.0, |s| s.transit / self.rate);
            *point = [
                hz,
                1_000.0 * (transit + chirp_delay(hz, a, self.stretch, self.rate)),
            ];
        }
        frame.curve_len = RESONANCE_CURVE;
        frame.values[0] = TRANSITION_HZ;
        frame.values[1] = self.params.at(DRIVE);
        self.capture.publish(&frame);
    }
}

impl Parameterized for SpringReverb {
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

/// How much of each spring each side hears: the outer two lean apart, the middle sits centre.
const PANS: [(f32, f32); SPRINGS] = [(1.0, 0.25), (0.65, 0.65), (0.25, 1.0)];

impl Effect for SpringReverb {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            Self::ID,
            "Spring Reverb",
            "Three coiled springs in a tank: every echo a rising chirp, the drip of an amp's reverb",
            PluginCategory::Reverb,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = sane_rate(ctx.sample_rate);
        self.stretch = ((self.rate / (2.0 * TRANSITION_HZ)).round() as usize).clamp(1, MAX_STRETCH);
        self.springs = TRANSIT_MS
            .iter()
            .map(|ms| {
                let samples = (ms * 0.001 * MAX_LENGTH * self.rate).ceil() as usize + 2;
                Spring {
                    out: DelayLine::new(samples),
                    back: DelayLine::new(samples),
                    gain: 0.0,
                    pole: 0.0,
                    state: 0.0,
                    transit: 2.0,
                    arrived: 0.0,
                    level: Level::default(),
                }
            })
            .collect();
        self.outward = Cascade::new();
        self.returning = Cascade::new();
        self.low_cut.set_coefficients(BiquadCoefficients::highpass(
            f64::from(self.rate),
            140.0,
            0.7,
        ));
        self.laid = None;
        self.reset();
    }

    fn reset(&mut self) {
        for spring in &mut self.springs {
            spring.out.reset();
            spring.back.reset();
            spring.state = 0.0;
            spring.arrived = 0.0;
            spring.level.reset();
        }
        self.outward.reset();
        self.returning.reset();
        self.low_cut.reset();
        for filter in &mut self.tone {
            filter.reset();
        }
        self.mix = self.params.at(MIX);
        self.phase = 0;
        self.ticks = 0;
        self.input.reset();
        self.output.reset();
    }

    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext) {
        if self.springs.is_empty() {
            return;
        }
        self.arrange();
        let frames = buffer.frame_count().min(ctx.block_frames);
        let stereo = buffer.channel_count() >= 2;
        let drive = db_to_gain(self.params.at(DRIVE));
        let width = self.params.at(WIDTH);
        let mix_target = self.params.at(MIX);
        let a = self.coefficient();
        let stretch = self.stretch;
        let (mut heard, mut out) = (0.0_f32, 0.0_f32);
        let mut moving = [0.0_f32; SPRINGS];
        let channels = buffer.channels_mut();
        for frame in 0..frames {
            let left = finite(channels[0][frame]);
            let right = if stereo {
                finite(channels[1][frame])
            } else {
                left
            };
            heard = heard.max(left.abs()).max(right.abs());
            // The transducer: a small coil that flattens what it is pushed too hard with.
            let shaken =
                (self.low_cut.process_sample(0.5 * (left + right)) * drive).tanh() / drive.sqrt();
            let mut leaving = [0.0_f32; SPRINGS];
            for (spring, leaving) in self.springs.iter_mut().zip(&mut leaving) {
                // What came back to the transducer end reflects and sets off again with
                // the new push.
                let returned = spring.back.read(spring.transit);
                spring.state =
                    spring.gain * (1.0 - spring.pole) * returned + spring.pole * spring.state;
                spring.out.write(shaken + spring.state);
                *leaving = spring.out.read(spring.transit);
            }
            let arrived = self.outward.process(leaving, a, stretch);
            let reflected = self.returning.process(arrived, a, stretch);
            let mut wet = [0.0_f32; 2];
            for (index, (spring, pan)) in self.springs.iter_mut().zip(PANS).enumerate() {
                spring.arrived = arrived[index];
                spring.back.write(reflected[index]);
                moving[index] = moving[index].max(arrived[index].abs());
                wet[0] += arrived[index] * pan.0;
                wet[1] += arrived[index] * pan.1;
            }
            let wet = [
                self.tone[0].process_sample(wet[0] * 0.8),
                self.tone[1].process_sample(wet[1] * 0.8),
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
                for (spring, moved) in self.springs.iter_mut().zip(&mut moving) {
                    spring.level.feed(*moved);
                    *moved = 0.0;
                }
                (heard, out) = (0.0, 0.0);
                self.ticks = self.ticks.wrapping_add(1);
                if self.capture.is_watched() && self.ticks.is_multiple_of(PUBLISH_TICKS) {
                    self.publish();
                }
            }
        }
        self.mix = mix_target;
    }

    fn tail_frames(&self) -> usize {
        (self.params.at(DECAY) * 1.2 * self.rate) as usize
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

    fn prepared() -> SpringReverb {
        let mut effect = SpringReverb::new();
        effect.prepare(&PrepareContext::new(RATE, 512, 2));
        effect.set_param(ParamId(MIX), 1.0);
        effect.reset();
        effect
    }

    fn run(effect: &mut SpringReverb, input: &[f32]) -> Vec<f32> {
        let mut left = Vec::new();
        for (block, chunk) in input.chunks(500).enumerate() {
            let mut buffer = AudioBuffer::stereo(chunk.len(), RATE);
            buffer.channel_mut(0).copy_from_slice(chunk);
            buffer.channel_mut(1).copy_from_slice(chunk);
            effect.process(
                &mut buffer,
                &ProcessContext::realtime(RATE, chunk.len(), (block * 500) as u64, 120.0, true),
            );
            left.extend_from_slice(buffer.channel(0));
        }
        left
    }

    #[test]
    fn a_higher_frequency_takes_longer_along_the_coil() {
        let rate = RATE as f32;
        let stretch = (rate / (2.0 * TRANSITION_HZ)).round() as usize;
        let low = chirp_delay(300.0, 0.57, stretch, rate);
        let high = chirp_delay(4_000.0, 0.57, stretch, rate);
        assert!(high > low * 3.0, "300 Hz {low} s, 4 kHz {high} s");
        assert!(high - low > 0.01);
    }

    #[test]
    fn the_first_echo_arrives_after_one_crossing() {
        let mut effect = prepared();
        effect.set_param(ParamId(DRIVE), 0.0);
        let mut click = vec![0.0; 9_600];
        click[0] = 0.5;
        let out = run(&mut effect, &click);
        let first = out
            .iter()
            .position(|sample| sample.abs() > 1.0e-3)
            .expect("the springs answered");
        // The shortest spring is 33 ms long; nothing arrives before it.
        assert!(first as f32 >= 0.033 * RATE as f32 - 2.0, "{first}");
        assert!((first as f32) < 0.06 * RATE as f32, "{first}");
    }

    #[test]
    fn the_tank_rings_on_and_then_dies_away() {
        let mut effect = prepared();
        effect.set_param(ParamId(DECAY), 1.5);
        let mut click = vec![0.0; 4 * 48_000];
        click[0] = 0.5;
        let out = run(&mut effect, &click);
        let early = rms(&out[2_400..26_400]);
        let late = rms(&out[3 * 48_000..]);
        assert!(early > 1.0e-3, "{early}");
        assert!(
            20.0 * (early / late.max(1.0e-12)).log10() > 50.0,
            "{early} {late}"
        );
        assert!(out.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn a_watched_tank_shows_the_wave_along_each_spring() {
        let mut effect = prepared();
        let monitor = effect.resonance_monitor().unwrap();
        monitor.watch(true);
        let mut click = vec![0.0; 4_800];
        click[0] = 0.8;
        let _ = run(&mut effect, &click);
        let frame = monitor.read().unwrap();
        assert_eq!(frame.kind, ResonanceKind::Spring);
        assert_eq!(frame.string_count, SPRINGS);
        assert!(
            frame.strings[..SPRINGS]
                .iter()
                .any(|spring| spring.points.iter().any(|point| point.abs() > 1.0e-5))
        );
        assert!(frame.curve[RESONANCE_CURVE - 1][1] > frame.curve[0][1]);
    }
}
