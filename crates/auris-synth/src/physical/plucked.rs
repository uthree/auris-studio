//! Plucked strings: an acoustic guitar, an electric guitar and a bass.
//!
//! A pluck is the simplest excitation there is — the string is held aside at one point and let
//! go — and everything that follows is the string's own. Held aside it is a triangle, whose
//! `n`th harmonic is `sin(n π position) / n²`; the finger or pick lets go over a finite time,
//! which leaves out the partials faster than that; and from then on each partial turns at its
//! own slightly sharp frequency and dies at its own rate, which is where steel and nylon part
//! company. Nothing circulates and nothing is interpolated, so a string is in tune at every
//! pitch and sample rate and decays as its material says it should.
//!
//! An acoustic string moves in two planes. The one across the soundboard drives the bridge, is
//! heard at once and is drained by the body; the one along it barely moves the bridge, and
//! rings on underneath, a cent or so away. That is the guitar's two-stage decay and its slow
//! shimmer, and it costs a second set of partials for the low end of the string.
//!
//! What is listened to differs by instrument. An acoustic's bridge force goes to its body. An
//! electric's string is watched by a magnet part-way along it, which hears the string's
//! velocity there and so a comb of the partials that have a node elsewhere, through the
//! resonant low-pass its coil and cable make.

use std::f32::consts::PI;

use auris_core::motion::{MOTION_BODY, MOTION_PARTIALS, MOTION_POINTS, MotionVoice};
use auris_core::{ParamDescriptor, ParamUnit};
use auris_dsp::{Biquad, BiquadCoefficients};

use super::body::{self, Radiator, seeded};
use super::rotors::{Rotor, Rotors, decay_rate, pole};
use super::strings::{
    BASS_STRINGS, GUITAR_STRINGS, Losses, fretted, material, partial_ratio, pluck_share,
    release_rolloff,
};
use super::{
    Declick, Engine, Model, Mute, Note, Performance, Quality, TICK, hertz, touch_position,
};
use crate::ParamBank;

const P_HARDNESS: u32 = 0;
const P_POSITION: u32 = 1;
const P_DECAY: u32 = 2;
const P_DAMPING: u32 = 3;
/// Body resonance on the acoustic models; the pickup's place on the electric guitar.
const P_BODY: u32 = 4;
const P_PICKUP_POSITION: u32 = 4;
const P_RELEASE: u32 = 5;
/// The guitar's under-saddle pickup blend; the pickup resonance of the electric models.
const P_PICKUP: u32 = 6;
const P_TONE: u32 = 6;
const P_PICKUP_Q: u32 = 7;
const P_DECAY_RATIO: u32 = 8;

/// Most partials of one polarisation.
const PARTIALS: usize = 128;
/// Partials that get a second polarisation: the ones that ring long enough to beat.
const PAIRED: usize = 40;
const ROTORS: usize = PARTIALS + PAIRED;
/// How much of the plane along the soundboard reaches the bridge.
const ALONG: f32 = 0.3;
/// Below this a voice's phasors are taken to have stopped: about -110 dB under a full pluck.
const FLOOR: f32 = 3.0e-6;
/// Scale of the picture a frontend draws, chosen so a full pluck fills its lane.
const DISPLAY: f32 = 0.05;

/// How far a player goes toward evening out a note the box favours or refuses, as the power
/// of how much it does.
const EVENING: f32 = 0.6;

