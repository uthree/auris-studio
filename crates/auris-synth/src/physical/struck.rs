//! Struck strings: a grand piano and a hammered dulcimer.
//!
//! A hammer is thrown at a unison of stiff strings, and the force between them for the few
//! milliseconds they touch is solved in `hammer`. That force sets every partial of every string
//! going; the strings of a note share a bridge, and `unison` has already taken them apart into
//! the motions that do not mix, so what rings afterwards is a bank of independent phasors — up
//! to ninety-six partials of three strings each on the piano. What they do to the bridge
//! drives the soundboard.
//!
//! A piano is more than its sounding strings. The blow itself knocks the case and the
//! keybed, which ring for a moment on their own; a damper falls on each string when its key
//! comes up, unless the
//! pedal is holding them all off; and every string whose damper is up answers the others —
//! the wash under a pedalled chord, and the faint ring of the top octaves, which have no
//! dampers at all. Each of those is here as the thing it is.
//!
//! How fast a piano's partials die across the keyboard — the strings' own loss, what the bridge
//! takes from the motion a unison shares, and the internal friction that takes the top — is
//! measured, not designed: the curves below were fitted to recordings of a Steinway B from the
//! University of Iowa Musical Instrument Samples, one value for each C.
//!
//! Those same curves set how loud each register is. A string can only give the bridge the
//! energy it loses, so the bridge moves in proportion to how fast the string decays and in
//! inverse proportion to how heavy it is: a treble string pulls harder on the bridge than a
//! middle one and moves it less.
//!
//! What is voiced is what a technician voices. The hammers' felt is needled against the same
//! recordings — the published felt, thrown at these strings, is a good deal harder than this
//! Steinway's, most of all in the fifth octave — and each register is brought up to where the
//! recorded one stands. Both are a number for each C, in [`FELT`] and [`REGISTER`]. The
//! whole-keyboard measure that found them is `tools/eval/piano_register.py`, and what it
//! reads on this piano is in `docs/physical-instruments.md`.

use std::f32::consts::{PI, TAU};

use auris_core::motion::{MOTION_BODY, MOTION_PARTIALS, MOTION_POINTS, MotionVoice};
use auris_core::{ParamDescriptor, ParamUnit};

use super::body::{self, Radiator, seeded};
use super::calibration::dial;
use super::hammer::{Blow, Hammer, across_keys, grand};
use super::rotors::{NEPERS_PER_DB, Rotor, Rotors, decay_rate, pole};
use super::strings::{Losses, material, partial_ratio};
use super::unison::{STRINGS, normals};
use super::{
    BODY_DISPLAY, Declick, Engine, Model, Mute, Note, Performance, Quality, TICK, dial_across,
    dials, hertz, touch_position,
};
use crate::ParamBank;

const P_HARDNESS: u32 = 0;
const P_POSITION: u32 = 1;
const P_DECAY: u32 = 2;
const P_DAMPING: u32 = 3;
const P_BODY: u32 = 4;
const P_RELEASE: u32 = 5;
const P_STIFFNESS: u32 = 6;
const P_DETUNE: u32 = 7;
const P_SYMPATHETIC: u32 = 8;

/// Most partials of one string.
const PARTIALS: usize = 96;
const ROTORS: usize = PARTIALS * STRINGS;
/// Highest key with a damper, F6. Strings above it ring on after the key is released.
const HIGHEST_DAMPER: u8 = 89;
/// Below this a voice's phasors are taken to have stopped.
const FLOOR: f32 = 2.0e-6;
/// Scale of the picture a frontend draws.
const DISPLAY: f32 = 0.08;
/// The force a blow's picture is full scale at.
const BLOW_NEWTONS: f32 = 60.0;
/// The decay control's setting at which the piano follows the measured curves.
const NATURAL_DECAY: f32 = 10.0;
/// Hammer speed at full velocity, in metres per second: a fortissimo.
const FULL_SPEED: f32 = 4.6;
/// A piano hammer's speed at full velocity, in metres per second.
const GRAND_SPEED: f32 = 5.6;
/// The power of a key's velocity a piano hammer's speed follows. With [`GRAND_SPEED`] it is
/// read off the recorded Steinway's three dynamics from C3 to G4, taken as velocities of 0.35,
/// 0.65 and 0.95: its pianissimo lies 13 dB under its mezzo-forte and its fortissimo 10 dB
/// over, and the piano's lie 12.7 dB under and 7.0 dB over.
const TOUCH: f32 = 1.9;
/// Brings a blow's impulse, in newton seconds, to the scale the rest of the instrument works at.
const DRIVE: f32 = 60.0;
/// How hard a blow knocks the case, against how hard it drives a string.
const KNOCK: f32 = 0.022;
/// Modes of the case and keybed that a blow knocks, and the span they lie across, in hertz.
const CASE_MODES: usize = 32;
const CASE_SPAN: (f32, f32) = (45.0, 2_900.0);
/// How fast the knock dies, in decibels per second: a third of a second to nothing.
const CASE_DECAY: f32 = 170.0;
/// Below this the case moves little air, and its modes count for less the lower they are.
const CASE_FLOOR: f32 = 320.0;
/// A key struck more softly than this does not throw its hammer as far as the strings: it
/// lifts the damper and nothing else, which is how a pianist takes a note silently to let
/// its strings answer others.
const SILENT: f32 = 0.012;

