//! Sample-free hammered dulcimer based on coupled courses of stiff strings.
//!
//! Each note excites two or three lightly detuned strings with a short raised-cosine hammer
//! contact.  The modal bank is a compact wave-equation approximation: partial frequencies bend
//! upward with stiffness, while a passive mixing junction models the shared bridge and soundboard.
//! It is intentionally a playable physical approximation rather than a recording replacement.

use std::f32::consts::{PI, TAU};

use auris_core::param::db_to_gain;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, Parameterized,
    PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_dsp::Adsr;

use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};
use crate::voice::VoiceAllocator;

const VOICES: usize = 24;
const MODES: usize = 24;
const STRINGS: usize = 3;
const MAX_BEND: f32 = 24.0;
const P_HARDNESS: u32 = 0;
const P_POSITION: u32 = 1;
const P_DECAY: u32 = 2;
const P_DAMPING: u32 = 3;
const P_STIFFNESS: u32 = 4;
const P_DETUNE: u32 = 5;
const P_RELEASE: u32 = 6;
const P_LEVEL: u32 = 7;

#[derive(Clone, Copy, Debug, Default)]
struct Mode {
    frequency_hz: f32,
    real: f32,
    imag: f32,
    sine: f32,
    cosine: f32,
    radius: f32,
    drive: f32,
}

#[derive(Clone, Debug)]
struct Voice {
    modes: [[Mode; STRINGS]; MODES],
    pitch: f32,
    velocity: f32,
    strings: usize,
    contact_frames: usize,
    age: usize,
    coupling: f32,
    envelope: Adsr,
}

impl Voice {
    fn new(rate: f32) -> Self {
        let mut envelope = Adsr::new();
        envelope.set_sample_rate(rate);
        Self {
            modes: [[Mode::default(); STRINGS]; MODES],
            pitch: 60.0,
            velocity: 0.0,
            strings: STRINGS,
            contact_frames: 2,
            age: 0,
            coupling: 0.0,
            envelope,
        }
    }

    fn reset(&mut self) {
        self.modes = [[Mode::default(); STRINGS]; MODES];
        self.velocity = 0.0;
        self.age = 0;
        self.envelope.silence();
    }

    fn energy(&self) -> f32 {
        self.modes
            .iter()
            .flat_map(|mode| &mode[..self.strings])
            .map(|mode| mode.real.abs() + mode.imag.abs())
            .sum()
    }
}

/// A polyphonic sample-free hammered dulcimer with detuned bridge courses.
#[derive(Clone, Debug)]
pub struct HammeredDulcimer {
    params: ParamBank,
    voices: Vec<Voice>,
    allocator: VoiceAllocator,
    rate: f32,
    bend: f32,
    channel_volume: f32,
    expression: f32,
    expression_current: f32,
    expression_coeff: f32,
}

impl Default for HammeredDulcimer {
    fn default() -> Self {
        Self::new()
    }
}

impl HammeredDulcimer {
    /// Stable plugin id stored in project files.
    pub const ID: &'static str = "auris.physical.hammered_dulcimer";

