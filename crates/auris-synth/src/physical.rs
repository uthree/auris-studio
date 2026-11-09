//! Sample-free instruments with editable excitation, loss and body resonance.
//!
//! Piano, bell and mallet use reduced modal expansions. Guitar and bass release a displaced
//! travelling-wave string; violin drives two travelling waves through nonlinear bow friction.
//! These are deliberately compact, expressive instruments rather than sampled replicas.

use auris_core::motion::{
    MOTION_VOICES, MotionCapture, MotionFrame, MotionGeometry, TOUCH_CONTROLLER,
};
use auris_core::param::db_to_gain;
use auris_core::plugin::pitch_to_hz;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, Parameterized,
    PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_dsp::Adsr;
use std::borrow::Cow;

use crate::params::finite_or;
use crate::render::{StereoImage, downmix_to_extra_channels, pan_gains};
use crate::{ParamBank, SegmentRenderer, VoiceAllocator, render_segments};

mod body;
mod calibration;
mod decimator;
mod legato;
mod modal;
mod pack;
mod string;

use body::Body;
use decimator::Decimator;
use legato::Legato;
use modal::{Modal, VOICING};
use string::StringModel;

#[cfg(not(feature = "piano-calibration"))]
use modal::PianoVoicing;
#[cfg(feature = "piano-calibration")]
pub use modal::piano::PianoVoicing;

const VOICES: usize = 24;
/// Largest internal sample-rate multiple of any [`Quality`].
const MAX_OVERSAMPLING: usize = 2;
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
const P_PICKUP_POSITION: u32 = 4;
const P_TONE: u32 = 7;
const P_PICKUP_Q: u32 = 8;
const P_DECAY_RATIO: u32 = 9;

/// How finely a physical model is simulated.
///
/// Every instrument has two of these controls: one for live playback, where the audio callback's
/// budget is the limit, and one for offline rendering (export, freeze, bounce), where nothing but
/// time is. A level is chosen when a note is struck and kept for that note, so changing either
/// control never disturbs a sounding string and never allocates.
///
/// [`Quality::Standard`] is the model every calibration in this module was fitted at. The other
/// two levels move from it only where a measurement said the move was worth something:
///
/// | Model | Draft | High |
/// |---|---|---|
/// | Piano | a third of the partial budget: 64 partial-strings a voice instead of 192 | as Standard |
/// | Bell, Mallet | the first 8 modes instead of 32 | as Standard |
/// | Guitar, Electric Guitar, Bass, Strings | as Standard | the string loop runs 2× oversampled |
///
/// The piano is the one expensive model, about eight times the cost of any other with 24 notes
/// held, and Draft takes that to a little over half. What it costs in sound is small: over a
/// second of a struck note it differs from Standard by -31 dB RMS on A0, -38 dB on A1 and less
/// than -52 dB from A2 up. Giving the piano *more* partials instead was measured and left out:
/// every partial below the band edge changed A0 by -73 dB and higher notes by nothing, because
/// the hammer's contact spectrum and the strings' frictional loss have already taken them.
/// Bars and bells are cheap at any level; their Draft is there so the control means the same
/// thing on every model, and changes a struck bar by about -52 dB.
///
/// Oversampling a delay-line string halves the interpolation error in its loop. The audible
/// place for that is the top of a plucked string's range: at the host rate the guitar's E6
/// stretches its third to fifth harmonics one to three cents sharp, and at twice the rate they
/// are harmonic again. A decimator band-limits the doubled rate back to the host's, and costs
/// each string voice about as much as the string itself. Neither the bowed nor the plucked
/// strings were measurably aliasing at the host rate, so that is not what High is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quality {
    /// Cheaper than the calibrated model, for dense live arrangements on a slow machine.
    Draft,
    /// The calibrated model.
    #[default]
    Standard,
    /// More of the physics than a live callback can always afford; the default for rendering.
    High,
}

