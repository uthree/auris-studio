//! Sample-free tin whistle based on a fipple jet and an open-open bore.
//!
//! This is a compact physical approximation: a turbulent jet is shaped by the pressure
//! returned from a fractional-delay bore. A damped first open-pipe mode selects the
//! fundamental inside the positive round trip. This reduced acoustic model omits the
//! full jet convection and tone-hole network; its tone is produced by jet feedback.

use auris_core::param::db_to_gain;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, Parameterized,
    PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_dsp::{Adsr, Biquad, BiquadCoefficients};

use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};
use crate::voice::VoiceAllocator;

const VOICES: usize = 16;
const P_PRESSURE: u32 = 0;
const P_JET: u32 = 1;
const P_NOISE: u32 = 2;
const P_ATTACK: u32 = 3;
const P_RELEASE: u32 = 4;
const P_LEVEL: u32 = 5;
const MAX_BEND: f32 = 24.0;

#[derive(Clone, Debug)]
struct Voice {
    delay: Vec<f32>,
    write: usize,
    length: usize,
    fraction: f32,
    pitch: f32,
    velocity: f32,
    envelope: Adsr,
    noise: u32,
    bore_mode: Biquad,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Rig, count_allocations, goertzel, peak, rms};

    fn measured_pitch(samples: &[f32], rate: f64, expected: f64) -> f64 {
        let mut best = (expected, 0.0);
        for step in -30..=30 {
            let frequency = expected * 2.0f64.powf(step as f64 / 120.0);
            let amplitude = goertzel(samples, rate, frequency);
            if amplitude > best.1 {
                best = (frequency, amplitude);
            }
        }
        best.0
    }

    #[test]
    fn whistle_sustains_with_a_dominant_fundamental_from_d5_to_d6() {
        for rate in [24_000.0, 48_000.0] {
            for pitch in 74u8..=86 {
                for velocity in [0.35, 0.65, 0.95] {
                    let mut rig = Rig::new(Box::new(TinWhistle::new()), rate, 256, 1);
                    let audio = rig.render(
                        (rate * 0.8) as usize,
                        &[NoteEvent::NoteOn {
                            frame: 0,
                            pitch,
                            velocity,
                        }],
                    );
                    let body = &audio[(rate * 0.3) as usize..];
                    let expected = pitch_to_hz(f32::from(pitch)) as f64;
                    let measured = measured_pitch(body, rate, expected);
                    assert!(
                        (1200.0 * (measured / expected).log2()).abs() < 14.0,
                        "pitch {pitch}, measured {measured}, expected {expected}"
                    );
                    assert!(rms(body) > 0.0003, "pitch {pitch} became silent");
                    assert!(body.iter().all(|sample| sample.is_finite()));
                    let fundamental = goertzel(body, rate, expected);
                    assert!(
                        fundamental > 1.0e-4,
                        "pitch {pitch} has no measurable fundamental"
                    );
                    let fundamental = goertzel(body, rate, measured);
                    for harmonic in [2.0, 3.0, 4.0] {
                        assert!(
                            goertzel(body, rate, measured * harmonic) < fundamental * 0.5,
                            "pitch {pitch} velocity {velocity}: harmonic {harmonic} dominates"
                        );
                    }
                    let mean = body.iter().map(|sample| f64::from(*sample)).sum::<f64>()
                        / body.len() as f64;
                    assert!(
                        mean.abs() < f64::from(rms(body)) * 0.25,
                        "pitch {pitch} has DC"
                    );
                    let low = goertzel(body, rate, 22.0);
                    assert!(
                        low < fundamental * 0.75,
                        "pitch {pitch} velocity {velocity} fell into low-frequency mode"
                    );
                }
            }
        }
    }

    #[test]
    fn a_closed_air_supply_cannot_excite_a_new_bore() {
        let mut rig = Rig::new(Box::new(TinWhistle::new()), 48_000.0, 256, 1);
        rig.set_param("pressure", 0.0);
        let audio = rig.render(
            48_000,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 74,
                velocity: 0.8,
            }],
        );
        assert_eq!(peak(&audio), 0.0);
    }

    #[test]
    fn expression_and_breath_controls_change_level_without_nan() {
        let mut quiet = Rig::new(Box::new(TinWhistle::new()), 24_000.0, 128, 1);
        quiet.set_param("pressure", 0.2);
        let quiet_audio = quiet.render(
            12_000,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 86,
                velocity: 0.8,
            }],
        );
        let mut loud = Rig::new(Box::new(TinWhistle::new()), 24_000.0, 128, 1);
        loud.set_param("pressure", 0.8);
        let loud_audio = loud.render(
            12_000,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 86,
                velocity: 0.8,
            }],
        );
        assert!(rms(&loud_audio[8_000..]) > rms(&quiet_audio[8_000..]) * 1.2);
        let mut modulated = Rig::new(Box::new(TinWhistle::new()), 24_000.0, 128, 1);
        let audio = modulated.render(
            12_000,
            &[
                NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 86,
                    velocity: 0.8,
                },
                NoteEvent::Controller {
                    frame: 4_000,
                    number: 11,
                    value: 0.0,
                },
            ],
        );
        assert!(rms(&audio[8_000..]) < rms(&loud_audio[8_000..]) * 0.2);
        assert!(audio.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn callback_is_allocation_free_and_block_size_independent() {
        let events = [
            NoteEvent::NoteOn {
                frame: 73,
                pitch: 86,
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
                pitch: 86,
            },
        ];
        let mut small = Rig::new(Box::new(TinWhistle::new()), 24_000.0, 128, 1);
        let mut large = Rig::new(Box::new(TinWhistle::new()), 24_000.0, 512, 1);
        assert_eq!(small.render(12_000, &events), large.render(12_000, &events));
        assert_eq!(small.instrument.active_voices(), 0);
        let mut instrument = TinWhistle::new();
        instrument.prepare(&PrepareContext::new(48_000.0, 128, 2));
        let mut buffer = AudioBuffer::stereo(128, 48_000.0);
        let context = ProcessContext::realtime(48_000.0, 128, 0, 120.0, true);
        let allocations = count_allocations(|| {
            instrument.process(
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 86,
                    velocity: 0.8,
                }],
                &mut buffer,
                &context,
            );
            instrument.process(
                &[NoteEvent::AllSoundOff { frame: 0 }],
                &mut buffer,
                &context,
            );
        });
        assert_eq!(allocations, 0);
    }

    #[test]
    fn reset_is_deterministic_and_extreme_rates_remain_bounded() {
        let mut rig = Rig::new(Box::new(TinWhistle::new()), 48_000.0, 128, 1);
        let first = rig.render(
            4_096,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 86,
                velocity: 0.8,
            }],
        );
        rig.instrument.reset();
        let second = rig.render(
            4_096,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 86,
                velocity: 0.8,
            }],
        );
        assert_eq!(first, second);
        for rate in [8_000.0, 192_000.0] {
            let mut edge = Rig::new(Box::new(TinWhistle::new()), rate, 128, 1);
            let audio = edge.render(
                1_024,
                &[NoteEvent::NoteOn {
                    frame: 0,
                    pitch: 127,
                    velocity: 1.0,
                }],
            );
            assert!(audio.iter().all(|sample| sample.is_finite()));
            assert!(peak(&audio) < 4.0);
        }
    }
}

