//! Bells: a cast bell, a tubular chime, a handbell and a singing bowl.
//!
//! A bell's partials are not a harmonic series, and which ones it has is the whole of its
//! character. A founder tunes a church bell's lowest five by turning metal off the inside: the
//! hum an octave below the note, the prime on it, the tierce a minor third up — the interval
//! that makes a bell sound like a bell — the quint and the nominal an octave above. A tubular
//! chime has no partial at the note it is heard to play at all: its fourth, fifth and sixth lie
//! close to two, three and four times a pitch the ear supplies.
//!
//! Nothing cast is perfectly round. Each of a bell's modes is really two, with the same shape
//! turned through half a lobe and a frequency a fraction of a hertz apart, and where the
//! clapper lands decides how much of each it rouses. The two beat, and that slow rise and fall
//! in every partial, each at its own rate, is the *warble* — and here, since the two halves of
//! a pair radiate from different parts of the rim, it is also where the bell's width comes from.

use std::borrow::Cow;
use std::f32::consts::TAU;

use auris_core::motion::MotionVoice;
use auris_core::{ParamDescriptor, ParamUnit};

use super::bars::contact_spectrum;
use super::body::seeded;
use super::rotors::{Rotor, Rotors, decay_rate, pole};
use super::{Declick, Engine, Mute, Note, Performance, Quality, TICK, hertz};
use crate::ParamBank;

const P_HARDNESS: u32 = 0;
const P_POSITION: u32 = 1;
const P_DECAY: u32 = 2;
const P_DAMPING: u32 = 3;
const P_WARBLE: u32 = 4;
const P_RELEASE: u32 = 5;
const P_BELL: u32 = 6;

/// Most partials of one bell; each is a pair.
const PARTIALS: usize = 12;
const FLOOR: f32 = 1.0e-6;
const DISPLAY: f32 = 0.05;
const REFERENCE_HZ: f32 = 261.63;
/// How far to either side the two halves of a pair are heard, at unit width.
const SPREAD: f32 = 0.45;

/// One partial: its frequency over the strike note's, how strongly a blow rouses it, how long
/// it rings relative to the bell's decay time, and how many lobes it has around the rim.
type Partial = (f32, f32, f32, u8);

/// What is struck.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Bell {
    /// A cast bell with a minor-third tierce: a church bell, a carillon.
    #[default]
    Cast,
    /// A hung tube, struck at its top.
    Tubular,
    /// A small bell rung by hand: little but its fundamental and a twelfth.
    Hand,
    /// A bowl struck on its rim, whose pairs beat slowly and for a long time.
    Bowl,
}

const BELLS: &[Cow<'static, str>] = &[
    Cow::Borrowed("Cast bell"),
    Cow::Borrowed("Tubular bell"),
    Cow::Borrowed("Handbell"),
    Cow::Borrowed("Singing bowl"),
];

impl Bell {
    fn from_value(value: f32) -> Self {
        match value.round() as i32 {
            i32::MIN..=0 => Self::Cast,
            1 => Self::Tubular,
            2 => Self::Hand,
            _ => Self::Bowl,
        }
    }

