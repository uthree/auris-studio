//! Sample-free instruments with editable excitation, loss and body resonance.
//!
//! Piano, bell and mallet use reduced modal expansions. Guitar and bass release a displaced
//! travelling-wave string; violin drives two travelling waves through nonlinear bow friction.
//! These are deliberately compact, expressive instruments rather than sampled replicas.

use auris_core::param::db_to_gain;
use auris_core::plugin::pitch_to_hz;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, Parameterized,
    PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_dsp::Adsr;

use crate::params::finite_or;
use crate::{ParamBank, SegmentRenderer, VoiceAllocator, render_segments, spread_to_all_channels};

mod body;
mod legato;
mod modal;
mod string;

use body::Body;
use legato::Legato;
use modal::Modal;
use string::StringModel;

const VOICES: usize = 24;
const P_HARDNESS: u32 = 0;
const P_POSITION: u32 = 1;
const P_DECAY: u32 = 2;
const P_DAMPING: u32 = 3;
const P_BODY: u32 = 4;
const P_RELEASE: u32 = 5;
const P_LEVEL: u32 = 6;
const P_STIFFNESS: u32 = 7;
const P_PRESSURE: u32 = 7;
const P_PICKUP: u32 = 7;
const P_BOW_SPEED: u32 = 8;
const P_LEGATO: u32 = 9;
const P_BOW_RESPONSE: u32 = 10;

/// Physical structure and excitation used by an instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// Hammer-excited stiff string modes.
    Piano,
    /// Plucked string with a pick-controlled initial displacement.
    Guitar,
    /// Long plucked string with a lower body resonance.
    Bass,
    /// Struck shell with inharmonic modes.
    Bell,
    /// Struck free bar with bending modes.
    Mallet,
    /// Bowed string with a nonlinear friction junction.
    Violin,
}

impl Model {
    /// Every physical instrument, ordered by excitation family.
    pub const ALL: [Self; 6] = [
        Self::Piano,
        Self::Guitar,
        Self::Bass,
        Self::Bell,
        Self::Mallet,
        Self::Violin,
    ];

