//! A complete synthesized kit in one instrument and one voice pool.

use auris_core::param::db_to_gain;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, Parameterized, PluginCategory,
    PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_dsp::{Adsr, Biquad, BiquadCoefficients};

use crate::oscillator::{Oscillator, Waveform};
use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};
use crate::voice::VoiceAllocator;

const VOICE_COUNT: usize = 24;
const PAD_COUNT: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pad {
    Kick,
    Snare,
    ClosedHat,
    OpenHat,
    Crash,
    Ride,
    Tom,
}

impl Pad {
    const ALL: [Self; PAD_COUNT] = [
        Self::Kick,
        Self::Snare,
        Self::ClosedHat,
        Self::OpenHat,
        Self::Crash,
        Self::Ride,
        Self::Tom,
    ];

    fn at_key(key: u8) -> Option<Self> {
        Some(match key {
            35 | 36 => Self::Kick,
            37..=40 => Self::Snare,
            42 | 44 => Self::ClosedHat,
            46 => Self::OpenHat,
            49 | 52 | 55 | 57 => Self::Crash,
            51 | 53 | 59 => Self::Ride,
            41 | 43 | 45 | 47 | 48 | 50 => Self::Tom,
            _ => return None,
        })
    }

    fn is_hat(self) -> bool {
        matches!(self, Self::ClosedHat | Self::OpenHat)
    }

