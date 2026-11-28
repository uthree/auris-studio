//! Timpani: a calfskin or plastic head stretched over a copper kettle, struck with felt.
//!
//! A drum head is a membrane, and a membrane's modes are nowhere near harmonic: on its own, a
//! head with its first mode at 100 Hz has the next ones at 159, 213, 230 and 265. What makes a
//! kettledrum a drum with a *note* is the air under the head. The kettle's air stiffens the
//! modes that would squeeze it — the ones with no nodal diameter, which move the head's middle
//! up and down — and loads the rest; and the head moving the air of the room as it does lowers
//! its modes too, the large ones most. A well-made drum comes out of that with the modes that
//! have one, two, three and four nodal diameters close to a fifth, an octave and a tenth above
//! the first of them: the harmonics two to five of a note an octave down, which is the note
//! the ear hears. The table is Rossing's, measured on a 65-centimetre drum.
//!
//! The modes that squeeze the kettle push air in and out of the whole head at once and so
//! hand their energy to the room within a fraction of a second: they are the thud of the
//! stroke, and in the first tenth of a second they are most of what is heard. The others,
//! whose halves cancel each other's push, ring for seconds — and not the first of them
//! longest: recordings of struck timpani have the fifth over it ringing on after the note
//! itself has faded, the octave after that, and a cloud of the head's higher modes under
//! both. A head is never quite even, either, so each mode with nodal diameters rings in two
//! orientations a few cents apart, and beats. A model that left all that out, with the
//! first ringing mode strongest throughout and nothing between its partials, sounded like a
//! plucked bass string.
//!
//! Where the stick lands decides which modes it wakes, as it does on a bar: each mode moves as
//! a Bessel function of the distance from the middle, and a timpanist plays about a quarter of
//! the way in from the rim, where the ringing modes are strong and the thud is weak. How long
//! the felt stays on the head decides how high the spectrum reaches. A roll is the same
//! stroke, repeated from alternating sticks for as long as the note is held.

use auris_core::motion::MotionVoice;
use auris_core::{ParamDescriptor, ParamUnit};

use super::bars::contact_spectrum;
use super::rotors::{Rotor, Rotors, decay_rate, pole};
use super::{Declick, Engine, Mute, Note, Performance, Quality, TICK, blow_scope, hertz};
use crate::ParamBank;
use std::borrow::Cow;

const P_HARDNESS: u32 = 0;
const P_POSITION: u32 = 1;
const P_DECAY: u32 = 2;
const P_DAMPING: u32 = 3;
const P_RELEASE: u32 = 4;
const P_TECHNIQUE: u32 = 5;
const P_ROLL_RATE: u32 = 6;

