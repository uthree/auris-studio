//! A loop of tape past a record head and three playback heads.

use std::borrow::Cow;
use std::f32::consts::TAU;
use std::sync::Arc;

use auris_core::param::{ParamUnit, ParamValueCurve, db_to_gain};
use auris_core::resonance::{
    RESONANCE_CURVE, ResonanceCapture, ResonanceFrame, ResonanceKind, ResonanceMonitor,
};
use auris_core::{
    AudioBuffer, Effect, ParamDescriptor, ParamId, Parameterized, PluginCategory, PluginDescriptor,
    PrepareContext, ProcessContext,
};
use auris_dsp::{Biquad, BiquadCoefficients};

use super::{Level, PUBLISH_TICKS, TICK, finite, hermite, sane_rate};
use crate::ParamBank;
use crate::physical::body::seeded;

/// Where each playback head sits, as a multiple of the first head's distance from the
/// record head: the spacing of a familiar three-head machine.
const HEADS: [f32; 3] = [1.0, 1.95, 2.9];
/// The first playback head's distance from the record head, in metres.
const HEAD_GAP_M: f32 = 0.04;
/// Longest echo the first head is set to, in milliseconds.
const MAX_TIME_MS: f32 = 1_000.0;
/// Deepest wow, as a fraction of the tape's speed, at full *Wow*.
const WOW_DEPTH: f32 = 0.006;
/// Deepest flutter at full *Flutter*.
const FLUTTER_DEPTH: f32 = 0.0025;
/// The capstan's turn, which is what flutters, in hertz.
const FLUTTER_HZ: f32 = 6.8;
/// The pinch roller and reel's slow wander, in hertz.
const WOW_HZ: f32 = 0.55;
/// Sub-steps the magnetisation is followed through each sample.
const SUBSTEPS: usize = 4;

/// Jiles–Atherton coefficients of the tape's oxide, in units where full scale drives it to
/// about two thirds of saturation: saturation, the anhysteretic curve's width, the coupling
/// between domains, the coercivity that holds the loop open, and the share that is reversible.
///
/// A recorder never puts the signal on the tape bare: it rides on an inaudible bias tone
/// that shakes the domains loose, which is why recorded tape follows the anhysteretic curve
/// rather than the virgin one and has no dead zone at low levels. The bias itself is far too
/// high to follow sample by sample, so its effect is taken as given: a loop much narrower
/// than the oxide's own, and a large reversible share.
const SATURATION: f32 = 1.0;
const WIDTH: f32 = 0.42;
const COUPLING: f32 = 0.05;
const COERCIVITY: f32 = 0.03;
const REVERSIBLE: f32 = 0.6;

const TIME: u32 = 0;
const HEAD_CHOICE: u32 = 1;
const FEEDBACK: u32 = 2;
const WOW: u32 = 3;
const FLUTTER: u32 = 4;
const DRIVE: u32 = 5;
const AGE: u32 = 6;
const MIX: u32 = 7;

const HEAD_CHOICES: [Cow<'static, str>; 4] = [
    Cow::Borrowed("Head 1"),
    Cow::Borrowed("Heads 1+2"),
    Cow::Borrowed("Heads 1+3"),
    Cow::Borrowed("Heads 1+2+3"),
];

/// Which heads a choice plays, by bit.
fn heads_of(choice: f32) -> u8 {
    match choice.round() as i32 {
        1 => 0b011,
        2 => 0b101,
        3 => 0b111,
        _ => 0b001,
    }
}

/// The Langevin function, `coth x - 1/x`, the shape a population of free domains lines up in.
fn langevin(x: f32) -> f32 {
    if x.abs() < 1.0e-3 {
        x / 3.0
    } else {
        1.0 / x.tanh() - 1.0 / x
    }
}

/// Its slope.
fn langevin_slope(x: f32) -> f32 {
    if x.abs() < 1.0e-3 {
        1.0 / 3.0
    } else {
        let sinh = x.sinh();
        (1.0 / (x * x) - 1.0 / (sinh * sinh)).max(0.0)
    }
}

/// The oxide's magnetisation, following the field the record head puts on it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Hysteresis {
    /// Magnetisation, between minus and plus saturation.
    pub(crate) m: f32,
    /// The field a sample ago.
    pub(crate) h: f32,
}

