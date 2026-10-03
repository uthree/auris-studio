//! Sample-free cylindrical-bore clarinet.
//!
//! A clarinet is represented by a closed-open waveguide: a nonlinear reed injects pressure at
//! the mouthpiece and a delayed bore reflection returns the odd-harmonic pressure pattern. The
//! reduced model stores travelling pressure in a delay line, so excitation and note release
//! remain coupled to the bore. The pressure-flow law is a bounded phenomenological approximation.

use auris_core::param::db_to_gain;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, Parameterized,
    PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_dsp::Adsr;

use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};
use crate::voice::VoiceAllocator;

const VOICES: usize = 16;
const P_PRESSURE: u32 = 0;
const P_REED: u32 = 1;
const P_NOISE: u32 = 2;
const P_ATTACK: u32 = 3;
const P_RELEASE: u32 = 4;
const P_LEVEL: u32 = 5;
const MAX_BEND: f32 = 24.0;

#[derive(Clone, Debug)]
struct Voice {
    delay: Vec<f32>,
    write: usize,
    delay_len: usize,
    delay_fraction: f32,
    pitch: f32,
    velocity: f32,
    envelope: Adsr,
    noise: u32,
    last: f32,
    flow_mean: f32,
}

impl Voice {
    fn new(rate: f32) -> Self {
        let mut envelope = Adsr::new();
        envelope.set_sample_rate(rate);
        Self {
            delay: Vec::new(),
            write: 0,
            delay_len: 1,
            delay_fraction: 0.0,
            pitch: 60.0,
            velocity: 0.0,
            envelope,
            noise: 0x1234_5678,
            last: 0.0,
            flow_mean: 0.0,
        }
    }

    fn prepare(&mut self, rate: f32, max_delay: usize) {
        self.delay.resize(max_delay.max(2), 0.0);
        self.envelope.set_sample_rate(rate);
    }

    fn reset(&mut self) {
        self.delay.fill(0.0);
        self.write = 0;
        self.delay_len = 1;
        self.delay_fraction = 0.0;
        self.velocity = 0.0;
        self.last = 0.0;
        self.flow_mean = 0.0;
        self.noise = 0x1234_5678;
        self.envelope.silence();
    }