/// The strings' own loss at the fundamental, in decibels per second, for each C from C1 to C8:
/// the rate of the aftersound.
const AFTERSOUND: [f32; 8] = [1.839, 1.345, 1.495, 1.737, 2.614, 5.659, 14.08, 42.86];
/// What the bridge takes from the motion a unison shares, in decibels per second.
const BRIDGE: [f32; 8] = [1.499, 6.62, 10.05, 16.63, 9.688, 7.19, 9.785, 13.3];
/// Exponent of the bridge's loss over partial number.
const BRIDGE_SLOPE: f32 = -0.1426;
/// Internal friction of the partials above the fundamental, in decibels per second per
/// kilohertz squared.
const FRICTION: f32 = 0.9172;
/// How the hammers are voiced, for each C from C1 to C8: decades by which their felt is
/// softer or stiffer than the published measurements, at half a millimetre of compression.
/// A technician needles a grand's hammers until it sounds as it should, and these are needled
/// against the same Steinway: softest where the ear is keenest, in the fifth octave, and
/// nearly as measured at either end.
const FELT: [f32; 8] = [-0.9, -0.9, -0.7, -1.3, -1.7, -1.4, -0.7, -0.3];
/// How much more steeply than measured the voiced felt stiffens as it is driven in, added to
/// the exponent of its force law. It is what makes a soft blow so much rounder than a hard one.
const FELT_SHAPE: f32 = 0.4;
/// Compression about which a change of exponent turns the felt's force law, in metres.
const FELT_PIVOT: f32 = 5.0e-4;
/// What each register is brought up by against the same recorded grand, in decibels: how far
/// the bridge moves follows from how fast the strings decay, but not all of how well the
/// soundboard then turns the treble's short strokes into sound.
const REGISTER: [f32; 8] = [2.0, 1.0, 1.0, 1.0, 4.0, 5.0, 8.0, 11.0];
/// Damper time relative to the release control: bass dampers take longer to stop their strings.
const DAMPER: [f32; 8] = [2.5, 2.0, 1.4, 1.0, 0.8, 0.7, 0.6, 0.6];
/// Unison mistuning the hertz bound below applies to, in cents: a freshly tuned piano.
const TUNED: f32 = 0.258;
/// The beat rate a tuner leaves between the strings of a treble unison, in hertz.
const TUNED_BEAT: f32 = 0.092_23;
/// How unequally the hammer strikes the strings of a unison.
const UNEVEN: f64 = 0.5198;
/// A single string's second polarisation: how it pulls on the bridge, how much of the blow it
/// takes, and how far it is tuned from the first, in cents.
const POLARISATION: [f64; 3] = [0.7, 0.743, 0.392];
/// Wave impedance of one dulcimer wire, in kilograms per second: a fifth of a piano's.
const WIRE: f32 = 0.45;

/// Geometric interpolation of a C1..C8 curve at a fractional MIDI pitch.
fn register(curve: &[f32; 8], pitch: f32) -> f32 {
    let position = ((pitch - 24.0) / 12.0).clamp(0.0, 7.0);
    let index = (position as usize).min(6);
    let fraction = position - index as f32;
    let (low, high) = (curve[index].max(1.0e-6), curve[index + 1].max(1.0e-6));
    low * (high / low).powf(fraction)
}

/// Straight-line interpolation of a C1..C8 curve at a fractional MIDI pitch.
fn between(curve: &[f32; 8], pitch: f32) -> f32 {
    let position = ((pitch - 24.0) / 12.0).clamp(0.0, 7.0);
    let low = (position as usize).min(6);
    curve[low] + (curve[low + 1] - curve[low]) * (position - low as f32)
}

/// The strings' own loss at the fundamental of `pitch`, in decibels per second.
fn aftersound(pitch: f32) -> f32 {
    register(&AFTERSOUND, pitch) * dial_across(dials::AFTERSOUND, pitch, 1.0)
}

/// What the bridge takes from the motion a unison shares at `pitch`, in decibels per second.
fn bridge(pitch: f32) -> f32 {
    register(&BRIDGE, pitch) * dial_across(dials::BRIDGE, pitch, 1.0)
}

/// Exponent of the bridge's loss over partial number.
fn bridge_slope() -> f32 {
    BRIDGE_SLOPE + dial(dials::BRIDGE_SLOPE, 0.0)
}

pub(super) fn descriptors(model: Model) -> Vec<ParamDescriptor> {
    let piano = model == Model::Piano;
    let mut descriptors = vec![
        ParamDescriptor::percent(
            P_HARDNESS,
            "hardness",
            if piano {
                "Contact Hardness"
            } else {
                "Hammer Hardness"
            },
            if piano { 0.5 } else { 0.4 },
        ),
        ParamDescriptor::new(
            P_POSITION,
            "position",
            if piano {
                "Excitation Position"
            } else {
                "Strike Position"
            },
            0.03,
            0.45,
            if piano { 0.12 } else { 0.126 },
        )
        .with_unit(ParamUnit::Percent),
        ParamDescriptor::new(
            P_DECAY,
            "decay",
            if piano {
                "Resonance Decay"
            } else {
                "String Decay"
            },
            0.2,
            20.0,
            if piano { NATURAL_DECAY } else { 5.0 },
        )
        .with_unit(ParamUnit::Seconds),
        ParamDescriptor::percent(
            P_DAMPING,
            "damping",
            if piano { "Damping" } else { "Bridge Damping" },
            if piano { 0.0 } else { 0.16 },
        ),
        ParamDescriptor::percent(P_BODY, "body", "Body Resonance", 1.0),
        ParamDescriptor::new(
            P_RELEASE,
            "release",
            "Release",
            0.01,
            4.0,
            if piano { 0.2 } else { 0.085 },
        )
        .with_unit(ParamUnit::Seconds),
        ParamDescriptor::new(P_STIFFNESS, "stiffness", "String Stiffness", 0.0, 3.0, 1.0)
            .with_unit(ParamUnit::Ratio),
        ParamDescriptor::new(
            P_DETUNE,
            "detune",
            if piano {
                "Unison Detune"
            } else {
                "Course Detune"
            },
            0.0,
            12.0,
            if piano { TUNED } else { 0.78 },
        ),
    ];
    if piano {
        descriptors.push(ParamDescriptor::percent(
            P_SYMPATHETIC,
            "sympathetic",
            "Sympathetic Resonance",
            0.3,
        ));
    }
    descriptors
}

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    hardness: f32,
    position: f32,
    decay: f32,
    damping: f32,
    body: f32,
    release: f32,
    stiffness: f32,
    detune: f32,
    sympathetic: f32,
}

/// The strings one hammer strikes: how many, how far apart they are tuned, how each pulls on
/// the bridge and how much of the blow each takes.
#[derive(Clone, Copy, Debug)]
struct Stringing {
    strings: usize,
    cents: [f64; STRINGS],
    weights: [f64; STRINGS],
    shares: [f64; STRINGS],
}

#[derive(Clone, Debug, Default)]
struct Voice {
    rotors: Rotors,
    hz: Vec<f32>,
    nepers: Vec<f32>,
    hammer: Hammer,
    count: usize,
    partials: usize,
    strings: usize,
    key: u8,
    fundamental: f32,
    stiffness: f32,
    /// Where the hammer strikes, from the near end.
    contact: f32,
    /// How much less evenly than usual this blow fell across the unison.
    uneven: f32,
    /// How hard this blow knocks the case.
    knock: f32,
    sounding: bool,
    held: bool,
    /// Released, but kept from its damper by the pedal.
    deferred: bool,
    damped: bool,
    pan: f32,
    pan_now: f32,
    declick: Declick,
    mute: Mute,
    last: f32,
    moving: f32,
    loudest: f32,
}

