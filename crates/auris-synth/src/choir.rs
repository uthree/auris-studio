//! A wordless ensemble instrument with glottal excitation and physical vocal tracts.
//!
//! Each played note is four singers. In each, lung pressure drives a pair of two-mass vocal
//! folds that oscillate on their own (`folds`), and the flow they let through is solved
//! against the pressure coming back down an eight-section Kelly–Lochbaum vocal tract
//! (`tract`), so source and tract act on each other. Calibrated area profiles morph between
//! rounded "oo", open "ah" and front "ee" vowels. No waveform is written anywhere: the pitch
//! is the folds' own, held to the note by measuring every period and correcting the tension,
//! and breath is turbulence riding on the glottal jet. Ensemble variation separates tuning,
//! vibrato phase/rate and tract length. All storage is prepared off the audio thread, and
//! identical input gives identical stereo output.

use std::f32::consts::TAU;

use auris_core::param::db_to_gain;
use auris_core::plugin::pitch_to_hz;
use auris_core::{
    AudioBuffer, CC_EXPRESSION, CC_MODULATION, Instrument, NoteEvent, ParamDescriptor, ParamId,
    ParamUnit, ParamValueCurve, Parameterized, PluginCategory, PluginDescriptor, PrepareContext,
    ProcessContext,
};
use auris_dsp::Adsr;

use crate::observe::{Observer, recent_voices};
use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments};
use crate::voice::{VoiceAllocator, VoiceState};
use auris_core::motion::{MotionFrame, MotionGeometry};

mod folds;
mod tract;

#[cfg(test)]
mod tests;

use folds::{ADDUCTED, Folds, OVERSAMPLE};
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
    folds: Folds,
    vibrato_phase: f32,
    drift_phase: f32,
    noise: u32,
    noise_memory: f32,
    source_memory: f32,
    width: f32,
}

impl Singer {
    fn new(sample_rate: f32) -> Self {
        Self {
            tract: Tract::new(sample_rate),
            folds: Folds::new(sample_rate),
            vibrato_phase: 0.0,
            drift_phase: 0.0,
            noise: 1,
            noise_memory: 0.0,
            source_memory: 0.0,
            width: 0.0,
        }
    }

    fn reset(&mut self, index: usize, ensemble: f32, width: f32, hertz: f32) {
        self.tract.reset();
        self.folds.reset(hertz);
        self.vibrato_phase = index as f32 * 0.193 * ensemble;
        self.drift_phase = index as f32 * 0.211 * ensemble;
        self.noise = 0x9e37_79b9_u32.wrapping_mul(index as u32 + 1);
        self.noise_memory = 0.0;
        self.source_memory = 0.0;
        self.width = width;
    }

    /// One sample of this singer: `lung` is the subglottal pressure in pascals.
    fn next(&mut self, index: usize, frequency: f32, lung: f64, sound: &Sound) -> f32 {
        let vibrato = (TAU * self.vibrato_phase).sin() * sound.vibrato;
        let drift = (TAU * self.drift_phase).sin() * 0.04 * sound.ensemble;
        let pitch_offset = DETUNE[index] * sound.ensemble + vibrato + drift;
        let frequency = (frequency * (pitch_offset / 12.0).exp2()).min(sound.sample_rate * 0.2);
        let vibrato_rate = 5.1 + (index as f32 - 1.5) * 0.31 * sound.ensemble;
        self.vibrato_phase = (self.vibrato_phase + vibrato_rate / sound.sample_rate).fract();
        self.drift_phase =
            (self.drift_phase + (0.19 + index as f32 * 0.047) / sound.sample_rate).fract();

        let incoming = f64::from(self.tract.incoming());
        let z0 = self.tract.glottal_impedance();
        let mut flow = 0.0;
        let mut breath = 0.0;
        for _ in 0..OVERSAMPLE {
            let glottis = self
                .folds
                .step(frequency, lung, sound.rest, sound.chink, incoming, z0);
            flow += glottis.flow;
            // Turbulence at the jet: as loud as the pressure it loses, and quieter through a
            // nearly closed slit than an open one.
            let opening = (glottis.area / 0.1e-4).min(3.0).sqrt();
            breath += glottis.drop.abs() * opening;
        }
        flow /= OVERSAMPLE as f64;
        breath /= OVERSAMPLE as f64;

        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        let white = self.noise as i32 as f32 / 2_147_483_648.0;
        self.noise_memory += (white - self.noise_memory) * 0.05;
        let turbulence = (white - self.noise_memory) * breath as f32 * sound.aspiration;
        // The closing folds are soft and the tract walls give: the injected flow loses its top.
        let pressure = (z0 * flow) as f32;
        self.source_memory += sound.source_smoothing * (pressure - self.source_memory);
        self.width += (sound.width - self.width) * sound.smoothing;
        let radiated = self
            .tract
            .next(incoming as f32 + self.source_memory + turbulence);
        radiated * SOURCE_SCALE
    }
}

