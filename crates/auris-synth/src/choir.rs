//! A wordless ensemble instrument with glottal excitation and physical vocal tracts.
//!
//! Each played note drives four eight-section Kelly–Lochbaum tube waveguides. Designed
//! area profiles morph between rounded "oo", open "ah" and front "ee" vowels. The source
//! is prescribed band-limited glottal flow, with independent breath noise; the acoustic
//! tract is a reduced physical model, rather than a simulation of vocal-fold biomechanics.
//! Ensemble variation separates pitch, vibrato phase/rate and tract length. All storage
//! is prepared off the audio thread, and identical input gives identical stereo output.

use std::f32::consts::TAU;

use auris_core::param::db_to_gain;
use auris_core::plugin::pitch_to_hz;
use auris_core::{
    AudioBuffer, CC_EXPRESSION, CC_MODULATION, Instrument, NoteEvent, ParamDescriptor, ParamId,
    ParamUnit, ParamValueCurve, Parameterized, PluginCategory, PluginDescriptor, PrepareContext,
    ProcessContext,
};
use auris_dsp::Adsr;

use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments};
use crate::voice::VoiceAllocator;

mod source;
mod tract;

#[cfg(test)]
mod tests;

use source::GlottalTables;
use tract::Tract;

const VOICE_COUNT: usize = 16;
const SINGERS: usize = 4;
const P_VOWEL: u32 = 0;
const P_SIZE: u32 = 1;
const P_ENSEMBLE: u32 = 2;
const P_BREATH: u32 = 3;
const P_TONE: u32 = 4;
const P_VIBRATO: u32 = 5;
const P_WIDTH: u32 = 6;
const P_ATTACK: u32 = 7;
const P_RELEASE: u32 = 8;
const P_LEVEL: u32 = 9;

const DETUNE: [f32; SINGERS] = [-0.18, 0.16, -0.06, 0.08];
const LENGTH_VARIATION: [f32; SINGERS] = [-0.035, 0.025, -0.015, 0.035];
const PAN: [f32; SINGERS] = [-0.9, -0.3, 0.3, 0.9];

struct Singer {
    tract: Tract,
    phase: f32,
    vibrato_phase: f32,
    drift_phase: f32,
    noise: u32,
    source_memory: f32,
    width: f32,
}

impl Singer {
    fn new(sample_rate: f32) -> Self {
        Self {
            tract: Tract::new(sample_rate),
            phase: 0.0,
            vibrato_phase: 0.0,
            drift_phase: 0.0,
            noise: 1,
            source_memory: 0.0,
            width: 0.0,
        }
    }

    fn reset(&mut self, index: usize, ensemble: f32, width: f32) {
        self.tract.reset();
        self.phase = index as f32 * 0.237 * ensemble;
        self.vibrato_phase = index as f32 * 0.193 * ensemble;
        self.drift_phase = index as f32 * 0.211 * ensemble;
        self.noise = 0x9e37_79b9_u32.wrapping_mul(index as u32 + 1);
        self.source_memory = 0.0;
        self.width = width;
    }

    fn next(&mut self, index: usize, frequency: f32, tables: &GlottalTables, sound: &Sound) -> f32 {
        let vibrato = (TAU * self.vibrato_phase).sin() * sound.vibrato;
        let drift = (TAU * self.drift_phase).sin() * 0.04 * sound.ensemble;
        let pitch_offset = DETUNE[index] * sound.ensemble + vibrato + drift;
        let frequency = (frequency * (pitch_offset / 12.0).exp2()).min(sound.sample_rate * 0.2);
        let pulse = tables.sample(self.phase, frequency, sound.sample_rate);
        self.phase = (self.phase + frequency / sound.sample_rate).fract();
        let vibrato_rate = 5.1 + (index as f32 - 1.5) * 0.31 * sound.ensemble;
        self.vibrato_phase = (self.vibrato_phase + vibrato_rate / sound.sample_rate).fract();
        self.drift_phase =
            (self.drift_phase + (0.19 + index as f32 * 0.047) / sound.sample_rate).fract();

        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        let noise = self.noise as i32 as f32 / 2_147_483_648.0;
        self.source_memory += sound.source_smoothing * (pulse - self.source_memory);
        self.width += (sound.width - self.width) * sound.smoothing;
        self.tract
            .next(self.source_memory + noise * sound.breath * 0.12)
    }
}

struct ChoirVoice {
    singers: [Singer; SINGERS],
    envelope: Adsr,
    pitch: f32,
    velocity: f32,
}