impl Hysteresis {
    /// Moves the field to `h` in [`SUBSTEPS`] steps and returns the magnetisation there.
    pub(crate) fn drive(&mut self, h: f32) -> f32 {
        let h = h.clamp(-50.0, 50.0);
        let step = (h - self.h) / SUBSTEPS as f32;
        if step == 0.0 {
            return self.m;
        }
        let direction = step.signum();
        for _ in 0..SUBSTEPS {
            self.h += step;
            let effective = (self.h + COUPLING * self.m) / WIDTH;
            let anhysteretic = SATURATION * langevin(effective);
            let lag = anhysteretic - self.m;
            // A domain wall only moves the way the field is pushing it, and only towards the
            // curve it would settle on.
            let irreversible = if lag * direction > 0.0 {
                let denominator = direction * COERCIVITY - COUPLING * lag;
                let denominator = if denominator * direction < 1.0e-3 {
                    1.0e-3 * direction
                } else {
                    denominator
                };
                (lag / denominator).max(0.0)
            } else {
                0.0
            };
            let reversible = SATURATION / WIDTH * langevin_slope(effective);
            let slope = (1.0 - REVERSIBLE) * irreversible + REVERSIBLE * reversible;
            self.m = (self.m + slope * step).clamp(-SATURATION, SATURATION);
        }
        self.h = h;
        if !self.m.is_finite() {
            self.m = 0.0;
        }
        self.m
    }
}

/// How much magnetisation a small field at `level` leaves, worked out by recording a sine.
fn small_signal_gain(level: f32) -> f32 {
    let mut tape = Hysteresis::default();
    let mut peak = 0.0_f32;
    for index in 0..960 {
        let m = tape.drive(level * (TAU * index as f32 / 96.0).sin());
        if index >= 480 {
            peak = peak.max(m.abs());
        }
    }
    (peak / level).max(1.0e-3)
}

/// A slow random wander, a new target a few times a second with the path between them smoothed.
#[derive(Clone, Copy, Debug, Default)]
struct Wander {
    value: f32,
    target: f32,
    count: u32,
}

impl Wander {
    fn next(&mut self, seed: u32, rate: f32) -> f32 {
        if self.count.is_multiple_of(12_000) {
            self.target = seeded(seed, self.count / 12_000) * 2.0 - 1.0;
        }
        self.count = self.count.wrapping_add(1);
        self.value += (self.target - self.value) * (6.0 / rate);
        self.value
    }
}

/// A tape echo.
///
/// The track is recorded onto a loop of tape, and up to three playback heads further along
/// hear it again as the loop carries it past them; what they hear is mixed back into the
/// record head for the repeats. Everything that makes it a tape machine is a consequence of
/// that. The echo time is the heads' distance over the tape's speed, and the speed is never
/// quite steady: the capstan flutters as it turns and the reels wow, so every repeat is
/// drawn a little sharp and flat. Turning *Time* slows the tape down rather than moving the
/// heads, so the echoes already on it glide down in pitch, and a slower tape has longer
/// wavelengths, so its head bump sits lower. The oxide is magnetised along a hysteresis loop
/// (Jiles–Atherton), which is where the tape's compression and its warm, rounded overdrive
/// come from, repeat after repeat. An older tape lifts off the heads, and loses its top
/// the more the slower it runs.
///
/// The window shows the oxide's loop as the record head is drawing it, and the tape's speed
/// wandering over the last two seconds.
pub struct TapeEcho {
    params: ParamBank,
    rate: f32,
    tape: Vec<f32>,
    mask: usize,
    written: usize,
    /// The speed each head has been read at, averaged over the time its echo spent on tape.
    speed_seen: [f32; 3],
    oxide: Hysteresis,
    makeup: f32,
    /// Each head's bump and the tape's loss of top, per head.
    bump: [Biquad; 3],
    spacing: [[Biquad; 2]; 3],
    /// What the heads played a sample ago, for the record head.
    returned: f32,
    laid: Option<(f32, f32)>,
    wow_phase: f32,
    flutter_phase: f32,
    wander: Wander,
    speed: f32,
    mix: f32,
    phase: usize,
    ticks: u32,
    input: Level,
    output: Level,
    /// The loop as drawn: field and magnetisation, every few samples.
    loop_trace: [[f32; 2]; RESONANCE_CURVE],
    loop_at: usize,
    loop_count: u32,
    speeds: [f32; RESONANCE_CURVE],
    speed_at: usize,
    capture: ResonanceCapture,
}