    fn profile(self) -> Profile {
        // The head of a kick is tonal, the snare adds a noise wire, and cymbals have energy
        // above both. These are different excitations, not transpositions of one noise patch.
        match self {
            Self::Kick => Profile {
                tone: 55.0,
                sweep: 130.0,
                sweep_time: 0.025,
                decay: 0.42,
                body: 0.95,
                noise: 0.035,
                highpass: 1_000.0,
                lowpass: 4_000.0,
                overtone: 0.0,
                ratio: 1.0,
            },
            Self::Snare => Profile {
                tone: 185.0,
                sweep: 65.0,
                sweep_time: 0.015,
                decay: 0.32,
                body: 0.35,
                noise: 1.0,
                highpass: 650.0,
                lowpass: 6_000.0,
                overtone: 0.32,
                ratio: 1.63,
            },
            Self::ClosedHat | Self::OpenHat => Profile {
                tone: 8_200.0,
                sweep: 0.0,
                sweep_time: 0.01,
                decay: if self == Self::ClosedHat { 0.10 } else { 0.85 },
                body: 0.0,
                noise: 0.68,
                highpass: 7_000.0,
                lowpass: 18_000.0,
                overtone: 0.0,
                ratio: 1.0,
            },
            Self::Crash => Profile {
                tone: 3_100.0,
                sweep: 0.0,
                sweep_time: 0.01,
                decay: 2.4,
                body: 0.09,
                noise: 0.85,
                highpass: 2_500.0,
                lowpass: 16_000.0,
                overtone: 0.75,
                ratio: 1.413,
            },
            Self::Ride => Profile {
                tone: 2_650.0,
                sweep: 0.0,
                sweep_time: 0.01,
                decay: 1.25,
                body: 0.36,
                noise: 0.38,
                highpass: 3_500.0,
                lowpass: 15_000.0,
                overtone: 0.6,
                ratio: 1.731,
            },
            Self::Tom => Profile {
                tone: 130.0,
                sweep: 80.0,
                sweep_time: 0.035,
                decay: 0.55,
                body: 0.85,
                noise: 0.075,
                highpass: 500.0,
                lowpass: 3_000.0,
                overtone: 0.18,
                ratio: 1.59,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Profile {
    tone: f32,
    sweep: f32,
    sweep_time: f32,
    decay: f32,
    body: f32,
    noise: f32,
    highpass: f32,
    lowpass: f32,
    overtone: f32,
    ratio: f32,
}

#[derive(Clone, Debug)]
struct Hit {
    pad: Pad,
    profile: Profile,
    amplitude: Adsr,
    oscillator: Oscillator,
    overtone: Oscillator,
    highpass: Biquad,
    lowpass: Biquad,
    noise_state: u32,
    sweep: f32,
    sweep_multiplier: f32,
    body_level: f32,
    body_multiplier: f32,
    velocity: f32,
}

impl Hit {
    fn prepared(pad: Pad, sample_rate: f32) -> Self {
        let profile = pad.profile();
        let mut amplitude = Adsr::new();
        amplitude.set_sample_rate(sample_rate);
        amplitude.set_adsr(0.001, profile.decay, 0.0, profile.decay);
        let mut oscillator = Oscillator::new();
        oscillator.set_sample_rate(sample_rate);
        Self {
            pad,
            profile,
            amplitude,
            overtone: oscillator.clone(),
            oscillator,
            highpass: Biquad::new(BiquadCoefficients::highpass(
                sample_rate as f64,
                profile.highpass,
                std::f32::consts::FRAC_1_SQRT_2,
            )),
            lowpass: Biquad::new(BiquadCoefficients::lowpass(
                sample_rate as f64,
                profile.lowpass,
                std::f32::consts::FRAC_1_SQRT_2,
            )),
            noise_state: 1,
            sweep: profile.sweep,
            sweep_multiplier: (-1.0 / (sample_rate * profile.sweep_time)).exp(),
            body_level: 1.0,
            // The head loses its pitched ring before the snare wires stop buzzing. Giving
            // both the same envelope would make a bass-heavy pitched drum with a little hiss.
            body_multiplier: if pad == Pad::Snare {
                (-1.0 / (sample_rate * 0.012)).exp()
            } else {
                1.0
            },
            velocity: 0.0,
        }
    }

    fn next(&mut self) -> f32 {
        if !self.amplitude.is_active() {
            return 0.0;
        }
        let frequency = self.profile.tone + self.sweep;
        self.sweep *= self.sweep_multiplier;
        self.oscillator.set_frequency(frequency);
        self.overtone.set_frequency(frequency * self.profile.ratio);
        let body = self.oscillator.next(Waveform::Sine)
            + self.profile.overtone * self.overtone.next(Waveform::Sine);
        self.body_level *= self.body_multiplier;
        // One independent white-noise draw per sample. The clock is the sample rate, so a
        // cymbal's high-pass receives broadband excitation even when its pitched body is low.
        self.noise_state ^= self.noise_state << 13;
        self.noise_state ^= self.noise_state >> 17;
        self.noise_state ^= self.noise_state << 5;
        let noise = (self.noise_state >> 8) as f32 / 8_388_608.0 - 1.0;
        let noise = self
            .lowpass
            .process_sample(self.highpass.process_sample(noise));
        (body * self.profile.body * self.body_level + noise * self.profile.noise)
            * self.amplitude.process()
            * self.velocity
    }
}

#[derive(Clone, Debug)]
struct KitVoice {
    current: Hit,
    // A stolen hit retains its oscillators and filters during the envelope's de-click ramp.
    retiring: Hit,
}

/// A synthesized drum kit with tonal kicks, noisy snares and bright cymbals.
///
/// General MIDI percussion keys select pads; unassigned keys are silent. Every pad shares the
/// voice pool, and either hat closes previous hats without cutting kicks, snares or cymbals.
/// Hits ring through note-off. All-sound-off fades them using the common ADSR de-click ramp.
///
/// All filters and envelopes are prepared off the audio thread. Starting a hit copies a fixed
/// prepared voice; processing, choking, stealing and reset allocate nothing and take no locks.
#[derive(Clone, Debug)]
pub struct DrumKit {
    params: ParamBank,
    templates: Option<[Hit; PAD_COUNT]>,
    voices: Vec<KitVoice>,
    allocator: VoiceAllocator,
    strike: u32,
    gain: f32,
}

impl Default for DrumKit {
    fn default() -> Self {
        Self::new()
    }
}

impl DrumKit {
    /// Stable plugin id stored in project files.
    pub const ID: &'static str = "auris.synth.drumkit";

    /// A kit with its default pad balance and a six-decibel output attenuation.
    pub fn new() -> Self {
        let params = ParamBank::new(vec![ParamDescriptor::decibels(
            0u32, "level", "Level", -60.0, 6.0, -6.0,
        )]);
        let gain = db_to_gain(params.at(0));
        Self {
            params,
            templates: None,
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            strike: 0,
            gain,
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: f32) {
        let Some(pad) = Pad::at_key(pitch) else {
            return;
        };
        let velocity = finite_or(velocity, 0.0).clamp(0.0, 1.0);
        let Some(templates) = self.templates.as_ref().filter(|_| velocity > 0.0) else {
            return;
        };
        let Some(template) = templates.get(pad as usize) else {
            return;
        };
        if pad.is_hat() {
            for voice in &mut self.voices {
                for hit in [&mut voice.current, &mut voice.retiring] {
                    if hit.pad.is_hat() {
                        hit.amplitude.kill();
                    }
                }
            }
        }
        let Some(assignment) = self.allocator.note_on(pitch, velocity) else {
            return;
        };
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        voice.retiring = voice.current.clone();
        voice.retiring.amplitude.kill();
        voice.current = template.clone();
        self.strike = self.strike.wrapping_add(1);
        voice.current.noise_state = self.strike.wrapping_mul(0x9e37_79b9) | 1;
        voice.current.velocity = velocity;
        if pad == Pad::Tom {
            let ratio = ((f32::from(pitch) - 47.0) / 12.0).exp2();
            voice.current.profile.tone *= ratio;
            voice.current.sweep *= ratio;
        }
        voice.current.amplitude.trigger();
    }
}

impl Parameterized for DrumKit {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }

    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }

    fn set_param(&mut self, id: ParamId, value: f32) {
        if self.params.set(id, value) {
            self.gain = db_to_gain(self.params.at(0));
        }
    }
}

impl SegmentRenderer for DrumKit {
    fn handle_event(&mut self, event: &NoteEvent) {
        match *event {
            NoteEvent::NoteOn {
                pitch, velocity, ..
            } => self.note_on(pitch, velocity),
            NoteEvent::NoteOff { pitch, .. } => {
                self.allocator.note_off(pitch);
            }
            NoteEvent::AllNotesOff { .. } => {
                self.allocator.release_all();
            }
            NoteEvent::AllSoundOff { .. } => {
                self.allocator.release_all();
                for voice in &mut self.voices {
                    voice.current.amplitude.kill();
                    voice.retiring.amplitude.kill();
                }
            }
            NoteEvent::PitchBend { .. } | NoteEvent::Controller { .. } => {}
        }
    }

    fn render_segment(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        let Some(mono) = out.channels_mut().first_mut() else {
            return;
        };
        let Some(dst) = mono.get_mut(start..end) else {
            return;
        };
        dst.fill(0.0);
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if !voice.current.amplitude.is_active() && !voice.retiring.amplitude.is_active() {
                continue;
            }
            for sample in dst.iter_mut() {
                *sample += (voice.current.next() + voice.retiring.next()) * self.gain;
            }
            self.allocator.set_level(
                index,
                voice.current.amplitude.level() * voice.current.velocity,
            );
            if voice.current.amplitude.is_finished() && voice.retiring.amplitude.is_finished() {
                self.allocator.retire(index);
            }
        }
    }
}

impl Instrument for DrumKit {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            Self::ID,
            "Drum Kit",
            "A complete synthesized percussion kit with shared hat choking",
            PluginCategory::Drum,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        let sample_rate = crate::sample_rate_f32(ctx.sample_rate);
        let templates = Pad::ALL.map(|pad| Hit::prepared(pad, sample_rate));
        self.voices = (0..VOICE_COUNT)
            .map(|_| KitVoice {
                current: templates[0].clone(),
                retiring: templates[0].clone(),
            })
            .collect();
        self.templates = Some(templates);
        self.allocator.prepare(VOICE_COUNT);
        self.strike = 0;
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.current.amplitude.silence();
            voice.retiring.amplitude.silence();
        }
        self.allocator.clear();
        self.strike = 0;
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = ctx.block_frames.min(out.frame_count());
        render_segments(self, events, out, frames);
        spread_to_all_channels(out, frames);
    }