pub(super) fn descriptors(model: Model) -> Vec<ParamDescriptor> {
    let (hardness, position, decay, damping, release) = match model {
        Model::ElectricGuitar => (0.6, 0.13, 9.0, 0.03, 0.1),
        Model::Bass => (0.35, 0.16, 7.0, 0.12, 0.1),
        _ => (0.2, 0.26, 6.5, 0.1, 0.12),
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
        ParamDescriptor::percent(P_DAMPING, "damping", "Damping", damping),
    ];
    descriptors.push(match model {
        Model::ElectricGuitar => ParamDescriptor::new(
            P_PICKUP_POSITION,
            "pickup_position",
            "Pickup Position",
            0.02,
            0.3,
            0.16,
        )
        .with_unit(ParamUnit::Percent),
        Model::Bass => ParamDescriptor::percent(P_BODY, "body", "Body Resonance", 0.0),
        _ => ParamDescriptor::percent(P_BODY, "body", "Body Resonance", 1.0),
    });
    descriptors.push(
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 3.0, release)
            .with_unit(ParamUnit::Seconds),
    );
    match model {
        Model::ElectricGuitar => {
            descriptors.push(
                ParamDescriptor::new(P_TONE, "tone", "Pickup Resonance", 800.0, 12_000.0, 3_200.0)
                    .with_unit(ParamUnit::Hertz),
            );
            descriptors.push(ParamDescriptor::new(
                P_PICKUP_Q, "pickup_q", "Pickup Q", 0.5, 2.5, 1.6,
            ));
            descriptors.push(
                ParamDescriptor::new(
                    P_DECAY_RATIO,
                    "decay_ratio",
                    "Decay per Octave",
                    0.2,
                    1.5,
                    0.6,
                )
                .with_unit(ParamUnit::Ratio),
            );
        }
        Model::Bass => descriptors.push(
            ParamDescriptor::new(P_TONE, "tone", "Pickup Resonance", 400.0, 8_000.0, 2_400.0)
                .with_unit(ParamUnit::Hertz),
        ),
        _ => descriptors.push(ParamDescriptor::percent(
            P_PICKUP,
            "pickup",
            "Pickup blend",
            0.0,
        )),
    }
    descriptors
}

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    hardness: f32,
    position: f32,
    decay: f32,
    damping: f32,
    /// Share of the sound that comes off a body rather than a magnet.
    body: f32,
    release: f32,
    /// The acoustic guitar's saddle pickup, heard instead of its body.
    saddle: f32,
    pickup_position: f32,
    tone: f32,
    pickup_q: f32,
    decay_ratio: f32,
}

/// What differs between the three instruments and is not a control.
struct Build {
    strings: &'static [u8],
    /// Inharmonicity of each open string.
    stiffness: &'static [f32],
    /// Pitch at which the decay control is the fundamental's decay time.
    reference: f32,
    /// Exponent by which higher notes decay faster: the terminations see more of them.
    tracking: f32,
    /// Cents a full pluck pulls the string sharp before it settles.
    glide: f32,
    /// Factor on the material's loss factors: wound strings are deader than plain ones.
    wound: f32,
    /// Loudness of one pluck before the instrument's own normalisation.
    drive: f32,
}