impl Default for TapeEcho {
    fn default() -> Self {
        Self::new()
    }
}

impl TapeEcho {
    /// Stable plugin identifier stored in projects.
    pub const ID: &str = "auris.fx.tape";

    /// A tape echo of a third of a second. Allocates nothing until `prepare`.
    pub fn new() -> Self {
        let params = ParamBank::new(vec![
            ParamDescriptor::new(TIME, "time", "Time", 60.0, MAX_TIME_MS, 320.0)
                .with_unit(ParamUnit::Milliseconds)
                .with_curve(ParamValueCurve::Logarithmic),
            ParamDescriptor::new(HEAD_CHOICE, "heads", "Heads", 0.0, 3.0, 0.0)
                .with_choices(&HEAD_CHOICES),
            ParamDescriptor::new(FEEDBACK, "feedback", "Feedback", 0.0, 1.1, 0.45)
                .with_unit(ParamUnit::Percent),
            ParamDescriptor::percent(WOW, "wow", "Wow", 0.3),
            ParamDescriptor::percent(FLUTTER, "flutter", "Flutter", 0.3),
            ParamDescriptor::decibels(DRIVE, "drive_db", "Drive", -12.0, 18.0, 0.0),
            ParamDescriptor::percent(AGE, "age", "Age", 0.3),
            ParamDescriptor::percent(MIX, "mix", "Mix", 0.35),
        ]);
        Self {
            params,
            rate: 48_000.0,
            tape: Vec::new(),
            mask: 0,
            written: 0,
            speed_seen: [1.0; 3],
            oxide: Hysteresis::default(),
            makeup: 1.0,
            bump: Default::default(),
            spacing: Default::default(),
            returned: 0.0,
            laid: None,
            wow_phase: 0.0,
            flutter_phase: 0.0,
            wander: Wander::default(),
            speed: 1.0,
            mix: 0.35,
            phase: 0,
            ticks: 0,
            input: Level::default(),
            output: Level::default(),
            loop_trace: [[0.0; 2]; RESONANCE_CURVE],
            loop_at: 0,
            loop_count: 0,
            speeds: [0.0; RESONANCE_CURVE],
            speed_at: 0,
            capture: ResonanceCapture::default(),
        }
    }

    /// The tape's nominal speed, in metres a second, for the time set.
    fn tape_speed(&self) -> f32 {
        HEAD_GAP_M / (self.params.at(TIME) * 0.001).max(1.0e-3)
    }

    /// Sets the heads' bump and the tape's loss for the speed and age. Allocates nothing.
    fn arrange(&mut self) {
        let wanted = (self.params.at(TIME), self.params.at(AGE));
        if self.laid == Some(wanted) {
            return;
        }
        let speed = self.tape_speed();
        let rate = f64::from(self.rate);
        // A head's contact with the tape is a few millimetres long; a wavelength about that
        // long is reinforced by its edges.
        let bump_hz = (speed / 0.0035).clamp(25.0, 180.0);
        let bump = BiquadCoefficients::peaking(rate, bump_hz, 1.1, 3.0);
        // Spacing loss: 54.6 dB per wavelength of gap between the oxide and the head.
        let gap = (0.3 + 4.0 * self.params.at(AGE)) * 1.0e-6;
        let top = (3.0 * speed / (54.6 * gap)).clamp(800.0, self.rate * 0.45);
        let spacing = BiquadCoefficients::lowpass(rate, top, std::f32::consts::FRAC_1_SQRT_2);
        for (head_bump, head_spacing) in self.bump.iter_mut().zip(&mut self.spacing) {
            head_bump.set_coefficients(bump);
            for filter in head_spacing {
                filter.set_coefficients(spacing);
            }
        }
        self.laid = Some(wanted);
    }

