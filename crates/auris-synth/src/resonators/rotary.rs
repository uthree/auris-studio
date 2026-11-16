//! A rotating horn over a rotating drum, heard by two microphones.

use std::borrow::Cow;
use std::f32::consts::{PI, TAU};
use std::sync::Arc;

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

/// Speed of sound, in metres a second.
const SOUND: f32 = 343.0;
/// Radius the horn's mouth turns at, and the drum's opening, in metres.
const HORN_RADIUS: f32 = 0.15;
const DRUM_RADIUS: f32 = 0.10;
/// Where the treble driver hands over to the bass driver, in hertz.
const CROSSOVER_HZ: f32 = 800.0;
/// Rotation speeds, in turns a second: horn and drum, slow and fast.
const CHORALE: (f32, f32) = (0.83, 0.67);
const TREMOLO: (f32, f32) = (6.7, 5.83);
/// How long each rotor takes to come up to speed and to slow down, as time constants in
/// seconds. The horn is light and gets there in under a second; the drum is heavy and takes
/// several, which is why a switch from slow to fast is heard as the treble arriving first.
const HORN_INERTIA: (f32, f32) = (0.7, 0.9);
const DRUM_INERTIA: (f32, f32) = (4.5, 3.5);
/// The cabinet's back wall: how much later the reflection of the horn arrives, and how loud.
const REFLECTION_S: f32 = 0.0011;
const REFLECTION: f32 = 0.35;
/// Longest path a delay line has to hold, in seconds.
const LONGEST_S: f32 = 0.012;

const SPEED: u32 = 0;
const BALANCE: u32 = 1;
const DISTANCE: u32 = 2;
const SPREAD: u32 = 3;
const DRIVE: u32 = 4;
const MIX: u32 = 5;

const SPEED_CHOICES: [Cow<'static, str>; 3] = [
    Cow::Borrowed("Stop"),
    Cow::Borrowed("Chorale"),
    Cow::Borrowed("Tremolo"),
];

fn targets(choice: f32) -> (f32, f32) {
    match choice.round() as i32 {
        1 => CHORALE,
        2 => TREMOLO,
        _ => (0.0, 0.0),
    }
}

/// A turning rotor's angle and speed, with the inertia that carries it between speeds.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Rotor {
    /// Angle, in radians, from facing the listener.
    pub(crate) angle: f32,
    /// Speed, in turns a second.
    pub(crate) speed: f32,
}

impl Rotor {
    fn turn(&mut self, target: f32, inertia: (f32, f32), rate: f32) {
        let tau = if target > self.speed {
            inertia.0
        } else {
            inertia.1
        };
        self.speed += (target - self.speed) / (tau * rate);
        self.angle = (self.angle + TAU * self.speed / rate).rem_euclid(TAU);
    }
}

/// How loudly a mouth pointing along `angle` is heard from a microphone at `towards`, its
/// beam narrowed by `focus` from omnidirectional (0) to a horn's (1).
fn beam(angle: f32, towards: f32, focus: f32) -> f32 {
    let facing = 0.5 * (1.0 + (angle - towards).cos());
    (1.0 - focus) + focus * facing * facing
}

/// The level of [`beam`] averaged over a turn: the microphones' gain is set so that a rotor
/// is heard on average at the level it was given.
fn beam_average(focus: f32) -> f32 {
    // The square of a raised cosine averages three eighths.
    (1.0 - focus) + focus * 0.375
}

/// How narrow the horn's beam is and how narrow the drum's opening.
const HORN_FOCUS: f32 = 0.85;
const DRUM_FOCUS: f32 = 0.55;

/// The path, in metres, from a mouth at `angle` on a circle of `radius` to a microphone
/// `distance` away at `towards`.
fn path(angle: f32, radius: f32, distance: f32, towards: f32) -> f32 {
    let dx = distance * towards.cos() - radius * angle.cos();
    let dy = distance * towards.sin() - radius * angle.sin();
    (dx * dx + dy * dy).sqrt()
}