impl Build {
    fn of(model: Model) -> Self {
        match model {
            Model::ElectricGuitar => Self {
                strings: &GUITAR_STRINGS,
                // A plain third string is the stiffest on the instrument.
                stiffness: &[5.0e-5, 3.5e-5, 2.5e-5, 1.1e-4, 6.0e-5, 3.0e-5],
                reference: 64.0,
                tracking: 0.0,
                glide: 9.0,
                wound: 1.0,
                drive: 1.0,
            },
            Model::Bass => Self {
                strings: &BASS_STRINGS,
                stiffness: &[9.0e-5, 7.0e-5, 5.0e-5, 4.0e-5],
                reference: 40.0,
                tracking: 0.45,
                glide: 12.0,
                wound: 2.5,
                drive: 1.0,
            },
            _ => Self {
                strings: &GUITAR_STRINGS,
                stiffness: &[4.5e-5, 3.5e-5, 3.0e-5, 2.5e-5, 5.0e-5, 3.0e-5],
                reference: 55.0,
                tracking: 0.45,
                glide: 5.0,
                wound: 1.0,
                drive: 1.0,
            },
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Voice {
    rotors: Rotors,
    /// Frequency and decay rate of every phasor, from which a bend or a finger works out its
    /// pole.
    hz: Vec<f32>,
    nepers: Vec<f32>,
    count: usize,
    /// Partials of the plane across the soundboard.
    partials: usize,
    /// Partials that also move along it.
    paired: usize,
    pitch: f32,
    velocity: f32,
    fundamental: f32,
    stiffness: f32,
    /// Sounding length as a fraction of the open string's.
    fret: f32,
    contact: f32,
    sounding: bool,
    held: bool,
    pan: f32,
    pan_now: f32,
    /// Phase of the finger letting go, from zero to one.
    attack: f32,
    attack_step: f32,
    declick: Declick,
    mute: Mute,
    last: f32,
    /// How much is moving now and how much was at the pluck.
    moving: f32,
    plucked: f32,
    /// Energy of the lowest partials at the pluck, against which the pitch glide is measured.
    energy: f32,
    glide: f32,
    /// Cents the phasors are currently tuned away from the note.
    tuned: f32,
    /// The string the note is fretted on, while the finger holds it there.
    string: Option<usize>,
}

impl Voice {
    /// Index of partial `n` (from zero) in the plane across the soundboard.
    fn across(&self, n: usize) -> usize {
        if n < self.paired {
            2 * n
        } else {
            self.paired + n
        }
    }

    /// Energy of the lowest few partials, which is what stretches the string.
    fn low_energy(&self) -> f32 {
        (0..self.partials.min(6))
            .map(|n| {
                let (re, im) = self.rotors.at(self.across(n));
                re * re + im * im
            })
            .sum()
    }

    /// Recomputes every pole for a pitch `cents` away from the note.
    fn retune(&mut self, cents: f32, rate: f32) {
        let factor = (cents / 1200.0).exp2();
        for index in 0..self.count {
            let (Some(hz), Some(nepers)) = (self.hz.get(index), self.nepers.get(index)) else {
                break;
            };
            let hz = hz * factor;
            if hz >= rate * 0.49 {
                // Bent out of the band: stopped rather than folded back into it.
                self.rotors.turn(index, 0.0, 0.0);
                continue;
            }
            let (cosine, sine) = pole(hz, *nepers, rate);
            self.rotors.turn(index, cosine, sine);
        }
        self.tuned = cents;
    }
}

/// The three plucked instruments.
pub(super) struct Plucked {
    model: Model,
    build: Build,
    rate: f32,
    settings: Settings,
    performance: Performance,
    voices: Vec<Voice>,
    body: Radiator,
    /// The acoustic guitar's strings that are not being played.
    open: OpenStrings,
    /// The pickup's coil and cable: a resonant low-pass, on the mid and the side signal.
    electronics: [Biquad; 2],
    pan_step: f32,
}

impl Plucked {
    pub(super) fn new(model: Model) -> Self {
        Self {
            model,
            build: Build::of(model),
            rate: 48_000.0,
            settings: Settings::default(),
            performance: Performance::default(),
            voices: Vec::new(),
            body: Radiator::default(),
            open: OpenStrings::default(),
            electronics: Default::default(),
            pan_step: 1.0,
        }
    }

    fn is_electric(&self) -> bool {
        self.model == Model::ElectricGuitar
    }

    /// Share of the sound taken from the bridge and given to a body.
    fn acoustic(&self) -> f32 {
        if self.is_electric() {
            0.0
        } else {
            self.settings.body
        }
    }

    /// Whether a magnet is heard at all.
    fn magnetic(&self) -> f32 {
        match self.model {
            Model::ElectricGuitar => 1.0,
            Model::Bass => 1.0 - self.settings.body,
            _ => 0.0,
        }
    }

    /// The fundamental's decay rate for a note at `pitch`, in nepers per second.
    fn fundamental_rate(&self, pitch: f32) -> f32 {
        let settings = &self.settings;
        let seconds = if self.is_electric() {
            (f64::from(settings.decay)
                * f64::from(settings.decay_ratio).powf((f64::from(pitch) - 64.0) / 12.0))
            .clamp(0.1, 40.0) as f32
        } else {
            settings.decay
        };
        let register = hertz(pitch) / hertz(self.build.reference);
        decay_rate(seconds) * (1.0 + 3.0 * settings.damping) * register.powf(self.build.tracking)
    }

    /// Works out the decay rate of every phasor of `voice` from the controls as they stand.
    fn losses(&self, index: usize) -> Losses {
        let voice = &self.voices[index];
        let (friction, bending) = material(self.settings.damping);
        Losses::tuned(
            self.fundamental_rate(voice.pitch),
            voice.fundamental,
            voice.stiffness,
            friction * self.build.wound,
            bending * self.build.wound,
        )
    }

    fn set_rates(&mut self, index: usize) {
        let losses = self.losses(index);
        let acoustic = self.acoustic();
        let spec = self.body_spec();
        let scale = self.body_scale();
        let release = self.settings.release;
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        for n in 0..voice.partials {
            let order = (n + 1) as f32;
            let across = voice.across(n);
            let Some(hz) = voice.hz.get(across).copied() else {
                break;
            };
            let mut nepers = losses.rate(hz, voice.stiffness * order * order);
            if !voice.held {
                // A finger laid on the string takes the top first.
                nepers += decay_rate(release) * (1.0 + n as f32 / 6.0);
            }
            if n < voice.paired
                && let Some(slot) = voice.nepers.get_mut(across + 1)
            {
                *slot = nepers;
            }
            // The plane that drives the bridge is the one the body drains.
            let drained =
                nepers + acoustic * 0.006 * voice.fundamental * spec.conductance(hz, scale);
            if let Some(slot) = voice.nepers.get_mut(across) {
                *slot = drained;
            }
        }
    }

    fn body_spec(&self) -> &'static body::BodySpec {
        if self.model == Model::Bass {
            &body::VIOLIN
        } else {
            &body::GUITAR
        }
    }

    /// Whether the unplayed strings ring: an acoustic guitar's, which share its bridge.
    fn has_open_strings(&self) -> bool {
        self.model == Model::Guitar
    }

    /// Tunes the open strings to the controls: they lose what a sounding string of their
    /// pitch loses, and take from the bridge what the bridge takes from them.
    fn tune_open_strings(&mut self) {
        if !self.has_open_strings() {
            return;
        }
        let acoustic = self.acoustic() * (1.0 - self.settings.saddle);
        let spec = self.body_spec();
        let scale = self.body_scale();
        for (index, pitch) in self.build.strings.iter().enumerate() {
            let pitch = f32::from(*pitch);
            let hz = hertz(pitch);
            let drained = 0.006 * hz * spec.conductance(hz, scale);
            let nepers = self.fundamental_rate(pitch) + self.acoustic() * drained;
            let takes = OPEN_COUPLING * acoustic * (0.25 + spec.conductance(hz, scale));
            self.open.tune(index, hz, nepers, takes, self.rate);
        }
    }

    /// Lifts the finger of voice `index` off its string, which is then free to ring.
    fn leave_string(&mut self, index: usize) {
        if let Some(string) = self
            .voices
            .get_mut(index)
            .and_then(|voice| voice.string.take())
            && let Some(open) = self.open.strings.get_mut(string)
        {
            open.busy = open.busy.saturating_sub(1);
        }
    }

    fn body_scale(&self) -> f32 {
        if self.model == Model::Bass { 0.23 } else { 1.0 }
    }

    fn bend_cents(&self) -> f32 {
        self.performance.bend * 100.0
    }
}

impl Engine for Plucked {
    fn prepare(&mut self, rate: f32, voices: usize) {
        self.rate = rate;
        self.voices = (0..voices)
            .map(|_| {
                let mut voice = Voice::default();
                voice.rotors.prepare(ROTORS);
                voice.hz = vec![0.0; ROTORS];
                voice.nepers = vec![0.0; ROTORS];
                voice
            })
            .collect();
        if !self.is_electric() {
            self.body.prepare(self.body_spec(), self.body_scale(), rate);
        }
        if self.has_open_strings() {
            self.open.prepare(self.build.strings, rate);
            self.tune_open_strings();
        }
        self.pan_step = 1.0 - (-1.0 / (0.010 * rate)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.rotors.clear();
            voice.sounding = false;
            voice.held = false;
            voice.declick = Declick::default();
            voice.mute.open();
            voice.last = 0.0;
            voice.string = None;
        }
        self.body.reset();
        self.open.reset();
        for filter in &mut self.electronics {
            filter.reset();
        }
    }

    fn configure(&mut self, params: &ParamBank) {
        let electric = self.is_electric();
        self.settings = Settings {
            hardness: params.at(P_HARDNESS),
            position: params.at(P_POSITION),
            decay: params.at(P_DECAY),
            damping: params.at(P_DAMPING),
            body: if electric { 0.0 } else { params.at(P_BODY) },
            release: params.at(P_RELEASE),
            saddle: if self.model == Model::Guitar {
                params.at(P_PICKUP)
            } else {
                0.0
            },
            pickup_position: match self.model {
                Model::ElectricGuitar => params.at(P_PICKUP_POSITION),
                // A bass's single pickup, a sixth of the way up the string.
                _ => 0.17,
            },
            tone: if self.model == Model::Guitar {
                0.0
            } else {
                params.at(P_TONE)
            },
            pickup_q: if electric { params.at(P_PICKUP_Q) } else { 1.2 },
            decay_ratio: if electric {
                params.at(P_DECAY_RATIO)
            } else {
                1.0
            },
        };
        if self.settings.tone > 0.0 {
            let coefficients = BiquadCoefficients::lowpass(
                f64::from(self.rate),
                self.settings.tone.min(self.rate * 0.4),
                self.settings.pickup_q,
            );
            for filter in &mut self.electronics {
                filter.set_coefficients(coefficients);
            }
        }
        self.tune_open_strings();
        // A sounding string answers its decay and damping at once; where it was plucked and
        // how hard is already in it.
        let cents = self.bend_cents();
        for index in 0..self.voices.len() {
            if !self.voices[index].sounding {
                continue;
            }
            self.set_rates(index);
            let rate = self.rate;
            let voice = &mut self.voices[index];
            let glide = voice.glide * (voice.low_energy() / voice.energy).min(1.0);
            voice.retune(cents + glide, rate);
        }
    }

    fn start(&mut self, index: usize, note: &Note, quality: Quality) {
        let (rate, settings) = (self.rate, self.settings);
        let pitch = f32::from(note.pitch);
        let fundamental = hertz(pitch);
        let (string, fret) = fretted(pitch, self.build.strings);
        let stiffness =
            (self.build.stiffness.get(string).copied().unwrap_or(4.0e-5) * fret * fret).min(5.0e-3);
        // The plucking hand stays where it is while the fretting hand shortens the string, so
        // a place measured on the open string is further along the sounding one.
        let wobble = 1.0 + 0.08 * (seeded(note.seed, 0) - 0.5);
        let contact = match note.touch {
            Some(at) => touch_position(at),
            None => (settings.position * fret * wobble).clamp(0.02, 0.5),
        };
        let pickup = (settings.pickup_position * fret).clamp(0.02, 0.9);
        let hardness = settings.hardness * (0.55 + 0.45 * note.velocity);
        let corner = 500.0 * 24.0_f32.powf(hardness);
        let budget = match quality {
            Quality::Draft => 40,
            Quality::Standard => 96,
            Quality::High => PARTIALS,
        };
        let limit = (rate * 0.45).min(18_000.0);
        let (acoustic, magnetic) = (self.acoustic(), self.magnetic());
        let glide = self.build.glide * note.velocity * note.velocity;
        // A note the box favours is plucked a little more gently, and one it refuses a
        // little harder: what a player's hand learns of an instrument.
        let evened = if acoustic > 0.0 {
            self.body
                .favour(fundamental)
                .powf(-EVENING * acoustic)
                .clamp(0.6, 1.5)
        } else {
            1.0
        };
        let amplitude = note.velocity * self.build.drive * evened;
        // The string leaves the finger at an angle to the soundboard that no two plucks share.
        let angle = if acoustic > 0.0 {
            0.35 + 0.5 * seeded(note.seed, 1)
        } else {
            0.0
        };
        let split = 4.0e-4 * (1.0 + seeded(note.seed, 2));
        self.leave_string(index);
        if self.has_open_strings()
            && let Some(open) = self.open.strings.get_mut(string)
        {
            open.busy = open.busy.saturating_add(1);
        }
        let open_strings = self.has_open_strings();
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        voice.string = open_strings.then_some(string);
        voice.declick = if note.stolen {
            Declick::from(voice.last, rate)
        } else {
            Declick::default()
        };
        voice.rotors.clear();
        voice.mute.open();
        voice.pitch = pitch;
        voice.velocity = note.velocity;
        voice.fundamental = fundamental;
        voice.stiffness = stiffness;
        voice.fret = fret;
        voice.contact = contact;
        voice.pan = note.pan;
        voice.pan_now = note.pan * self.performance.width;
        voice.held = true;
        voice.sounding = true;
        voice.glide = glide;
        voice.attack = 0.0;
        voice.attack_step = 1.0 / ((0.000_2 + 0.002_5 * (1.0 - hardness).powi(2)) * rate).max(1.0);
        let mut partials = 0;
        while partials < budget
            && fundamental * partial_ratio((partials + 1) as f32, stiffness) < limit
        {
            partials += 1;
        }
        voice.partials = partials;
        voice.paired = if acoustic > 0.0 {
            partials.min(PAIRED)
        } else {
            0
        };
        voice.count = partials + voice.paired;
        for n in 0..partials {
            let order = (n + 1) as f32;
            let hz = fundamental * partial_ratio(order, stiffness);
            let share = amplitude * pluck_share(order, contact) * release_rolloff(hz, corner);
            let across = voice.across(n);
            // The bridge feels the string's slope, which is every partial alike; a magnet
            // feels its velocity where it sits, and so a comb.
            let magnet = magnetic * (PI * order * pickup).sin();
            voice.rotors.set(
                across,
                Rotor {
                    listen_re: magnet,
                    listen_im: acoustic.max(1.0 - magnetic),
                    ..Rotor::default()
                },
            );
            if let Some(slot) = voice.hz.get_mut(across) {
                *slot = hz;
            }
            if n < voice.paired {
                let jitter = 1.0 + 4.0e-5 * (seeded(note.seed, 16 + n as u32) - 0.5);
                voice.rotors.set(
                    across + 1,
                    Rotor {
                        listen_im: ALONG * acoustic,
                        ..Rotor::default()
                    },
                );
                if let Some(slot) = voice.hz.get_mut(across + 1) {
                    *slot = hz * (1.0 + split) * jitter;
                }
            }
            voice.rotors.place(across, 0.0, share * angle.cos());
            if n < voice.paired {
                voice.rotors.place(across + 1, 0.0, share * angle.sin());
            }
        }
        voice.rotors.set_len(voice.count);
        voice.energy = voice.low_energy().max(1.0e-12);
        voice.plucked = 0.0;
        self.set_rates(index);
        let cents = self.bend_cents();
        let voice = &mut self.voices[index];
        voice.retune(cents + glide, rate);
        voice.plucked = voice.rotors.settle(0.0).max(1.0e-9);
        voice.moving = voice.plucked;
    }

    fn stop(&mut self, index: usize) {
        if let Some(voice) = self.voices.get_mut(index)
            && voice.held
        {
            voice.held = false;
            self.leave_string(index);
            self.set_rates(index);
            let rate = self.rate;
            let voice = &mut self.voices[index];
            let tuned = voice.tuned;
            voice.retune(tuned, rate);
        }
    }

    fn silence(&mut self, index: usize) {
        self.leave_string(index);
        let rate = self.rate;
        if let Some(voice) = self.voices.get_mut(index) {
            voice.held = false;
            voice.mute.close(rate);
        }
    }

    fn perform(&mut self, performance: &Performance) {
        let bent = performance.bend != self.performance.bend;
        self.performance = *performance;
        if bent {
            let (cents, rate) = (self.bend_cents(), self.rate);
            for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
                let glide = voice.glide * (voice.low_energy() / voice.energy).min(1.0);
                voice.retune(cents + glide, rate);
            }
        }
    }

    fn render(&mut self, mid: &mut [f32], side: &mut [f32], tick: bool) {
        let frames = mid.len().min(side.len()).min(TICK);
        let (cents, rate, width) = (self.bend_cents(), self.rate, self.performance.width);
        if tick {
            self.body.tick();
            self.open.tick();
            for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
                voice.moving = voice.rotors.settle(FLOOR);
                if voice.moving == 0.0 || voice.mute.is_closed() {
                    voice.sounding = false;
                    voice.rotors.rest();
                    continue;
                }
                // A hard pluck stretches the string, and it comes back to pitch as it calms.
                let glide = voice.glide * (voice.low_energy() / voice.energy).min(1.0);
                if (cents + glide - voice.tuned).abs() > 0.25 {
                    voice.retune(cents + glide, rate);
                }
            }
        }
        let (mut bus_mid, mut bus_side) = ([0.0_f32; TICK], [0.0_f32; TICK]);
        let mut string = [0.0_f32; TICK];
        for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
            let string = &mut string[..frames];
            string.fill(0.0);
            voice.rotors.ring(string);
            let target = voice.pan * width;
            for ((sample, to_mid), to_side) in string
                .iter()
                .zip(bus_mid.iter_mut())
                .zip(bus_side.iter_mut())
            {
                // The finger letting go: the force at the bridge arrives over a moment.
                voice.attack = (voice.attack + voice.attack_step).min(1.0);
                let let_go = 0.5 - 0.5 * (PI * voice.attack).cos();
                let heard = sample * let_go * voice.mute.next() + voice.declick.next();
                voice.pan_now += (target - voice.pan_now) * self.pan_step;
                voice.last = heard;
                *to_mid += heard;
                *to_side += heard * voice.pan_now;
            }
        }
        let (bus_mid, bus_side) = (&mut bus_mid[..frames], &mut bus_side[..frames]);
        match self.model {
            Model::ElectricGuitar => {
                for (filter, bus) in self
                    .electronics
                    .iter_mut()
                    .zip([&mut *bus_mid, &mut *bus_side])
                {
                    filter.process_block(bus);
                }
            }
            Model::Bass => {
                let acoustic = self.settings.body;
                let (mut wired_mid, mut wired_side) = ([0.0_f32; TICK], [0.0_f32; TICK]);
                wired_mid[..frames].copy_from_slice(bus_mid);
                wired_side[..frames].copy_from_slice(bus_side);
                self.electronics[0].process_block(&mut wired_mid[..frames]);
                self.electronics[1].process_block(&mut wired_side[..frames]);
                self.body.process(bus_mid, bus_side, 1.0, width);
                for frame in 0..frames {
                    bus_mid[frame] =
                        acoustic * bus_mid[frame] + (1.0 - acoustic) * wired_mid[frame];
                    bus_side[frame] =
                        acoustic * bus_side[frame] + (1.0 - acoustic) * wired_side[frame];
                }
            }
            _ => {
                if self.has_open_strings() {
                    self.open.process(bus_mid);
                }
                // A saddle pickup hears the bridge itself, before the body has had its say.
                let amount = self.settings.body * (1.0 - self.settings.saddle);
                self.body.process(bus_mid, bus_side, amount, width);
            }
        }
        for frame in 0..frames {
            mid[frame] += bus_mid[frame];
            side[frame] += bus_side[frame];
        }
    }

    fn level(&self, index: usize) -> f32 {
        self.voices
            .get(index)
            .filter(|voice| voice.sounding)
            .map_or(0.0, |voice| {
                (voice.moving / voice.plucked).clamp(1.0e-6, 1.0)
            })
    }

    fn is_held(&self, index: usize) -> bool {
        self.voices.get(index).is_some_and(|voice| voice.held)
    }

    fn observe(&self, index: usize, picture: &mut MotionVoice) {
        let Some(voice) = self.voices.get(index) else {
            return;
        };
        picture.level = self.level(index);
        picture.contact = voice.contact;
        picture.excitation = 0.0;
        // Displacement is the bridge force's share of each partial over its number again.
        let shown = voice.partials.min(24);
        let last = (picture.points.len() - 1) as f32;
        for (at, point) in picture.points.iter_mut().enumerate() {
            let x = at as f32 / last;
            *point = (0..shown)
                .map(|n| {
                    let order = (n + 1) as f32;
                    voice.rotors.at(voice.across(n)).1 / order * (PI * order * x).sin()
                })
                .sum::<f32>()
                * DISPLAY;
        }
        picture.points[0] = 0.0;
        picture.points[MOTION_POINTS - 1] = 0.0;
        for n in 0..MOTION_PARTIALS {
            let (re, im) = voice.rotors.at(voice.across(n));
            let size = if n < voice.partials {
                re.hypot(im) * DISPLAY
            } else {
                0.0
            };
            picture.partials[n] = size;
            if let Some(mode) = picture.modes.get_mut(n) {
                *mode = size;
            }
        }
    }

    fn observe_body(&self, body: &mut [f32; MOTION_BODY]) {
        // A magnet has no box behind it.
        let amount = match self.model {
            Model::ElectricGuitar => 0.0,
            Model::Bass => self.settings.body,
            _ => self.settings.body * (1.0 - self.settings.saddle),
        };
        if amount > 0.0 {
            self.body.observe(body);
            for mode in body.iter_mut() {
                *mode *= amount * super::BODY_DISPLAY;
            }
        }
    }
}