    /// Stable plugin id stored in a project.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Piano => "auris.physical.piano",
            Self::Guitar => "auris.physical.guitar",
            Self::Bass => "auris.physical.bass",
            Self::Bell => "auris.physical.bell",
            Self::Mallet => "auris.physical.mallet",
            Self::Violin => "auris.physical.violin",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Piano => "Physical Piano",
            Self::Guitar => "Physical Guitar",
            Self::Bass => "Physical Bass",
            Self::Bell => "Physical Bell",
            Self::Mallet => "Physical Mallet",
            Self::Violin => "Physical Violin",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Piano => "Hammer-excited stiff strings: hardness, strike position and soundboard",
            Self::Guitar => "Plucked string: pick hardness, pluck position, damping and body",
            Self::Bass => "Plucked bass string: finger/pick hardness, damping and body",
            Self::Bell => "Struck bell: inharmonic shell modes, beater hardness and decay",
            Self::Mallet => "Mallet bar: bending modes, beater hardness and damping",
            Self::Violin => {
                "Bowed violin string: nonlinear friction, bow pressure, position and body"
            }
        }
    }

    fn is_string(self) -> bool {
        matches!(self, Self::Guitar | Self::Bass | Self::Violin)
    }

    fn output_normalization(self) -> f32 {
        // Keep the median factory-note RMS after real-recording mel calibration. The
        // public level dial keeps its existing meaning for saved projects and automation.
        match self {
            Self::Piano => 0.164_845,
            Self::Guitar => 0.178_368,
            Self::Violin => 1.787_516,
            _ => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Settings {
    hardness: f32,
    position: f32,
    decay: f32,
    damping: f32,
    stiffness: f32,
    pickup: f32,
    bow_speed: f32,
    bow_response: f32,
}

#[derive(Clone, Debug)]
struct Voice {
    modal: Modal,
    string: StringModel,
    envelope: Adsr,
    held: bool,
    deferred: bool,
    age: usize,
    limit: usize,
    last: f32,
    tail: f32,
    fade: f32,
}

impl Voice {
    fn new(model: Model, rate: f32) -> Self {
        let mut modal = Modal::default();
        modal.prepare(model);
        let mut string = StringModel::default();
        if model.is_string() {
            string.prepare(rate);
        }
        let mut envelope = Adsr::new();
        envelope.set_sample_rate(rate);
        Self {
            modal,
            string,
            envelope,
            held: false,
            deferred: false,
            age: 0,
            limit: 0,
            last: 0.0,
            tail: 0.0,
            fade: 0.0,
        }
    }
}

/// A polyphonic physical instrument, registered once for each [`Model`].
///
/// Parameters and MIDI CCs are shared across the family. Velocity changes both excitation
/// strength and contact hardness; CC1 scales bow pressure, CC7 sets channel volume, CC11 drives
/// expression/bow speed, and CC64 holds released piano strings. State uses ordinary plugin
/// parameters, so automation, undo, save/load and model-facing tools need no special format.
#[derive(Clone, Debug)]
pub struct Physical {
    model: Model,
    params: ParamBank,
    voices: Vec<Voice>,
    allocator: VoiceAllocator,
    body: Body,
    rate: f32,
    bend: f32,
    volume: f32,
    expression: f32,
    expression_current: f32,
    expression_step: f32,
    pressure: f32,
    pedal: bool,
    gain: f32,
    legato: Legato,
}

impl Physical {
    /// Builds an unprepared physical instrument. All voice storage is allocated in `prepare`.
    pub fn new(model: Model) -> Self {
        let (decay, release, position, hardness) = match model {
            Model::Piano => (11.296, 0.20, 0.228_641, 0.701_331),
            Model::Guitar => (5.638_205, 0.12, 0.298_280, 0.249_201),
            Model::Bass => (3.5, 0.15, 0.30, 0.40),
            Model::Bell => (6.0, 1.8, 0.35, 0.65),
            Model::Mallet => (1.7, 0.35, 0.40, 0.40),
            Model::Violin => (10.460_195, 1.5, 0.103_416, 0.319_371),
        };
        let mut descriptors = vec![
            ParamDescriptor::percent(P_HARDNESS, "hardness", "Contact Hardness", hardness),
            ParamDescriptor::new(
                P_POSITION,
                "position",
                "Excitation Position",
                0.05,
                0.45,
                position,
            )
            .with_unit(ParamUnit::Percent),
            ParamDescriptor::new(P_DECAY, "decay", "Resonance Decay", 0.1, 12.0, decay)
                .with_unit(ParamUnit::Seconds),
            ParamDescriptor::percent(
                P_DAMPING,
                "damping",
                "Damping",
                match model {
                    Model::Piano => 0.000_108,
                    Model::Guitar => 0.0,
                    Model::Violin => 0.359_287,
                    _ => 0.12,
                },
            ),
            ParamDescriptor::percent(
                P_BODY,
                "body",
                "Body Resonance",
                if matches!(model, Model::Piano | Model::Guitar | Model::Violin) {
                    0.65
                } else {
                    0.35
                },
            ),
            ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 3.0, release)
                .with_unit(ParamUnit::Seconds),
            ParamDescriptor::new(P_LEVEL, "level", "Level", -60.0, 6.0, -12.0)
                .with_unit(ParamUnit::Decibels),
        ];
        if model == Model::Piano {
            descriptors.push(ParamDescriptor::new(
                P_STIFFNESS,
                "stiffness",
                "String Stiffness",
                0.0,
                0.008,
                0.000_769_281,
            ));
        }
        if model == Model::Violin {
            descriptors.push(ParamDescriptor::percent(
                P_PRESSURE,
                "bow_pressure",
                "Bow Pressure",
                0.336_820,
            ));
            descriptors.push(ParamDescriptor::percent(
                P_BOW_SPEED,
                "bow_speed",
                "Bow speed",
                0.203_998,
            ));
            descriptors.push(ParamDescriptor::toggle(P_LEGATO, "legato", "Legato", false));
            descriptors.push(
                ParamDescriptor::new(
                    P_BOW_RESPONSE,
                    "bow_response",
                    "Bow response",
                    0.002,
                    0.12,
                    0.057_120_9,
                )
                .with_unit(ParamUnit::Seconds),
            );
        }
        if model == Model::Guitar {
            descriptors.push(ParamDescriptor::percent(
                P_PICKUP,
                "pickup",
                "Pickup blend",
                0.0,
            ));
        }
        let params = ParamBank::new(descriptors);
        Self {
            model,
            params,
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            body: Body::default(),
            rate: 48_000.0,
            bend: 0.0,
            volume: 1.0,
            expression: 1.0,
            expression_current: 1.0,
            expression_step: 1.0,
            pressure: 1.0,
            pedal: false,
            gain: db_to_gain(-12.0) * model.output_normalization(),
            legato: Legato::default(),
        }
    }

    fn settings(&self) -> Settings {
        Settings {
            hardness: self.params.at(P_HARDNESS),
            position: self.params.at(P_POSITION),
            decay: self.params.at(P_DECAY),
            damping: self.params.at(P_DAMPING),
            stiffness: if self.model == Model::Piano {
                self.params.at(P_STIFFNESS)
            } else {
                0.0
            },
            pickup: if self.model == Model::Guitar {
                self.params.at(P_PICKUP)
            } else {
                0.0
            },
            bow_speed: if self.model == Model::Violin {
                self.params.at(P_BOW_SPEED)
            } else {
                0.0
            },
            bow_response: if self.model == Model::Violin {
                self.params.at(P_BOW_RESPONSE)
            } else {
                0.012
            },
        }
    }

    fn is_legato(&self) -> bool {
        self.model == Model::Violin && self.params.at(P_LEGATO) >= 0.5
    }

    fn update_expression_step(&mut self) {
        self.expression_step = if self.model == Model::Violin {
            // Output gain responds twice as fast as bow motion. Computing this
            // coefficient at control rate leaves only a multiply/add per sample.
            1.0 - (-2.0 / (self.rate * self.params.at(P_BOW_RESPONSE))).exp()
        } else {
            1.0
        };
    }

    fn retarget(&mut self, index: usize, pitch: u8, velocity: f32) -> bool {
        let frequency = pitch_to_hz(f32::from(pitch) + self.bend);
        let Some(voice) = self.voices.get_mut(index) else {
            return false;
        };
        if !voice.held || !voice.envelope.is_active() {
            return false;
        }
        if !self.allocator.retarget(index, pitch, velocity) {
            return false;
        }
        voice.string.glide_to(frequency);
        voice.string.set_velocity(velocity);
        true
    }

    fn note_on(&mut self, pitch: u8, velocity: f32) {
        let pitch = pitch.min(127);
        let velocity = finite_or(velocity, 0.0).clamp(0.0, 1.0);
        if velocity == 0.0 {
            self.note_off(pitch);
            return;
        }
        if self.is_legato() {
            self.legato.note_on(pitch, velocity);
            if let Some(index) = self.legato.voice
                && self.retarget(index, pitch, velocity)
            {
                return;
            }
        }
        let Some(assignment) = self.allocator.note_on(pitch.min(127), velocity) else {
            return;
        };
        let settings = self.settings();
        if self.is_legato() {
            self.legato.voice = Some(assignment.index);
        }
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        voice.tail = if assignment.stolen { voice.last } else { 0.0 };
        voice.fade = if assignment.stolen { 1.0 } else { 0.0 };
        voice.envelope.silence();
        voice.envelope.set_adsr(
            if self.model == Model::Violin {
                0.025
            } else {
                0.001
            },
            0.0,
            1.0,
            self.params.at(P_RELEASE),
        );
        voice.envelope.trigger();
        voice.held = true;
        voice.deferred = false;
        voice.age = 0;
        voice.limit = (settings.decay * 2.0 * self.rate) as usize;
        let hz = pitch_to_hz(f32::from(pitch.min(127)) + self.bend).min(self.rate * 0.2);
        if self.model.is_string() {
            voice
                .string
                .excite(self.model, hz, velocity, self.rate, settings);
        } else {
            voice
                .modal
                .excite(self.model, hz, velocity, self.rate, settings);
        }
    }

    fn note_off(&mut self, pitch: u8) {
        let pitch = pitch.min(127);
        if self.is_legato() {
            if !self.legato.note_off(pitch) {
                return;
            }
            if let Some(index) = self.legato.voice
                && let Some((target, velocity)) = self.legato.last()
            {
                self.retarget(index, target, velocity);
                return;
            }
            self.legato.voice = None;
        }
        for index in self.allocator.note_off(pitch) {
            if let Some(voice) = self.voices.get_mut(index) {
                voice.held = false;
                if self.model == Model::Piano && self.pedal {
                    voice.deferred = true;
                } else {
                    voice.envelope.release();
                }
            }
        }
    }
}

impl Parameterized for Physical {
    fn parameters(&self) -> &[ParamDescriptor] {
        self.params.descriptors()
    }
    fn param(&self, id: ParamId) -> f32 {
        self.params.get(id)
    }
    fn set_param(&mut self, id: ParamId, value: f32) {
        let was_legato = self.is_legato();
        if self.params.set(id, value) {
            if was_legato != self.is_legato() {
                self.legato.clear();
                for index in self.allocator.release_all() {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.held = false;
                        voice.envelope.release();
                    }
                }
            }
            self.gain = db_to_gain(self.params.at(P_LEVEL)) * self.model.output_normalization();
            if id.0 == P_BOW_RESPONSE && self.model == Model::Violin {
                self.update_expression_step();
            }
            let settings = self.settings();
            let changes_resonance = matches!(id.0, P_DECAY | P_DAMPING | P_HARDNESS)
                || self.model == Model::Guitar && id.0 == P_PICKUP
                || self.model == Model::Violin
                    && matches!(id.0, P_BOW_SPEED | P_POSITION | P_BOW_RESPONSE);
            for voice in &mut self.voices {
                voice.envelope.set_adsr(
                    if self.model == Model::Violin {
                        0.025
                    } else {
                        0.001
                    },
                    0.0,
                    1.0,
                    self.params.at(P_RELEASE),
                );
                if voice.envelope.is_active() && changes_resonance {
                    if self.model.is_string() {
                        voice.string.update_loss(settings);
                    } else {
                        voice.modal.update_loss(self.model, self.rate, settings);
                    }
                    voice.limit = (settings.decay * 2.0 * self.rate) as usize;
                }
            }
        }
    }
}