    fn active_voices(&self) -> usize {
        self.allocator.active_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Rig, band_amplitude, peak, rms};

    fn hit(frame: u32, pitch: u8) -> NoteEvent {
        NoteEvent::NoteOn {
            frame,
            pitch,
            velocity: 1.0,
        }
    }

    fn rig(block: usize) -> Rig {
        Rig::new(Box::new(DrumKit::new()), 48_000.0, block, 2)
    }

    fn sound(pitch: u8) -> Vec<f32> {
        rig(512).render(48_000, &[hit(0, pitch)])
    }

    #[test]
    fn kicks_have_a_low_body_and_cymbals_have_high_frequency_energy() {
        let kick = sound(36);
        let snare = sound(38);
        let hat = sound(42);
        let low = |samples: &[f32]| band_amplitude(samples, 48_000.0, 65.0);
        let mid = |samples: &[f32]| band_amplitude(samples, 48_000.0, 2_000.0);
        let high = |samples: &[f32]| band_amplitude(samples, 48_000.0, 10_000.0);
        assert!(low(&kick) > high(&kick) * 100.0);
        assert!(mid(&snare) > high(&snare) * 2.0);
        assert!(high(&hat) > mid(&hat) * 8.0);
        assert!(mid(&snare) / low(&snare) > mid(&kick) / low(&kick) * 20.0);
        let wire = &snare[1_200..9_600];
        assert!(
            band_amplitude(wire, 48_000.0, 2_000.0) > band_amplitude(wire, 48_000.0, 185.0) * 2.0,
            "the snare's pitched head outlasted its noise wires"
        );
    }