/// A short line to read a moving mouth's sound from, a whole number of frames long.
#[derive(Clone, Debug, Default)]
struct Line {
    samples: Vec<f32>,
    mask: usize,
    at: usize,
}

impl Line {
    fn new(longest: usize) -> Self {
        let length = (longest + 8).next_power_of_two();
        Self {
            samples: vec![0.0; length],
            mask: length - 1,
            at: 0,
        }
    }

    fn write(&mut self, sample: f32) {
        self.samples[self.at] = sample;
        self.at = (self.at + 1) & self.mask;
    }

    fn read(&self, delay: f32) -> f32 {
        let delay = delay.clamp(2.0, (self.samples.len() - 4) as f32);
        let whole = delay as usize;
        let at = |back: usize| self.samples[(self.at + self.samples.len() - back) & self.mask];
        hermite(
            [at(whole - 1), at(whole), at(whole + 1), at(whole + 2)],
            delay - whole as f32,
        )
    }

    fn reset(&mut self) {
        self.samples.fill(0.0);
        self.at = 0;
    }
}

/// A rotary speaker.
///
/// The organ's signal is split at 800 Hz. The treble goes up into a horn that turns on its
/// axis, the bass down into a drum that turns beneath its woofer, and two microphones stand a
/// little way off, either side. Everything that is heard comes from that motion: as the
/// horn's mouth swings towards a microphone the path shortens and the pitch rises, as it
/// swings away the pitch falls (Doppler), and the beam that faces one microphone has turned
/// away from the other, so the sound sweeps across. The cabinet's back wall sends back what
/// the horn throws away from the front. Neither rotor changes speed at once: the horn comes
/// up to *Tremolo* in under a second and the drum takes several, so a switch is heard as the
/// treble arriving first and the bass catching up.
///
/// The window shows the cabinet from above, both rotors turning, and their speeds over the
/// last few seconds.
pub struct RotarySpeaker {
    params: ParamBank,
    rate: f32,
    crossover: [Biquad; 4],
    horn_line: Line,
    drum_line: Line,
    /// The back wall dulls what it sends back.
    wall: [Biquad; 2],
    horn: Rotor,
    drum: Rotor,
    mix: f32,
    phase: usize,
    ticks: u32,
    input: Level,
    output: Level,
    horn_speeds: [f32; RESONANCE_CURVE],
    drum_speeds: [f32; RESONANCE_CURVE],
    speeds_at: usize,
    capture: ResonanceCapture,
}

impl Default for RotarySpeaker {
    fn default() -> Self {
        Self::new()
    }
}

impl RotarySpeaker {
    /// Stable plugin identifier stored in projects.
    pub const ID: &str = "auris.fx.rotary";

    /// A rotary speaker turning slowly. Allocates nothing until `prepare`.
    pub fn new() -> Self {
        let params = ParamBank::new(vec![
            ParamDescriptor::new(SPEED, "speed", "Speed", 0.0, 2.0, 1.0)
                .with_choices(&SPEED_CHOICES),
            ParamDescriptor::new(BALANCE, "balance", "Horn/Drum", -1.0, 1.0, 0.0),
            ParamDescriptor::percent(DISTANCE, "distance", "Mic Distance", 0.35),
            ParamDescriptor::new(SPREAD, "spread", "Mic Spread", 20.0, 180.0, 120.0),
            ParamDescriptor::percent(DRIVE, "drive", "Drive", 0.2),
            ParamDescriptor::percent(MIX, "mix", "Mix", 1.0),
        ]);
        Self {
            params,
            rate: 48_000.0,
            crossover: Default::default(),
            horn_line: Line::default(),
            drum_line: Line::default(),
            wall: Default::default(),
            horn: Rotor::default(),
            drum: Rotor {
                angle: 1.3,
                speed: 0.0,
            },
            mix: 1.0,
            phase: 0,
            ticks: 0,
            input: Level::default(),
            output: Level::default(),
            horn_speeds: [0.0; RESONANCE_CURVE],
            drum_speeds: [0.0; RESONANCE_CURVE],
            speeds_at: 0,
            capture: ResonanceCapture::default(),
        }
    }

