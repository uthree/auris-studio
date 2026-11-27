//! A complete synthesized kit in one instrument and one voice pool.

use auris_core::motion::{
    MOTION_VOICES, MotionCapture, MotionFrame, MotionGeometry, TOUCH_CONTROLLER,
};
use auris_core::param::db_to_gain;
use auris_core::{
    AudioBuffer, Instrument, NoteEvent, ParamDescriptor, ParamId, ParamUnit, Parameterized,
    PluginCategory, PluginDescriptor, PrepareContext, ProcessContext,
};
use auris_dsp::adsr::SILENCE_LEVEL;
use auris_dsp::{Adsr, Biquad, BiquadCoefficients};

use crate::params::{ParamBank, finite_or};
use crate::render::{SegmentRenderer, render_segments, spread_to_all_channels};
use crate::voice::VoiceAllocator;

mod acoustics;
mod calibration;
mod model;
use acoustics::{Acoustics, BAND_HZ};
use model::{Projection, Resonators};

const BANDS: usize = BAND_HZ.len();

const VOICE_COUNT: usize = 24;
const PAD_COUNT: usize = 7;
/// How far velocity moves the beater hardness of a membrane, around a velocity of 0.75.
const VELOCITY_HARDNESS: f32 = 0.5;
/// How much faster than velocity a membrane's diffuse field grows, per octave above 250 Hz.
const FIELD_VELOCITY_TILT: f32 = 0.3;
const TOM_KEYS: [u8; 6] = [41, 43, 45, 47, 48, 50];
const PAD_KEYS: [u8; PAD_COUNT] = [36, 38, 42, 46, 49, 51, 47];

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

    fn geometry(self) -> MotionGeometry {
        if self.is_metal() {
            MotionGeometry::Plate
        } else {
            MotionGeometry::Membrane
        }
    }

    fn is_metal(self) -> bool {
        matches!(
            self,
            Self::ClosedHat | Self::OpenHat | Self::Crash | Self::Ride
        )
    }

    fn profile(self) -> Profile {
        match self {
            Self::Kick => Profile {
                tone: 42.0,
                body: 0.95,
                glide: 0.30,
            },
            Self::Snare => Profile {
                tone: 185.0,
                body: 0.9,
                glide: 0.04,
            },
            Self::ClosedHat | Self::OpenHat => Profile {
                tone: 7_200.0,
                body: 0.2,
                glide: 0.0,
            },
            Self::Crash => Profile {
                tone: 900.0,
                body: 0.15,
                glide: 0.0,
            },
            Self::Ride => Profile {
                tone: 2_650.0,
                body: 0.65,
                glide: 0.0,
            },
            Self::Tom => Profile {
                tone: 103.0,
                body: 0.85,
                glide: 0.12,
            },
        }
    }

    /// The pad's measured bands; a tom's lie between the low and the high tom's.
    fn acoustics(self, pitch: u8) -> Acoustics {
        match self {
            Self::Kick => acoustics::KICK,
            Self::Snare => acoustics::SNARE,
            Self::ClosedHat => acoustics::CLOSEDHAT,
            Self::OpenHat => acoustics::OPENHAT,
            Self::Crash => acoustics::CRASH,
            Self::Ride => acoustics::RIDE,
            Self::Tom => {
                let (low, high) = (acoustics::TOMLOW, acoustics::TOMHIGH);
                let at = tom_register(pitch);
                // Decay times mix as ratios, levels as decibels.
                let mix = |a: [f32; BANDS], b: [f32; BANDS]| -> [f32; BANDS] {
                    std::array::from_fn(|band| a[band] + (b[band] - a[band]) * at)
                };
                let ratio = |a: [f32; BANDS], b: [f32; BANDS]| -> [f32; BANDS] {
                    std::array::from_fn(|band| {
                        (a[band].max(1e-4).ln() * (1.0 - at) + b[band].max(1e-4).ln() * at).exp()
                    })
                };
                Acoustics {
                    t30: ratio(low.t30, high.t30),
                    bloom: mix(low.bloom, high.bloom),
                    field_db: mix(low.field_db, high.field_db),
                }
            }
        }
    }
}

/// How much faster every loss runs at `damping` than at the factory 0.35.
fn damping_scale(damping: f32) -> f32 {
    (0.5 + damping * 2.0) / 1.2
}

/// Where a tom key sits between the low tom (41) and the high tom (50) the captures hold.
fn tom_register(pitch: u8) -> f32 {
    ((f32::from(pitch) - 41.0) / 9.0).clamp(-0.5, 1.5)
}