impl Quality {
    /// Choice labels in parameter-value order.
    pub const CHOICES: &'static [Cow<'static, str>] = &[
        Cow::Borrowed("Draft"),
        Cow::Borrowed("Standard"),
        Cow::Borrowed("High"),
    ];

    /// The level a choice parameter's value names, rounding to the nearest step.
    pub fn from_value(value: f32) -> Self {
        match finite_or(value, 1.0).round() as i32 {
            i32::MIN..=0 => Self::Draft,
            1 => Self::Standard,
            _ => Self::High,
        }
    }

    /// The choice parameter's value for this level.
    pub const fn value(self) -> f32 {
        match self {
            Self::Draft => 0.0,
            Self::Standard => 1.0,
            Self::High => 2.0,
        }
    }

    /// Internal sample-rate multiple of a delay-line string at this level.
    fn oversampling(self) -> usize {
        if self == Self::High { 2 } else { 1 }
    }
}

/// Physical structure and excitation used by an instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// Hammer-excited stiff string modes.
    Piano,
    /// Plucked string with a pick-controlled initial displacement.
    Guitar,
    /// Plucked steel string observed through a magnetic pickup and its electrical resonance.
    ElectricGuitar,
    /// Long plucked string with a lower body resonance.
    Bass,
    /// Struck shell with inharmonic modes.
    Bell,
    /// Struck bar tuned like a marimba's, its overtones two and about three octaves up.
    Mallet,
    /// Bowed string with a nonlinear friction junction.
    Violin,
}

impl Model {
    /// Factory radiation modes as `(frequency_hz, quality_factor, signed_gain)`.
    ///
    /// These fitted spectral resonances are not isolated measured bridge admittances.
    /// They follow the broad coloration and scale with the body control, anchored at 0.65.
    pub fn radiation_modes(self) -> [(f32, f32, f32); 6] {
        body::radiation_modes(self)
    }

    /// Factory exponent mapping channel expression to bow motion and output expression.
    pub fn expression_exponent(self) -> f64 {
        if self == Self::Violin { 1.18 } else { 1.0 }
    }