    fn partials(self) -> &'static [Partial] {
        match self {
            // Hum, prime, tierce, quint, nominal, and the upper partials of a tuned bell.
            Self::Cast => &[
                (0.5, 0.55, 1.6, 2),
                (1.0, 1.0, 1.0, 2),
                (1.2, 0.8, 0.85, 3),
                (1.5, 0.35, 0.6, 3),
                (2.0, 0.9, 0.55, 4),
                (2.5, 0.3, 0.4, 4),
                (2.66, 0.2, 0.35, 5),
                (3.0, 0.45, 0.3, 5),
                (4.0, 0.3, 0.22, 6),
                (5.33, 0.15, 0.15, 7),
                (6.67, 0.1, 0.1, 8),
                (8.0, 0.06, 0.08, 9),
            ],
            // A free tube's bending modes run as the squares of odd numbers. The fourth,
            // fifth and sixth are what the ear takes for two, three and four times the note.
            Self::Tubular => &[
                (0.224, 0.05, 1.2, 1),
                (0.617, 0.2, 1.1, 1),
                (1.21, 0.5, 1.0, 1),
                (2.0, 1.0, 0.9, 1),
                (2.99, 0.9, 0.75, 1),
                (4.17, 0.7, 0.6, 1),
                (5.56, 0.45, 0.45, 1),
                (7.14, 0.3, 0.35, 1),
                (8.9, 0.15, 0.25, 1),
            ],
            Self::Hand => &[
                (1.0, 1.0, 1.0, 2),
                (2.0, 0.12, 0.7, 2),
                (3.0, 0.4, 0.5, 3),
                (4.2, 0.12, 0.35, 3),
                (5.6, 0.15, 0.3, 4),
                (7.5, 0.06, 0.2, 4),
                (9.0, 0.03, 0.15, 5),
            ],
            Self::Bowl => &[
                (1.0, 1.0, 1.0, 2),
                (2.83, 0.55, 0.7, 3),
                (5.37, 0.3, 0.45, 4),
                (8.5, 0.15, 0.3, 5),
                (12.2, 0.07, 0.2, 6),
            ],
        }
    }

    /// How far apart the two halves of each pair are tuned, as a fraction of their frequency,
    /// with the warble control at one.
    fn split(self) -> f32 {
        match self {
            Self::Cast => 2.4e-3,
            Self::Tubular => 8.0e-4,
            Self::Hand => 1.2e-3,
            Self::Bowl => 8.0e-3,
        }
    }

    /// Length of the clapper's contact at middle C and medium hardness, in seconds.
    fn contact(self) -> f32 {
        match self {
            Self::Cast => 0.000_9,
            Self::Tubular => 0.000_7,
            Self::Hand => 0.000_6,
            Self::Bowl => 0.001_6,
        }
    }
}

pub(super) fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::percent(P_HARDNESS, "hardness", "Beater hardness", 0.5),
        ParamDescriptor::new(
            P_POSITION,
            "position",
            "Excitation Position",
            0.05,
            0.45,
            0.3,
        )
        .with_unit(ParamUnit::Percent),
        ParamDescriptor::new(P_DECAY, "decay", "Resonance Decay", 0.1, 30.0, 8.0)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::percent(P_DAMPING, "damping", "Damping", 0.0),
        ParamDescriptor::percent(P_WARBLE, "warble", "Warble", 0.5),
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 6.0, 1.8)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::new(P_BELL, "bell", "Bell", 0.0, 3.0, 0.0).with_choices(BELLS),
    ]
}

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    hardness: f32,
    position: f32,
    decay: f32,
    damping: f32,
    warble: f32,
    release: f32,
    bell: Bell,
}

#[derive(Clone, Debug, Default)]
struct Voice {
    /// The two halves of every pair, each heard on its own side.
    halves: [Rotors; 2],
    hz: [[f32; PARTIALS]; 2],
    nepers: [f32; PARTIALS],
    partials: usize,
    bell: Bell,
    pitch: f32,
    contact: f32,
    sounding: bool,
    held: bool,
    damped: bool,
    touching: usize,
    /// How long the clapper stayed on the bell, in seconds, and how hard it came down.
    blow: (f32, f32),
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
        for (half, rotors) in self.halves.iter_mut().enumerate() {
            for index in 0..self.partials {
                let hz = self.hz[half][index] * factor;
                if hz >= rate * 0.49 || hz <= 0.0 {
                    rotors.turn(index, 0.0, 0.0);
                    continue;
                }
                let (cosine, sine) = pole(hz, self.nepers[index], rate);
                rotors.turn(index, cosine, sine);
            }
        }
    }
}

/// The bells.
pub(super) struct Bells {
    rate: f32,
    settings: Settings,
    performance: Performance,
    voices: Vec<Voice>,
    pan_step: f32,
}

impl Bells {
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
        let table = voice.bell.partials();
        let strike = hertz(voice.pitch);
        // A bell twice the size rings an octave lower and nearly twice as long.
        let seconds = (settings.decay * (REFERENCE_HZ / strike).powf(0.8)).clamp(0.05, 60.0);
        let highest = table.last().map_or(1.0, |partial| partial.0);
        for (slot, partial) in voice.nepers.iter_mut().zip(table) {
            let mut nepers = decay_rate(seconds * partial.2)
                * (1.0 + 6.0 * settings.damping * partial.0 / highest);
            if voice.damped {
                nepers += decay_rate(settings.release);
            }
            *slot = nepers;
        }
    }
}