    #[test]
    fn each_pad_has_an_audible_attack_and_a_finite_one_shot_tail() {
        for key in [36, 38, 42, 46, 49, 51, 47] {
            let mut rig = rig(512);
            let audio = rig.render(144_000, &[hit(0, key)]);
            assert!(peak(&audio[..4_800]) > 0.08, "key {key} is too quiet");
            assert!(audio.iter().all(|sample| sample.is_finite()));
            assert_eq!(peak(&audio[132_000..]), 0.0, "key {key} did not decay");
            assert_eq!(rig.instrument.active_voices(), 0);
        }
        assert!(rms(&sound(46)[9_600..14_400]) > rms(&sound(42)[9_600..14_400]) + 0.001);
        assert!(rms(&sound(49)[24_000..28_800]) > 0.001);
    }

    #[test]
    fn a_closed_hat_chokes_an_open_hat_but_leaves_a_crash_ringing() {
        let open = rig(512).render(24_000, &[hit(0, 46)]);
        let closed = rig(512).render(24_000, &[hit(0, 46), hit(4_800, 42)]);
        assert!(rms(&open[9_600..]) > 0.001);
        assert_eq!(peak(&closed[10_000..]), 0.0);
        let cymbal = rig(512).render(24_000, &[hit(0, 49), hit(4_800, 42)]);
        assert!(rms(&cymbal[9_600..]) > 0.01);
    }

    #[test]
    fn unassigned_keys_and_zero_velocity_do_not_claim_a_voice() {
        let mut rig = rig(512);
        let mut zero = hit(0, 36);
        if let NoteEvent::NoteOn { velocity, .. } = &mut zero {
            *velocity = 0.0;
        }
        let output = rig.render(512, &[hit(0, 0), hit(0, 60), hit(0, 127), zero]);
        assert_eq!(peak(&output), 0.0);
        assert_eq!(rig.instrument.active_voices(), 0);
    }

    #[test]
    fn timing_reset_and_block_sizes_do_not_change_a_groove() {
        let events = [hit(101, 36), hit(333, 46), hit(3_201, 38), hit(5_119, 42)];
        let expected = rig(512).render(16_000, &events);
        assert_eq!(peak(&expected[..101]), 0.0);
        assert!(peak(&expected[101..]) > 0.1);
        assert_eq!(expected, rig(127).render(16_000, &events));
        let mut again = rig(512);
        again.render(1_000, &[hit(0, 49)]);
        again.instrument.reset();
        assert_eq!(expected, again.render(16_000, &events));
    }

    #[test]
    fn note_off_keeps_a_hit_but_all_sound_off_fades_it() {
        let expected = sound(49);
        let released = rig(512).render(
            48_000,
            &[
                hit(0, 49),
                NoteEvent::NoteOff {
                    frame: 100,
                    pitch: 49,
                },
            ],
        );
        assert_eq!(expected, released);
        let stopped = rig(512).render(
            48_000,
            &[hit(0, 49), NoteEvent::AllSoundOff { frame: 4_800 }],
        );
        assert!(peak(&stopped[..4_800]) > 0.1);
        assert_eq!(peak(&stopped[5_000..]), 0.0);
    }

    #[test]
    fn velocity_scales_amplitude_without_moving_the_pad() {
        let loud = sound(38);
        let quiet = rig(512).render(
            48_000,
            &[NoteEvent::NoteOn {
                frame: 0,
                pitch: 38,
                velocity: 0.5,
            }],
        );
        assert!(
            loud.iter()
                .zip(&quiet)
                .all(|(a, b)| (*a * 0.5 - *b).abs() < 1e-7)
        );
    }

    #[test]
    fn dense_chokes_voice_steals_and_reset_allocate_nothing() {
        let mut kit = DrumKit::new();
        kit.prepare(&PrepareContext::new(48_000.0, 512, 2));
        let mut output = AudioBuffer::stereo(512, 48_000.0);
        let context = ProcessContext::realtime(48_000.0, 512, 0, 120.0, true);
        let events: [NoteEvent; 80] =
            std::array::from_fn(|index| hit(index as u32 * 5, [36, 38, 46, 49, 42][index % 5]));
        let allocations = crate::test_support::count_allocations(|| {
            kit.process(&events, &mut output, &context);
            kit.process(
                &[NoteEvent::AllSoundOff { frame: 0 }],
                &mut output,
                &context,
            );
            kit.reset();
        });
        assert_eq!(allocations, 0);
        assert!(output.channel(0).iter().all(|sample| sample.is_finite()));
        assert_eq!(kit.active_voices(), 0);
    }
}