    /// Reads the tape `delay` samples back, with a cubic through four neighbours.
    fn read(&self, delay: f32) -> f32 {
        let delay = delay.clamp(2.0, (self.tape.len() - 4) as f32);
        let whole = delay as usize;
        let fraction = delay - whole as f32;
        let at = |back: usize| self.tape[(self.written + self.tape.len() - back) & self.mask];
        hermite(
            [at(whole - 1), at(whole), at(whole + 1), at(whole + 2)],
            fraction,
        )
    }

    fn publish(&mut self, heads: u8) {
        let mut frame = ResonanceFrame {
            kind: ResonanceKind::Tape,
            input: self.input.get(),
            output: self.output.get(),
            ..ResonanceFrame::default()
        };
        // Oldest first.
        for index in 0..RESONANCE_CURVE {
            frame.trace[index] = self.loop_trace[(self.loop_at + index) % RESONANCE_CURVE];
            let speed = self.speeds[(self.speed_at + index) % RESONANCE_CURVE];
            frame.curve[index] = [
                -2.0 + 2.0 * index as f32 / (RESONANCE_CURVE - 1) as f32,
                100.0 * speed,
            ];
        }
        frame.trace_len = RESONANCE_CURVE;
        frame.curve_len = RESONANCE_CURVE;
        frame.values[0] = 100.0 * (self.speed - 1.0);
        frame.values[1] = self.params.at(TIME);
        frame.values[2] = f32::from(heads);
        frame.values[3] = 100.0 * self.tape_speed();
        frame.values[4] = self.params.at(DRIVE);
        frame.values[5] = self.params.at(FEEDBACK);
        frame.values[6] = self.oxide.m;
        frame.values[7] = self.oxide.h;
        self.capture.publish(&frame);
    }
}

impl Parameterized for TapeEcho {
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

impl Effect for TapeEcho {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            Self::ID,
            "Tape Echo",
            "A tape loop past three playback heads: wow, flutter, head bump and the oxide's loop",
            PluginCategory::Delay,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = sane_rate(ctx.sample_rate);
        let longest =
            MAX_TIME_MS * 0.001 * HEADS[2] * (1.0 + WOW_DEPTH + FLUTTER_DEPTH) * self.rate;
        let length = (longest as usize + 16).next_power_of_two();
        self.tape = vec![0.0; length];
        self.mask = length - 1;
        self.makeup = 1.0 / small_signal_gain(0.25);
        self.laid = None;
        self.reset();
    }