impl Engine for Bells {
    fn prepare(&mut self, rate: f32, voices: usize) {
        self.rate = rate;
        self.voices = (0..voices)
            .map(|_| {
                let mut voice = Voice::default();
                for half in &mut voice.halves {
                    half.prepare(PARTIALS);
                }
                voice
            })
            .collect();
        self.pan_step = 1.0 - (-1.0 / (0.010 * rate)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            for half in &mut voice.halves {
                half.clear();
            }
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
            warble: params.at(P_WARBLE),
            release: params.at(P_RELEASE),
            bell: Bell::from_value(params.at(P_BELL)),
        };
        let (cents, rate) = (self.performance.bend * 100.0, self.rate);
        for index in 0..self.voices.len() {
            if self.voices[index].sounding {
                self.set_rates(index);
                self.voices[index].retune(cents, rate);
            }
        }
    }

    fn start(&mut self, index: usize, note: &Note, quality: Quality) {
        let (rate, settings) = (self.rate, self.settings);
        let bell = settings.bell;
        let table = bell.partials();
        let pitch = note.pitch;
        let strike = hertz(pitch);
        let contact = note.touch.unwrap_or(settings.position).clamp(0.0, 1.0);
        // A harder clapper and a harder blow both shorten the contact; a bigger bell holds
        // its clapper longer.
        let seconds = bell.contact()
            * 6.0_f32.powf(0.5 - settings.hardness)
            * note.velocity.max(0.05).powf(-0.2)
            * (REFERENCE_HZ / strike).clamp(0.2, 5.0).sqrt();
        let amplitude = note.velocity;
        let limit = rate * 0.45;
        let budget = if quality == Quality::Draft {
            6
        } else {
            PARTIALS
        };
        let split = bell.split() * settings.warble;
        let width = self.performance.width;
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        voice.declick = if note.stolen {
            Declick::from(voice.last, rate)
        } else {
            Declick::default()
        };
        for half in &mut voice.halves {
            half.clear();
        }
        voice.mute.open();
        voice.bell = bell;
        voice.pitch = pitch;
        voice.contact = contact;
        voice.pan = note.pan;
        voice.pan_now = note.pan * width;
        voice.held = true;
        voice.damped = false;
        voice.sounding = true;
        voice.touching = (seconds * rate) as usize;
        voice.blow = (seconds, amplitude);
        voice.partials = table.len().min(budget);
        voice.hz = [[0.0; PARTIALS]; 2];
        for (number, partial) in table.iter().enumerate().take(voice.partials) {
            let (ratio, strength, _, lobes) = *partial;
            let centre = strike * ratio;
            if centre >= limit {
                voice.partials = number;
                break;
            }
            // Every casting is out of round in its own way, the same way every time.
            let apart = split
                * (0.6
                    + 0.8
                        * seeded(
                            0xBE11 + super::nearest_key(note.pitch) as u32,
                            number as u32,
                        ));
            voice.hz[0][number] = centre * (1.0 - apart * 0.5);
            voice.hz[1][number] = centre * (1.0 + apart * 0.5);
            // A pair's two shapes are the same pattern turned by half a lobe, so where round
            // the rim the clapper lands shares the blow between them.
            let around = TAU * contact * f32::from(lobes);
            let (sine, cosine) = around.sin_cos();
            let struck = amplitude * strength * contact_spectrum(centre, seconds);
            // Each shape is heard as strongly as it was struck: the two together are as loud
            // as the blow wherever round the rim it lands, and how deeply they beat is what
            // the place decides. Struck on one shape's node, a bell does not warble at all.
            for (half, share) in voice.halves.iter_mut().zip([cosine, sine]) {
                half.set(
                    number,
                    Rotor {
                        listen_im: share,
                        ..Rotor::default()
                    },
                );
                half.place(number, struck * share, 0.0);
            }
        }
        let partials = voice.partials;
        for half in &mut voice.halves {
            half.set_len(partials);
        }
        self.set_rates(index);
        let cents = self.performance.bend * 100.0;
        let voice = &mut self.voices[index];
        voice.retune(cents, rate);
        voice.moving = voice.halves[0].settle(0.0) + voice.halves[1].settle(0.0);
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
        voice.damped = true;
        self.set_rates(index);
        let (cents, rate) = (self.performance.bend * 100.0, self.rate);
        self.voices[index].retune(cents, rate);
    }