impl Voice {
    fn retune(&mut self, cents: f32, rate: f32) {
        let factor = (cents / 1200.0).exp2();
        for index in 0..self.count {
            let (Some(hz), Some(nepers)) = (self.hz.get(index), self.nepers.get(index)) else {
                break;
            };
            let hz = hz * factor;
            if hz >= rate * 0.49 || hz <= 0.0 {
                self.rotors.turn(index, 0.0, 0.0);
                continue;
            }
            let (cosine, sine) = pole(hz, *nepers, rate);
            self.rotors.turn(index, cosine, sine);
        }
    }

    /// Adds `nepers` a second to every phasor's decay, or takes it away again.
    fn damp(&mut self, nepers: f32, rate: f32) {
        for slot in self.nepers.iter_mut().take(self.count) {
            *slot = (*slot + nepers).max(0.0);
        }
        self.rotors.shrink((-nepers / rate).exp());
    }
}

/// Lowest key with a string that can answer the others.
const LOWEST_KEY: u8 = 21;
const KEYS: usize = 88;
/// Pole of the low-pass in each free string's loop: its upper partials die sooner.
const FREE_POLE: f32 = 0.3;
/// How strongly the bridge passes what it is doing to a free string, against the first-order
/// account of it in `Sympathetic::tune`. That account takes the bridge for a pure resistance
/// and the strings of a free unison for one string; neither is so, and with the control at
/// its default this leaves a note under the pedal two or three decibels fuller a second on,
/// which is about what the pedal does.
const FREE_COUPLING: f32 = 0.5;
/// Below this a free string is taken to have stopped.
const FREE_FLOOR: f32 = 1.0e-7;

/// One string free to ring: a wave going round it, once a period.
#[derive(Clone, Copy, Debug, Default)]
struct Free {
    /// Where its delay line begins in the shared store, and how long it is.
    start: usize,
    size: usize,
    cursor: usize,
    /// Its period in samples.
    period: f32,
    /// What it keeps of itself each time round while its damper is up.
    ring: f32,
    /// What it takes of the bridge's motion each time round.
    takes: f32,
    low: f32,
    open: bool,
    moving: bool,
    loudest: f32,
}

/// Every string of the piano that is free to ring when another is struck.
///
/// With the pedal down that is all of them, and each is far too finely tuned to answer much of
/// any one note — but there are two hundred and thirty of them with partials every few hertz
/// between them, so what answers is a faint, very long reverberation tuned to the piano
/// itself. A string here is a single wave going round a delay line, which gives it every one
/// of its partials for the cost of one.
#[derive(Clone, Debug)]
struct Sympathetic {
    lines: Vec<f32>,
    strings: [Free; KEYS],
    /// Notes sounding on each key, which are that key's own strings and not answered twice.
    busy: [u8; KEYS],
    /// Keys that are down, whose dampers are up whether or not their note still sounds.
    down: [bool; KEYS],
    any: bool,
}

impl Default for Sympathetic {
    fn default() -> Self {
        Self {
            lines: Vec::new(),
            strings: [Free::default(); KEYS],
            busy: [0; KEYS],
            down: [false; KEYS],
            any: false,
        }
    }
}

impl Sympathetic {
    /// Allocates a delay line for every key, long enough for it at `rate` whatever the tuning.
    fn prepare(&mut self, rate: f32) {
        let mut start = 0;
        for (key, string) in self.strings.iter_mut().enumerate() {
            let lowest = hertz(f32::from(LOWEST_KEY) + key as f32 - 1.0);
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
            string.open = false;
            string.moving = false;
            string.loudest = 0.0;
        }
        self.busy = [0; KEYS];
        self.down = [false; KEYS];
        self.any = false;
    }

    /// Tunes every string: `stretch` is each key's offset from equal temperament in cents.
    fn tune(&mut self, stretch: &[f32; 128], stiffness: f32, rate: f32) {
        for (key, string) in self.strings.iter_mut().enumerate() {
            let pitch = f32::from(LOWEST_KEY) + key as f32;
            let fundamental = hertz(pitch + stretch[key + usize::from(LOWEST_KEY)] / 100.0);
            let b = across_keys(grand::INHARMONICITY, pitch) * stiffness;
            // One wave going round has harmonics, not a stiff string's partials. It is tuned
            // so that its second lands on the string's second, which splits the difference
            // over the few partials that answer most.
            let hz = fundamental * partial_ratio(2.0, b) * 0.5;
            let filter = FREE_POLE / (1.0 - FREE_POLE);
            // Before `prepare` there is no line yet, and nothing to hold a period to.
            let longest = (string.size.saturating_sub(3) as f32).max(2.0);
            string.period = (rate / hz - filter).clamp(2.0, longest);
            let free = aftersound(pitch) + bridge(pitch) / 3.0;
            string.ring = 10.0_f32.powf(-free / (20.0 * hz));
            // A bridge that moves adds its own velocity to the wave a string sends back from
            // it, once a period; and what the strings of a unison then do to the bridge in
            // return is what the bridge takes from them when they are struck, the measured
            // loss of their shared motion. The two together, each time round:
            string.takes = bridge(pitch) * NEPERS_PER_DB / hz;
        }
    }

    /// Lifts or lowers each damper as the pedal and the keys now stand.
    fn update(&mut self, pedal: bool, amount: f32) {
        let mut any = false;
        for (key, string) in self.strings.iter_mut().enumerate() {
            let has_damper = key + usize::from(LOWEST_KEY) <= usize::from(HIGHEST_DAMPER);
            let free = pedal || self.down[key] || !has_damper;
            // A key that is itself sounding is its own strings, already there.
            string.open = amount > 0.0 && free && self.busy[key] == 0;
            any |= string.open || string.moving;
        }
        self.any = any;
    }

    fn key(pitch: u8) -> Option<usize> {
        usize::from(pitch)
            .checked_sub(usize::from(LOWEST_KEY))
            .filter(|key| *key < KEYS)
    }