impl SegmentRenderer for Physical {
    fn handle_event(&mut self, event: &NoteEvent) {
        match *event {
            NoteEvent::NoteOn {
                pitch, velocity, ..
            } => self.note_on(pitch, velocity),
            NoteEvent::NoteOff { pitch, .. } => self.note_off(pitch),
            NoteEvent::AllNotesOff { .. } | NoteEvent::AllSoundOff { .. } => {
                self.legato.clear();
                self.pedal = false;
                for index in self.allocator.release_all() {
                    if let Some(voice) = self.voices.get_mut(index) {
                        voice.held = false;
                        voice.deferred = false;
                        if matches!(event, NoteEvent::AllSoundOff { .. }) {
                            voice.envelope.kill();
                        } else {
                            voice.envelope.release();
                        }
                    }
                }
            }
            NoteEvent::PitchBend { semitones, .. } => {
                let bend = finite_or(semitones, 0.0).clamp(-12.0, 12.0);
                let ratio = ((bend - self.bend) / 12.0).exp2();
                for voice in &mut self.voices {
                    if self.model.is_string() {
                        voice.string.retune(ratio);
                    } else {
                        voice.modal.retune(ratio);
                    }
                }
                self.bend = bend;
            }
            NoteEvent::Controller { number, value, .. } => {
                let value = finite_or(value, 0.0).clamp(0.0, 1.0);
                match number {
                    1 => self.pressure = 0.5 + value * 0.5,
                    7 => self.volume = value.powi(2),
                    11 => {
                        self.expression = value;
                        if self.allocator.active_count() == 0 {
                            self.expression_current = value;
                        }
                    }
                    64 if self.model == Model::Piano => {
                        self.pedal = value >= 0.5;
                        if !self.pedal {
                            for voice in &mut self.voices {
                                if voice.deferred {
                                    voice.envelope.release();
                                    voice.deferred = false;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn render_segment(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        let Some((mono, _)) = out.channels_mut().split_first_mut() else {
            return;
        };
        let Some(samples) = mono.get_mut(start..end) else {
            return;
        };
        samples.fill(0.0);
        let pressure = self.pressure * self.params.at(P_PRESSURE);
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if !voice.envelope.is_active() {
                continue;
            }
            for sample in samples.iter_mut() {
                let raw = if self.model.is_string() {
                    let bow = if voice.held { self.expression } else { 0.0 };
                    voice.string.next(bow, pressure)
                } else {
                    voice.modal.next()
                };
                let envelope = voice.envelope.process();
                let current = raw * envelope;
                voice.last = current * (1.0 - voice.fade) + voice.tail * voice.fade;
                voice.fade = (voice.fade - 1.0 / (0.002 * self.rate)).max(0.0);
                *sample += voice.last;
                voice.age = voice.age.saturating_add(1);
                if self.model != Model::Violin && voice.age >= voice.limit {
                    voice.envelope.release();
                }
            }
            self.allocator.set_level(index, voice.last.abs());
            if voice.envelope.is_finished()
                || (!self.model.is_string() && voice.age > 100 && voice.modal.energy() < 1.0e-5)
            {
                voice.envelope.silence();
                self.allocator.retire(index);
            }
        }
        let body = self.params.at(P_BODY)
            * if self.model == Model::Guitar {
                1.0 - self.params.at(P_PICKUP)
            } else {
                1.0
            };
        for sample in samples {
            let expression = if self.model == Model::Violin {
                self.expression_current +=
                    (self.expression - self.expression_current) * self.expression_step;
                self.expression_current
            } else {
                self.expression
            };
            *sample = self.body.next(*sample, body) * self.gain * self.volume * expression;
        }
    }
}

impl Instrument for Physical {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            self.model.id(),
            self.model.name(),
            self.model.description(),
            PluginCategory::Synth,
        )
    }
    fn prepare(&mut self, ctx: &PrepareContext) {
        self.rate = crate::sample_rate_f32(ctx.sample_rate).clamp(8_000.0, 192_000.0);
        self.voices = (0..VOICES)
            .map(|_| Voice::new(self.model, self.rate))
            .collect();
        self.allocator.prepare(VOICES);
        self.body.prepare(self.model, self.rate);
        self.update_expression_step();
        self.reset();
    }
    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.envelope.silence();
            voice.last = 0.0;
            voice.fade = 0.0;
        }
        self.body.reset();
        self.allocator.clear();
        self.bend = 0.0;
        self.volume = 1.0;
        self.expression = 1.0;
        self.expression_current = 1.0;
        self.pressure = 1.0;
        self.pedal = false;
        self.legato.clear();
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
#[path = "physical/tests.rs"]
mod tests;