    /// Every physical instrument, ordered by excitation family.
    pub const ALL: [Self; 7] = [
        Self::Piano,
        Self::Guitar,
        Self::ElectricGuitar,
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
            Self::ElectricGuitar => "auris.physical.electric_guitar",
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
            Self::ElectricGuitar => "Electric Guitar",
            Self::Bass => "Physical Bass",
            Self::Bell => "Physical Bell",
            Self::Mallet => "Physical Mallet",
            Self::Violin => "Physical Strings",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Piano => "Hammer-excited stiff strings: hardness, strike position and soundboard",
            Self::Guitar => "Plucked string: pick hardness, pluck position, damping and body",
            Self::ElectricGuitar => {
                "Steel string DI: independent pick/pickup position and pickup resonance"
            }
            Self::Bass => "Plucked bass string: finger/pick hardness, damping and body",
            Self::Bell => "Struck bell: inharmonic shell modes, beater hardness and decay",
            Self::Mallet => "Tuned mallet bar: marimba overtones, beater hardness and damping",
            Self::Violin => "Bowed strings: nonlinear friction, bow pressure, position and body",
        }
    }

    /// Where the notes sit across the stereo field, heard from the player's seat.
    ///
    /// Keyboards and bars run low to high from left to right; the strings follow orchestral
    /// seating, violins on the left and basses on the right. The electric guitar is a DI
    /// signal and stays close to the centre.
    pub(crate) fn stereo_image(self) -> StereoImage {
        match self {
            Self::Piano => StereoImage::new(64.5, 43.5, 0.6),
            Self::Guitar => StereoImage::new(62.0, 22.0, 0.3),
            Self::ElectricGuitar => StereoImage::new(62.0, 22.0, 0.15),
            Self::Bass => StereoImage::new(45.0, 18.0, 0.2),
            Self::Bell => StereoImage::new(72.0, 18.0, 0.5),
            Self::Mallet => StereoImage::new(64.5, 31.5, 0.6),
            Self::Violin => StereoImage::new(60.0, 28.0, -0.5),
        }
    }

    fn is_string(self) -> bool {
        matches!(
            self,
            Self::Guitar | Self::ElectricGuitar | Self::Bass | Self::Violin
        )
    }

    fn output_normalization(self) -> f32 {
        // Keep the median factory-note RMS after real-recording mel calibration. The
        // public level dial keeps its existing meaning for saved projects and automation.
        match self {
            Self::Piano => 0.077_622,
            Self::Guitar => 0.178_368,
            Self::ElectricGuitar => 0.8,
            Self::Violin => 1.787_516,
            _ => pack::profile(self).map_or(1.0, |profile| profile.normalization),
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
    release: f32,
    pickup: f32,
    electric: bool,
    pickup_position: f32,
    tone: f32,
    pickup_q: f32,
    decay_ratio: f32,
    bow_speed: f32,
    bow_response: f32,
}

#[derive(Clone, Debug)]
struct Voice {
    pitch: u8,
    modal: Modal,
    string: StringModel,
    envelope: Adsr,
    presence: crate::presence::Presence,
    /// Internal samples per output frame for a delay-line string, fixed at the strike.
    oversampling: usize,
    /// Band-limits an oversampled string back to the host rate.
    decimator: Decimator,
    held: bool,
    deferred: bool,
    age: usize,
    limit: usize,
    last: f32,
    tail: f32,
    fade: f32,
    contact: f32,
    /// Pan at unit width, fixed when the note is struck.
    position: f32,
    /// Pan currently applied, chasing `position` times the width control.
    pan: f32,
}

impl Voice {
    fn new(model: Model, rate: f32, voicing: PianoVoicing) -> Self {
        let mut modal = Modal::default();
        modal.prepare(model, voicing);
        let mut string = StringModel::default();
        if model.is_string() {
            // Sized for the highest quality's internal rate, so a change of quality never
            // allocates on the audio thread.
            string.prepare(rate * MAX_OVERSAMPLING as f32);
        }
        let mut envelope = Adsr::new();
        envelope.set_sample_rate(rate);
        Self {
            pitch: 60,
            modal,
            string,
            envelope,
            presence: crate::presence::Presence::new(
                rate,
                match model {
                    Model::Piano => -3.5,
                    Model::Guitar => -2.0,
                    Model::ElectricGuitar | Model::Violin => -3.0,
                    _ => 0.0,
                },
            ),
            oversampling: 1,
            decimator: Decimator::default(),
            held: false,
            deferred: false,
            age: 0,
            limit: 0,
            last: 0.0,
            tail: 0.0,
            fade: 0.0,
            contact: 0.25,
            position: 0.0,
            pan: 0.0,
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
    /// One radiation filter per side. Both are linear, so the downmix of the two is the
    /// mono instrument's single body.
    body: [Body; 2],
    rate: f32,
    bend: f32,
    volume: f32,
    expression: f32,
    expression_current: f32,
    expression_step: f32,
    pressure: f32,
    pedal: bool,
    gain: f32,
    motion: MotionCapture,
    motion_frames: usize,
    /// Where a frontend touched the body, waiting for the note it excites.
    touch: Option<f32>,
    legato: Legato,
    voicing: PianoVoicing,
    /// The piano's soundboard knock for one segment, mixed after radiation.
    knock: Vec<f32>,
    /// Stands in for the right channel when the output buffer is mono.
    scratch: Vec<f32>,
    /// Id of the stereo width parameter, which follows each model's own parameters.
    width_param: u32,
    /// One-pole coefficient by which a voice's pan follows the width control.
    pan_step: f32,
    /// Id of the live-playback [`Quality`] choice, which follows the width.
    quality_param: u32,
    /// Id of the offline-render [`Quality`] choice, which follows the live one.
    render_quality_param: u32,
    /// Whether the block being rendered is an offline one, which picks the render quality.
    offline: bool,
}

impl Physical {
    /// Builds an unprepared physical instrument. All voice storage is allocated in `prepare`.
    pub fn new(model: Model) -> Self {
        let (mut decay, release, mut position, mut hardness) = match model {
            Model::Piano => (11.498_04, 0.20, 0.244_4, 0.606_004),
            Model::Guitar => (5.638_205, 0.12, 0.298_280, 0.249_201),
            Model::ElectricGuitar => (7.493_398, 0.12, 0.069_559, 0.186_606),
            Model::Bass => (3.5, 0.15, 0.30, 0.40),
            Model::Bell => (6.0, 1.8, 0.35, 0.65),
            Model::Mallet => (1.7, 0.35, 0.40, 0.40),
            Model::Violin => (10.460_195, 1.5, 0.103_416, 0.319_371),
        };
        if let Some(profile) = pack::profile(model) {
            decay = profile.decay;
            position = profile.position;
            hardness = profile.hardness;
        }
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
            ParamDescriptor::new(
                P_DECAY,
                "decay",
                "Resonance Decay",
                0.1,
                if model == Model::ElectricGuitar {
                    40.0
                } else {
                    12.0
                },
                decay,
            )
            .with_unit(ParamUnit::Seconds),
            ParamDescriptor::percent(
                P_DAMPING,
                "damping",
                "Damping",
                match model {
                    Model::Piano => 0.009_015,
                    Model::Guitar => 0.0,
                    Model::ElectricGuitar => 0.021_633,
                    Model::Violin => 0.359_287,
                    _ => pack::profile(model).map_or(0.12, |profile| profile.damping),
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
        if model == Model::ElectricGuitar {
            descriptors[P_BODY as usize] = ParamDescriptor::new(
                P_PICKUP_POSITION,
                "pickup_position",
                "Pickup Position",
                0.02,
                0.3,
                0.235_997,
            )
            .with_unit(ParamUnit::Percent);
            descriptors.push(
                ParamDescriptor::new(
                    P_TONE,
                    "tone",
                    "Pickup Resonance",
                    800.0,
                    12_000.0,
                    2717.688,
                )
                .with_unit(ParamUnit::Hertz),
            );
            descriptors.push(ParamDescriptor::new(
                P_PICKUP_Q, "pickup_q", "Pickup Q", 0.5, 2.5, 1.883_881,
            ));
            descriptors.push(
                ParamDescriptor::new(
                    P_DECAY_RATIO,
                    "decay_ratio",
                    "Decay per Octave",
                    0.2,
                    1.5,
                    0.55,
                )
                .with_unit(ParamUnit::Ratio),
            );
        }
        if model == Model::Piano {
            descriptors.push(ParamDescriptor::new(
                P_STIFFNESS,
                "stiffness",
                "String Stiffness",
                0.0,
                0.008,
                0.000_749_371,
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
        let width_param = descriptors.len() as u32;
        descriptors.push(ParamDescriptor::new(
            width_param,
            "width",
            "Stereo Width",
            0.0,
            2.0,
            1.0,
        ));
        let quality_param = width_param + 1;
        let render_quality_param = width_param + 2;
        descriptors.push(
            ParamDescriptor::new(
                quality_param,
                "quality",
                "Live Quality",
                0.0,
                2.0,
                Quality::Standard.value(),
            )
            .with_choices(Quality::CHOICES),
        );
        descriptors.push(
            ParamDescriptor::new(
                render_quality_param,
                "render_quality",
                "Render Quality",
                0.0,
                2.0,
                Quality::High.value(),
            )
            .with_choices(Quality::CHOICES),
        );
        let params = ParamBank::new(descriptors);
        Self {
            model,
            params,
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            body: Default::default(),
            rate: 48_000.0,
            bend: 0.0,
            volume: 1.0,
            expression: 1.0,
            expression_current: 1.0,
            expression_step: 1.0,
            pressure: 1.0,
            pedal: false,
            gain: db_to_gain(-12.0) * model.output_normalization(),
            motion: MotionCapture::default(),
            motion_frames: 0,
            touch: None,
            legato: Legato::default(),
            voicing: VOICING,
            knock: Vec::new(),
            scratch: Vec::new(),
            width_param,
            pan_step: 1.0,
            quality_param,
            render_quality_param,
            offline: false,
        }
    }

    /// The level a note struck now is simulated at: the render quality while rendering
    /// offline, the live quality otherwise.
    pub fn quality(&self) -> Quality {
        Quality::from_value(self.params.at(if self.offline {
            self.render_quality_param
        } else {
            self.quality_param
        }))
    }

    /// Replaces the piano's register curves and soundboard for offline fitting.
    ///
    /// Available only with the development `piano-calibration` feature, and only on
    /// [`Model::Piano`]. Strikes from the next note on use it. Call off the
    /// audio thread.
    #[cfg(feature = "piano-calibration")]
    pub fn set_piano_voicing(&mut self, voicing: PianoVoicing) {
        if self.model != Model::Piano {
            return;
        }
        self.voicing = voicing;
        for voice in &mut self.voices {
            voice.modal.set_voicing(voicing);
        }
        for body in &mut self.body {
            body.prepare(self.model, self.rate, &self.voicing);
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
            release: self.params.at(P_RELEASE),
            pickup: if self.model == Model::Guitar {
                self.params.at(P_PICKUP)
            } else {
                0.0
            },
            electric: self.model == Model::ElectricGuitar,
            pickup_position: if self.model == Model::ElectricGuitar {
                self.params.at(P_PICKUP_POSITION)
            } else {
                self.params.at(P_POSITION)
            },
            tone: if self.model == Model::ElectricGuitar {
                self.params.at(P_TONE)
            } else {
                12_000.0
            },
            pickup_q: if self.model == Model::ElectricGuitar {
                self.params.at(P_PICKUP_Q)
            } else {
                0.707
            },
            decay_ratio: if self.model == Model::ElectricGuitar {
                self.params.at(P_DECAY_RATIO)
            } else {
                1.0
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

    /// Semitones by which a key's string is tuned from equal temperament: the piano's stretch.
    fn stretch(&self, pitch: u8) -> f32 {
        if self.model == Model::Piano {
            calibration::piano_stretch(pitch, self.settings()) / 100.0
        } else {
            0.0
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

    fn publish_motion(&mut self) {
        let mut frame = MotionFrame {
            geometry: match self.model {
                Model::Bell => MotionGeometry::Shell,
                Model::Mallet => MotionGeometry::Bar,
                _ => MotionGeometry::String,
            },
            active: self.allocator.active_count(),
            expression: if self.model == Model::Violin {
                self.expression_current
            } else {
                self.expression
            },
            pressure: if self.model == Model::Violin {
                self.pressure * self.params.at(P_PRESSURE)
            } else {
                0.0
            },
            pedal: self.pedal,
            ..Default::default()
        };
        let mut selected: [Option<usize>; MOTION_VOICES] = [None; MOTION_VOICES];
        for (index, slot) in self.allocator.slots().iter().enumerate() {
            if !self.voices[index].envelope.is_active() {
                continue;
            }
            for rank in 0..MOTION_VOICES {
                if selected[rank]
                    .is_none_or(|previous| self.allocator.slots()[previous].age < slot.age)
                {
                    selected[rank..].rotate_right(1);
                    selected[rank] = Some(index);
                    break;
                }
            }
        }
        for (observed, index) in frame.voices.iter_mut().zip(selected) {
            let Some(index) = index else {
                break;
            };
            let voice = &self.voices[index];
            observed.pitch = f32::from(self.allocator.slots()[index].pitch) + self.bend;
            observed.level = voice.envelope.level();
            observed.held = voice.held || voice.deferred;
            observed.contact = voice.contact;
            if self.model.is_string() {
                (observed.contact, observed.excitation) = voice.string.motion(&mut observed.points);
            } else {
                observed.excitation =
                    voice
                        .modal
                        .motion(&mut observed.points, &mut observed.modes, self.model);
            }
            for point in &mut observed.points {
                *point *= observed.level;
            }
            for mode in &mut observed.modes {
                *mode *= observed.level;
            }
        }
        self.motion.publish(&frame);
    }

    fn retarget(&mut self, index: usize, pitch: u8, velocity: f32) -> bool {
        let frequency = pitch_to_hz(f32::from(pitch) + self.bend);
        let settings = calibration::register(self.model, pitch, self.settings());
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
        voice.pitch = pitch;
        voice.string.update_loss(settings);
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
        let mut settings = calibration::register(self.model, pitch, self.settings());
        if let Some(at) = self.touch.take() {
            // Along the length that sounds, which is the one a person sees and points at.
            settings.position = touch_position(at);
        }
        let stretch = self.stretch(pitch);
        let quality = self.quality();
        if self.is_legato() {
            self.legato.voice = Some(assignment.index);
        }
        let Some(voice) = self.voices.get_mut(assignment.index) else {
            return;
        };
        voice.tail = if assignment.stolen { voice.last } else { 0.0 };
        voice.fade = if assignment.stolen { 1.0 } else { 0.0 };
        voice.envelope.silence();
        voice.presence.reset();
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
        voice.pitch = pitch;
        voice.deferred = false;
        voice.age = 0;
        voice.contact = settings.position;
        voice.position = self.model.stereo_image().position(f32::from(pitch));
        voice.pan = voice.position * self.params.at(self.width_param);
        let hz = pitch_to_hz(f32::from(pitch) + self.bend + stretch).min(self.rate * 0.2);
        if self.model.is_string() {
            voice.oversampling = quality.oversampling();
            voice.decimator.reset();
            voice.string.excite(
                self.model,
                hz,
                velocity,
                self.rate * voice.oversampling as f32,
                voice.oversampling as f32,
                settings,
            );
        } else {
            voice
                .modal
                .excite(self.model, hz, velocity, self.rate, quality, settings);
        }
        voice.limit = voice
            .modal
            .lifetime(self.rate, settings)
            .unwrap_or((settings.decay * 2.0 * self.rate) as usize);
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
        let settings = self.settings();
        for index in self.allocator.note_off(pitch) {
            if let Some(voice) = self.voices.get_mut(index) {
                voice.held = false;
                if self.model != Model::Piano {
                    voice.envelope.release();
                } else if self.pedal {
                    voice.deferred = true;
                } else {
                    // The damper stops the strings; the keys above it let them ring.
                    let settings = calibration::register(self.model, voice.pitch, settings);
                    voice.modal.damp(self.rate, settings);
                }
            }
        }
    }

    fn set_pedal(&mut self, down: bool) {
        if down == self.pedal {
            return;
        }
        self.pedal = down;
        let settings = self.settings();
        for voice in &mut self.voices {
            if !voice.envelope.is_active() || voice.envelope.is_releasing() {
                continue;
            }
            let settings = calibration::register(self.model, voice.pitch, settings);
            if !down && voice.deferred {
                voice.deferred = false;
                voice.modal.damp(self.rate, settings);
            } else if down && !voice.held && voice.modal.is_damped() {
                // Pressing the pedal lifts every damper, catching a note already decaying.
                voice.deferred = true;
                voice.modal.lift(self.rate, settings);
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
                || self.model == Model::Piano && id.0 == P_RELEASE
                || self.model == Model::Guitar && id.0 == P_PICKUP
                || self.model == Model::ElectricGuitar
                    && matches!(
                        id.0,
                        P_PICKUP_POSITION | P_TONE | P_PICKUP_Q | P_DECAY_RATIO
                    )
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
                    let settings = calibration::register(self.model, voice.pitch, settings);
                    if self.model.is_string() {
                        voice.string.update_loss(settings);
                    } else {
                        voice.modal.update_loss(self.model, self.rate, settings);
                    }
                    voice.limit = voice
                        .modal
                        .lifetime(self.rate, settings)
                        .unwrap_or((settings.decay * 2.0 * self.rate) as usize);
                }
            }
        }
    }
}

/// A touch anywhere along a string, folded onto the half the excitation position measures: a
/// string fixed at both ends sounds the same struck at a quarter from either end.
pub(crate) fn touch_position(at: f32) -> f32 {
    let at = finite_or(at, 0.25).clamp(0.0, 1.0);
    at.min(1.0 - at).clamp(0.02, 0.5)
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
                        self.expression = calibration::expression(self.model, value);
                        if self.allocator.active_count() == 0 {
                            self.expression_current = self.expression;
                        }
                    }
                    64 if self.model == Model::Piano => self.set_pedal(value >= 0.5),
                    TOUCH_CONTROLLER => self.touch = Some(value),
                    _ => {}
                }
            }
        }
    }

    fn render_segment(&mut self, out: &mut AudioBuffer, start: usize, end: usize) {
        let Some((first, rest)) = out.channels_mut().split_first_mut() else {
            return;
        };
        let Some(left) = first.get_mut(start..end) else {
            return;
        };
        left.fill(0.0);
        let stereo = !rest.is_empty();
        let right = match rest.first_mut() {
            Some(channel) => channel.get_mut(start..end),
            None => self.scratch.get_mut(..end - start),
        };
        let Some(right) = right else {
            return;
        };
        right.fill(0.0);
        let mut empty = [];
        let knock = self.knock.get_mut(..left.len()).unwrap_or(&mut empty[..]);
        knock.fill(0.0);
        let pressure = self.pressure * self.params.at(P_PRESSURE);
        let width = self.params.at(self.width_param);
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if !voice.envelope.is_active() {
                continue;
            }
            let frequency = pitch_to_hz(f32::from(voice.pitch) + self.bend);
            let target = voice.position * width;
            for (frame, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
                let raw = if self.model.is_string() {
                    let bow = if voice.held { self.expression } else { 0.0 };
                    if voice.oversampling > 1 {
                        let first = voice.string.next(bow, pressure);
                        let second = voice.string.next(bow, pressure);
                        voice.decimator.process(first, second)
                    } else {
                        voice.string.next(bow, pressure)
                    }
                } else {
                    voice.modal.next()
                };
                let raw = voice.presence.process(raw, frequency, 1.0);
                let envelope = voice.envelope.process();
                if let Some(thump) = knock.get_mut(frame) {
                    *thump += voice.modal.thump() * envelope;
                }
                let current = raw * envelope;
                voice.last = current * (1.0 - voice.fade) + voice.tail * voice.fade;
                voice.fade = (voice.fade - 1.0 / (0.002 * self.rate)).max(0.0);
                voice.pan += (target - voice.pan) * self.pan_step;
                let (to_left, to_right) = pan_gains(voice.pan);
                *l += voice.last * to_left;
                *r += voice.last * to_right;
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
        let [body_left, body_right] = &mut self.body;
        let mut thumps = knock.iter();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            // The soundboard's knock comes from the whole case, so it sits in the centre.
            let thump = thumps.next().copied().unwrap_or(0.0);
            let expression = if self.model == Model::Violin {
                self.expression_current +=
                    (self.expression - self.expression_current) * self.expression_step;
                self.expression_current
            } else {
                self.expression
            };
            let (signal_left, signal_right) = if self.model == Model::ElectricGuitar {
                (*l, *r)
            } else {
                (
                    body_left.next(*l, body) + thump,
                    body_right.next(*r, body) + thump,
                )
            };
            let gain = self.gain * self.volume * expression;
            *l = signal_left * gain;
            *r = signal_right * gain;
            if !stereo {
                *l = (*l + *r) * 0.5;
            }
        }
        downmix_to_extra_channels(out, start, end);
    }
}

impl Instrument for Physical {
    fn motion_monitor(&self) -> Option<std::sync::Arc<auris_core::motion::MotionMonitor>> {
        Some(self.motion.monitor())
    }
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
            .map(|_| Voice::new(self.model, self.rate, self.voicing))
            .collect();
        self.allocator.prepare(VOICES);
        for body in &mut self.body {
            body.prepare(self.model, self.rate, &self.voicing);
        }
        self.knock = if self.model == Model::Piano {
            vec![0.0; ctx.max_block_frames]
        } else {
            Vec::new()
        };
        self.scratch = vec![0.0; ctx.max_block_frames];
        // About ten milliseconds, so turning the width control never clicks.
        self.pan_step = 1.0 - (-1.0 / (0.010 * self.rate)).exp();
        self.update_expression_step();
        self.reset();
    }
    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.envelope.silence();
            voice.presence.reset();
            voice.last = 0.0;
            voice.fade = 0.0;
        }
        for body in &mut self.body {
            body.reset();
        }
        self.allocator.clear();
        self.bend = 0.0;
        self.volume = 1.0;
        self.expression = 1.0;
        self.expression_current = 1.0;
        self.pressure = 1.0;
        self.pedal = false;
        self.legato.clear();
        self.touch = None;
        self.motion_frames = 0;
        self.publish_motion();
    }
    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = ctx.block_frames.min(out.frame_count());
        self.offline = ctx.is_offline;
        render_segments(self, events, out, frames);
        if self.motion.is_watched() {
            self.motion_frames = self.motion_frames.saturating_add(frames);
            if self.motion_frames >= (self.rate / 30.0) as usize {
                self.motion_frames = 0;
                self.publish_motion();
            }
        }
    }
    fn active_voices(&self) -> usize {
        self.allocator.active_count()
    }
}

#[cfg(test)]
#[path = "physical/tests.rs"]
mod tests;