    fn next_noise(&mut self) -> f32 {
        self.noise = self
            .noise
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        (self.noise as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// A polyphonic sample-free clarinet with a nonlinear reed and cylindrical-bore waveguide.
#[derive(Clone, Debug)]
pub struct Clarinet {
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

impl Default for Clarinet {
    fn default() -> Self {
        Self::new()
    }
}

impl Clarinet {
    /// Stable plugin id stored in project files.
    pub const ID: &'static str = "auris.physical.clarinet";

    /// Builds a medium-register clarinet patch.
    pub fn new() -> Self {
        let mut clarinet = Self {
            params: ParamBank::new(vec![
                ParamDescriptor::percent(P_PRESSURE, "pressure", "Breath Pressure", 0.415_359),
                ParamDescriptor::percent(P_REED, "reed_stiffness", "Reed Stiffness", 0.481_810),
                ParamDescriptor::percent(P_NOISE, "breath_noise", "Breath Noise", 0.150_287),
                ParamDescriptor::new(P_ATTACK, "attack", "Attack", 0.001, 0.3, 0.079_192)
                    .with_unit(ParamUnit::Seconds),
                ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 4.0, 0.24)
                    .with_unit(ParamUnit::Seconds),
                ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, -8.0),
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
        clarinet.refresh();
        clarinet
    }

    fn refresh(&mut self) {
        let attack = self.params.at(P_ATTACK);
        let release = self.params.at(P_RELEASE);
        for voice in &mut self.voices {
            voice.envelope.set_adsr(attack, 0.0, 1.0, release);
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: f32) {
        let Some(assignment) = self.allocator.note_on(pitch, velocity) else {
            return;
        };
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        voice.pitch = f32::from(pitch);
        voice.velocity = finite_or(velocity, 0.0).clamp(0.0, 1.0);
        let delay = (self.rate / (2.0 * pitch_to_hz(voice.pitch + self.bend)))
            .clamp(2.0, voice.delay.len().saturating_sub(2) as f32);
        voice.delay_len = delay.floor() as usize;
        voice.delay_fraction = delay - voice.delay_len as f32;
        voice.write = 0;
        if !assignment.stolen {
            voice.delay.fill(0.0);
        }
        voice.envelope.trigger();
    }
}

fn pitch_to_hz(pitch: f32) -> f32 {
    440.0 * ((pitch - 69.0) / 12.0).exp2()
}

impl Parameterized for Clarinet {
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

impl SegmentRenderer for Clarinet {
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
            NoteEvent::AllNotesOff { .. } => {
                let mask = self.allocator.release_all();
                for index in mask {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.envelope.release();
                    }
                }
            }
            NoteEvent::AllSoundOff { .. } => {
                let mask = self.allocator.release_all();
                for index in mask {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.envelope.kill();
                    }
                }
            }
            NoteEvent::PitchBend { semitones, .. } => {
                self.bend = finite_or(semitones, 0.0).clamp(-MAX_BEND, MAX_BEND)
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
        let pressure = self.params.at(P_PRESSURE);
        let stiffness = self.params.at(P_REED).clamp(0.02, 1.0);
        let noise_amount = self.params.at(P_NOISE);
        let feedback = (0.996 - self.params.at(P_REED) * 0.08).clamp(0.88, 0.998);
        for voice in &mut self.voices {
            if !voice.envelope.is_active() {
                continue;
            }
            let delay = (self.rate / (2.0 * pitch_to_hz(voice.pitch + self.bend)))
                .clamp(2.0, voice.delay.len().saturating_sub(2) as f32);
            voice.delay_len = delay.floor() as usize;
            voice.delay_fraction = delay - voice.delay_len as f32;
        }
        let output = db_to_gain(self.params.at(P_LEVEL)) * self.channel_volume * 0.75;
        for sample in dst.iter_mut() {
            self.expression_current +=
                self.expression_coeff * (self.expression - self.expression_current);
            let wind = 0.35 + 0.65 * self.expression_current;
            for voice in &mut self.voices {
                if !voice.envelope.is_active() {
                    continue;
                }
                let env = voice.envelope.process();
                let read = (voice.write + voice.delay.len() - voice.delay_len) % voice.delay.len();
                let next_read = (read + voice.delay.len() - 1) % voice.delay.len();
                let bore = voice.delay[read] * (1.0 - voice.delay_fraction)
                    + voice.delay[next_read] * voice.delay_fraction;
                let mouth = pressure * voice.velocity * env * wind;
                // Reed flow is pressure-difference driven. Removing its slow mean prevents
                // the constant breath pressure from becoming a DC pressure in the bore.
                let reed_flow = (mouth - bore).max(0.0) * (1.0 - stiffness * bore.abs()).max(0.05);
                voice.flow_mean += 0.002 * (reed_flow - voice.flow_mean);
                let input = (reed_flow - voice.flow_mean) * 0.9
                    + voice.next_noise() * noise_amount * mouth * 0.08;
                // The open end inverts pressure; the negative round-trip feedback gives the
                // closed-open tube its odd-harmonic quarter-wave resonance.
                let next = (input - bore * feedback).clamp(-2.0, 2.0);
                voice.delay[voice.write] = next;
                voice.write += 1;
                if voice.write == voice.delay.len() {
                    voice.write = 0;
                }
                voice.last = bore;
                // A small mouthpiece observation keeps the attack present while the first
                // round-trip travels down the preallocated bore line.
                *sample += (bore * 0.7 + input * 0.18) * env * voice.velocity;
            }
            *sample *= output * self.expression_current;
        }
        for (slot, voice) in self.voices.iter().enumerate() {
            self.allocator
                .set_level(slot, voice.envelope.level() * voice.velocity);
            if voice.envelope.is_finished() {
                self.allocator.retire(slot);
            }
        }
    }
}

impl Instrument for Clarinet {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            Self::ID,
            "Physical Clarinet",
            "Closed-open cylindrical bore with nonlinear reed and breath noise",
            PluginCategory::Synth,
        )
    }
    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = crate::sample_rate_f32(ctx.sample_rate).clamp(8_000.0, 192_000.0);
        let max_delay = (self.rate / pitch_to_hz(0.0)).ceil() as usize + 2;
        self.voices.clear();
        self.voices.reserve(VOICES);
        for _ in 0..VOICES {
            let mut voice = Voice::new(self.rate);
            voice.prepare(self.rate, max_delay);
            self.voices.push(voice);
        }
        self.allocator.prepare(VOICES);
        self.bend = 0.0;
        self.channel_volume = 1.0;
        self.expression = 1.0;
        self.expression_current = 1.0;
        self.expression_coeff = (1.0 - (-1.0 / (0.020 * f64::from(self.rate))).exp()) as f32;
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
        for step in -24..=24 {
            let frequency = expected * 2.0f64.powf(step as f64 / 120.0);
            let amplitude = goertzel(samples, rate, frequency);
            if amplitude > best.1 {
                best = (frequency, amplitude);
            }
        }
        best.0
    }

    #[test]
    fn clarinet_has_stable_descriptor_and_sound() {
        let clarinet = Clarinet::new();
        assert_eq!(clarinet.descriptor().id, Clarinet::ID);
        assert_eq!(clarinet.parameters().len(), 6);
        let mut rig = Rig::new(Box::new(clarinet), 48_000.0, 256, 2);
        let rendered = rig.render(
            4096,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: 0.8,
            }],
        );
        assert!(peak(&rendered) > 0.001);
        assert!(rendered.iter().all(|sample| sample.is_finite()));
        assert!(rms(&rendered) > 0.0001);
    }