    /// The microphones' distance from the cabinet's centre, in metres.
    fn mic_distance(&self) -> f32 {
        0.35 + 0.9 * self.params.at(DISTANCE)
    }

    fn publish(&mut self) {
        let (horn_target, drum_target) = targets(self.params.at(SPEED));
        let mut frame = ResonanceFrame {
            kind: ResonanceKind::Rotary,
            input: self.input.get(),
            output: self.output.get(),
            values: [
                self.horn.angle,
                self.drum.angle,
                self.horn.speed,
                self.drum.speed,
                horn_target,
                drum_target,
                self.params.at(SPREAD),
                self.mic_distance(),
            ],
            ..ResonanceFrame::default()
        };
        let span = (RESONANCE_CURVE * PUBLISH_TICKS as usize * 3 * TICK) as f32 / self.rate;
        for index in 0..RESONANCE_CURVE {
            let at = (self.speeds_at + index) % RESONANCE_CURVE;
            let time = -span + span * index as f32 / (RESONANCE_CURVE - 1) as f32;
            frame.curve[index] = [time, self.horn_speeds[at]];
            frame.trace[index] = [time, self.drum_speeds[at]];
        }
        frame.curve_len = RESONANCE_CURVE;
        frame.trace_len = RESONANCE_CURVE;
        self.capture.publish(&frame);
    }
}

impl Parameterized for RotarySpeaker {
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

impl Effect for RotarySpeaker {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::effect(
            Self::ID,
            "Rotary Speaker",
            "A turning horn over a turning drum: Doppler, sweep and the inertia between speeds",
            PluginCategory::Modulation,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = sane_rate(ctx.sample_rate);
        let rate = f64::from(self.rate);
        let low = BiquadCoefficients::lowpass(rate, CROSSOVER_HZ, std::f32::consts::FRAC_1_SQRT_2);
        let high =
            BiquadCoefficients::highpass(rate, CROSSOVER_HZ, std::f32::consts::FRAC_1_SQRT_2);
        // Linkwitz–Riley: each side twice, so the two add back up flat.
        self.crossover[0].set_coefficients(low);
        self.crossover[1].set_coefficients(low);
        self.crossover[2].set_coefficients(high);
        self.crossover[3].set_coefficients(high);
        let wall = BiquadCoefficients::lowpass(rate, 3_000.0, 0.7);
        for filter in &mut self.wall {
            filter.set_coefficients(wall);
        }
        let longest = (LONGEST_S * self.rate) as usize;
        self.horn_line = Line::new(longest);
        self.drum_line = Line::new(longest);
        self.reset();
    }

    fn reset(&mut self) {
        for filter in self.crossover.iter_mut().chain(&mut self.wall) {
            filter.reset();
        }
        self.horn_line.reset();
        self.drum_line.reset();
        let (horn, drum) = targets(self.params.at(SPEED));
        // A rotary speaker that was already turning when the song started.
        self.horn = Rotor {
            angle: 0.0,
            speed: horn,
        };
        self.drum = Rotor {
            angle: 1.3,
            speed: drum,
        };
        self.mix = self.params.at(MIX);
        self.phase = 0;
        self.ticks = 0;
        self.input.reset();
        self.output.reset();
        self.horn_speeds = [horn; RESONANCE_CURVE];
        self.drum_speeds = [drum; RESONANCE_CURVE];
    }