/// Brings a singer's radiated pressure, in pascals at the tract's scale, to the level the
/// instrument was calibrated at.
const SOURCE_SCALE: f32 = 6.2e-4;

/// Lung pressure for a note, in pascals: singers push harder for higher notes.
fn lung_pressure(pitch: f32) -> f64 {
    f64::from(LUNG_PA + LUNG_PER_SEMITONE * (pitch - 55.0)).clamp(f64::from(LUNG_FLOOR), 1_800.0)
}

const LUNG_PA: f32 = 880.0;
/// How hard the singers push, against the pressure a note needs; fitted with the tract.
const LUNG_SCALE: f64 = 0.7736;
const LUNG_PER_SEMITONE: f32 = 30.0;
/// Low notes still need enough pressure to set large folds going against the tract.
const LUNG_FLOOR: f32 = 900.0;
/// Posterior glottal gap at full Breath, square metres before size scaling.
const BREATHY_CHINK: f64 = 0.06e-4;
/// Turbulence per pascal of glottal pressure drop, at the tract's pressure scale.
const ASPIRATION: f32 = 0.012;

struct ChoirVoice {
    singers: [Singer; SINGERS],
    envelope: Adsr,
    pitch: f32,
    velocity: f32,
    lung: f64,
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
            lung: 0.0,
        }
    }
}