    fn silence(&mut self, index: usize) {
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
            let (cents, rate) = (performance.bend * 100.0, self.rate);
            for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
                voice.retune(cents, rate);
            }
        }
    }

    fn render(&mut self, mid: &mut [f32], side: &mut [f32], tick: bool) {
        let frames = mid.len().min(side.len()).min(TICK);
        let width = self.performance.width;
        if tick {
            for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
                voice.moving = voice.halves[0].settle(FLOOR) + voice.halves[1].settle(FLOOR);
                if voice.moving == 0.0 || voice.mute.is_closed() {
                    voice.sounding = false;
                    for half in &mut voice.halves {
                        half.rest();
                    }
                }
            }
        }
        let (mut near, mut far) = ([0.0_f32; TICK], [0.0_f32; TICK]);
        for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
            let (near, far) = (&mut near[..frames], &mut far[..frames]);
            near.fill(0.0);
            far.fill(0.0);
            let [first, second] = &mut voice.halves;
            first.ring(near);
            second.ring(far);
            voice.touching = voice.touching.saturating_sub(frames);
            let target = voice.pan * width;
            for (((near, far), to_mid), to_side) in near
                .iter()
                .zip(far.iter())
                .zip(mid.iter_mut())
                .zip(side.iter_mut())
            {
                let gate = voice.mute.next();
                let heard = (near + far) * gate + voice.declick.next();
                voice.pan_now += (target - voice.pan_now) * self.pan_step;
                voice.last = heard;
                *to_mid += heard;
                // One half of each pair to either side of where the bell hangs.
                *to_side += heard * voice.pan_now + (far - near) * gate * SPREAD * width;
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
        let table = voice.bell.partials();
        // The rim, seen from above: each pair is a pattern of lobes and the same pattern
        // turned by half of one.
        let count = picture.points.len() as f32;
        for (at, point) in picture.points.iter_mut().enumerate() {
            let around = TAU * at as f32 / count;
            *point = (0..voice.partials)
                .map(|number| {
                    let lobes = f32::from(table[number].3);
                    let (sine, cosine) = (lobes * around).sin_cos();
                    (voice.halves[0].at(number).1 * cosine + voice.halves[1].at(number).1 * sine)
                        / table[number].0.max(0.2).sqrt()
                })
                .sum::<f32>()
                * DISPLAY;
        }
        for (number, value) in picture.partials.iter_mut().enumerate() {
            *value = if number < voice.partials {
                let (re, im) = voice.halves[0].at(number);
                let (other_re, other_im) = voice.halves[1].at(number);
                (re.hypot(im) + other_re.hypot(other_im)) * DISPLAY
            } else {
                0.0
            };
        }
        let shown = picture.modes.len();
        picture.modes.copy_from_slice(&picture.partials[..shown]);
        super::blow_scope(voice.blow.0, voice.blow.1, &mut picture.scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cast_bell_has_a_hum_an_octave_down_and_a_minor_third_above_its_note() {
        let table = Bell::Cast.partials();
        assert_eq!(table[0].0, 0.5);
        assert_eq!(table[1].0, 1.0);
        let third = 1200.0 * table[2].0.log2();
        assert!((third - 316.0).abs() < 2.0, "tierce at {third} cents");
        assert_eq!(table[4].0, 2.0);
    }

    #[test]
    fn a_chime_plays_a_note_it_has_no_partial_at() {
        let table = Bell::Tubular.partials();
        assert!(table.iter().all(|partial| (partial.0 - 1.0).abs() > 0.15));
        // Its strongest three stand close to two, three and four times that note.
        let mut strongest: Vec<Partial> = table.to_vec();
        strongest.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut ratios: Vec<f32> = strongest[..3].iter().map(|partial| partial.0).collect();
        ratios.sort_by(f32::total_cmp);
        for (ratio, harmonic) in ratios.iter().zip([2.0, 3.0, 4.0]) {
            assert!(
                (ratio / harmonic - 1.0).abs() < 0.05,
                "{ratio} for {harmonic}"
            );
        }
    }

    #[test]
    fn every_bell_fits_its_voice_and_lists_its_partials_low_to_high() {
        for bell in [Bell::Cast, Bell::Tubular, Bell::Hand, Bell::Bowl] {
            let table = bell.partials();
            assert!(table.len() <= PARTIALS);
            assert!(
                table.windows(2).all(|pair| pair[0].0 < pair[1].0),
                "{bell:?}"
            );
            assert!(
                table
                    .iter()
                    .all(|partial| partial.1 > 0.0 && partial.2 > 0.0)
            );
        }
    }
}