impl Voice {
    fn new(rate: f32) -> Self {
        let mut envelope = Adsr::new();
        envelope.set_sample_rate(rate);
        Self {
            delay: Vec::new(),
            write: 0,
            length: 2,
            fraction: 0.0,
            pitch: 62.0,
            velocity: 0.0,
            envelope,
            noise: 0x9e37_79b9,
            bore_mode: Biquad::new(BiquadCoefficients::identity()),
        }
    }

    fn prepare(&mut self, rate: f32, max_delay: usize) {
        self.delay.resize(max_delay.max(4), 0.0);
        self.envelope.set_sample_rate(rate);
    }

    fn reset(&mut self) {
        self.delay.fill(0.0);
        self.write = 0;
        self.length = 2;
        self.fraction = 0.0;
        self.velocity = 0.0;
        self.noise = 0x9e37_79b9;
        self.bore_mode.reset();
        self.envelope.silence();
    }

    fn noise(&mut self) -> f32 {
        self.noise = self
            .noise
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        (self.noise as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// A polyphonic sample-free tin whistle with a fipple jet and open-open bore.
#[derive(Clone, Debug)]
pub struct TinWhistle {
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

impl Default for TinWhistle {
    fn default() -> Self {
        Self::new()
    }
}

impl TinWhistle {
    /// Stable plugin id stored in project files.
    pub const ID: &'static str = "auris.physical.tin_whistle";

    /// Builds a medium-register tin whistle patch.
    pub fn new() -> Self {
        let mut whistle = Self {
            params: ParamBank::new(vec![
                ParamDescriptor::percent(P_PRESSURE, "pressure", "Breath Pressure", 0.3353),
                ParamDescriptor::percent(P_JET, "jet_shape", "Jet Shape", 0.11913),
                ParamDescriptor::percent(P_NOISE, "breath_noise", "Breath Noise", 0.10243),
                ParamDescriptor::new(P_ATTACK, "attack", "Attack", 0.001, 0.3, 0.012)
                    .with_unit(ParamUnit::Seconds),
                ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 4.0, 0.18)
                    .with_unit(ParamUnit::Seconds),
                ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, -9.0),
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
        whistle.refresh();
        whistle
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
        // A positive open-pipe round trip spans a full acoustic period.
        let delay = (self.rate / pitch_to_hz(voice.pitch + self.bend))
            .clamp(2.0, voice.delay.len().saturating_sub(2) as f32);
        voice.length = delay.floor() as usize;
        voice.fraction = delay - voice.length as f32;
        voice.write = 0;
        if !assignment.stolen {
            voice.delay.fill(0.0);
        }
        voice.bore_mode.reset();
        voice.envelope.trigger();
    }
}

fn pitch_to_hz(pitch: f32) -> f32 {
    440.0 * ((pitch - 69.0) / 12.0).exp2()
}

impl Parameterized for TinWhistle {
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

impl SegmentRenderer for TinWhistle {
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
        let pressure = self.params.at(P_PRESSURE).clamp(0.0, 1.0);
        let jet_shape = self.params.at(P_JET).clamp(0.0, 1.0);
        let noise_amount = self.params.at(P_NOISE).clamp(0.0, 1.0);
        for voice in &mut self.voices {
            if !voice.envelope.is_active() {
                continue;
            }
            let frequency = pitch_to_hz(voice.pitch + self.bend).min(self.rate * 0.4);
            // The first acoustic mode has unit gain and zero phase at its centre.
            // Other round-trip modes lose energy, including the spurious DC mode.
            voice
                .bore_mode
                .set_coefficients(BiquadCoefficients::bandpass(
                    f64::from(self.rate),
                    frequency,
                    3.0,
                ));
            let delay =
                (self.rate / frequency).clamp(2.0, voice.delay.len().saturating_sub(2) as f32);
            voice.length = delay.floor() as usize;
            voice.fraction = delay - voice.length as f32;
        }
        let gain = db_to_gain(self.params.at(P_LEVEL)) * self.channel_volume * 0.8;
        for sample in dst.iter_mut() {
            self.expression_current +=
                self.expression_coeff * (self.expression - self.expression_current);
            let wind = (0.25 + 0.75 * self.expression_current) * pressure;
            for voice in &mut self.voices {
                if !voice.envelope.is_active() {
                    continue;
                }
                let env = voice.envelope.process();
                let read = (voice.write + voice.delay.len() - voice.length) % voice.delay.len();
                let next = (read + voice.delay.len() - 1) % voice.delay.len();
                let bore =
                    voice.delay[read] * (1.0 - voice.fraction) + voice.delay[next] * voice.fraction;
                let acoustic = voice.bore_mode.process_sample(bore);
                // A fipple jet switches sign as its centreline is displaced by bore pressure.
                // The small noise term supplies turbulent onset without becoming the tone source.
                let jet_gain = wind * (0.6 + 0.4 * voice.velocity) * env * (1.5 + jet_shape * 1.5);
                let argument = acoustic * jet_gain + voice.noise() * noise_amount * wind * 0.08;
                // The labium phase makes the acoustic pressure reinforce the jet's alternating
                // volume flow. Keeping the slope just above unity gives a bounded limit cycle.
                let jet = argument.tanh() * 0.78;
                let wave = (jet + acoustic * (0.999 - jet_shape * 0.01)).clamp(-1.5, 1.5);
                voice.delay[voice.write] = wave;
                voice.write = (voice.write + 1) % voice.delay.len();
                // Observe acoustic pressure and a small amount of nonlinear jet radiation.
                *sample += (acoustic * 0.85 + jet * 0.15) * env * voice.velocity;
            }
            // Breath pressure also controls radiated amplitude after the jet loop saturates;
            // otherwise a limit-cycle whistle would make pressure changes inaudible.
            *sample *= gain * (0.35 + 0.65 * pressure) * self.expression_current;
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

impl Instrument for TinWhistle {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            Self::ID,
            "Physical Tin Whistle",
            "Fipple jet driving an open-open metal bore",
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