impl ChoirVoice {
    fn new(sample_rate: f32) -> Self {
        let mut envelope = Adsr::new();
        envelope.set_sample_rate(sample_rate);
        Self {
            singers: std::array::from_fn(|_| Singer::new(sample_rate)),
            envelope,
            pitch: 69.0,
            velocity: 0.0,
        }
    }
}

struct Sound {
    sample_rate: f32,
    ensemble: f32,
    breath: f32,
    vibrato: f32,
    width: f32,
    source_smoothing: f32,
    smoothing: f32,
}

/// A polyphonic, stereo physical vocal-tract ensemble played as an ordinary instrument.
pub struct Choir {
    params: ParamBank,
    voices: Vec<ChoirVoice>,
    allocator: VoiceAllocator,
    tables: GlottalTables,
    sound: Sound,
    bend: f32,
    modulation: f32,
    channel_volume: f32,
    expression: f32,
    output_gain: f32,
    target_gain: f32,
}

impl Default for Choir {
    fn default() -> Self {
        Self::new()
    }
}

impl Choir {
    /// Stable plugin ID stored in project files.
    pub const ID: &'static str = "auris.physical.choir";

    /// Creates a gentle "ah" ensemble with four singers per note and sixteen-note polyphony.
    pub fn new() -> Self {
        let mut choir = Self {
            params: ParamBank::new(descriptors()),
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            tables: GlottalTables::new(),
            sound: Sound {
                sample_rate: 48_000.0,
                ensemble: 0.0,
                breath: 0.0,
                vibrato: 0.0,
                width: 0.0,
                source_smoothing: 1.0,
                smoothing: 1.0,
            },
            bend: 0.0,
            modulation: 0.0,
            channel_volume: 1.0,
            expression: 1.0,
            output_gain: 1.0,
            target_gain: 1.0,
        };
        choir.refresh();
        choir.output_gain = choir.target_gain;
        choir
    }

    fn refresh(&mut self) {
        let sample_rate = self.sound.sample_rate;
        self.sound.ensemble = self.params.at(P_ENSEMBLE);
        self.sound.breath = self.params.at(P_BREATH);
        self.sound.vibrato = self.params.at(P_VIBRATO) + self.modulation * 0.3;
        self.sound.width = self.params.at(P_WIDTH);
        self.sound.smoothing = 1.0 - (-1.0 / (sample_rate * 0.02)).exp();
        let cutoff = (800.0 + 7_200.0 * self.params.at(P_TONE)).min(sample_rate * 0.4);
        self.sound.source_smoothing = 1.0 - (-TAU * cutoff / sample_rate).exp();
        self.target_gain =
            db_to_gain(self.params.at(P_LEVEL)) * self.expression * self.channel_volume;
        let length = 0.14 + 0.065 * self.params.at(P_SIZE);
        for voice in &mut self.voices {
            voice.envelope.set_adsr(
                self.params.at(P_ATTACK),
                0.08,
                0.9,
                self.params.at(P_RELEASE),
            );
            for (index, singer) in voice.singers.iter_mut().enumerate() {
                let length = length * (1.0 + LENGTH_VARIATION[index] * self.sound.ensemble);
                singer
                    .tract
                    .configure(self.params.at(P_VOWEL), length, sample_rate);
            }
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: f32) {
        let velocity = finite_or(velocity, 1.0).clamp(0.0, 1.0);
        let Some(assignment) = self.allocator.note_on(pitch, velocity) else {
            return;
        };
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        voice.pitch = f32::from(pitch);
        voice.velocity = velocity;
        if !assignment.stolen {
            for (index, singer) in voice.singers.iter_mut().enumerate() {
                singer.reset(index, self.sound.ensemble, self.sound.width);
            }
        }
        voice.envelope.trigger();
    }
}

fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::new(P_VOWEL, "vowel", "Vowel (Oo / Ah / Ee)", 0.0, 2.0, 1.0),
        ParamDescriptor::percent(P_SIZE, "voice_size", "Voice Size", 0.46),
        ParamDescriptor::percent(P_ENSEMBLE, "ensemble", "Ensemble Variation", 0.65),
        ParamDescriptor::percent(P_BREATH, "breath", "Breath", 0.12),
        ParamDescriptor::percent(P_TONE, "tone", "Tone", 0.6),
        ParamDescriptor::new(P_VIBRATO, "vibrato", "Vibrato", 0.0, 0.5, 0.09)
            .with_unit(ParamUnit::Semitones),
        ParamDescriptor::percent(P_WIDTH, "width", "Stereo Width", 0.8),
        ParamDescriptor::new(P_ATTACK, "attack", "Attack", 0.001, 3.0, 0.12)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.005, 6.0, 0.7)
            .with_unit(ParamUnit::Seconds)
            .with_curve(ParamValueCurve::Power(3.0)),
        ParamDescriptor::decibels(P_LEVEL, "level", "Level", -60.0, 6.0, -10.0),
    ]
}