/// A tom's settled pitch. The captured low and high toms settle at 63 and 135 Hz, a little over
/// an octave across nine keys: a kit's toms are spread wider than the keys that name them.
fn tom_tone(pitch: u8) -> f32 {
    63.0 * (135.0_f32 / 63.0).powf(tom_register(pitch))
}

#[derive(Clone, Copy, Debug)]
struct Profile {
    /// The lowest mode, once the head has settled.
    tone: f32,
    /// Level of the modes against the diffuse field.
    body: f32,
    /// How far above `tone` a full-velocity strike starts, as a fraction: tension modulation.
    glide: f32,
}

/// Seconds a band takes to fall 30 dB, read at `hz` between the measured octave centres.
fn t30_at(table: &[f32; BANDS], hz: f32) -> f32 {
    let position = (hz.max(1.0) / BAND_HZ[0])
        .log2()
        .clamp(0.0, (BANDS - 1) as f32);
    let below = (position as usize).min(BANDS - 2);
    let fraction = position - below as f32;
    // Interpolate the logarithm: a decay time is a ratio, not a level.
    (table[below].ln() * (1.0 - fraction) + table[below + 1].ln() * fraction).exp()
}

/// Per-sample amplitude factor of a fall of 30 dB in `seconds`.
fn t30_loss(seconds: f32, rate: f32) -> f32 {
    (-1.5 * std::f32::consts::LN_10 / (seconds.max(1e-3) * rate)).exp()
}

#[derive(Clone, Debug)]
struct Hit {
    pad: Pad,
    pitch: u8,
    profile: Profile,
    /// Attack, choke and steal ramps only; the hit's own losses decide how long it rings.
    amplitude: Adsr,
    resonators: Resonators,
    shell: Biquad,
    /// The diffuse field: snare wires, shell and beater noise, and a cymbal's unresolved modes.
    field: [Biquad; BANDS],
    field_gain: [f32; BANDS],
    field_level: [f32; BANDS],
    field_loss: [f32; BANDS],
    /// What is still to bloom in each band, falling from one to zero.
    bloom: [f32; BANDS],
    bloom_loss: [f32; BANDS],
    /// The slowest of the hit's losses: once it is 80 dB down, the hit is over.
    tail: f32,
    tail_loss: f32,
    noise_state: u32,
    velocity: f32,
    contact: f32,
}

impl Hit {
    fn prepared(pad: Pad, pitch: u8, sample_rate: f32, projection: &Projection) -> Self {
        let mut profile = pad.profile();
        if pad == Pad::Tom {
            profile.tone = tom_tone(pitch);
            // Looser, lower heads bend further when struck.
            profile.glide *= (0.5 - tom_register(pitch)).exp2();
        }
        let bands = pad.acoustics(pitch);
        let mut amplitude = Adsr::new();
        amplitude.set_sample_rate(sample_rate);
        amplitude.set_adsr(0.001, 0.0, 1.0, 0.002);
        // White noise of a fixed variance spreads thinner per hertz at a higher rate.
        let density = (sample_rate / 48_000.0).sqrt();
        let audible = |band: usize| BAND_HZ[band] < sample_rate * 0.45;
        let longest = bands.t30.iter().copied().fold(0.0, f32::max);
        Self {
            pad,
            pitch,
            profile,
            amplitude,
            resonators: Resonators::prepared(projection, sample_rate, profile.tone, |hz| {
                t30_at(&bands.t30, hz)
            }),
            shell: Biquad::new(BiquadCoefficients::bandpass(
                sample_rate as f64,
                (profile.tone * 0.8).min(sample_rate * 0.4),
                0.9,
            )),
            field: std::array::from_fn(|band| {
                Biquad::new(if audible(band) {
                    BiquadCoefficients::bandpass(
                        f64::from(sample_rate),
                        BAND_HZ[band],
                        std::f32::consts::SQRT_2,
                    )
                } else {
                    BiquadCoefficients::identity()
                })
            }),
            field_gain: std::array::from_fn(|band| {
                if audible(band) {
                    db_to_gain(bands.field_db[band]) * density
                } else {
                    0.0
                }
            }),
            field_level: [1.0; BANDS],
            field_loss: std::array::from_fn(|band| t30_loss(bands.t30[band], sample_rate)),
            bloom: [1.0; BANDS],
            bloom_loss: std::array::from_fn(|band| {
                let rise = bands.bloom[band];
                if rise > 0.0 {
                    (-1.0 / (rise * sample_rate)).exp()
                } else {
                    0.0
                }
            }),
            tail: 1.0,
            tail_loss: t30_loss(longest, sample_rate),
            noise_state: 1,
            velocity: 0.0,
            contact: 0.6,
        }
    }