const TECHNIQUES: &[Cow<'static, str>] = &[Cow::Borrowed("Single"), Cow::Borrowed("Roll")];

/// One mode of the head: its nodal diameters and circles, the zero of the Bessel function
/// that is its shape, its frequency over the first ringing mode's, how long it rings against
/// the longest-ringing mode, how much of its motion reaches the room, and — for a mode with
/// nodal diameters — how far apart in cents its two orientations ring and where the head's
/// unevenness has turned them.
#[derive(Clone, Copy, Debug)]
struct Mode {
    diameters: u32,
    zero: f32,
    ratio: f32,
    sustain: f32,
    radiates: f32,
    split: f32,
    turned: f32,
}

const fn mode(
    diameters: u32,
    zero: f32,
    ratio: f32,
    sustain: f32,
    radiates: f32,
    split: f32,
    turned: f32,
) -> Mode {
    Mode {
        diameters,
        zero,
        ratio,
        sustain,
        radiates,
        split,
        turned,
    }
}

/// Rossing's modes of a 65 cm timpano, from the one with no nodal line but the rim.
const MODES: [Mode; 13] = [
    mode(0, 2.404_8, 0.85, 0.12, 3.0, 0.0, 0.0),
    mode(1, 3.831_7, 1.0, 0.5, 1.6, 5.0, 0.5),
    mode(2, 5.135_6, 1.5, 1.0, 0.9, 8.0, 1.1),
    mode(0, 5.520_1, 1.68, 0.15, 2.5, 0.0, 0.0),
    mode(3, 6.380_2, 1.99, 1.0, 0.3, 11.0, 0.3),
    mode(1, 7.015_6, 2.09, 0.8, 0.9, 14.0, 0.9),
    mode(4, 7.588_3, 2.44, 0.8, 0.15, 9.0, 1.3),
    mode(2, 8.417_2, 2.67, 0.8, 0.8, 17.0, 0.7),
    mode(0, 8.653_7, 2.79, 0.15, 2.0, 0.0, 0.0),
    mode(5, 8.771_5, 2.89, 0.8, 0.7, 12.0, 0.4),
    mode(3, 9.761_0, 3.17, 0.7, 0.7, 19.0, 1.0),
    mode(6, 9.936_1, 3.34, 0.8, 0.6, 15.0, 0.6),
    mode(7, 11.086_4, 3.78, 0.7, 0.6, 21.0, 1.2),
];
const COUNT: usize = MODES.len();
/// Two phasors a mode: a mode with nodal diameters rings in two orientations at right angles,
/// which an uneven head tunes a little apart. A mode with none uses only the first.
const PHASORS: usize = 2 * COUNT;
const FLOOR: f32 = 1.0e-6;
const DISPLAY: f32 = 0.05;
/// The note at which the decay control is the decay time of the longest-ringing mode, the
/// fifth over the note.
const REFERENCE_HZ: f32 = 110.0;
/// Power of how much higher a drum is tuned by which its modes die faster: a tighter, smaller
/// head hands its energy to the air sooner.
const LOSS_SLOPE: f32 = 0.5;
/// Cents a full stroke raises the head while it moves furthest: a struck head is stretched by
/// its own motion, and settles as the motion dies.
const STRETCH: f32 = 18.0;
/// How far each roll stroke's strength wanders from the last, and how far its timing does,
/// as a share of the time between strokes.
const ROLL_SCATTER: f32 = 0.18;
const ROLL_JITTER: f32 = 0.12;
/// How hard a roll's strokes are against a single stroke at the same velocity: a roll is
/// played from the wrists, close to the head.
const ROLL_STRENGTH: f32 = 0.45;

/// The Bessel function of the first kind of order `order`, from its power series: enough for
/// the arguments a head's modes reach, which are under twelve.
fn bessel(order: u32, x: f32) -> f32 {
    let half = f64::from(x) * 0.5;
    let mut term = half.powi(order as i32) / (1..=order).map(f64::from).product::<f64>();
    let mut sum = term;
    for k in 1..40 {
        term *= -half * half / (f64::from(k) * f64::from(k + order));
        sum += term;
        if term.abs() < 1.0e-12 * sum.abs().max(1.0e-30) {
            break;
        }
    }
    sum as f32
}

/// How far mode `mode` moves at `radius` from the middle, as a share of the rim, scaled so
/// that modes of the same energy are struck alike.
fn shape(mode: &Mode, radius: f32) -> f32 {
    let norm = bessel(mode.diameters + 1, mode.zero).abs().max(1.0e-6);
    let single = if mode.diameters == 0 {
        std::f32::consts::FRAC_1_SQRT_2
    } else {
        1.0
    };
    bessel(mode.diameters, mode.zero * radius.clamp(0.0, 1.0)) / norm * single
}

pub(super) fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::percent(P_HARDNESS, "hardness", "Mallet Hardness", 0.35),
        ParamDescriptor::new(
            P_POSITION,
            "position",
            "Excitation Position",
            0.05,
            0.6,
            0.25,
        )
        .with_unit(ParamUnit::Percent),
        ParamDescriptor::new(P_DECAY, "decay", "Resonance Decay", 0.2, 10.0, 4.3)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::percent(P_DAMPING, "damping", "Damping", 0.0),
        ParamDescriptor::new(
            P_RELEASE,
            "release",
            "Release",
            0.05,
            super::LET_RING,
            super::LET_RING,
        )
        .with_unit(ParamUnit::Seconds),
        ParamDescriptor::new(P_TECHNIQUE, "technique", "Technique", 0.0, 1.0, 0.0)
            .with_choices(TECHNIQUES),
        ParamDescriptor::new(P_ROLL_RATE, "roll_rate", "Roll Rate", 4.0, 24.0, 11.0)
            .with_unit(ParamUnit::Hertz),
    ]
}

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    hardness: f32,
    /// How far in from the rim the stick lands, as a share of the head's radius.
    position: f32,
    decay: f32,
    damping: f32,
    release: f32,
    roll: bool,
    roll_rate: f32,
}