struct Sound {
    sample_rate: f32,
    ensemble: f32,
    /// A gap at the back of the glottis that never closes, square metres before scaling.
    chink: f64,
    /// Turbulence per pascal of jet pressure drop.
    aspiration: f32,
    /// Half-gap the folds are held at, square metres before size scaling.
    rest: f64,
    /// Factor on every note's lung pressure.
    lung: f64,
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
    sound: Sound,
    bend: f32,
    modulation: f32,
    channel_volume: f32,
    expression: f32,
    output_gain: f32,
    target_gain: f32,
    observer: Observer,
    #[cfg(feature = "choir-calibration")]
    calibration: ([[f32; 8]; 3], f32, [f32; 3]),
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
            sound: Sound {
                sample_rate: 48_000.0,
                ensemble: 0.0,
                chink: 0.0,
                aspiration: 0.0,
                rest: ADDUCTED,
                lung: LUNG_SCALE,
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
            observer: Observer::default(),
            #[cfg(feature = "choir-calibration")]
            calibration: (tract::AREAS, tract::RADIATION_HZ, tract::OUTPUT_GAINS),
        };
        choir.refresh();
        choir.output_gain = choir.target_gain;
        choir
    }

    /// Overrides tract areas, lip radiation and level normalization for offline fitting.
    ///
    /// Available only with the development `choir-calibration` feature. Each profile
    /// contains eight areas in square centimetres, from glottis to lips, ordered
    /// oo/ah/ee; output gains have the same order. Invalid input leaves the instrument
    /// unchanged. Call off the audio thread.
    #[cfg(feature = "choir-calibration")]
    pub fn set_calibration_tract(
        &mut self,
        areas: [[f32; 8]; 3],
        radiation_hz: f32,
        output_gains: [f32; 3],
    ) -> bool {
        if !areas
            .iter()
            .flatten()
            .all(|area| area.is_finite() && (0.1..=10.0).contains(area))
            || !radiation_hz.is_finite()
            || !(30.0..=4_000.0).contains(&radiation_hz)
            || !output_gains
                .iter()
                .all(|gain| gain.is_finite() && (0.05..=20.0).contains(gain))
        {
            return false;
        }
        self.calibration = (areas, radiation_hz, output_gains);
        self.refresh();
        true
    }

    /// Overrides how closed the folds rest (square centimetres of half-gap) and a factor on
    /// lung pressure, for offline fitting. Available only with the development
    /// `choir-calibration` feature; invalid input leaves the instrument unchanged. Call off
    /// the audio thread.
    #[cfg(feature = "choir-calibration")]
    pub fn set_calibration_source(&mut self, adduction: f32, lung: f32) -> bool {
        if !(adduction.is_finite() && (-0.05..=0.3).contains(&adduction))
            || !(lung.is_finite() && (0.4..=2.5).contains(&lung))
        {
            return false;
        }
        self.sound.rest = f64::from(adduction) * 1e-4;
        self.sound.lung = f64::from(lung);
        true
    }

    fn refresh(&mut self) {
        let sample_rate = self.sound.sample_rate;
        self.sound.ensemble = self.params.at(P_ENSEMBLE);
        let breath = self.params.at(P_BREATH);
        self.sound.chink = f64::from(breath) * BREATHY_CHINK;
        self.sound.aspiration = ASPIRATION * (0.25 + 2.75 * breath);
        self.sound.vibrato = self.params.at(P_VIBRATO) + self.modulation * 0.3;
        self.sound.width = self.params.at(P_WIDTH);
        self.sound.smoothing = 1.0 - (-1.0 / (sample_rate * 0.02)).exp();
        let cutoff = (800.0 + 7_200.0 * self.params.at(P_TONE)).min(sample_rate * 0.4);
        self.sound.source_smoothing = 1.0 - (-TAU * cutoff / sample_rate).exp();
        self.target_gain =
            db_to_gain(self.params.at(P_LEVEL)) * self.expression * self.channel_volume;
        for index in 0..self.voices.len() {
            let voice = &mut self.voices[index];
            voice.envelope.set_adsr(
                self.params.at(P_ATTACK),
                0.08,
                0.9,
                self.params.at(P_RELEASE),
            );
            self.shape(index);
        }
    }

    /// Sets a voice's tracts for its note: the vowel asked for, opened towards "ah" and
    /// shortened where the note would otherwise rise past the vowel's first resonance.
    fn shape(&mut self, index: usize) {
        let sample_rate = self.sound.sample_rate;
        let size = 0.14 + 0.065 * self.params.at(P_SIZE);
        let vowel = self.params.at(P_VOWEL);
        let ensemble = self.sound.ensemble;
        let voice = &mut self.voices[index];
        let hertz = pitch_to_hz(voice.pitch);
        for (index, singer) in voice.singers.iter_mut().enumerate() {
            let length = size * (1.0 + LENGTH_VARIATION[index] * ensemble);
            let (vowel, length) = tract::tuned(vowel, length, hertz);
            #[cfg(not(feature = "choir-calibration"))]
            singer.tract.configure(vowel, length, sample_rate);
            #[cfg(feature = "choir-calibration")]
            singer.tract.configure_profile(
                vowel,
                length,
                sample_rate,
                &self.calibration.0,
                self.calibration.1,
                &self.calibration.2,
            );
        }
    }

    fn note_on(&mut self, key: u8, pitch: f32, velocity: f32) {
        let velocity = finite_or(velocity, 1.0).clamp(0.0, 1.0);
        let Some(assignment) = self.allocator.note_on(key, velocity) else {
            return;
        };
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        voice.pitch = crate::params::sounding_pitch(key, pitch);
        voice.velocity = velocity;
        voice.lung = lung_pressure(voice.pitch) * self.sound.lung;
        let hertz = pitch_to_hz(voice.pitch);
        self.shape(assignment.index);
        let voice = &mut self.voices[assignment.index];
        if assignment.stolen {
            // A stolen voice keeps its sounding tract and takes the new note's larynx.
            for singer in &mut voice.singers {
                singer.folds.choose_size(hertz);
            }
        } else {
            for (index, singer) in voice.singers.iter_mut().enumerate() {
                singer.reset(index, self.sound.ensemble, self.sound.width, hertz);
            }
        }
        voice.envelope.trigger();
    }

    /// The newest notes' first singers: pressure along the tract and the shape of it.
    fn publish_motion(&mut self) {
        let mut frame = MotionFrame {
            geometry: MotionGeometry::Tract,
            active: self.allocator.active_count(),
            expression: self.expression,
            pressure: self.params.at(P_BREATH),
            ..MotionFrame::default()
        };
        let voices = &self.voices;
        let selected = recent_voices(&self.allocator, |index| voices[index].envelope.is_active());
        for (observed, index) in frame.voices.iter_mut().zip(selected) {
            let Some(index) = index else {
                break;
            };
            let voice = &self.voices[index];
            observed.pitch = voice.pitch + self.bend;
            observed.level = voice.envelope.level() * voice.velocity;
            observed.held = self
                .allocator
                .slot(index)
                .is_some_and(|slot| slot.state == VoiceState::Held);
            observed.contact = 0.0;
            observed.excitation = observed.level;
            voice.singers[0]
                .tract
                .motion(&mut observed.points, &mut observed.modes);
            for point in &mut observed.points {
                *point *= TRACT_DISPLAY;
            }
        }
        self.observer.publish(&frame);
    }
}