/// How strongly the bridge passes what it is doing to an open string, each time round.
const OPEN_COUPLING: f32 = 0.007;
/// Pole of the low-pass in each open string's loop: its upper partials die sooner.
const OPEN_POLE: f32 = 0.3;
/// Below this an open string is taken to have stopped.
const OPEN_FLOOR: f32 = 1.0e-7;

/// One open string, free to ring: a wave going round it, once a period.
#[derive(Clone, Copy, Debug, Default)]
struct Open {
    start: usize,
    size: usize,
    cursor: usize,
    period: f32,
    /// What it keeps of itself each time round.
    ring: f32,
    /// What it takes of the bridge's motion each time round.
    takes: f32,
    low: f32,
    /// Notes being played on this string, which are its own motion and do not ring it twice.
    busy: u8,
    moving: bool,
    loudest: f32,
}

/// The strings of a guitar nobody is playing, which the bridge sets ringing.
///
/// A note played on one string moves the bridge, and every other string is tied to that
/// bridge: those with a partial near one of the note's ring on after it, quietly and for a
/// long time. It is why an A on the third string leaves the open fifth humming, and much of
/// why a guitar sounds like a box of strings rather than one string at a time.
#[derive(Clone, Debug, Default)]
struct OpenStrings {
    lines: Vec<f32>,
    strings: [Open; 6],
    count: usize,
    any: bool,
}