    /// Mean level of the diffuse field, for the motion display.
    fn field_energy(&self) -> f32 {
        self.field_level
            .iter()
            .zip(&self.bloom)
            .map(|(level, bloom)| level * (1.0 - bloom))
            .sum::<f32>()
            / BANDS as f32
    }

    fn next(&mut self) -> f32 {
        if !self.amplitude.is_active() {
            return 0.0;
        }
        let body = self.resonators.next();
        let body = if self.pad.is_metal() {
            body
        } else {
            // A lossy cavity takes its excitation from the moving head, not a second oscillator.
            body + self.shell.process_sample(body) * 0.18
        };
        self.noise_state ^= self.noise_state << 13;
        self.noise_state ^= self.noise_state >> 17;
        self.noise_state ^= self.noise_state << 5;
        let noise = (self.noise_state >> 8) as f32 / 8_388_608.0 - 1.0;
        let mut field = 0.0;
        for band in 0..BANDS {
            let gain = self.field_gain[band];
            if gain == 0.0 {
                continue;
            }
            self.field_level[band] *= self.field_loss[band];
            self.bloom[band] *= self.bloom_loss[band];
            let drive = noise * gain * self.field_level[band] * (1.0 - self.bloom[band]);
            field += self.field[band].process_sample(drive);
        }
        self.tail *= self.tail_loss;
        // Once: a ramp restarted every sample shrinks geometrically and never reaches zero.
        if self.tail < SILENCE_LEVEL && !self.amplitude.is_releasing() {
            self.amplitude.kill();
        }
        (body * self.profile.body + field) * self.amplitude.process() * self.velocity
    }
}

#[derive(Clone, Debug)]
struct KitVoice {
    current: Hit,
    // A stolen hit retains its modal state and filters during the envelope's de-click ramp.
    retiring: Hit,
}