/// Tract pressure swings about a kilopascal, the lungs' own pressure; this publishes it at
/// the scale a string's displacement is.
const TRACT_DISPLAY: f32 = 4.0e-5;

fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::new(P_VOWEL, "vowel", "Vowel (Oo / Ah / Ee)", 0.0, 2.0, 1.0),
        ParamDescriptor::percent(P_SIZE, "voice_size", "Voice Size", 0.46),
        ParamDescriptor::percent(P_ENSEMBLE, "ensemble", "Ensemble Variation", 0.65),
        ParamDescriptor::percent(P_BREATH, "breath", "Breath", 0.069603),
        ParamDescriptor::percent(P_TONE, "tone", "Tone", 0.787636),
        ParamDescriptor::new(P_VIBRATO, "vibrato", "Vibrato", 0.0, 0.5, 0.09)
            .with_unit(ParamUnit::Semitones),
        ParamDescriptor::percent(P_WIDTH, "width", "Stereo Width", 0.8),
        ParamDescriptor::new(P_ATTACK, "attack", "Attack", 0.001, 3.0, 0.243677)
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
                key,
                pitch,
                velocity,
                ..
            } => self.note_on(key, pitch, velocity),
            NoteEvent::NoteOff { key, .. } => {
                for index in self.allocator.note_off(key) {
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
                // The envelope is the singer's breath: the folds start when the pressure is
                // enough to set them going and stop when it is not. A fast gate still has the
                // last word, so a killed note is silent when the de-click says it is.
                let envelope = voice.envelope.process();
                let lung = voice.lung * f64::from((envelope * 3.0).min(1.0));
                let gain = envelope * voice.velocity / SINGERS as f32;
                for (index, singer) in voice.singers.iter_mut().enumerate() {
                    let sample = singer.next(index, frequency, lung, &self.sound) * gain;
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
            "Wordless ensemble vowels from self-oscillating vocal folds and vocal-tract waveguides",
            PluginCategory::Synth,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        self.sound.sample_rate = crate::sample_rate_f32(ctx.sample_rate);
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
                singer.reset(index, self.sound.ensemble, self.sound.width, 220.0);
            }
        }
        self.allocator.clear();
        self.observer.reset(MotionGeometry::Tract);
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = ctx.block_frames.min(out.frame_count());
        render_segments(self, events, out, frames);
        if self.observer.due(frames, self.sound.sample_rate) {
            self.publish_motion();
        }
    }

    fn motion_monitor(&self) -> Option<std::sync::Arc<auris_core::motion::MotionMonitor>> {
        Some(self.observer.monitor())
    }

    fn active_voices(&self) -> usize {
        self.allocator.active_count()
    }
}