    /// Lets the free strings hear what is at the bridge, and adds what they answer to it.
    fn process(&mut self, bus: &mut [f32], amount: f32) {
        if !self.any || self.lines.is_empty() {
            return;
        }
        let frames = bus.len().min(TICK);
        let mut answer = [0.0_f32; TICK];
        for string in &mut self.strings {
            if (!string.open && !string.moving) || string.size < 4 {
                continue;
            }
            let Some(line) = self.lines.get_mut(string.start..string.start + string.size) else {
                continue;
            };
            let whole = string.period as usize;
            let part = string.period - whole as f32;
            let coupling = amount * FREE_COUPLING * string.takes;
            for (heard, answer) in bus.iter().zip(answer.iter_mut()).take(frames) {
                let newer = (string.cursor + string.size - whole) % string.size;
                let older = (newer + string.size - 1) % string.size;
                let back = line[newer] + part * (line[older] - line[newer]);
                string.low += (1.0 - FREE_POLE) * (back - string.low);
                // A damped string keeps a fifth of itself each time round: gone in a moment.
                let kept = if string.open { string.ring } else { 0.2 };
                let fed = if string.open { coupling * heard } else { 0.0 };
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
        if !self.any {
            return;
        }
        let mut any = false;
        for string in &mut self.strings {
            let was = string.moving;
            string.moving = string.loudest > FREE_FLOOR;
            string.loudest = 0.0;
            if was && !string.moving && !string.open {
                if let Some(line) = self.lines.get_mut(string.start..string.start + string.size) {
                    line.fill(0.0);
                }
                string.low = 0.0;
            }
            any |= string.open || string.moving;
        }
        self.any = any;
    }
}

/// The case, the keybed and the frame of a piano, which every blow knocks.
///
/// A hammer's blow does not all go into its strings. The key lands on the keybed, the action
/// recoils, the frame takes what the strings send to their near end, and the wood rings: a
/// quarter of a second of pitchless knock under every note, much the same whichever key it is —
/// which is why it is nothing beside a bass note and half of what is heard of the top octave.
/// It is a small bank of modes of its own, each heard in a sense of its own so that what comes
/// out is a thump and not a click, and it is driven by the force of every hammer on its strings:
/// a treble blow is over sooner and knocks higher.
#[derive(Clone, Debug, Default)]
struct Case {
    modes: Rotors,
    ringing: bool,
}

impl Case {
    fn prepare(&mut self, rate: f32) {
        const SEED: u32 = 0x00CA_5E00;
        self.modes.prepare(CASE_MODES);
        let nepers = CASE_DECAY * NEPERS_PER_DB;
        for index in 0..CASE_MODES {
            let along = (index as f32 + 0.5) / CASE_MODES as f32;
            let astray = 1.0 + 0.06 * (seeded(SEED, index as u32 * 2) - 0.5);
            let hz = CASE_SPAN.0 * (CASE_SPAN.1 / CASE_SPAN.0).powf(along) * astray;
            if hz >= rate * 0.45 {
                self.modes.set(index, Rotor::default());
                continue;
            }
            let (cosine, sine) = pole(hz, nepers, rate);
            let weight = (hz / CASE_FLOOR).min(1.0).powf(1.8);
            let angle = TAU * seeded(SEED, index as u32 * 2 + 1);
            self.modes.set(
                index,
                Rotor {
                    cosine,
                    sine,
                    drive: DRIVE / rate,
                    listen_re: weight * angle.cos(),
                    listen_im: weight * angle.sin(),
                },
            );
        }
        self.modes.set_len(CASE_MODES);
        self.reset();
    }

    fn reset(&mut self) {
        self.modes.rest();
        self.ringing = false;
    }

    /// Adds the knock to `out`: `force` is every hammer's push on its strings, already scaled
    /// by how hard each knocks, and `struck` says whether any hammer is at its strings.
    fn process(&mut self, force: &[f32], struck: bool, out: &mut [f32]) {
        if struck {
            self.modes.ring_driven(force, out);
            self.ringing = true;
        } else if self.ringing {
            self.modes.ring(out);
        }
    }

    /// Notes when the knock has died away, so that a silent piano does no work for it.
    fn tick(&mut self) {
        if self.ringing {
            self.ringing = self.modes.settle(FLOOR) > 0.0;
        }
    }
}

/// A piano or a hammered dulcimer.
pub(super) struct Struck {
    model: Model,
    rate: f32,
    settings: Settings,
    performance: Performance,
    voices: Vec<Voice>,
    body: Radiator,
    sympathetic: Sympathetic,
    case: Case,
    /// Cents each key is tuned from equal temperament, and the stiffness that was worked out for.
    stretch: [f32; 128],
    tuned_for: f32,
    pan_step: f32,
}

impl Struck {
    pub(super) fn new(model: Model) -> Self {
        Self {
            model,
            rate: 48_000.0,
            settings: Settings::default(),
            performance: Performance::default(),
            voices: Vec::new(),
            body: Radiator::default(),
            sympathetic: Sympathetic::default(),
            case: Case::default(),
            stretch: [0.0; 128],
            tuned_for: f32::NAN,
            pan_step: 1.0,
        }
    }

    fn is_piano(&self) -> bool {
        self.model == Model::Piano
    }

    /// The stiffness of the string of `key`: how sharp its partials run.
    fn inharmonicity(&self, key: f32) -> f32 {
        if self.is_piano() {
            across_keys(grand::INHARMONICITY, key) * self.settings.stiffness
        } else {
            // A dulcimer's short plain wires, all much alike.
            4.8e-4 * self.settings.stiffness
        }
    }

    /// How far each key is tuned from equal temperament, in cents.
    ///
    /// A stiff string's overtones run sharp of its harmonics. A piano with every fundamental
    /// on equal temperament therefore beats in its octaves, and its bass sounds sharp, because
    /// the ear hears a low note's pitch in its overtones. It is tuned the way a technician
    /// tunes one, outward from A4. The octave under A4 is laid first, evenly; every other key
    /// then lies an octave from one already tuned, by a width that splits the difference
    /// between two pure octaves: the 2:1 and the 4:2 from A2 up, the 6:3 and the 8:4 from C2
    /// down, and between those keys a pair that moves from the one to the other. Without
    /// stiffness every octave is pure and the piano is in equal temperament.
    ///
    /// With the stiffness of a recorded Steinway B's strings, which `grand::INHARMONICITY`
    /// is, this lays the strings where that Steinway's were found.
    /// `docs/piano-stretch-tuning.md` reads both pianos off their sound.
    fn tune_keyboard(&mut self) {
        self.tuned_for = self.settings.stiffness;
        if !self.is_piano() {
            self.stretch = [0.0; 128];
            return;
        }
        /// The octave that is laid first: A3 to A4.
        const A3: usize = 57;
        const A4: usize = 69;
        /// The lowest key whose octave is judged on the 2:1 and the 4:2: A2.
        const TENOR: usize = 45;
        /// The highest key whose octave is judged on the 6:3 and the 8:4: C2.
        const BASS: usize = 36;
        let stiffness = |key: i32| self.inharmonicity(key.clamp(0, 127) as f32);
        let partial = |key: i32, n: f32| {
            let b = stiffness(key);
            600.0 * ((1.0 + b * n * n) / (1.0 + b)).log2()
        };
        // Width of the octave from `key` to `key + 12` beyond 1200 cents.
        let octave = |key: usize| {
            let key = key as i32;
            // How far the pair of partials has moved from the tenor's to the bass's.
            let low = ((TENOR as f32 - key as f32) / (TENOR - BASS) as f32).clamp(0.0, 1.0);
            let (nearer, further) = (1.0 + 2.0 * low, 1.0 + low);
            let narrow = partial(key, 2.0 * nearer) - partial(key + 12, nearer);
            let wide = partial(key, 4.0 * further) - partial(key + 12, 2.0 * further);
            0.5 * (narrow + wide)
        };
        let mut stretch = [0.0_f32; 128];
        let first = octave(A3);
        for (pitch, slot) in stretch.iter_mut().enumerate().take(A4).skip(A3) {
            *slot = -first * (A4 - pitch) as f32 / 12.0;
        }
        for pitch in A4 + 1..stretch.len() {
            stretch[pitch] = stretch[pitch - 12] + octave(pitch - 12);
        }
        for pitch in (0..A3).rev() {
            stretch[pitch] = stretch[pitch + 12] - octave(pitch);
        }
        self.stretch = stretch;
    }

    fn stringing(&self, fundamental: f32, uneven: f32) -> Stringing {
        let detune = f64::from(self.settings.detune);
        let uneven = UNEVEN * f64::from(uneven);
        // A tuner leaves a treble unison beating no faster than it would lower down.
        let tuned = f64::from(1200.0 * (1.0 + TUNED_BEAT / fundamental).log2() / TUNED).min(1.0);
        let apart = detune * if self.is_piano() { tuned } else { 1.0 };
        let pair = Stringing {
            strings: 2,
            cents: [-apart, apart, 0.0],
            weights: [1.0, 1.0, 0.0],
            shares: [1.0 + uneven, 1.0 - uneven, 0.0],
        };
        let triple = Stringing {
            strings: 3,
            cents: [-apart, 0.0, apart],
            weights: [1.0; 3],
            shares: [1.0 + uneven, 1.0, 1.0 - uneven],
        };
        if !self.is_piano() {
            return if fundamental < 110.0 { pair } else { triple };
        }
        if fundamental < 65.0 {
            // One wound string, and the plane it does not mostly move in.
            let [weight, share, offset] = POLARISATION;
            Stringing {
                strings: 2,
                cents: [0.0, offset * detune / f64::from(TUNED), 0.0],
                weights: [1.0, weight, 0.0],
                shares: [1.0, share, 0.0],
            }
        } else if fundamental < 180.0 {
            pair
        } else {
            triple
        }
    }

    /// Strings that share the hammer on the note at `fundamental`, as the impedance sees them.
    fn struck_strings(&self, fundamental: f32) -> f32 {
        if self.is_piano() && fundamental < 65.0 {
            1.0
        } else {
            self.stringing(fundamental, 1.0).strings as f32
        }
    }

    /// Wave impedance of one string of `key`, in kilograms per second.
    fn impedance(&self, key: f32) -> f32 {
        if self.is_piano() {
            across_keys(grand::IMPEDANCE, key)
        } else {
            WIRE
        }
    }

    /// Own loss of partial `order` of the note on `key`, in nepers per second, undamped.
    fn own_loss(&self, key: f32, fundamental: f32, hz: f32, order: f32, stiffness: f32) -> f32 {
        let settings = &self.settings;
        if self.is_piano() {
            let scale = NATURAL_DECAY / settings.decay.max(0.01) * (1.0 + 6.0 * settings.damping);
            let (khz, low) = (hz * 1.0e-3, fundamental * 1.0e-3);
            let friction = FRICTION * dial(dials::FRICTION, 1.0);
            (aftersound(key) + friction * (khz * khz - low * low).max(0.0)) * scale * NEPERS_PER_DB
        } else {
            let (friction, bending) = material(0.05);
            let target = decay_rate(settings.decay) * (fundamental / 293.66).powf(0.4);
            Losses::tuned(target, fundamental, stiffness, friction, bending)
                .rate(hz, stiffness * order * order)
        }
    }

    /// What the bridge takes from the shared motion of partial `order`, in nepers per second.
    fn bridge_loss(&self, key: f32, order: f32) -> f32 {
        if self.is_piano() {
            bridge(key) * order.powf(bridge_slope()) * NEPERS_PER_DB
        } else {
            (3.0 + 36.0 * self.settings.damping) * NEPERS_PER_DB
        }
    }

    /// How far the bridge moves for a given pull from partial `order` of `key`, relative to
    /// middle C's fundamental on the piano as built.
    ///
    /// The bridge can only take from a string what the string is losing, and a string of
    /// impedance `Z` pulling with force `F` at frequency `f` loses energy at a rate set by the
    /// bridge's give. Working that backwards from the measured decay, the bridge's velocity
    /// for a unit of what the hammer put into the partial is the decay rate over the
    /// impedance of every string that shares it — and the partial's frequency drops out.
    fn give(&self, key: f32, fundamental: f32, order: f32) -> f32 {
        let strings = self.struck_strings(fundamental);
        if self.is_piano() {
            let taken = bridge(key) * order.powf(bridge_slope()) + aftersound(key);
            let middle =
                (bridge(60.0) + aftersound(60.0)) / (3.0 * across_keys(grand::IMPEDANCE, 60.0));
            let brought = between(&REGISTER, key) + dial_across(dials::LEVEL, key, 0.0);
            let trim = 10.0_f32.powf(brought / 20.0);
            taken / (strings * self.impedance(key)) / middle * trim
        } else {
            // On the same scale: light wires move a bridge further for the same blow.
            let middle = (BRIDGE[3] + AFTERSOUND[3]) / (3.0 * across_keys(grand::IMPEDANCE, 60.0));
            (3.0 + 36.0 * self.settings.damping) / (strings * WIRE) / middle
        }
    }

    /// Decay a damper adds to the strings of `key`, in nepers per second: 80 dB in the release
    /// time, longer in the bass.
    fn damper(&self, key: f32) -> f32 {
        let curve = if self.is_piano() {
            register(&DAMPER, key)
        } else {
            1.0
        };
        80.0 * NEPERS_PER_DB / (self.settings.release.max(0.005) * curve)
    }

    /// Works out every phasor of `voice` from the controls as they stand: where it turns, how
    /// fast it dies, how the hammer drives it and how the bridge hears it.
    fn string_up(&mut self, index: usize) {
        let rate = self.rate;
        let Some(voice) = self.voices.get(index) else {
            return;
        };
        let (key, fundamental, stiffness) =
            (f32::from(voice.key), voice.fundamental, voice.stiffness);
        let (partials, contact, damped) = (voice.partials, voice.contact, voice.damped);
        let stringing = self.stringing(fundamental, voice.uneven);
        let strings = stringing.strings;
        let damper = if damped { self.damper(key) } else { 0.0 };
        let drive = DRIVE / rate;
        for n in 0..partials {
            let order = (n + 1) as f32;
            let hz = fundamental * partial_ratio(order, stiffness);
            let own = self.own_loss(key, fundamental, hz, order, stiffness) + damper;
            let bridge = self.bridge_loss(key, order);
            let give = self.give(key, fundamental, order);
            let detune: [f64; STRINGS] = std::array::from_fn(|string| {
                f64::from(TAU * hz) * ((stringing.cents[string] / 1200.0).exp2() - 1.0)
            });
            let (found, count) = normals(
                &detune[..strings],
                &stringing.weights[..strings],
                &stringing.shares[..strings],
                f64::from(bridge),
            );
            // The bridge is at the far end, where successive partials pull opposite ways.
            let sign = if n % 2 == 0 { give } else { -give };
            let struck = (PI * order * contact).sin() * drive;
            let Some(voice) = self.voices.get_mut(index) else {
                return;
            };
            for string in 0..strings {
                let at = n * strings + string;
                let normal = found.get(string).filter(|_| string < count);
                let (Some(hz_slot), Some(nepers_slot)) =
                    (voice.hz.get_mut(at), voice.nepers.get_mut(at))
                else {
                    break;
                };
                let Some(normal) = normal else {
                    *hz_slot = 0.0;
                    *nepers_slot = 0.0;
                    voice.rotors.set(at, Rotor::default());
                    continue;
                };
                *hz_slot = hz + normal.shift / TAU;
                *nepers_slot = own + normal.decay;
                // The blow drives the velocity half; the bridge feels the displacement half,
                // through the weight the unison gives this motion.
                voice.rotors.set(
                    at,
                    Rotor {
                        drive: struck,
                        listen_re: sign * normal.heard.1,
                        listen_im: sign * normal.heard.0,
                        ..Rotor::default()
                    },
                );
            }
        }
        let cents = self.bend_cents();
        if let Some(voice) = self.voices.get_mut(index) {
            voice.strings = strings;
            voice.count = partials * strings;
            voice.retune(cents, rate);
        }
    }

    fn bend_cents(&self) -> f32 {
        self.performance.bend * 100.0
    }

    fn lower_damper(&mut self, index: usize) {
        let Some(voice) = self.voices.get(index) else {
            return;
        };
        if voice.damped || (self.is_piano() && voice.key > HIGHEST_DAMPER) {
            return;
        }
        let (damper, rate) = (self.damper(f32::from(voice.key)), self.rate);
        let voice = &mut self.voices[index];
        voice.damped = true;
        voice.damp(damper, rate);
    }

    fn lift_damper(&mut self, index: usize) {
        let Some(voice) = self.voices.get(index).filter(|voice| voice.damped) else {
            return;
        };
        let (damper, rate) = (self.damper(f32::from(voice.key)), self.rate);
        let voice = &mut self.voices[index];
        voice.damped = false;
        voice.damp(-damper, rate);
    }

    fn free_amount(&self) -> f32 {
        if self.is_piano() {
            self.settings.sympathetic
        } else {
            0.0
        }
    }

    fn update_sympathetic(&mut self) {
        if self.is_piano() {
            self.sympathetic
                .update(self.performance.pedal, self.free_amount());
        }
    }

    /// Marks the strings of `voice` as no longer sounding.
    fn finish(&mut self, index: usize) {
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        voice.sounding = false;
        voice.deferred = false;
        voice.rotors.rest();
        voice.hammer.rest();
        if let Some(key) = Sympathetic::key(voice.key) {
            self.sympathetic.busy[key] = self.sympathetic.busy[key].saturating_sub(1);
        }
        voice.held = false;
    }
}

impl Engine for Struck {
    fn prepare(&mut self, rate: f32, voices: usize) {
        self.rate = rate;
        self.voices = (0..voices)
            .map(|_| {
                let mut voice = Voice::default();
                voice.rotors.prepare(ROTORS);
                voice.hz = vec![0.0; ROTORS];
                voice.nepers = vec![0.0; ROTORS];
                voice.hammer.prepare();
                voice
            })
            .collect();
        let spec = if self.is_piano() {
            &body::SOUNDBOARD
        } else {
            &body::DULCIMER
        };
        self.body.prepare(spec, 1.0, rate);
        if self.is_piano() {
            self.sympathetic.prepare(rate);
            self.case.prepare(rate);
        }
        self.pan_step = 1.0 - (-1.0 / (0.010 * rate)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.rotors.clear();
            voice.hammer.rest();
            voice.sounding = false;
            voice.held = false;
            voice.deferred = false;
            voice.damped = false;
            voice.declick = Declick::default();
            voice.mute.open();
            voice.last = 0.0;
        }
        self.body.reset();
        if self.is_piano() {
            self.case.reset();
            self.sympathetic.reset();
            self.sympathetic
                .tune(&self.stretch, self.settings.stiffness, self.rate);
            self.update_sympathetic();
        }
    }

    fn configure(&mut self, params: &ParamBank) {
        let before = self.settings;
        self.settings = Settings {
            hardness: params.at(P_HARDNESS),
            position: params.at(P_POSITION),
            decay: params.at(P_DECAY),
            damping: params.at(P_DAMPING),
            body: params.at(P_BODY),
            release: params.at(P_RELEASE),
            stiffness: params.at(P_STIFFNESS),
            detune: params.at(P_DETUNE),
            sympathetic: if self.is_piano() {
                params.at(P_SYMPATHETIC)
            } else {
                0.0
            },
        };
        if self.tuned_for != self.settings.stiffness {
            self.tune_keyboard();
            if self.is_piano() {
                self.sympathetic
                    .tune(&self.stretch, self.settings.stiffness, self.rate);
            }
        }
        self.update_sympathetic();
        // Decay, damping and release reach the strings that are already sounding. Stiffness,
        // hardness and the striking point belong to the blow, and wait for the next one.
        let resounds = before.decay != self.settings.decay
            || before.damping != self.settings.damping
            || before.release != self.settings.release
            || before.detune != self.settings.detune;
        if resounds {
            for index in 0..self.voices.len() {
                if self.voices[index].sounding {
                    self.string_up(index);
                }
            }
        }
    }

    fn start(&mut self, index: usize, note: &Note, quality: Quality) {
        let (rate, settings) = (self.rate, self.settings);
        let key = f32::from(note.pitch);
        let fundamental = hertz(key + self.stretch[usize::from(note.pitch.min(127))] / 100.0);
        let stiffness = self.inharmonicity(key);
        let strings = self.stringing(fundamental, 1.0).strings;
        let struck = self.struck_strings(fundamental);
        let budget = match quality {
            Quality::Draft => 64,
            Quality::Standard => 192,
            Quality::High => ROTORS,
        };
        let limit = rate * 0.45;
        let mut partials = 0;
        while partials < (budget / strings).min(PARTIALS)
            && fundamental * partial_ratio((partials + 1) as f32, stiffness) * 1.002 < limit
        {
            partials += 1;
        }
        let piano = self.is_piano();
        let contact = match note.touch {
            Some(at) => touch_position(at),
            None if piano => (settings.position
                * across_keys(grand::STRIKE, key)
                * dial_across(dials::STRIKE, key, 1.0))
            .clamp(0.01, 0.5),
            None => settings.position.clamp(0.01, 0.5),
        };
        // Harder felt is stiffer by a decade either way, and stiffens a little more sharply.
        // The voicing, then the control: harder felt is stiffer by a decade either way. A
        // change of exponent turns the force law about what it gives at the pivot, so that it
        // changes the law's shape and not its size.
        let (voiced, shape) = if piano {
            (
                between(&FELT, key) + dial_across(dials::FELT, key, 0.0),
                FELT_SHAPE + dial_across(dials::EXPONENT, key, 0.0),
            )
        } else {
            (0.0, 0.0)
        };
        let felt =
            10.0_f32.powf(2.0 * (settings.hardness - 0.5) + voiced) * FELT_PIVOT.powf(-shape);
        // A felt hammer's blow grows a little faster than its speed, and a key's velocity
        // is asked for nearly the square of the speed: together they are the range of touch
        // a piano has, twice as wide in decibels as an instrument that is simply louder.
        let speed = if piano {
            dial(dials::SPEED, GRAND_SPEED) * note.velocity.powf(dial(dials::SPEED_LAW, TOUCH))
        } else {
            FULL_SPEED * note.velocity
        };
        let impedance = self.impedance(key) * struck;
        // A dulcimer player strikes the courses the box favours a little more lightly. A
        // piano's keys are evened out by whoever regulates it, and by its soundboard.
        // Below its lowest course, G2, a dulcimer has no strings: a note written there is
        // struck the more gently the further down it is.
        let evened = if piano {
            1.0
        } else {
            self.body.favour(fundamental).powf(-0.6).clamp(0.6, 1.5) * (fundamental / 98.0).min(1.0)
        };
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        let (was_sounding, previous_key) = (voice.sounding, voice.key);
        voice.declick = if note.stolen {
            Declick::from(voice.last, rate)
        } else {
            Declick::default()
        };
        voice.rotors.clear();
        voice.mute.open();
        voice.key = note.pitch;
        voice.fundamental = fundamental;
        voice.stiffness = stiffness;
        voice.contact = contact;
        // No two blows fall across a unison quite alike.
        voice.uneven = 0.85 + 0.3 * seeded(note.seed, 0);
        // The knock is the key's and the action's as much as the hammer's, and a light
        // treble hammer knocks as hard as a heavy one: it goes by the blow's speed alone.
        let middle = across_keys(grand::MASS, 60.0) * 3.0;
        let heavier = across_keys(grand::MASS, key) * struck / middle;
        voice.knock = if piano {
            KNOCK / heavier * 10.0_f32.powf(dial_across(dials::KNOCK, key, 0.0) / 20.0)
        } else {
            0.0
        };
        voice.partials = partials;
        voice.pan = note.pan;
        voice.pan_now = note.pan * self.performance.width;
        voice.held = true;
        voice.deferred = false;
        voice.damped = false;
        voice.sounding = true;
        voice.moving = 0.0;
        voice.loudest = 0.0;
        let blow = if piano {
            // The published hammer is one string's share of it: a third of the mass and a
            // third of the felt, against one string of three. Thrown at every string of the
            // note it is that many times as heavy and as stiff.
            Blow {
                speed,
                mass: across_keys(grand::MASS, key) * struck,
                stiffness: across_keys(grand::STIFFNESS, key) * struck * felt,
                exponent: across_keys(grand::EXPONENT, key)
                    + 0.4 * (settings.hardness - 0.5)
                    + shape,
                loss: dial(dials::FELT_LOSS, 0.1),
                impedance,
                period: 1.0 / fundamental,
                position: contact,
            }
        } else {
            // A light wooden hammer faced with leather, on wires a fifth as heavy as a
            // piano's.
            Blow {
                speed: speed * 0.8 * evened,
                mass: 1.6e-3,
                stiffness: 6.0e8 * felt,
                exponent: 2.3,
                loss: 0.05,
                impedance,
                period: 1.0 / fundamental,
                position: contact,
            }
        };
        voice.hammer.throw(blow, rate);
        if let Some(key) = Sympathetic::key(previous_key).filter(|_| was_sounding) {
            self.sympathetic.busy[key] = self.sympathetic.busy[key].saturating_sub(1);
        }
        if let Some(key) = Sympathetic::key(note.pitch) {
            self.sympathetic.down[key] = true;
            if piano && note.velocity < SILENT {
                // The key is down and its damper up, and that is all: the voice the shell
                // set aside for it has nothing to play.
                if let Some(voice) = self.voices.get_mut(index) {
                    voice.sounding = false;
                    voice.held = false;
                    voice.hammer.rest();
                }
                self.update_sympathetic();
                return;
            }
            self.sympathetic.busy[key] = self.sympathetic.busy[key].saturating_add(1);
        }
        self.string_up(index);
        if let Some(voice) = self.voices.get_mut(index) {
            voice.rotors.set_len(voice.count);
        }
        self.update_sympathetic();
    }

    fn stop(&mut self, index: usize) {
        let pedal = self.performance.pedal && self.is_piano();
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        if !voice.held {
            return;
        }
        voice.held = false;
        if pedal {
            voice.deferred = true;
        } else {
            self.lower_damper(index);
        }
        self.update_sympathetic();
    }

    fn lift(&mut self, pitch: u8) {
        if let Some(key) = Sympathetic::key(pitch).filter(|key| self.sympathetic.down[*key]) {
            self.sympathetic.down[key] = false;
            self.update_sympathetic();
        }
    }

    fn silence(&mut self, index: usize) {
        let rate = self.rate;
        if let Some(voice) = self.voices.get_mut(index) {
            voice.mute.close(rate);
            voice.held = false;
            voice.deferred = false;
        }
    }

    fn perform(&mut self, performance: &Performance) {
        let bent = performance.bend != self.performance.bend;
        let pedalled = performance.pedal != self.performance.pedal;
        self.performance = *performance;
        if bent {
            let (cents, rate) = (self.bend_cents(), self.rate);
            for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
                voice.retune(cents, rate);
            }
        }
        if pedalled && self.is_piano() {
            for index in 0..self.voices.len() {
                let voice = &self.voices[index];
                if !voice.sounding {
                    continue;
                }
                if !performance.pedal && voice.deferred {
                    self.voices[index].deferred = false;
                    self.lower_damper(index);
                } else if performance.pedal && !voice.held && voice.damped {
                    // The pedal lifts every damper, and catches a note already fading.
                    self.lift_damper(index);
                    self.voices[index].deferred = true;
                }
            }
            self.update_sympathetic();
        }
    }

    fn render(&mut self, mid: &mut [f32], side: &mut [f32], tick: bool) {
        let frames = mid.len().min(side.len()).min(TICK);
        let width = self.performance.width;
        if tick {
            self.body.tick();
            self.sympathetic.tick();
            self.case.tick();
            let mut ended = false;
            for index in 0..self.voices.len() {
                let voice = &mut self.voices[index];
                if !voice.sounding || voice.hammer.is_striking() {
                    continue;
                }
                voice.moving = voice.rotors.settle(FLOOR);
                voice.loudest = voice.loudest.max(voice.moving);
                if voice.moving == 0.0 || voice.mute.is_closed() {
                    self.finish(index);
                    ended = true;
                }
            }
            if ended {
                self.update_sympathetic();
            }
        }
        let (mut bus_mid, mut bus_side) = ([0.0_f32; TICK], [0.0_f32; TICK]);
        let (mut strings, mut force) = ([0.0_f32; TICK], [0.0_f32; TICK]);
        let mut knocks = [0.0_f32; TICK];
        let mut knocked = false;
        for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
            let strings = &mut strings[..frames];
            strings.fill(0.0);
            if voice.hammer.is_striking() {
                let force = &mut force[..frames];
                voice.hammer.strike(force);
                voice.rotors.ring_driven(force, strings);
                // What the blow does to the case, beside what it does to the strings.
                for (sample, push) in knocks.iter_mut().zip(force.iter()) {
                    *sample += push * voice.knock;
                }
                knocked = true;
            } else {
                voice.rotors.ring(strings);
            }
            let target = voice.pan * width;
            for ((sample, to_mid), to_side) in strings
                .iter()
                .zip(bus_mid.iter_mut())
                .zip(bus_side.iter_mut())
            {
                let heard = sample * voice.mute.next() + voice.declick.next();
                voice.pan_now += (target - voice.pan_now) * self.pan_step;
                voice.last = heard;
                *to_mid += heard;
                *to_side += heard * voice.pan_now;
            }
        }
        let (bus_mid, bus_side) = (&mut bus_mid[..frames], &mut bus_side[..frames]);
        if self.is_piano() {
            let amount = self.free_amount();
            self.sympathetic.process(bus_mid, amount);
        }
        self.body
            .process(bus_mid, bus_side, self.settings.body, width);
        if self.is_piano() {
            self.case.process(&knocks[..frames], knocked, bus_mid);
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
                if voice.loudest > 0.0 {
                    (voice.moving / voice.loudest).clamp(1.0e-6, 1.0)
                } else {
                    1.0
                }
            })
    }

    fn is_held(&self, index: usize) -> bool {
        self.voices
            .get(index)
            .is_some_and(|voice| voice.held || voice.deferred)
    }

    fn observe(&self, index: usize, picture: &mut MotionVoice) {
        let Some(voice) = self.voices.get(index) else {
            return;
        };
        picture.level = self.level(index);
        picture.contact = voice.contact;
        picture.excitation = voice.hammer.contact();
        let strings = voice.strings.max(1);
        let key = f32::from(voice.key);
        // What the bridge hears of each partial is its slope there; the displacement is that
        // over the partial's number, with the alternating sign and the bridge's give taken
        // back out.
        let mut shape = [0.0_f32; 24];
        for (n, amount) in shape.iter_mut().enumerate().take(voice.partials) {
            let order = (n + 1) as f32;
            let give = self.give(key, voice.fundamental, order).max(1.0e-6);
            let sign = if n % 2 == 0 { 1.0 } else { -1.0 };
            let heard: f32 = (0..strings)
                .map(|string| voice.rotors.heard(n * strings + string))
                .sum();
            *amount = sign * heard / (give * order);
        }
        let last = (MOTION_POINTS - 1) as f32;
        for (at, point) in picture.points.iter_mut().enumerate() {
            let x = at as f32 / last;
            *point = shape
                .iter()
                .enumerate()
                .map(|(n, amount)| amount * (PI * (n + 1) as f32 * x).sin())
                .sum::<f32>()
                * DISPLAY;
        }
        picture.points[0] = 0.0;
        picture.points[MOTION_POINTS - 1] = 0.0;
        for n in 0..MOTION_PARTIALS {
            let size = if n < voice.partials {
                (0..strings)
                    .map(|string| {
                        let (re, im) = voice.rotors.at(n * strings + string);
                        re.hypot(im)
                    })
                    .sum::<f32>()
                    / strings as f32
                    * DISPLAY
            } else {
                0.0
            };
            picture.partials[n] = size;
            if let Some(mode) = picture.modes.get_mut(n) {
                *mode = size;
            }
        }
        for (sample, force) in picture.scope.iter_mut().zip(voice.hammer.trace()) {
            *sample = force / BLOW_NEWTONS;
        }
    }

    fn observe_body(&self, body: &mut [f32; MOTION_BODY]) {
        self.body.observe(body);
        for mode in body.iter_mut() {
            *mode *= BODY_DISPLAY * self.settings.body;
        }
    }
}