impl OpenStrings {
    fn prepare(&mut self, open: &[u8], rate: f32) {
        self.count = open.len().min(self.strings.len());
        let mut start = 0;
        for (string, pitch) in self.strings.iter_mut().zip(open) {
            // Room for the string tuned down a semitone, whatever the sample rate.
            let lowest = hertz(f32::from(*pitch) - 1.0);
            string.start = start;
            string.size = (rate / lowest).ceil() as usize + 4;
            start += string.size;
        }
        self.lines = vec![0.0; start];
        self.reset();
    }

    fn reset(&mut self) {
        self.lines.fill(0.0);
        for string in &mut self.strings {
            string.cursor = 0;
            string.low = 0.0;
            string.busy = 0;
            string.moving = false;
            string.loudest = 0.0;
        }
        self.any = false;
    }

    /// Tunes string `index` to `hz`; it loses `nepers` a second of its own, and the bridge
    /// passes it `takes` of its motion each period.
    fn tune(&mut self, index: usize, hz: f32, nepers: f32, takes: f32, rate: f32) {
        let Some(string) = self.strings.get_mut(index) else {
            return;
        };
        let filter = OPEN_POLE / (1.0 - OPEN_POLE);
        let longest = (string.size.saturating_sub(3) as f32).max(2.0);
        string.period = (rate / hz - filter).clamp(2.0, longest);
        string.ring = (-nepers / hz).exp();
        string.takes = takes;
    }