/// A sample-free physical kit with struck membranes, snappy wires and bending metal plates.
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
    templates: Vec<Hit>,
    projections: Vec<Projection>,
    voices: Vec<KitVoice>,
    allocator: VoiceAllocator,
    strike: u32,
    gain: f32,
    rate: f32,
    motion: MotionCapture,
    motion_frames: usize,
    /// Where on the head a frontend touched, from the centre out, waiting for its strike.
    touch: Option<f32>,
    pad_audio: [Vec<f32>; PAD_COUNT],
    radiation: [[Biquad; 12]; PAD_COUNT],
    radiation_kill: [usize; PAD_COUNT],
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
        let params = ParamBank::new(vec![
            ParamDescriptor::decibels(0u32, "level", "Level", -60.0, 6.0, -1.6),
            ParamDescriptor::percent(1u32, "hardness", "Beater hardness", 0.65),
            ParamDescriptor::percent(2u32, "position", "Strike position", 0.6),
            ParamDescriptor::percent(3u32, "damping", "Damping", 0.35),
            ParamDescriptor::new(4u32, "decay", "Resonance decay", 0.5, 2.0, 1.0)
                .with_unit(ParamUnit::Percent),
        ]);
        let gain = db_to_gain(params.at(0));
        Self {
            params,
            templates: Vec::new(),
            projections: Vec::new(),
            voices: Vec::new(),
            allocator: VoiceAllocator::new(),
            strike: 0,
            gain,
            rate: 48_000.0,
            motion: MotionCapture::default(),
            motion_frames: 0,
            touch: None,
            pad_audio: std::array::from_fn(|_| Vec::new()),
            radiation: std::array::from_fn(|_| std::array::from_fn(|_| Biquad::default())),
            radiation_kill: [0; PAD_COUNT],
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: f32) {
        let Some(pad) = Pad::at_key(pitch) else {
            return;
        };
        let velocity = finite_or(velocity, 0.0).clamp(0.0, 1.0);
        if velocity <= 0.0 {
            return;
        }
        self.radiation_kill[pad as usize] = 0;
        let template_index = if pad == Pad::Tom {
            PAD_COUNT + TOM_KEYS.iter().position(|key| *key == pitch).unwrap_or(3)
        } else {
            pad as usize
        };
        let Some(template) = self.templates.get(template_index) else {
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
        voice.current.pitch = pitch;
        let calibrated = calibration::profile(pad);
        let membrane = !pad.is_metal();
        // A harder blow flattens the felt or the stick tip for less time: the head is struck
        // by a harder hammer, and its upper modes and beater noise grow faster than its thump.
        let blow = if membrane {
            VELOCITY_HARDNESS * (velocity - 0.75)
        } else {
            0.0
        };
        let hardness = (self.params.at(1) - 0.65 + calibrated.hardness + blow).clamp(0.0, 1.0);
        let position = match self.touch.take() {
            // A touch says where the stick landed; the calibration only says where it usually does.
            Some(at) => finite_or(at, 0.5).clamp(0.0, 0.95),
            None => (self.params.at(2) - 0.6 + calibrated.position).clamp(0.0, 1.0),
        };
        let damping = (self.params.at(3) - 0.35 + calibrated.damping).clamp(0.0, 1.0);
        voice.current.contact = position;
        let decay = self.params.at(4) * calibrated.decay;
        // Every loss shortens with damping and lengthens with decay; neither moves a pitch.
        let exponent = damping_scale(damping) / decay;
        let hit = &mut voice.current;
        for band in 0..BANDS {
            hit.field_loss[band] = hit.field_loss[band].powf(exponent);
            if membrane {
                let octaves = (BAND_HZ[band] / 250.0).log2().clamp(0.0, 5.0);
                hit.field_gain[band] *= velocity.powf(FIELD_VELOCITY_TILT * octaves);
            }
        }
        hit.tail_loss = hit.tail_loss.powf(exponent);
        let glide = hit.profile.glide * velocity * velocity;
        hit.resonators
            .strike(hardness, position, damping_scale(damping), decay, glide);
        if pad == Pad::Ride {
            voice.current.resonators.soften_ride_strike();
        }
        voice.current.amplitude.trigger();
    }

    fn publish_motion(&mut self) {
        let mut frame = MotionFrame {
            geometry: MotionGeometry::Membrane,
            expression: 1.0,
            ..Default::default()
        };
        let mut selected: [Option<usize>; MOTION_VOICES] = [None; MOTION_VOICES];
        for (index, voice) in self.voices.iter().enumerate() {
            if !voice.current.amplitude.is_active() {
                continue;
            }
            frame.active += 1;
            for rank in 0..MOTION_VOICES {
                if selected[rank].is_none_or(|previous| {
                    self.allocator.slots()[previous].age < self.allocator.slots()[index].age
                }) {
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
            let hit = &self.voices[index].current;
            observed.geometry = Some(hit.pad.geometry());
            observed.pitch = f32::from(hit.pitch);
            observed.level = hit.amplitude.level() * hit.velocity;
            observed.contact = hit.contact;
            observed.excitation = hit.field_energy() * observed.level;
            self.projections[usize::from(hit.pad.is_metal())].observe(
                &hit.resonators,
                observed.level,
                observed,
            );
        }
        self.motion.publish(&frame);
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
                key: pitch,
                velocity,
                ..
            } => self.note_on(pitch, velocity),
            NoteEvent::NoteOff { key: pitch, .. } => {
                self.allocator.note_off(pitch);
            }
            NoteEvent::AllNotesOff { .. } => {
                self.allocator.release_all();
            }
            NoteEvent::AllSoundOff { .. } => {
                self.allocator.release_all();
                self.radiation_kill
                    .fill((self.rate * 0.002).ceil() as usize);
                for voice in &mut self.voices {
                    voice.current.amplitude.kill();
                    voice.retiring.amplitude.kill();
                }
            }
            NoteEvent::Controller {
                number: TOUCH_CONTROLLER,
                value,
                ..
            } => self.touch = Some(value),
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
        if self.pad_audio.iter().any(|audio| audio.len() < end) {
            return;
        }
        for audio in &mut self.pad_audio {
            audio[start..end].fill(0.0);
        }
        for (index, voice) in self.voices.iter_mut().enumerate() {
            if !voice.current.amplitude.is_active() && !voice.retiring.amplitude.is_active() {
                continue;
            }
            for frame in start..end {
                self.pad_audio[voice.current.pad as usize][frame] += voice.current.next();
                self.pad_audio[voice.retiring.pad as usize][frame] += voice.retiring.next();
            }
            self.allocator.set_level(
                index,
                voice.current.amplitude.level() * voice.current.velocity,
            );
            if voice.current.amplitude.is_finished() && voice.retiring.amplitude.is_finished() {
                self.allocator.retire(index);
            }
        }
        // Sum each family before its linear radiation bank, including stolen/choked
        // hits. Storage and coefficients are prepared; these loops never allocate.
        for pad in Pad::ALL {
            let sections = &mut self.radiation[pad as usize];
            let remaining = &mut self.radiation_kill[pad as usize];
            let gain = self.gain * calibration::profile(pad).normalization;
            for (sample, input) in dst
                .iter_mut()
                .zip(&self.pad_audio[pad as usize][start..end])
            {
                let fade = if *remaining == 0 {
                    1.0
                } else {
                    *remaining as f32 / (self.rate * 0.002).ceil()
                };
                *sample += gain
                    * fade
                    * sections
                        .iter_mut()
                        .fold(*input, |value, section| section.process_sample(value));
                if *remaining > 0 {
                    *remaining -= 1;
                    if *remaining == 0 {
                        for section in sections.iter_mut() {
                            section.reset();
                        }
                    }
                }
            }
        }
    }
}

impl Instrument for DrumKit {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor::instrument(
            Self::ID,
            "Physical Drums",
            "Struck membranes, snappy wires and metal plates with shared hat choking",
            PluginCategory::Drum,
        )
    }

    fn prepare(&mut self, ctx: &PrepareContext) {
        let sample_rate = crate::sample_rate_f32(ctx.sample_rate).clamp(8_000.0, 192_000.0);
        self.rate = sample_rate;
        self.projections = vec![Projection::new(false), Projection::new(true)];
        let templates: Vec<_> = Pad::ALL
            .into_iter()
            .zip(PAD_KEYS)
            .chain(TOM_KEYS.into_iter().map(|key| (Pad::Tom, key)))
            .map(|(pad, key)| {
                Hit::prepared(
                    pad,
                    key,
                    sample_rate,
                    &self.projections[usize::from(pad.is_metal())],
                )
            })
            .collect();
        self.voices = (0..VOICE_COUNT)
            .map(|_| KitVoice {
                current: templates[0].clone(),
                retiring: templates[0].clone(),
            })
            .collect();
        self.templates = templates;
        self.pad_audio = std::array::from_fn(|_| vec![0.0; ctx.max_block_frames]);
        for pad in Pad::ALL {
            for (index, (section, gain)) in self.radiation[pad as usize]
                .iter_mut()
                .zip(calibration::profile(pad).gains)
                .enumerate()
            {
                let hz = 90.0_f32 * (10_000.0_f32 / 90.0).powf(index as f32 / 11.0);
                section.set_coefficients(if hz < sample_rate * 0.45 {
                    BiquadCoefficients::peaking(f64::from(sample_rate), hz, 0.9, gain)
                } else {
                    BiquadCoefficients::identity()
                });
                section.reset();
            }
        }
        self.allocator.prepare(VOICE_COUNT);
        self.strike = 0;
        self.motion_frames = 0;
        self.radiation_kill.fill(0);
        self.publish_motion();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.current.amplitude.silence();
            voice.retiring.amplitude.silence();
        }
        self.allocator.clear();
        self.radiation_kill.fill(0);
        for section in self.radiation.iter_mut().flatten() {
            section.reset();
        }
        self.touch = None;
        self.strike = 0;
        self.motion_frames = 0;
        self.publish_motion();
    }

    fn process(&mut self, events: &[NoteEvent], out: &mut AudioBuffer, ctx: &ProcessContext) {
        let frames = ctx.block_frames.min(out.frame_count());
        render_segments(self, events, out, frames);
        spread_to_all_channels(out, frames);
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

    fn motion_monitor(&self) -> Option<std::sync::Arc<auris_core::motion::MotionMonitor>> {
        Some(self.motion.monitor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Rig, band_amplitude, peak, rms};

    fn hit(frame: u32, pitch: u8) -> NoteEvent {
        NoteEvent::note_on(frame, pitch, 1.0)
    }

    /// The level the absolute amplitudes below were measured at, before the kit's default was
    /// trimmed to the other instruments' loudness.
    const MEASURED_LEVEL: f32 = -6.0;

    fn rig(block: usize) -> Rig {
        let mut rig = Rig::new(Box::new(DrumKit::new()), 48_000.0, block, 2);
        rig.set_param("level", MEASURED_LEVEL);
        rig
    }

    fn sound(pitch: u8) -> Vec<f32> {
        rig(512).render(48_000, &[hit(0, pitch)])
    }

    #[test]
    fn the_snare_head_carries_the_strike_and_the_wires_outlast_it() {
        // The captured snares put their first 20 ms in the head's 250 Hz octave, 19 dB above
        // the 2 kHz one, and their wires take twice as long as the head to fall 30 dB.
        let snare = sound(38);
        let attack = &snare[..960];
        let late = &snare[4_800..9_600];
        let head = |audio: &[f32]| band_amplitude(audio, 48_000.0, 250.0);
        let wires = |audio: &[f32]| band_amplitude(audio, 48_000.0, 2_000.0);
        assert!(
            head(attack) > wires(attack) * 2.0,
            "head {} wires {}",
            head(attack),
            wires(attack)
        );
        assert!(
            wires(late) / head(late) > wires(attack) / head(attack) * 2.0,
            "the wires died with the head: attack {}, late {}",
            wires(attack) / head(attack),
            wires(late) / head(late),
        );
    }

    #[test]
    fn a_ride_rings_for_seconds_at_every_rate() {
        // The captured rides fall 30 dB in two to three seconds across 1-4 kHz.
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            let mut rig = Rig::new(Box::new(DrumKit::new()), rate, 256, 2);
            rig.set_param("level", MEASURED_LEVEL);
            let audio = rig.render((rate * 3.0) as usize, &[hit(0, 51)]);
            let window = |at: f64| &audio[(rate * at) as usize..(rate * (at + 0.2)) as usize];
            let ping = band_amplitude(window(0.05), rate, 2_650.0);
            let ringing = rms(window(2.0)) / rms(window(0.05));
            assert!(ping > 1e-4, "{rate}: lost the ping");
            assert!(
                (0.03..0.6).contains(&ringing),
                "{rate}: two seconds on, {ringing}"
            );
        }
    }

    #[test]
    fn kicks_have_a_low_body_and_cymbals_have_high_frequency_energy() {
        use auris_dsp::drum_analysis::analyze_drum_audio;
        let centroid = |key| {
            let audio = AudioBuffer::from_planar(vec![sound(key)], 48_000.0).unwrap();
            analyze_drum_audio(&audio).unwrap().spectrum.centroid_hz
        };
        let kick = sound(36);
        assert!(
            band_amplitude(&kick, 48_000.0, 45.0)
                > band_amplitude(&kick, 48_000.0, 10_000.0) * 100.0
        );
        // The captures' spectral centroids: kick 54-61 Hz, snare 400-580 Hz, closed hat 7 kHz.
        // The kit is close but not equal; these bands are where each drum stops being itself.
        assert!(centroid(36) < 150.0, "kick {}", centroid(36));
        assert!(
            (250.0..1_500.0).contains(&centroid(38)),
            "snare {}",
            centroid(38)
        );
        assert!(centroid(42) > 5_000.0, "closed hat {}", centroid(42));
        for key in [46, 49, 51] {
            assert!(centroid(key) > 2_000.0, "cymbal {key}: {}", centroid(key));
        }
    }

    #[test]
    fn each_pad_has_an_audible_attack_and_a_finite_one_shot_tail() {
        // A ride's bell rings longest: its 500 Hz octave takes over five seconds to fall 30 dB
        // in the captures. The hit ends when its slowest band is 80 dB down.
        let seconds = 30 * 48_000;
        for key in [36, 38, 42, 46, 49, 51, 47] {
            let mut rig = rig(512);
            let audio = rig.render(seconds, &[hit(0, key)]);
            assert!(peak(&audio[..4_800]) > 0.05, "key {key} is too quiet");
            assert!(audio.iter().all(|sample| sample.is_finite()));
            assert!(
                peak(&audio[seconds - 48_000..]) < 1e-7,
                "key {key} did not decay"
            );
            assert_eq!(
                rig.instrument.active_voices(),
                0,
                "key {key} kept its voice"
            );
        }
        assert!(rms(&sound(46)[9_600..14_400]) > rms(&sound(42)[9_600..14_400]) * 3.0);
        assert!(rms(&sound(49)[24_000..28_800]) > 0.001);
    }

    #[test]
    fn a_closed_hat_chokes_an_open_hat_but_leaves_a_crash_ringing() {
        let open = rig(512).render(24_000, &[hit(0, 46)]);
        let closed = rig(512).render(24_000, &[hit(0, 46), hit(4_800, 42)]);
        let alone = rig(512).render(24_000, &[hit(4_800, 42)]);
        assert!(rms(&open[9_600..]) > 0.001);
        // What is left is the closed hat alone: the open one was cut within its ramp.
        assert!(rms(&closed[9_600..]) < rms(&open[9_600..]) * 0.5);
        assert!(rms(&closed[9_600..]) < rms(&alone[9_600..]) * 1.3);
        assert!(peak(&closed[20_000..]) < peak(&open[20_000..]) * 0.05);
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
                    key: 49,
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
    fn velocity_brightens_a_drum_head_but_only_scales_a_cymbal() {
        let at = |key, velocity| rig(512).render(48_000, &[NoteEvent::note_on(0, key, velocity)]);
        let loud = at(49, 1.0);
        let quiet = at(49, 0.5);
        assert!(
            loud.iter()
                .zip(&quiet)
                .all(|(a, b)| (*a * 0.5 - *b).abs() < 1e-7)
        );
        // The captured kicks' and snares' centroids rise by half between soft and hard hits.
        use auris_dsp::drum_analysis::analyze_drum_audio;
        let centroid = |audio: Vec<f32>| {
            let audio = AudioBuffer::from_planar(vec![audio[..9_600].to_vec()], 48_000.0).unwrap();
            analyze_drum_audio(&audio).unwrap().spectrum.centroid_hz
        };
        // The captured toms' centroids move with pitch more than with velocity; they are left out.
        for key in [36, 38] {
            let (hard, soft) = (centroid(at(key, 1.0)), centroid(at(key, 0.35)));
            assert!(hard > soft * 1.1, "key {key}: hard {hard}, soft {soft}");
        }
    }

    #[test]
    fn a_struck_kick_starts_sharp_and_settles_onto_its_tuning() {
        // The captured kick reads 51 Hz in its first 60 ms and 41 Hz by 300 ms.
        let kick = sound(36);
        let strongest = |from: usize| {
            let window = &kick[from..from + 2_880];
            (30..90)
                .map(|hz| (hz, band_amplitude(window, 48_000.0, f64::from(hz))))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap()
                .0
        };
        assert!(
            strongest(0) > strongest(14_400) + 3,
            "{} -> {}",
            strongest(0),
            strongest(14_400)
        );
        assert!(
            (38..47).contains(&strongest(14_400)),
            "{}",
            strongest(14_400)
        );
    }

    #[test]
    fn dense_chokes_voice_steals_and_reset_allocate_nothing() {
        let mut kit = DrumKit::new();
        kit.prepare(&PrepareContext::new(48_000.0, 512, 2));
        kit.motion_monitor().unwrap().watch(true);
        let mut output = AudioBuffer::stereo(512, 48_000.0);
        let context = ProcessContext::realtime(48_000.0, 512, 0, 120.0, true);
        let events: [NoteEvent; 80] =
            std::array::from_fn(|index| hit(index as u32 * 5, [36, 38, 46, 49, 42][index % 5]));
        let allocations = crate::test_support::count_allocations(|| {
            kit.process(&events, &mut output, &context);
            for _ in 0..4 {
                kit.process(&[], &mut output, &context);
            }
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

    #[test]
    fn mixed_mechanical_observation_matches_live_modes_and_never_changes_audio() {
        let mut watched = rig(256);
        let mut plain = rig(256);
        let monitor = watched.instrument.motion_monitor().unwrap();
        monitor.watch(true);
        let events = [hit(0, 36), hit(100, 49), hit(200, 47)];
        assert_eq!(watched.render(2_000, &events), plain.render(2_000, &events));
        let frame = monitor.read().unwrap();
        assert_eq!(frame.active, 3);
        assert_eq!(frame.voices[0].pitch, 47.0);
        assert_eq!(frame.voices[0].geometry, Some(MotionGeometry::Membrane));
        assert_eq!(frame.voices[1].geometry, Some(MotionGeometry::Plate));
        for voice in frame.voices.iter().take(3) {
            assert!(voice.points.iter().any(|value| value.abs() > 1e-6));
            assert!(voice.modes.iter().any(|value| *value > 1e-6));
            assert!(voice.points.iter().all(|value| value.is_finite()));
        }
        monitor.watch(false);
        watched.render(2_000, &[]);
        assert_eq!(monitor.read(), Some(frame));
        watched.instrument.reset();
        let reset = monitor.read().unwrap();
        assert_eq!(reset.active, 0);
        assert!(reset.voices.iter().all(|voice| voice.points == [0.0; 64]));
    }

    const ROLES_THE_ANALYZER_READS_IN_CAPTURES: [(u8, auris_core::DrumRole); 5] = [
        (36, auris_core::DrumRole::Kick),
        (42, auris_core::DrumRole::ClosedHat),
        (47, auris_core::DrumRole::Tom),
        (48, auris_core::DrumRole::Tom),
        (50, auris_core::DrumRole::Tom),
    ];

    #[test]
    fn acoustic_analysis_recognizes_the_generated_kit_roles_without_note_labels() {
        use auris_dsp::drum_analysis::analyze_drum_audio;
        // Only the pads whose captured hits the analyzer itself reads correctly: it calls the
        // captured snare a tom, the captured open hat and ride crashes, and the captured low tom
        // a kick, and the kit that sounds like them is read the same way.
        for (pitch, role) in ROLES_THE_ANALYZER_READS_IN_CAPTURES {
            let audio = rig(512).render(144_000, &[hit(0, pitch)]);
            let mut buffer = AudioBuffer::new(1, audio.len(), 48_000.0);
            buffer.channel_mut(0).copy_from_slice(&audio);
            let measured = analyze_drum_audio(&buffer).unwrap();
            let fitness = measured.fitness[&role];
            assert!(fitness > 0.6, "{role:?}: {fitness}");
            assert!(
                measured
                    .fitness
                    .iter()
                    .all(|(candidate, score)| *candidate == role || *score < fitness),
                "ambiguous {role:?}: {:?}",
                measured.fitness
            );
        }
    }

    #[test]
    fn drum_controls_change_excitation_and_decay_and_toms_keep_their_tuning() {
        let render = |key, parameter: &str, value| {
            let mut instrument = rig(256);
            instrument.set_param(parameter, value);
            instrument.render(48_000, &[hit(0, key)])
        };
        for key in [36, 38, 42, 49, 47] {
            for parameter in ["hardness", "position", "damping"] {
                let first = render(key, parameter, 0.15);
                let second = render(key, parameter, 0.85);
                assert!(
                    first.iter().zip(&second).any(|(a, b)| (a - b).abs() > 1e-4),
                    "{key} ignored {parameter}"
                );
            }
        }
        let damped = render(36, "damping", 1.0);
        let ringing = render(36, "damping", 0.0);
        assert!(rms(&ringing[4_800..9_600]) > rms(&damped[4_800..9_600]) * 1.1);
        let short = render(49, "decay", 0.5);
        let long = render(49, "decay", 2.0);
        assert!(rms(&long[24_000..]) > rms(&short[24_000..]) * 2.0);
        for key in TOM_KEYS {
            // After the glide has settled.
            let audio = &sound(key)[9_600..];
            let fundamental = f64::from(tom_tone(key));
            assert!(
                band_amplitude(audio, 48_000.0, fundamental)
                    > band_amplitude(audio, 48_000.0, fundamental * 1.15) * 2.0,
                "tom {key} lost its fundamental"
            );
        }
        assert!((tom_tone(41) - 63.0).abs() < 0.5 && (tom_tone(50) - 135.0).abs() < 0.5);
    }

    #[test]
    fn extreme_controls_rates_and_repeated_preparation_remain_finite() {
        for rate in [8_000.0, 22_050.0, 44_100.0, 96_000.0, 192_000.0] {
            let mut kit = DrumKit::new();
            kit.prepare(&PrepareContext::new(rate, 256, 2));
            let monitor = kit.motion_monitor().unwrap();
            monitor.watch(true);
            let mut out = AudioBuffer::stereo(256, rate);
            let context = ProcessContext::realtime(rate, 256, 0, 120.0, true);
            for value in [0.0, 1.0, f32::NAN, f32::INFINITY] {
                kit.set_param_by_key("hardness", value);
                kit.set_param_by_key("position", value);
                kit.set_param_by_key("damping", value);
                kit.set_param_by_key("decay", value);
                for key in PAD_KEYS {
                    kit.process(&[hit(0, key)], &mut out, &context);
                    assert!(out.channel(0).iter().all(|value| value.is_finite()));
                }
                let frame = monitor.read().unwrap();
                assert!(
                    frame
                        .voices
                        .iter()
                        .all(|voice| voice.points.iter().all(|value| value.is_finite()))
                );
            }
            kit.prepare(&PrepareContext::new(rate, 256, 2));
            assert_eq!(monitor.read().unwrap().active, 0);
        }
    }
}