impl Parameterized for Choir {
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

impl SegmentRenderer for Choir {
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
                for index in self.allocator.release_all() {
                    if let Some(voice) = self.voices.get_mut(index) {
                        if matches!(event, NoteEvent::AllSoundOff { .. }) {
                            voice.envelope.kill();
                        } else {
                            voice.envelope.release();
                        }
                    }
                }
            }
            NoteEvent::PitchBend { semitones, .. } => {
                self.bend = finite_or(semitones, 0.0).clamp(-24.0, 24.0);
            }
            NoteEvent::Controller { number, value, .. } => {
                match number {
                    7 => self.channel_volume = finite_or(value, 1.0).clamp(0.0, 1.0).powi(2),
                    CC_EXPRESSION => self.expression = finite_or(value, 0.0).clamp(0.0, 1.0),
                    CC_MODULATION => self.modulation = finite_or(value, 0.0).clamp(0.0, 1.0),
                    _ => return,
                }
                self.refresh();
            }
        }
    }

    fn render_segment(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        for channel in out.channels_mut() {
            channel[start..end].fill(0.0);
        }
        let Some((first, rest)) = out.channels_mut().split_first_mut() else {
            return;
        };
        let mut right = rest.first_mut();
        for (slot, voice) in self.voices.iter_mut().enumerate() {
            if !voice.envelope.is_active() {
                continue;
            }
            let frequency = pitch_to_hz(voice.pitch + self.bend);
            for frame in start..end {
                let envelope = voice.envelope.process() * voice.velocity / SINGERS as f32;
                for (index, singer) in voice.singers.iter_mut().enumerate() {
                    let sample =
                        singer.next(index, frequency, &self.tables, &self.sound) * envelope;
                    if let Some(right) = right.as_deref_mut() {
                        let pan = PAN[index] * singer.width;
                        first[frame] += sample * (1.0 - pan);
                        right[frame] += sample * (1.0 + pan);
                    } else {
                        first[frame] += sample;
                    }
                }
            }
            self.allocator
                .set_level(slot, voice.envelope.level() * voice.velocity);
            if voice.envelope.is_finished() {
                self.allocator.retire(slot);
            }
        }
        for frame in start..end {
            self.output_gain += (self.target_gain - self.output_gain) * self.sound.smoothing;
            first[frame] *= self.output_gain;
            if let Some(right) = right.as_deref_mut() {
                right[frame] *= self.output_gain;
            }
        }
        // Additional buses receive the mono downmix; one- and two-channel buffers are native.
        if let Some((right, extra)) = rest.split_first_mut() {
            for channel in extra {
                for frame in start..end {
                    channel[frame] = (first[frame] + right[frame]) * 0.5;
                }
            }
        }
    }
}

impl Instrument for Choir {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            Self::ID,
            "Physical Choir",
            "Wordless ensemble vowels from glottal excitation and vocal-tract waveguides",
            PluginCategory::Synth,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sound.sample_rate = crate::sample_rate_f32(ctx.sample_rate);
        self.tables.prepare();
        self.voices = (0..VOICE_COUNT)
            .map(|_| ChoirVoice::new(self.sound.sample_rate))
            .collect();
        self.allocator.prepare(VOICE_COUNT);
        self.reset();
    }

    fn reset(&mut self) {
        self.bend = 0.0;
        self.modulation = 0.0;
        self.channel_volume = 1.0;
        self.expression = 1.0;
        self.refresh();
        self.output_gain = self.target_gain;
        for voice in &mut self.voices {
            voice.envelope.silence();
            for (index, singer) in voice.singers.iter_mut().enumerate() {
                singer.reset(index, self.sound.ensemble, self.sound.width);
            }
        }
        self.allocator.clear();
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        render_segments(self, events, out, ctx.block_frames.min(out.frame_count()));
    }

    fn active_voices(&self) -> usize {
        self.allocator.active_count()
    }
}