    /// Creates the calibrated default dulcimer patch.
    pub fn new() -> Self {
        let mut instrument = Self {
            params: ParamBank::new(vec![
                ParamDescriptor::percent(P_HARDNESS, "hardness", "Hammer Hardness", 0.2893),
                ParamDescriptor::new(
                    P_POSITION,
                    "position",
                    "Strike Position",
                    0.01,
                    0.49,
                    0.12627,
                )
                .with_unit(ParamUnit::Percent),
                ParamDescriptor::new(P_DECAY, "decay", "String Decay", 0.2, 20.0, 4.98569)
                    .with_unit(ParamUnit::Seconds),
                ParamDescriptor::percent(P_DAMPING, "damping", "Bridge Damping", 0.15979),
                ParamDescriptor::new(
                    P_STIFFNESS,
                    "stiffness",
                    "String Stiffness",
                    0.0,
                    0.002,
                    0.00048121,
                )
                .with_unit(ParamUnit::Plain),
                ParamDescriptor::new(P_DETUNE, "detune", "Course Detune", 0.0, 12.0, 0.78208)
                    .with_unit(ParamUnit::Plain),
                ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 4.0, 0.08443)
                    .with_unit(ParamUnit::Seconds),
                ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, -10.0),
            ]),
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            rate: 48_000.0,
            bend: 0.0,
            channel_volume: 1.0,
            expression: 1.0,
            expression_current: 1.0,
            expression_coeff: 0.0,
        };
        instrument.refresh();
        instrument
    }

    fn refresh(&mut self) {
        let release = self.params.at(P_RELEASE);
        for voice in &mut self.voices {
            voice.envelope.set_adsr(0.001, 0.0, 1.0, release);
            voice.coupling = (1.0 - (-0.8 / self.rate).exp()).clamp(0.00001, 0.1);
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: f32) {
        let Some(assignment) = self.allocator.note_on(pitch, velocity) else {
            return;
        };
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        let base_pitch = f32::from(pitch);
        let frequency = pitch_to_hz(base_pitch + self.bend);
        let hardness = self.params.at(P_HARDNESS).clamp(0.0, 1.0);
        let position = self.params.at(P_POSITION).clamp(0.01, 0.49);
        let stiffness = self.params.at(P_STIFFNESS).clamp(0.0, 0.002);
        let detune = self.params.at(P_DETUNE).clamp(0.0, 12.0);
        voice.pitch = base_pitch;
        voice.velocity = finite_or(velocity, 0.0).clamp(0.0, 1.0);
        voice.strings = if frequency < 110.0 { 2 } else { 3 };
        voice.contact_frames = ((0.00015 + 0.0018 * (1.0 - hardness).powi(2)) * self.rate)
            .round()
            .max(2.0) as usize;
        voice.age = 0;
        if !assignment.stolen {
            voice.modes = [[Mode::default(); STRINGS]; MODES];
        }
        for (index, modes) in voice.modes.iter_mut().enumerate() {
            let n = (index + 1) as f32;
            let ratio = n * ((1.0 + stiffness * n * n) / (1.0 + stiffness)).sqrt();
            let amplitude =
                (PI * n * position).sin() / n * voice.velocity.powf(1.1) * (0.75 + 0.25 * hardness);
            for (string, cents) in modes
                .iter_mut()
                .zip([-detune, 0.0, detune])
                .take(voice.strings)
            {
                let hz = frequency * ratio * (cents / 1200.0).exp2();
                *string = Mode::default();
                string.frequency_hz = hz;
                if hz < self.rate * 0.45 {
                    (string.sine, string.cosine) = (TAU * hz / self.rate).sin_cos();
                    string.radius = (-6.907_755
                        * (1.0 + self.params.at(P_DAMPING).clamp(0.0, 1.0) * 4.0 * n * n)
                        / (self.params.at(P_DECAY).clamp(0.2, 20.0) * self.rate))
                        .exp();
                    string.drive = amplitude;
                }
            }
        }
        voice.envelope.trigger();
    }
}

fn pitch_to_hz(pitch: f32) -> f32 {
    440.0 * ((pitch - 69.0) / 12.0).exp2()
}