    #[test]
    fn low_and_high_rates_remain_finite() {
        for rate in [8_000.0, 192_000.0] {
            let mut rig = Rig::new(Box::new(Clarinet::new()), rate, 127, 1);
            let rendered = rig.render(
                2048,
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 24,
                    velocity: 1.0,
                }],
            );
            assert!(rendered.iter().all(|sample| sample.is_finite()));
        }
    }

    #[test]
    fn invalid_host_rates_are_bounded_before_allocating_the_bore() {
        for rate in [1.0, 0.0, f64::NAN, f64::INFINITY, 1.0e20] {
            let mut rig = Rig::new(Box::new(Clarinet::new()), rate, 128, 1);
            let audio = rig.render(
                1024,
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 0,
                    velocity: 0.8,
                }],
            );
            assert!(audio.iter().all(|sample| sample.is_finite()));
        }
    }

    #[test]
    fn prepared_callback_does_not_allocate() {
        let mut instrument = Clarinet::new();
        instrument.prepare(&PrepareContext::new(48_000.0, 128, 2));
        let mut buffer = AudioBuffer::stereo(128, 48_000.0);
        let context = ProcessContext::realtime(48_000.0, 128, 0, 120.0, true);
        let allocations = count_allocations(|| {
            for pitch in 48..72 {
                instrument.set_param_by_key("pressure", 0.6);
                instrument.process(
                    &[NoteEvent::NoteOn {
                        frame: 0,
                        pitch,
                        velocity: 0.8,
                    }],
                    &mut buffer,
                    &context,
                );
            }
            instrument.process(
                &[NoteEvent::AllSoundOff { frame: 0 }],
                &mut buffer,
                &context,
            );
        });
        assert_eq!(allocations, 0);
    }

    #[test]
    fn sustained_notes_are_tuned_and_have_odd_harmonic_tendency() {
        for (pitch, rate) in [
            (48u8, 24_000.0),
            (60, 48_000.0),
            (72, 24_000.0),
            (84, 48_000.0),
        ] {
            let mut rig = Rig::new(Box::new(Clarinet::new()), rate, 256, 1);
            let rendered = rig.render(
                (rate * 1.2) as usize,
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch,
                    velocity: 0.8,
                }],
            );
            let body = &rendered[(rate * 0.35) as usize..];
            let expected = pitch_to_hz(f32::from(pitch)) as f64;
            let measured = measured_pitch(body, rate, expected);
            assert!(
                (1200.0 * (measured / expected).log2()).abs() < 12.0,
                "pitch {pitch} measured {measured} expected {expected}"
            );
            let fundamental = goertzel(body, rate, expected);
            let third = goertzel(body, rate, expected * 3.0);
            assert!(
                third > fundamental * 0.05,
                "pitch {pitch} lacks odd harmonic energy"
            );
            let mean =
                body.iter().map(|sample| f64::from(*sample)).sum::<f64>() / body.len() as f64;
            assert!(
                mean.abs() < f64::from(rms(body)) * 0.2,
                "pitch {pitch} has DC fraction"
            );
        }
    }

    #[test]
    fn moderate_breath_pressure_keeps_the_bore_sounding() {
        let mut rig = Rig::new(Box::new(Clarinet::new()), 24_000.0, 256, 1);
        rig.set_param("pressure", 0.3);
        let rendered = rig.render(
            24_000,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: 0.8,
            }],
        );
        assert!(rms(&rendered[12_000..]) > 0.0005);
    }

    #[test]
    fn expression_changes_wind_without_changing_channel_volume() {
        let mut quiet = Rig::new(Box::new(Clarinet::new()), 24_000.0, 128, 1);
        let quiet_audio = quiet.render(
            12_000,
            &[
                NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 60,
                    velocity: 0.8,
                },
                NoteEvent::Controller {
                    frame: 4_000,
                    number: 11,
                    value: 0.0,
                },
            ],
        );
        let mut loud = Rig::new(Box::new(Clarinet::new()), 24_000.0, 128, 1);
        let loud_audio = loud.render(
            12_000,
            &[
                NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 60,
                    velocity: 0.8,
                },
                NoteEvent::Controller {
                    frame: 4_000,
                    number: 11,
                    value: 1.0,
                },
            ],
        );
        assert!(rms(&loud_audio[8_000..]) > rms(&quiet_audio[8_000..]) * 1.5);
        let mut full = Rig::new(Box::new(Clarinet::new()), 24_000.0, 128, 1);
        let full_audio = full.render(
            4_000,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: 0.8,
            }],
        );
        let mut cc7 = Rig::new(Box::new(Clarinet::new()), 24_000.0, 128, 1);
        let audio = cc7.render(
            4_000,
            &[
                NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 60,
                    velocity: 0.8,
                },
                NoteEvent::Controller {
                    frame: 0,
                    number: 7,
                    value: 0.5,
                },
            ],
        );
        assert!((rms(&audio[1_000..]) / rms(&full_audio[1_000..]) - 0.5).abs() < 0.02);
    }

    #[test]
    fn factory_register_and_extreme_controls_stay_finite() {
        for rate in [8_000.0, 48_000.0, 192_000.0] {
            for pitch in [0u8, 50, 72, 95, 127] {
                for velocity in [0.35, 0.65, 0.95] {
                    let mut rig = Rig::new(Box::new(Clarinet::new()), rate, 128, 1);
                    rig.set_param("pressure", velocity);
                    let rendered = rig.render(
                        512,
                        &[NoteEvent::NoteOn {
                            frame: 0,
                            pitch,
                            velocity,
                        }],
                    );
                    assert!(rendered.iter().all(|sample| sample.is_finite()));
                    assert!(peak(&rendered) < 4.0);
                }
            }
        }
    }

    #[test]
    fn bend_changes_a_sounding_note_and_reset_is_deterministic() {
        let mut first = Rig::new(Box::new(Clarinet::new()), 24_000.0, 128, 1);
        let baseline = first.render(
            12_000,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: 0.8,
            }],
        );
        first.instrument.reset();
        let reset = first.render(
            12_000,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 60,
                velocity: 0.8,
            }],
        );
        assert_eq!(baseline, reset);

        let mut bent = Rig::new(Box::new(Clarinet::new()), 24_000.0, 128, 1);
        let bent_audio = bent.render(
            12_000,
            &[
                NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 60,
                    velocity: 0.8,
                },
                NoteEvent::PitchBend {
                    frame: 4_000,
                    semitones: 2.0,
                },
            ],
        );
        let body = &bent_audio[6_000..];
        let expected = pitch_to_hz(62.0) as f64;
        let measured = measured_pitch(body, 24_000.0, expected);
        assert!((1200.0 * (measured / expected).log2()).abs() < 15.0);
    }

    #[test]
    fn event_render_is_block_size_independent_and_release_retires_voice() {
        let events = [
            NoteEvent::NoteOn {
                frame: 73,
                pitch: 60,
                velocity: 0.8,
            },
            NoteEvent::Controller {
                frame: 2_000,
                number: 11,
                value: 0.25,
            },
            NoteEvent::Controller {
                frame: 3_000,
                number: 7,
                value: 0.7,
            },
            NoteEvent::NoteOff {
                frame: 4_000,
                pitch: 60,
            },
        ];
        let mut small = Rig::new(Box::new(Clarinet::new()), 24_000.0, 128, 1);
        let mut large = Rig::new(Box::new(Clarinet::new()), 24_000.0, 512, 1);
        let a = small.render(12_000, &events);
        let b = large.render(12_000, &events);
        assert_eq!(a, b);
        assert_eq!(small.instrument.active_voices(), 0);
    }
}