    /// Lets the open strings hear the bridge, and adds what they answer to it.
    fn process(&mut self, bus: &mut [f32]) {
        if self.lines.is_empty() {
            return;
        }
        let frames = bus.len().min(TICK);
        let mut answer = [0.0_f32; TICK];
        let mut heard_any = false;
        for sample in bus.iter().take(frames) {
            heard_any |= *sample != 0.0;
        }
        if !heard_any && !self.any {
            return;
        }
        for string in self.strings.iter_mut().take(self.count) {
            let open = string.busy == 0;
            if !open && !string.moving {
                continue;
            }
            let Some(line) = self.lines.get_mut(string.start..string.start + string.size) else {
                continue;
            };
            let whole = string.period as usize;
            let part = string.period - whole as f32;
            for (heard, answer) in bus.iter().zip(answer.iter_mut()).take(frames) {
                let newer = (string.cursor + string.size - whole) % string.size;
                let older = (newer + string.size - 1) % string.size;
                let back = line[newer] + part * (line[older] - line[newer]);
                string.low += (1.0 - OPEN_POLE) * (back - string.low);
                // A string being fretted is the note's own, and its open ring is stopped.
                let kept = if open { string.ring } else { 0.2 };
                let fed = if open { string.takes * heard } else { 0.0 };
                line[string.cursor] = kept * string.low + fed;
                string.cursor = (string.cursor + 1) % string.size;
                string.loudest = string.loudest.max(string.low.abs());
                *answer += kept * string.low;
            }
        }
        for (sample, answer) in bus.iter_mut().zip(answer) {
            *sample += answer;
        }
    }

    /// Notes which strings have died away, and empties their lines for the next time.
    fn tick(&mut self) {
        let mut any = false;
        for string in self.strings.iter_mut().take(self.count) {
            let was = string.moving;
            string.moving = string.loudest > OPEN_FLOOR;
            string.loudest = 0.0;
            if was && !string.moving {
                if let Some(line) = self.lines.get_mut(string.start..string.start + string.size) {
                    line.fill(0.0);
                }
                string.low = 0.0;
            }
            any |= string.moving;
        }
        self.any = any;
    }
}