impl Parameterized for HammeredDulcimer {
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

impl SegmentRenderer for HammeredDulcimer {
    fn handle_event(&mut self, event: &NoteEvent) {
        match *event {
            NoteEvent::NoteOn {
                pitch, velocity, ..
            } => self.note_on(pitch, velocity),
            NoteEvent::NoteOff { pitch, .. } => {
                for index in self.allocator.note_off(pitch) {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.envelope.release();
                    }
                }
            }
            NoteEvent::AllNotesOff { .. } | NoteEvent::AllSoundOff { .. } => {
                let mask = self.allocator.release_all();
                for index in mask {
                    if let Some(voice) = self.voices.get_mut(index) {
                        if matches!(*event, NoteEvent::AllSoundOff { .. }) {
                            voice.envelope.kill();
                        } else {
                            voice.envelope.release();
                        }
                    }
                }
            }
            NoteEvent::PitchBend { semitones, .. } => {
                let next = finite_or(semitones, 0.0).clamp(-MAX_BEND, MAX_BEND);
                let ratio = (2.0_f32).powf((next - self.bend) / 12.0);
                for voice in &mut self.voices {
                    if !voice.envelope.is_active() {
                        continue;
                    }
                    for modes in &mut voice.modes {
                        for mode in modes.iter_mut().take(voice.strings) {
                            mode.frequency_hz *= ratio;
                            if mode.frequency_hz > 0.0 && mode.frequency_hz < self.rate * 0.45 {
                                (mode.sine, mode.cosine) =
                                    (TAU * mode.frequency_hz / self.rate).sin_cos();
                            } else {
                                // Drop modes crossing the represented bandwidth. Keep their
                                // unwrapped frequency so a later bend cannot fold an alias back.
                                mode.sine = 0.0;
                                mode.cosine = 0.0;
                                mode.real = 0.0;
                                mode.imag = 0.0;
                            }
                        }
                    }
                }
                self.bend = next
            }
            NoteEvent::Controller {
                number: 7, value, ..
            } => self.channel_volume = finite_or(value, 1.0).clamp(0.0, 1.0),
            NoteEvent::Controller {
                number: 11, value, ..
            } => self.expression = finite_or(value, 1.0).clamp(0.0, 1.0),
            _ => {}
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
        let damping = self.params.at(P_DAMPING).clamp(0.0, 1.0);
        let decay = self.params.at(P_DECAY).clamp(0.2, 20.0);
        let output = db_to_gain(self.params.at(P_LEVEL)) * self.channel_volume * 0.9;
        // Decay and bridge damping are automatable. Recompute the pre-exponential modal loss at
        // segment boundaries so changing either control affects already sounding courses while
        // keeping the sample loop fixed-cost.
        for voice in &mut self.voices {
            for (partial_index, modes) in voice.modes.iter_mut().enumerate() {
                let n = (partial_index + 1) as f32;
                for mode in modes {
                    if mode.sine == 0.0 && mode.cosine == 0.0 {
                        mode.radius = 0.0;
                        continue;
                    }
                    mode.radius =
                        (-6.907_755 * (1.0 + damping * 4.0 * n * n) / (decay * self.rate)).exp();
                }
            }
        }
        for sample in dst.iter_mut() {
            self.expression_current +=
                self.expression_coeff * (self.expression - self.expression_current);
            let mut sum = 0.0;
            for voice in &mut self.voices {
                if !voice.envelope.is_active() {
                    continue;
                }
                let env = voice.envelope.process();
                let force = if voice.age < voice.contact_frames {
                    let phase = (voice.age as f32 + 0.5) / voice.contact_frames as f32;
                    voice.age += 1;
                    (1.0 - (TAU * phase).cos()) / voice.contact_frames as f32
                } else {
                    0.0
                };
                let mut voice_output = 0.0;
                for modes in &mut voice.modes {
                    let mut mean_real = 0.0;
                    let mut mean_imag = 0.0;
                    let mut active = 0;
                    for mode in modes.iter_mut().take(voice.strings) {
                        if mode.frequency_hz <= 0.0 || mode.frequency_hz >= self.rate * 0.45 {
                            continue;
                        }
                        if force != 0.0 {
                            mode.imag += force * mode.drive;
                        }
                        let real = mode.radius * (mode.cosine * mode.real - mode.sine * mode.imag);
                        let imag = mode.radius * (mode.sine * mode.real + mode.cosine * mode.imag);
                        mode.real = real;
                        mode.imag = imag;
                        mean_real += real;
                        mean_imag += imag;
                        active += 1;
                    }
                    if active == 0 {
                        continue;
                    }
                    let inv = 1.0 / active as f32;
                    mean_real *= inv;
                    mean_imag *= inv;
                    for mode in modes.iter_mut().take(voice.strings) {
                        if mode.frequency_hz <= 0.0 || mode.frequency_hz >= self.rate * 0.45 {
                            continue;
                        }
                        mode.real += voice.coupling * (mean_real - mode.real);
                        mode.imag += voice.coupling * (mean_imag - mode.imag);
                    }
                    voice_output += mean_real / (1.0 + damping * 0.04);
                }
                sum += voice_output * env;
            }
            *sample = sum * output * self.expression_current;
        }
        for (slot, voice) in self.voices.iter().enumerate() {
            self.allocator
                .set_level(slot, voice.energy() * voice.velocity);
            if voice.envelope.is_finished() {
                self.allocator.retire(slot);
            }
        }
    }
}

impl Instrument for HammeredDulcimer {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            Self::ID,
            "Physical Hammered Dulcimer",
            "Hammer-excited coupled string courses with passive bridge coupling",
            PluginCategory::Synth,
        )
    }
    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = crate::sample_rate_f32(ctx.sample_rate).clamp(8_000.0, 192_000.0);
        self.voices.clear();
        self.voices.reserve(VOICES);
        for _ in 0..VOICES {
            self.voices.push(Voice::new(self.rate));
        }
        self.allocator.prepare(VOICES);
        self.bend = 0.0;
        self.channel_volume = 1.0;
        self.expression = 1.0;
        self.expression_current = 1.0;
        self.expression_coeff = (1.0 - (-1.0 / (0.020 * f64::from(self.rate))).exp()) as f32;
        let decay = self.params.at(P_DECAY).clamp(0.2, 20.0);
        let damping = self.params.at(P_DAMPING).clamp(0.0, 1.0);
        for voice in &mut self.voices {
            for modes in &mut voice.modes {
                for (index, mode) in modes.iter_mut().enumerate() {
                    let n = (index + 1) as f32;
                    mode.radius =
                        (-6.907_755 * (1.0 + damping * 4.0 * n * n) / (decay * self.rate)).exp();
                }
            }
        }
        self.refresh();
    }
    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset();
        }
        self.allocator.clear();
        self.bend = 0.0;
        self.channel_volume = 1.0;
        self.expression = 1.0;
        self.expression_current = 1.0;
    }
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        render_segments(self, events, out, ctx.block_frames.min(out.frame_count()));
        spread_to_all_channels(out, ctx.block_frames.min(out.frame_count()));
    }
    fn active_voices(&self) -> usize {
        self.allocator.active_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Rig, count_allocations, goertzel, peak, rms};

    fn measured_pitch(samples: &[f32], rate: f64, expected: f64) -> f64 {
        let mut best = (expected, 0.0);
        for step in -30..=30 {
            let frequency = expected * 2.0_f64.powf(step as f64 / 1200.0);
            let amplitude = goertzel(samples, rate, frequency);
            if amplitude > best.1 {
                best = (frequency, amplitude);
            }
        }
        best.0
    }

    #[test]
    fn dulcimer_is_tuned_and_has_sound() {
        let mut rig = Rig::new(Box::new(HammeredDulcimer::new()), 48_000.0, 256, 1);
        let audio = rig.render(
            4096,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: 0.8,
            }],
        );
        assert!(peak(&audio) > 0.001);
        assert!(rms(&audio) > 0.0001);
        assert!(audio.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn reset_is_deterministic_and_callback_does_not_allocate() {
        let mut instrument = HammeredDulcimer::new();
        instrument.prepare(&PrepareContext::new(48_000.0, 128, 1));
        let mut buffer = AudioBuffer::new(1, 128, 48_000.0);
        let ctx = ProcessContext::realtime(48_000.0, 128, 0, 120.0, true);
        let events = [NoteEvent::NoteOn {
            frame: 0,
            pitch: 60,
            velocity: 0.8,
        }];
        let allocations = count_allocations(|| instrument.process(&events, &mut buffer, &ctx));
        assert_eq!(allocations, 0);
        instrument.reset();
        let mut first = Rig::new(Box::new(HammeredDulcimer::new()), 24_000.0, 128, 1);
        let a = first.render(2048, &events);
        first.instrument.reset();
        let b = first.render(2048, &events);
        assert_eq!(a, b);
    }

    #[test]
    fn invalid_rates_and_extreme_notes_remain_finite() {
        for rate in [8_000.0, 192_000.0, 1.0, f64::NAN, f64::INFINITY] {
            let mut rig = Rig::new(Box::new(HammeredDulcimer::new()), rate, 128, 1);
            let audio = rig.render(
                512,
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 127,
                    velocity: 1.0,
                }],
            );
            assert!(audio.iter().all(|sample| sample.is_finite()));
        }
    }

    #[test]
    fn fundamental_stays_within_twelve_cents_across_range() {
        for rate in [24_000.0, 48_000.0] {
            for pitch in (48u8..=84).step_by(6) {
                let mut rig = Rig::new(Box::new(HammeredDulcimer::new()), rate, 256, 1);
                let audio = rig.render(
                    (rate * 0.35) as usize,
                    &[NoteEvent::NoteOn {
                        frame: 0,
                        pitch,
                        velocity: 0.8,
                    }],
                );
                let body = &audio[(rate * 0.16) as usize..];
                let expected = pitch_to_hz(f32::from(pitch)) as f64;
                let measured = measured_pitch(body, rate, expected);
                let cents = 1200.0 * (measured / expected).log2();
                assert!(cents.abs() < 12.0, "pitch {pitch} at {rate}: {cents} cents");
            }
        }
    }

    #[test]
    fn hard_hammer_has_more_upper_partial_energy() {
        let render = |hardness: f32| {
            let mut rig = Rig::new(Box::new(HammeredDulcimer::new()), 48_000.0, 256, 1);
            rig.set_param("hardness", hardness);
            rig.set_param("stiffness", 0.0);
            rig.set_param("detune", 0.0);
            rig.set_param("damping", 0.0);
            rig.render(
                12_000,
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 60,
                    velocity: 0.9,
                }],
            )
        };
        let soft = render(0.1);
        let hard = render(0.95);
        let fundamental = pitch_to_hz(60.0) as f64;
        let soft_upper = goertzel(&soft[4_000..], 48_000.0, fundamental * 5.0)
            / goertzel(&soft[4_000..], 48_000.0, fundamental);
        let hard_upper = goertzel(&hard[4_000..], 48_000.0, fundamental * 5.0)
            / goertzel(&hard[4_000..], 48_000.0, fundamental);
        assert!(
            hard_upper > soft_upper * 1.05,
            "soft={soft_upper} hard={hard_upper}"
        );
    }

    #[test]
    fn bridge_damping_suppresses_higher_partials_more_than_the_fundamental() {
        let render = |damping: f32| {
            let mut rig = Rig::new(Box::new(HammeredDulcimer::new()), 48_000.0, 256, 1);
            rig.set_param("damping", damping);
            rig.set_param("stiffness", 0.0);
            rig.set_param("detune", 0.0);
            rig.set_param("hardness", 0.95);
            rig.render(
                12_000,
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 60,
                    velocity: 0.9,
                }],
            )
        };
        let open = render(0.0);
        let damped = render(0.9);
        let fundamental = pitch_to_hz(60.0) as f64;
        let open_ratio = goertzel(&open[500..2_500], 48_000.0, fundamental * 7.0)
            / goertzel(&open[500..2_500], 48_000.0, fundamental);
        let damped_ratio = goertzel(&damped[500..2_500], 48_000.0, fundamental * 7.0)
            / goertzel(&damped[500..2_500], 48_000.0, fundamental);
        assert!(
            damped_ratio < open_ratio * 0.8,
            "open={open_ratio} damped={damped_ratio}"
        );
    }

    #[test]
    fn decay_control_changes_late_to_early_energy_ratio() {
        let render = |decay: f32| {
            let mut rig = Rig::new(Box::new(HammeredDulcimer::new()), 24_000.0, 256, 1);
            rig.set_param("decay", decay);
            rig.render(
                20_000,
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 60,
                    velocity: 0.8,
                }],
            )
        };
        let short = render(0.4);
        let long = render(12.0);
        let short_ratio = rms(&short[14_000..16_000]) / rms(&short[2_000..4_000]);
        let long_ratio = rms(&long[14_000..16_000]) / rms(&long[2_000..4_000]);
        assert!(
            long_ratio > short_ratio * 1.8,
            "short={short_ratio} long={long_ratio}"
        );
    }

    #[test]
    fn event_boundaries_are_block_independent_and_release_retires() {
        let events = [
            NoteEvent::NoteOn {
                frame: 73,
                pitch: 60,
                velocity: 0.8,
            },
            NoteEvent::Controller {
                frame: 1_600,
                number: 11,
                value: 0.25,
            },
            NoteEvent::PitchBend {
                frame: 2_000,
                semitones: 1.5,
            },
            NoteEvent::NoteOff {
                frame: 3_000,
                pitch: 60,
            },
        ];
        let mut small = Rig::new(Box::new(HammeredDulcimer::new()), 24_000.0, 128, 1);
        let mut large = Rig::new(Box::new(HammeredDulcimer::new()), 24_000.0, 512, 1);
        let a = small.render(12_000, &events);
        let b = large.render(12_000, &events);
        assert_eq!(a, b);
        assert_eq!(small.instrument.active_voices(), 0);

        let mut bent = Rig::new(Box::new(HammeredDulcimer::new()), 24_000.0, 128, 1);
        let bent_audio = bent.render(
            12_000,
            &[
                NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 60,
                    velocity: 0.8,
                },
                NoteEvent::PitchBend {
                    frame: 2_000,
                    semitones: 1.5,
                },
            ],
        );
        let expected = pitch_to_hz(61.5) as f64;
        let measured = measured_pitch(&bent_audio[4_000..], 24_000.0, expected);
        assert!((1200.0 * (measured / expected).log2()).abs() < 15.0);

        let mut pre_bent = Rig::new(Box::new(HammeredDulcimer::new()), 24_000.0, 128, 1);
        let pre_bent_audio = pre_bent.render(
            8_000,
            &[
                NoteEvent::PitchBend {
                    frame: 0,
                    semitones: 1.5,
                },
                NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 60,
                    velocity: 0.8,
                },
            ],
        );
        let measured = measured_pitch(&pre_bent_audio[2_000..], 24_000.0, expected);
        assert!((1200.0 * (measured / expected).log2()).abs() < 15.0);
    }

    #[test]
    fn a_large_bend_cannot_fold_ultrasonic_modes_into_the_output() {
        for (rate, pitch) in [(24_000.0, 105), (48_000.0, 117)] {
            let mut rig = Rig::new(Box::new(HammeredDulcimer::new()), rate, 128, 1);
            rig.set_param("hardness", 0.95);
            let audio = rig.render(
                8_000,
                &[
                    NoteEvent::NoteOn {
                        frame: 0,
                        pitch,
                        velocity: 0.8,
                    },
                    NoteEvent::PitchBend {
                        frame: 2_000,
                        semitones: 24.0,
                    },
                ],
            );
            assert!(rms(&audio[1_000..2_000]) > 1e-4);
            assert_eq!(peak(&audio[2_000..]), 0.0);
            let mut instrument = HammeredDulcimer::new();
            instrument.prepare(&PrepareContext::new(rate, 128, 1));
            instrument.handle_event(&NoteEvent::NoteOn {
                frame: 0,
                pitch,
                velocity: 0.8,
            });
            let original = instrument.voices[0].modes[0][0].frequency_hz;
            for bend in [24.0, -24.0, 0.0] {
                instrument.handle_event(&NoteEvent::PitchBend {
                    frame: 0,
                    semitones: bend,
                });
            }
            assert!((instrument.voices[0].modes[0][0].frequency_hz / original - 1.0).abs() < 1e-6);
        }
    }
}