    fn process(&mut self, buffer: &mut AudioBuffer, ctx: &ProcessContext) {
        if self.horn_line.samples.is_empty() {
            return;
        }
        let frames = buffer.frame_count().min(ctx.block_frames);
        let stereo = buffer.channel_count() >= 2;
        let rate = self.rate;
        let (horn_target, drum_target) = targets(self.params.at(SPEED));
        let balance = self.params.at(BALANCE);
        let (horn_level, drum_level) = (
            (1.0 + balance).min(1.0) / beam_average(HORN_FOCUS),
            (1.0 - balance).min(1.0) / beam_average(DRUM_FOCUS),
        );
        let distance = self.mic_distance();
        let spread = self.params.at(SPREAD).to_radians();
        let mics = [-0.5 * spread, 0.5 * spread];
        let drive = 1.0 + 7.0 * self.params.at(DRIVE);
        let mix_target = self.params.at(MIX);
        let samples_per_metre = rate / SOUND;
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
            // The organ's amplifier: unity for a quiet signal, rounding a loud one off.
            let amplified = ((0.5 * (left + right)) * drive).tanh() / drive;
            let [low_a, low_b, high_a, high_b] = &mut self.crossover;
            let bass = low_b.process_sample(low_a.process_sample(amplified));
            let treble = high_b.process_sample(high_a.process_sample(amplified));
            self.horn_line.write(treble);
            self.drum_line.write(bass);
            self.horn.turn(horn_target, HORN_INERTIA, rate);
            self.drum.turn(drum_target, DRUM_INERTIA, rate);
            let mut wet = [0.0_f32; 2];
            for ((side, towards), wall) in wet.iter_mut().zip(mics).zip(&mut self.wall) {
                let horn_path = path(self.horn.angle, HORN_RADIUS, distance, towards);
                let direct = self.horn_line.read(horn_path * samples_per_metre)
                    * beam(self.horn.angle, towards, HORN_FOCUS)
                    * (distance / horn_path);
                // What the horn throws backwards comes off the wall behind it a moment later.
                let back = self.horn.angle + PI;
                let bounced = self.horn_line.read(
                    (path(back, HORN_RADIUS, distance, towards) / SOUND + REFLECTION_S) * rate,
                ) * beam(back, towards, HORN_FOCUS)
                    * REFLECTION;
                let drum_path = path(self.drum.angle, DRUM_RADIUS, distance, towards);
                let drum = self.drum_line.read(drum_path * samples_per_metre)
                    * beam(self.drum.angle, towards, DRUM_FOCUS)
                    * (distance / drum_path);
                *side = horn_level * (direct + wall.process_sample(bounced)) + drum_level * drum;
            }
            let ramp = (frame + 1) as f32 / frames as f32;
            let mix = self.mix + (mix_target - self.mix) * ramp;
            let (out_left, out_right) = (
                (1.0 - mix) * left + mix * wet[0],
                (1.0 - mix) * right + mix * wet[1],
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
                if self.ticks.is_multiple_of(PUBLISH_TICKS * 3) {
                    self.horn_speeds[self.speeds_at] = self.horn.speed;
                    self.drum_speeds[self.speeds_at] = self.drum.speed;
                    self.speeds_at = (self.speeds_at + 1) % RESONANCE_CURVE;
                }
                if self.capture.is_watched() && self.ticks.is_multiple_of(PUBLISH_TICKS) {
                    self.publish();
                }
            }
        }
        self.mix = mix_target;
    }

    fn tail_frames(&self) -> usize {
        (LONGEST_S * self.rate) as usize + 64
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

    fn prepared(speed: f32) -> RotarySpeaker {
        let mut effect = RotarySpeaker::new();
        effect.prepare(&PrepareContext::new(RATE, 512, 2));
        effect.set_param(ParamId(SPEED), speed);
        effect.set_param(ParamId(DRIVE), 0.0);
        effect.reset();
        effect
    }

    fn run(effect: &mut RotarySpeaker, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (Vec::new(), Vec::new());
        for (block, chunk) in input.chunks(512).enumerate() {
            let mut buffer = AudioBuffer::stereo(chunk.len(), RATE);
            buffer.channel_mut(0).copy_from_slice(chunk);
            buffer.channel_mut(1).copy_from_slice(chunk);
            effect.process(
                &mut buffer,
                &ProcessContext::realtime(RATE, chunk.len(), (block * 512) as u64, 120.0, true),
            );
            left.extend_from_slice(buffer.channel(0));
            right.extend_from_slice(buffer.channel(1));
        }
        (left, right)
    }

    fn sine(hz: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|index| 0.3 * (TAU * hz * index as f32 / RATE as f32).sin())
            .collect()
    }