#[derive(Clone, Debug, Default)]
struct Voice {
    rotors: Rotors,
    hz: [f32; COUNT],
    nepers: [f32; COUNT],
    /// How each mode takes a stroke where this note's stick lands, before the felt.
    shapes: [f32; COUNT],
    modes: usize,
    pitch: f32,
    velocity: f32,
    /// How long the felt stays on the head for this note, in seconds.
    seconds: f32,
    contact: f32,
    sounding: bool,
    held: bool,
    /// Samples to the next stroke of a roll, and how many have been played.
    next_stroke: f32,
    strokes: u32,
    /// A number that differs from note to note, for the roll's unevenness.
    seed: u32,
    /// Samples the felt is still on the head, for the picture.
    touching: usize,
    blow: (f32, f32),
    pan: f32,
    pan_now: f32,
    declick: Declick,
    mute: Mute,
    last: f32,
    moving: f32,
    loudest: f32,
    tuned: f32,
}

impl Voice {
    fn retune(&mut self, cents: f32, rate: f32) {
        for (index, spec) in MODES.iter().enumerate().take(self.modes) {
            for (half, offset) in [(0, -0.5), (1, 0.5)] {
                let hz = self.hz[index] * ((cents + offset * spec.split) / 1200.0).exp2();
                let phasor = 2 * index + half;
                if hz >= rate * 0.49 || (half == 1 && spec.diameters == 0) {
                    self.rotors.turn(phasor, 0.0, 0.0);
                    continue;
                }
                let (cosine, sine) = pole(hz, self.nepers[index], rate);
                self.rotors.turn(phasor, cosine, sine);
            }
        }
        self.tuned = cents;
    }

    /// The stick comes down with `strength`: every mode is pushed by what the felt gives it.
    fn strike(&mut self, strength: f32) {
        for (mode, spec) in MODES.iter().enumerate().take(self.modes) {
            let push = strength * self.shapes[mode] * contact_spectrum(self.hz[mode], self.seconds);
            // The stick lands on neither orientation's antinode: each takes its projection.
            let shares = if spec.diameters == 0 {
                [1.0, 0.0]
            } else {
                [spec.turned.cos(), -spec.turned.sin()]
            };
            for (half, share) in shares.into_iter().enumerate() {
                let phasor = 2 * mode + half;
                let (re, im) = self.rotors.at(phasor);
                // Struck into the velocity half and heard on the other.
                self.rotors.place(phasor, re + push * share, im);
            }
        }
    }

    /// A number between zero and one that is the same for the same stroke of the same note.
    fn scatter(&self, salt: u32) -> f32 {
        let mut x = self
            .seed
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add(self.strokes.wrapping_mul(0x85EB_CA6B))
            .wrapping_add(salt.wrapping_mul(0xC2B2_AE35));
        x ^= x >> 16;
        x = x.wrapping_mul(0x7FEB_352D);
        x ^= x >> 15;
        x as f32 / u32::MAX as f32
    }
}

/// The timpani.
pub(super) struct Kettle {
    rate: f32,
    settings: Settings,
    performance: Performance,
    voices: Vec<Voice>,
    pan_step: f32,
}

impl Kettle {
    pub(super) fn new() -> Self {
        Self {
            rate: 48_000.0,
            settings: Settings::default(),
            performance: Performance::default(),
            voices: Vec::new(),
            pan_step: 1.0,
        }
    }

    fn set_rates(&mut self, index: usize) {
        let settings = self.settings;
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        let fundamental = hertz(voice.pitch);
        let base = decay_rate(settings.decay)
            * (fundamental / REFERENCE_HZ).max(0.05).powf(LOSS_SLOPE)
            * (1.0 + 8.0 * settings.damping);
        for (mode, spec) in MODES.iter().enumerate().take(voice.modes) {
            let mut nepers = base / spec.sustain;
            if !voice.held && settings.release < super::LET_RING {
                // A hand laid on the head takes the high modes first.
                nepers += decay_rate(settings.release) * spec.ratio;
            }
            voice.nepers[mode] = nepers;
        }
    }

    /// How long the felt stays on the head: a soft stick and a slack head keep it there
    /// longer, a faster stroke less, as the fifth root of its speed.
    fn contact_seconds(&self, fundamental: f32, velocity: f32) -> f32 {
        let hardness = self.performance.hardness(self.settings.hardness);
        0.006
            * 0.2_f32.powf(hardness)
            * velocity.max(0.05).powf(-0.2)
            * (REFERENCE_HZ / fundamental).clamp(0.25, 4.0).powf(0.3)
    }
}