    fn reset(&mut self) {
        self.tape.fill(0.0);
        self.written = 0;
        self.speed_seen = [1.0; 3];
        self.oxide = Hysteresis::default();
        for filter in self
            .bump
            .iter_mut()
            .chain(self.spacing.iter_mut().flatten())
        {
            filter.reset();
        }
        self.returned = 0.0;
        self.wow_phase = 0.0;
        self.flutter_phase = 0.0;
        self.wander = Wander::default();
        self.speed = 1.0;
        self.mix = self.params.at(MIX);
        self.phase = 0;
        self.ticks = 0;
        self.input.reset();
        self.output.reset();
        self.loop_trace = [[0.0; 2]; RESONANCE_CURVE];
        self.speeds = [0.0; RESONANCE_CURVE];
    }

    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext) {
        if self.tape.is_empty() {
            return;
        }
        self.arrange();
        let frames = buffer.frame_count().min(ctx.block_frames);
        let stereo = buffer.channel_count() >= 2;
        let rate = self.rate;
        let time = self.params.at(TIME) * 0.001 * rate;
        let heads = heads_of(self.params.at(HEAD_CHOICE));
        let playing = heads.count_ones().max(1) as f32;
        let feedback = self.params.at(FEEDBACK);
        let (wow, flutter) = (self.params.at(WOW), self.params.at(FLUTTER));
        let drive = db_to_gain(self.params.at(DRIVE));
        let mix_target = self.params.at(MIX);
        let (mut heard, mut out) = (0.0_f32, 0.0_f32);
        let channels = buffer.channels_mut();
        for frame in 0..frames {
            let left = finite(channels[0][frame]);
            let right = if stereo {
                finite(channels[1][frame])
            } else {
                left
            };
            heard = heard.max(left.abs()).max(right.abs());
            // The transport: the capstan's flutter over the reels' wow.
            self.wow_phase = (self.wow_phase + WOW_HZ / rate).fract();
            self.flutter_phase = (self.flutter_phase + FLUTTER_HZ / rate).fract();
            let wander = self.wander.next(0x7a9e, rate);
            self.speed = 1.0
                + wow * WOW_DEPTH * (0.7 * (TAU * self.wow_phase).sin() + 0.5 * wander)
                + flutter
                    * FLUTTER_DEPTH
                    * ((TAU * self.flutter_phase).sin()
                        + 0.4 * (2.0 * TAU * self.flutter_phase + 1.3).sin());
            // The record head: the track and the repeats, pushed into the oxide.
            let recorded = 0.5 * (left + right) + feedback * self.returned;
            let m = self.oxide.drive(recorded * drive * 0.5);
            let written = m * self.makeup * 2.0 / drive;
            self.tape[self.written] = written;
            self.written = (self.written + 1) & self.mask;
            self.loop_count = self.loop_count.wrapping_add(1);
            if self.loop_count.is_multiple_of(6) {
                self.loop_trace[self.loop_at] = [self.oxide.h, m];
                self.loop_at = (self.loop_at + 1) % RESONANCE_CURVE;
            }
            // The playback heads. What reaches a head left the record head when the tape
            // was running at the speed of a moment ago, so each head hears the speed averaged
            // over its own echo.
            let mut played = 0.0_f32;
            for (index, distance) in HEADS.iter().enumerate() {
                let delay = time * distance;
                let seen = self.speed_seen[index]
                    + (self.speed - self.speed_seen[index]) * (2.0 / delay.max(1.0));
                self.speed_seen[index] = seen;
                if heads & (1 << index) == 0 {
                    continue;
                }
                let raw = self.read(delay / seen);
                let bumped = self.bump[index].process_sample(raw);
                let [first, second] = &mut self.spacing[index];
                played += second.process_sample(first.process_sample(bumped));
            }
            let played = played / playing.sqrt();
            self.returned = played / playing.sqrt();
            let ramp = (frame + 1) as f32 / frames as f32;
            let mix = self.mix + (mix_target - self.mix) * ramp;
            let (out_left, out_right) = (
                (1.0 - mix) * left + mix * played,
                (1.0 - mix) * right + mix * played,
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
                self.ticks = self.ticks.wrapping_add(1);
                if self.ticks.is_multiple_of(PUBLISH_TICKS) {
                    self.speeds[self.speed_at] = self.speed - 1.0;
                    self.speed_at = (self.speed_at + 1) % RESONANCE_CURVE;
                    if self.capture.is_watched() {
                        self.publish(heads);
                    }
                }
            }
        }
        self.mix = mix_target;
    }

    fn tail_frames(&self) -> usize {
        // Repeats at full feedback go on for ever; at the default they are down sixty decibels
        // in a dozen echoes of the last head.
        let feedback = self.params.at(FEEDBACK).clamp(0.05, 0.95);
        let repeats = (-3.0 / feedback.log10()).ceil();
        (self.params.at(TIME) * 0.001 * HEADS[2] * repeats.min(40.0) * self.rate) as usize
    }

    fn resonance_monitor(&self) -> Option<Arc<ResonanceMonitor>> {
        Some(self.capture.monitor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{goertzel, rms};

    const RATE: f64 = 48_000.0;

    fn prepared() -> TapeEcho {
        let mut effect = TapeEcho::new();
        effect.prepare(&PrepareContext::new(RATE, 512, 2));
        effect.set_param(ParamId(MIX), 1.0);
        effect.reset();
        effect
    }

    fn run(effect: &mut TapeEcho, input: &[f32]) -> Vec<f32> {
        let mut left = Vec::new();
        for (block, chunk) in input.chunks(256).enumerate() {
            let mut buffer = AudioBuffer::stereo(chunk.len(), RATE);
            buffer.channel_mut(0).copy_from_slice(chunk);
            buffer.channel_mut(1).copy_from_slice(chunk);
            effect.process(
                &mut buffer,
                &ProcessContext::realtime(RATE, chunk.len(), (block * 256) as u64, 120.0, true),
            );
            left.extend_from_slice(buffer.channel(0));
        }
        left
    }

    fn sine(hz: f32, level: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|index| level * (TAU * hz * index as f32 / RATE as f32).sin())
            .collect()
    }

    #[test]
    fn the_oxide_compresses_a_loud_signal_and_keeps_a_quiet_one() {
        let quiet = small_signal_gain(0.05);
        let loud = small_signal_gain(3.0);
        assert!(loud < 0.7 * quiet, "quiet {quiet} loud {loud}");
        // Its loop is open: going up and coming down the field meet different magnetisations.
        let mut tape = Hysteresis::default();
        for index in 0..400 {
            tape.drive(0.8 * (TAU * index as f32 / 100.0).sin());
        }
        let rising = {
            let mut copy = tape;
            copy.drive(0.0)
        };
        let mut falling = tape;
        for index in 0..50 {
            falling.drive(0.8 * (TAU * (index as f32 + 25.0) / 100.0).sin());
        }
        let falling = falling.drive(0.0);
        assert!((rising - falling).abs() > 0.05, "{rising} {falling}");
    }

    #[test]
    fn the_first_repeat_comes_at_the_time_set() {
        let mut effect = prepared();
        effect.set_param(ParamId(WOW), 0.0);
        effect.set_param(ParamId(FLUTTER), 0.0);
        effect.set_param(ParamId(TIME), 250.0);
        let mut click = vec![0.0; 20_000];
        for sample in click.iter_mut().take(48) {
            *sample = 0.5;
        }
        let out = run(&mut effect, &click);
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(index, _)| index)
            .unwrap();
        let expected = (0.25 * RATE) as usize;
        assert!(peak.abs_diff(expected) < 200, "{peak} vs {expected}");
    }

    #[test]
    fn repeats_fade_at_the_feedback_set() {
        let mut effect = prepared();
        effect.set_param(ParamId(TIME), 200.0);
        effect.set_param(ParamId(FEEDBACK), 0.5);
        let mut tone = sine(500.0, 0.3, 2_400);
        tone.resize(48_000, 0.0);
        let out = run(&mut effect, &tone);
        let window = |echo: usize| {
            let start = (echo as f32 * 0.2 * RATE as f32) as usize;
            rms(&out[start..start + 2_400])
        };
        let drop = 20.0 * (window(1) / window(3)).log10();
        // Two more trips at half: about twelve decibels, give or take the tape's colour.
        assert!((8.0..18.0).contains(&drop), "{drop}");
    }

    #[test]
    fn flutter_spreads_a_held_tone() {
        let tone = sine(2_000.0, 0.2, 96_000);
        let steady = {
            let mut effect = prepared();
            effect.set_param(ParamId(WOW), 0.0);
            effect.set_param(ParamId(FLUTTER), 0.0);
            effect.set_param(ParamId(FEEDBACK), 0.0);
            run(&mut effect, &tone)
        };
        let wobbly = {
            let mut effect = prepared();
            effect.set_param(ParamId(WOW), 1.0);
            effect.set_param(ParamId(FLUTTER), 1.0);
            effect.set_param(ParamId(FEEDBACK), 0.0);
            run(&mut effect, &tone)
        };
        let at = |signal: &[f32]| goertzel(&signal[48_000..], RATE, 2_000.0);
        assert!(
            at(&wobbly) < 0.7 * at(&steady),
            "{} {}",
            at(&wobbly),
            at(&steady)
        );
    }

    #[test]
    fn a_watched_tape_draws_its_loop_and_its_speed() {
        let mut effect = prepared();
        let monitor = effect.resonance_monitor().unwrap();
        monitor.watch(true);
        let _ = run(&mut effect, &sine(220.0, 0.6, 9_600));
        let frame = monitor.read().unwrap();
        assert_eq!(frame.kind, ResonanceKind::Tape);
        assert_eq!(frame.trace_len, RESONANCE_CURVE);
        let widest = frame.trace.iter().map(|[h, _]| h.abs()).fold(0.0, f32::max);
        assert!(widest > 0.1, "{widest}");
        assert!(frame.values[3] > 5.0, "tape speed {} cm/s", frame.values[3]);
    }
}