    /// The level of `signal` in windows of `window` frames, in decibels.
    fn envelope(signal: &[f32], window: usize) -> Vec<f32> {
        signal
            .chunks(window)
            .map(|chunk| 20.0 * rms(chunk).max(1.0e-9).log10())
            .collect()
    }

    #[test]
    fn the_horn_gets_up_to_speed_long_before_the_drum() {
        let mut effect = prepared(1.0);
        effect.set_param(ParamId(SPEED), 2.0);
        let _ = run(&mut effect, &vec![0.0; 48_000]);
        let horn = (effect.horn.speed - CHORALE.0) / (TREMOLO.0 - CHORALE.0);
        let drum = (effect.drum.speed - CHORALE.1) / (TREMOLO.1 - CHORALE.1);
        assert!(horn > 0.7, "horn {horn}");
        assert!(drum < 0.35, "drum {drum}");
        let _ = run(&mut effect, &vec![0.0; 11 * 48_000]);
        assert!((effect.horn.speed - TREMOLO.0).abs() < 0.05);
        assert!((effect.drum.speed - TREMOLO.1).abs() < 0.5);
    }

    #[test]
    fn a_still_speaker_is_steady_and_a_turning_one_sweeps() {
        let tone = sine(3_000.0, 48_000);
        let (still, _) = run(&mut prepared(0.0), &tone);
        let (turning, turning_right) = run(&mut prepared(2.0), &tone);
        let swing = |signal: &[f32]| {
            let levels = envelope(&signal[9_600..], 480);
            let (low, high) = levels.iter().fold((f32::MAX, f32::MIN), |(l, h), level| {
                (l.min(*level), h.max(*level))
            });
            high - low
        };
        assert!(swing(&still) < 0.5, "{}", swing(&still));
        assert!(swing(&turning) > 4.0, "{}", swing(&turning));
        // The two microphones are swept at different moments.
        let difference: Vec<f32> = turning
            .iter()
            .zip(&turning_right)
            .map(|(l, r)| l - r)
            .collect();
        assert!(rms(&difference[9_600..]) > 0.2 * rms(&turning[9_600..]));
    }

    #[test]
    fn the_horn_moves_the_pitch_by_its_speed_over_the_speed_of_sound() {
        // A mouth on a 15 cm circle at 6.7 turns a second moves at 6.3 m/s: a 3 kHz tone is
        // pushed about 55 Hz either way, which is a path changing by up to 1.6 samples' delay
        // a hundredth of a second apart.
        let speed = TAU * HORN_RADIUS * TREMOLO.0;
        let shift = 3_000.0 * speed / SOUND;
        assert!((45.0..65.0).contains(&shift), "{shift}");
        let distance = 0.35 + 0.9 * 0.35;
        let near = path(0.0, HORN_RADIUS, distance, 0.0);
        let far = path(PI, HORN_RADIUS, distance, 0.0);
        assert!((far - near - 2.0 * HORN_RADIUS).abs() < 1.0e-4);
    }

    #[test]
    fn a_watched_speaker_publishes_its_rotors() {
        let mut effect = prepared(2.0);
        let monitor = effect.resonance_monitor().unwrap();
        monitor.watch(true);
        let _ = run(&mut effect, &sine(440.0, 9_600));
        let frame = monitor.read().unwrap();
        assert_eq!(frame.kind, ResonanceKind::Rotary);
        assert!((frame.values[2] - TREMOLO.0).abs() < 0.01);
        assert!((0.0..TAU).contains(&frame.values[0]));
        assert_eq!(frame.curve_len, RESONANCE_CURVE);
        assert!(frame.curve[0][0] < frame.curve[RESONANCE_CURVE - 1][0]);
    }
}