impl Engine for Kettle {
    fn prepare(&mut self, rate: f32, voices: usize) {
        self.rate = rate;
        self.voices = (0..voices)
            .map(|_| {
                let mut voice = Voice::default();
                voice.rotors.prepare(PHASORS);
                voice
            })
            .collect();
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
        }
    }

    fn configure(&mut self, params: &ParamBank) {
        self.settings = Settings {
            hardness: params.at(P_HARDNESS),
            position: params.at(P_POSITION),
            decay: params.at(P_DECAY),
            damping: params.at(P_DAMPING),
            release: params.at(P_RELEASE),
            roll: params.at(P_TECHNIQUE) >= 0.5,
            roll_rate: params.at(P_ROLL_RATE),
        };
        let rate = self.rate;
        for index in 0..self.voices.len() {
            if self.voices[index].sounding {
                self.set_rates(index);
                let voice = &mut self.voices[index];
                let tuned = voice.tuned;
                voice.retune(tuned, rate);
            }
        }
    }

    fn start(&mut self, index: usize, note: &Note, quality: Quality) {
        let (rate, settings) = (self.rate, self.settings);
        let pitch = note.pitch;
        // The note written is the one heard, which is the first ringing mode's.
        let fundamental = hertz(pitch);
        let contact = match note.touch {
            Some(at) => at.clamp(0.0, 0.98),
            None => 1.0 - settings.position,
        };
        let seconds = self.contact_seconds(fundamental, note.velocity);
        let budget = if quality == Quality::Draft { 6 } else { COUNT };
        let limit = rate * 0.45;
        let pan_width = self.performance.width;
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        voice.declick = if note.stolen {
            Declick::from(voice.last, rate)
        } else {
            Declick::default()
        };
        voice.rotors.clear();
        voice.mute.open();
        voice.pitch = pitch;
        voice.velocity = note.velocity;
        voice.seconds = seconds;
        voice.contact = contact;
        voice.pan = note.pan;
        voice.pan_now = note.pan * pan_width;
        voice.held = true;
        voice.sounding = true;
        voice.seed = note.seed;
        voice.strokes = 1;
        voice.next_stroke = rate / settings.roll_rate.max(1.0);
        voice.touching = (seconds * rate) as usize;
        voice.blow = (seconds, note.velocity);
        voice.modes = 0;
        for (mode, spec) in MODES.iter().enumerate().take(budget) {
            let hz = fundamental * spec.ratio;
            if hz >= limit {
                break;
            }
            voice.hz[mode] = hz;
            voice.shapes[mode] = shape(spec, contact);
            voice.modes = mode + 1;
        }
        voice.rotors.set_len(2 * voice.modes);
        self.set_rates(index);
        let voice = &mut self.voices[index];
        let stretch = STRETCH * note.velocity * note.velocity;
        voice.retune(self.performance.bend * 100.0 + stretch, rate);
        for phasor in 0..2 * voice.modes {
            let spec = &MODES[phasor / 2];
            let (cosine, sine) = voice.rotors.pole(phasor);
            let heard = phasor % 2 == 0 || spec.diameters > 0;
            voice.rotors.set(
                phasor,
                Rotor {
                    cosine,
                    sine,
                    drive: 0.0,
                    listen_re: 0.0,
                    listen_im: if heard { spec.radiates } else { 0.0 },
                },
            );
        }
        voice.strike(note.velocity);
        voice.moving = voice.rotors.settle(0.0);
        voice.loudest = voice.moving.max(1.0e-9);
    }

    fn stop(&mut self, index: usize) {
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        if !voice.held {
            return;
        }
        voice.held = false;
        self.set_rates(index);
        let rate = self.rate;
        let voice = &mut self.voices[index];
        let tuned = voice.tuned;
        voice.retune(tuned, rate);
    }

    fn silence(&mut self, index: usize) {
        let rate = self.rate;
        if let Some(voice) = self.voices.get_mut(index) {
            voice.held = false;
            voice.mute.close(rate);
        }
    }

    fn perform(&mut self, performance: &Performance) {
        self.performance = *performance;
    }

    fn render(&mut self, mid: &mut [f32], side: &mut [f32], tick: bool) {
        let frames = mid.len().min(side.len()).min(TICK);
        let width = self.performance.width;
        let (rate, settings) = (self.rate, self.settings);
        let bend = self.performance.bend * 100.0;
        if tick {
            for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
                voice.moving = voice.rotors.settle(FLOOR);
                let rolling = settings.roll && voice.held;
                if (voice.moving == 0.0 && !rolling) || voice.mute.is_closed() {
                    voice.sounding = false;
                    voice.rotors.rest();
                    continue;
                }
                voice.loudest = voice.loudest.max(voice.moving);
                // The head's stretch dies with its motion.
                let share = (voice.moving / voice.loudest).min(1.0);
                let stretch = STRETCH * voice.velocity * voice.velocity * share;
                voice.retune(bend + stretch, rate);
                if rolling {
                    voice.next_stroke -= TICK as f32;
                    if voice.next_stroke <= 0.0 {
                        voice.strokes = voice.strokes.wrapping_add(1);
                        let strength = voice.velocity
                            * ROLL_STRENGTH
                            * (1.0 + ROLL_SCATTER * (2.0 * voice.scatter(1) - 1.0));
                        voice.strike(strength);
                        let gap = rate / settings.roll_rate.max(1.0);
                        voice.next_stroke +=
                            gap * (1.0 + ROLL_JITTER * (2.0 * voice.scatter(2) - 1.0));
                        voice.touching = (voice.seconds * rate) as usize;
                    }
                }
            }
        }
        let mut head = [0.0_f32; TICK];
        for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
            let head = &mut head[..frames];
            head.fill(0.0);
            voice.rotors.ring(head);
            voice.touching = voice.touching.saturating_sub(frames);
            let target = voice.pan * width;
            for ((sample, to_mid), to_side) in head.iter().zip(mid.iter_mut()).zip(side.iter_mut())
            {
                let heard = sample * voice.mute.next() + voice.declick.next();
                voice.pan_now += (target - voice.pan_now) * self.pan_step;
                voice.last = heard;
                *to_mid += heard;
                *to_side += heard * voice.pan_now;
            }
        }
    }

    fn level(&self, index: usize) -> f32 {
        self.voices
            .get(index)
            .filter(|voice| voice.sounding)
            .map_or(0.0, |voice| {
                (voice.moving / voice.loudest).clamp(1.0e-6, 1.0)
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
        picture.excitation = if voice.touching > 0 { 1.0 } else { 0.0 };
        // The head seen from above on an eight-by-eight grid, the stick's side along the
        // first axis.
        let side = (picture.points.len() as f32).sqrt().round() as usize;
        for (at, point) in picture.points.iter_mut().enumerate() {
            let (row, column) = (at / side.max(1), at % side.max(1));
            let x = (column as f32 + 0.5) / side as f32 * 2.0 - 1.0;
            let y = (row as f32 + 0.5) / side as f32 * 2.0 - 1.0;
            let radius = x.hypot(y);
            *point = if radius > 1.0 {
                0.0
            } else {
                let angle = y.atan2(x);
                MODES
                    .iter()
                    .enumerate()
                    .take(voice.modes)
                    .map(|(mode, spec)| {
                        let turn = spec.diameters as f32 * angle - spec.turned;
                        let moving = voice.rotors.at(2 * mode).1 * turn.cos()
                            + voice.rotors.at(2 * mode + 1).1 * turn.sin();
                        moving / spec.ratio.sqrt() * shape(spec, radius)
                    })
                    .sum::<f32>()
                    * DISPLAY
            };
        }
        for (mode, value) in picture.partials.iter_mut().enumerate() {
            *value = if mode < voice.modes {
                let (a, b) = (voice.rotors.at(2 * mode), voice.rotors.at(2 * mode + 1));
                a.0.hypot(a.1).hypot(b.0.hypot(b.1)) * DISPLAY
            } else {
                0.0
            };
        }
        let shown = picture.modes.len();
        picture.modes.copy_from_slice(&picture.partials[..shown]);
        blow_scope(voice.blow.0, voice.blow.1, &mut picture.scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bessel_functions_cross_zero_where_the_modes_meet_the_rim() {
        for mode in MODES {
            assert!(
                bessel(mode.diameters, mode.zero).abs() < 2.0e-4,
                "J{}({}) = {}",
                mode.diameters,
                mode.zero,
                bessel(mode.diameters, mode.zero)
            );
        }
        assert!((bessel(0, 0.0) - 1.0).abs() < 1.0e-6);
        assert!(bessel(1, 0.0).abs() < 1.0e-6);
        assert!((bessel(1, 1.0) - 0.440_05).abs() < 1.0e-4);
    }

    #[test]
    fn a_stroke_near_the_rim_wakes_the_ringing_modes_more_than_the_thud() {
        let at = |mode: usize, radius: f32| shape(&MODES[mode], radius).abs();
        // In the middle only the modes with no nodal diameter move.
        assert!(at(1, 0.0) < 1.0e-6 && at(0, 0.0) > 0.5);
        // A quarter of the way in, the first ringing mode moves more than the thud does.
        assert!(at(1, 0.75) > at(0, 0.75), "{} {}", at(1, 0.75), at(0, 0.75));
        assert!(at(0, 0.75) < at(0, 0.0));
    }
}
