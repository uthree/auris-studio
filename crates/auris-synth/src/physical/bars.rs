//! Tuned bars: marimba, vibraphone, xylophone, glockenspiel and a comb of tines.
//!
//! A bar is a beam that bends, and a beam's overtones are not harmonics: a plain one free at
//! both ends has them at 2.76, 5.40 and 8.93 times its fundamental. That is a glockenspiel. A
//! marimba or vibraphone maker carves an arch under the bar until the first overtone is two
//! octaves up and the second a major third over three; a xylophone's are brought to a twelfth
//! and beyond. A tine clamped at one end — a music box's comb, a kalimba's tongue — has its
//! first overtone more than six times up, which is why it sounds almost pure.
//!
//! Where the mallet lands decides which of those modes it wakes, exactly as a pluck does on a
//! string: each mode has a shape along the bar, and a blow at one of its nodes leaves it out.
//! How long the mallet stays decides how high the spectrum reaches. Yarn on rosewood is in
//! contact for milliseconds and hard rubber on steel for a fraction of one, and the harder the
//! blow the shorter the contact, so loud is also bright.
//!
//! Under each bar of the wooden and aluminium instruments hangs a tube tuned to its
//! fundamental. It makes the fundamental louder and, having handed its energy to the air,
//! shorter; on a vibraphone a disc turns in the mouth of each tube and opens and shuts it,
//! which is the tremolo.

use std::f32::consts::{PI, TAU};

use auris_core::motion::MotionVoice;
use auris_core::{ParamDescriptor, ParamUnit};

use super::rotors::{Rotor, Rotors, decay_rate, pole};
use super::{Declick, Engine, Mute, Note, Performance, Quality, TICK, blow_scope, hertz};
use crate::ParamBank;
use std::borrow::Cow;

const P_HARDNESS: u32 = 0;
const P_POSITION: u32 = 1;
const P_DECAY: u32 = 2;
const P_DAMPING: u32 = 3;
const P_BODY: u32 = 4;
const P_RELEASE: u32 = 5;
const P_BAR: u32 = 6;
const P_TREMOLO: u32 = 7;
const P_TREMOLO_RATE: u32 = 8;

/// Modes of one bar.
const MODES: usize = 8;
const FLOOR: f32 = 1.0e-6;
const DISPLAY: f32 = 0.04;
/// Middle C, where the decay control is the fundamental's decay time on a marimba.
const REFERENCE_HZ: f32 = 261.63;
/// How much of each tube is heard by default.
const TUBES: f32 = 0.7;

/// What the bars are made of and how they are tuned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Bar {
    /// Rosewood, arched until the overtones are two octaves and three octaves and a third up.
    #[default]
    Marimba,
    /// Aluminium on the same tuning, which rings many times longer.
    Vibraphone,
    /// Rosewood tuned to a twelfth: shorter and brighter.
    Xylophone,
    /// Plain steel bars with a beam's own overtones.
    Glockenspiel,
    /// A tongue of steel clamped at one end: a music box, a kalimba.
    Tine,
}

const BARS: &[Cow<'static, str>] = &[
    Cow::Borrowed("Marimba"),
    Cow::Borrowed("Vibraphone"),
    Cow::Borrowed("Xylophone"),
    Cow::Borrowed("Glockenspiel"),
    Cow::Borrowed("Tine"),
];

impl Bar {
    fn from_value(value: f32) -> Self {
        match value.round() as i32 {
            i32::MIN..=0 => Self::Marimba,
            1 => Self::Vibraphone,
            2 => Self::Xylophone,
            3 => Self::Glockenspiel,
            _ => Self::Tine,
        }
    }

    /// Frequency of mode `index` (from zero) over the fundamental's.
    fn ratio(self, index: usize) -> f32 {
        let n = (index + 1) as f32;
        // Above the modes a maker tunes, a bar keeps a free beam's spacing: as the square of
        // odd numbers.
        let beam = |third: f32| third * ((n + 0.5) / 3.5).powi(2);
        match (self, index) {
            (_, 0) => 1.0,
            (Self::Marimba, 1) => 4.0,
            (Self::Marimba, 2) => 9.9,
            (Self::Marimba, _) => beam(9.9),
            (Self::Vibraphone, 1) => 4.0,
            (Self::Vibraphone, 2) => 10.0,
            (Self::Vibraphone, _) => beam(10.0),
            (Self::Xylophone, 1) => 3.0,
            (Self::Xylophone, 2) => 6.0,
            (Self::Xylophone, _) => beam(6.0),
            (Self::Glockenspiel, _) => ((n + 0.5) / 1.505_6).powi(2),
            (Self::Tine, 1) => 6.267,
            (Self::Tine, 2) => 17.55,
            (Self::Tine, _) => 17.55 * ((n - 0.5) / 2.5).powi(2),
        }
    }

    /// The fundamental's decay time at middle C over the decay control.
    fn sustain(self) -> f32 {
        match self {
            Self::Marimba => 1.0,
            Self::Vibraphone => 5.0,
            Self::Xylophone => 0.45,
            Self::Glockenspiel => 4.0,
            Self::Tine => 2.2,
        }
    }

    /// Exponent by which shorter bars and higher modes die faster. Measured on a marimba at
    /// 0.6; metal loses less to itself, so a little less.
    fn loss_slope(self) -> f32 {
        match self {
            Self::Marimba | Self::Xylophone => 0.6,
            _ => 0.5,
        }
    }

    /// Whether a tube hangs under each bar.
    fn has_tubes(self) -> bool {
        matches!(self, Self::Marimba | Self::Vibraphone | Self::Xylophone)
    }

    /// Whether the bar is held at one end rather than hung at two points.
    fn is_clamped(self) -> bool {
        self == Self::Tine
    }
}

/// Shape of bending mode `index` of a beam at `x` along it, from zero to one, scaled to about
/// one at its largest. Free at both ends, or clamped at zero and free at one.
fn mode_shape(index: usize, x: f32, clamped: bool) -> f32 {
    // Wavenumber times length, and the ratio of the two families of terms.
    let beta = match (clamped, index) {
        (false, 0) => 4.730_04,
        (false, 1) => 7.853_2,
        (false, 2) => 10.995_6,
        (false, _) => (index as f32 + 1.5) * PI,
        (true, 0) => 1.875_1,
        (true, 1) => 4.694_1,
        (true, 2) => 7.854_8,
        (true, _) => (index as f32 + 0.5) * PI,
    };
    let at = beta * x.clamp(0.0, 1.0);
    if index >= 3 {
        // Far from both ends a high mode is a plain sinusoid; near each end an evanescent
        // term bends it to meet that end's condition. Written this way because the exact
        // expression is the difference of two numbers near e^beta, which single precision
        // cannot hold.
        let near = (-at).exp();
        let far = (at - beta).exp() * if index.is_multiple_of(2) { 1.0 } else { -1.0 };
        let wave = if clamped {
            at.sin() - at.cos()
        } else {
            at.cos() - at.sin()
        };
        return 0.5 * (wave + near + far);
    }
    let (sinh, cosh) = (beta.sinh(), beta.cosh());
    let (sin, cos) = beta.sin_cos();
    if clamped {
        let sigma = (cosh + cos) / (sinh + sin);
        0.5 * (at.cosh() - at.cos() - sigma * (at.sinh() - at.sin()))
    } else {
        let sigma = (cosh - cos) / (sinh - sin);
        0.5 * (at.cosh() + at.cos() - sigma * (at.sinh() + at.sin()))
    }
}

/// How much of a mode at `hz` a contact lasting `seconds` leaves in it: the spectrum of a
/// half-sine pulse of unit area.
pub(super) fn contact_spectrum(hz: f32, seconds: f32) -> f32 {
    let turns = 2.0 * hz * seconds;
    let denominator = 1.0 - turns * turns;
    if denominator.abs() < 1.0e-3 {
        // The removable point at one cycle and a half per contact.
        return PI / 4.0;
    }
    ((PI * hz * seconds).cos() / denominator).abs()
}

pub(super) fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::percent(P_HARDNESS, "hardness", "Contact Hardness", 0.35),
        ParamDescriptor::new(
            P_POSITION,
            "position",
            "Excitation Position",
            0.05,
            0.5,
            0.42,
        )
        .with_unit(ParamUnit::Percent),
        ParamDescriptor::new(P_DECAY, "decay", "Resonance Decay", 0.1, 12.0, 1.5)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::percent(P_DAMPING, "damping", "Damping", 0.0),
        ParamDescriptor::percent(P_BODY, "body", "Resonator Tubes", TUBES),
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 3.0, 0.35)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::new(P_BAR, "bar", "Bar", 0.0, 4.0, 0.0).with_choices(BARS),
        ParamDescriptor::percent(P_TREMOLO, "tremolo", "Tremolo", 0.0),
        ParamDescriptor::new(
            P_TREMOLO_RATE,
            "tremolo_rate",
            "Tremolo Rate",
            1.0,
            12.0,
            5.0,
        )
        .with_unit(ParamUnit::Hertz),
    ]
}

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    hardness: f32,
    position: f32,
    decay: f32,
    damping: f32,
    tubes: f32,
    release: f32,
    bar: Bar,
    tremolo: f32,
    tremolo_rate: f32,
}

#[derive(Clone, Debug, Default)]
struct Voice {
    rotors: Rotors,
    hz: [f32; MODES],
    nepers: [f32; MODES],
    /// How strongly each mode was struck, for the picture.
    struck: [f32; MODES],
    modes: usize,
    bar: Bar,
    pitch: f32,
    contact: f32,
    sounding: bool,
    held: bool,
    deferred: bool,
    damped: bool,
    /// Frames the mallet is still on the bar, for the picture.
    touching: usize,
    /// How long the mallet stayed on the bar, in seconds, and how hard it came down.
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
        let factor = (cents / 1200.0).exp2();
        for index in 0..self.modes {
            let hz = self.hz[index] * factor;
            if hz >= rate * 0.49 {
                self.rotors.turn(index, 0.0, 0.0);
                continue;
            }
            let (cosine, sine) = pole(hz, self.nepers[index], rate);
            self.rotors.turn(index, cosine, sine);
        }
        self.tuned = cents;
    }
}

/// The mallet instruments.
pub(super) struct Bars {
    rate: f32,
    settings: Settings,
    performance: Performance,
    voices: Vec<Voice>,
    pan_step: f32,
    /// Where the vibraphone's discs are in their turn.
    motor: f32,
}

impl Bars {
    pub(super) fn new() -> Self {
        Self {
            rate: 48_000.0,
            settings: Settings::default(),
            performance: Performance::default(),
            voices: Vec::new(),
            pan_step: 1.0,
            motor: 0.0,
        }
    }

    /// How loud the tube makes the fundamental, with the discs as they stand.
    fn tube_gain(&self, bar: Bar) -> f32 {
        if !bar.has_tubes() {
            return 1.0;
        }
        // A disc across the mouth shuts the tube off; turning, it opens and shuts.
        let open = 1.0 - self.settings.tremolo * (0.5 - 0.5 * (TAU * self.motor).cos());
        1.0 + 2.5 * self.settings.tubes * open
    }

    fn set_rates(&mut self, index: usize) {
        let settings = self.settings;
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        let bar = voice.bar;
        let fundamental = hertz(voice.pitch);
        let slope = bar.loss_slope();
        let base = decay_rate(settings.decay * bar.sustain())
            * (fundamental / REFERENCE_HZ).max(0.01).powf(slope)
            * (1.0 + 6.0 * settings.damping);
        for mode in 0..voice.modes {
            let ratio = bar.ratio(mode);
            let mut nepers = base * ratio.powf(slope);
            if mode == 0 && bar.has_tubes() {
                // What the tube radiates, the bar has lost. The decay control is the time
                // with the tubes as they are by default, which is how a marimba is measured.
                nepers *= (1.0 + 0.6 * settings.tubes) / (1.0 + 0.6 * TUBES);
            }
            if voice.damped {
                nepers += decay_rate(settings.release);
            }
            voice.nepers[mode] = nepers;
        }
    }

    fn damp(&mut self, index: usize) {
        if let Some(voice) = self.voices.get_mut(index)
            && !voice.damped
        {
            voice.damped = true;
            self.set_rates(index);
            let rate = self.rate;
            let voice = &mut self.voices[index];
            let tuned = voice.tuned;
            voice.retune(tuned, rate);
        }
    }
}

impl Engine for Bars {
    fn prepare(&mut self, rate: f32, voices: usize) {
        self.rate = rate;
        self.voices = (0..voices)
            .map(|_| {
                let mut voice = Voice::default();
                voice.rotors.prepare(MODES);
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
            voice.deferred = false;
            voice.declick = Declick::default();
            voice.mute.open();
            voice.last = 0.0;
        }
        self.motor = 0.0;
    }

    fn configure(&mut self, params: &ParamBank) {
        self.settings = Settings {
            hardness: params.at(P_HARDNESS),
            position: params.at(P_POSITION),
            decay: params.at(P_DECAY),
            damping: params.at(P_DAMPING),
            tubes: params.at(P_BODY),
            release: params.at(P_RELEASE),
            bar: Bar::from_value(params.at(P_BAR)),
            tremolo: params.at(P_TREMOLO),
            tremolo_rate: params.at(P_TREMOLO_RATE),
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
        let bar = settings.bar;
        let pitch = note.pitch;
        let fundamental = hertz(pitch);
        // A tine is plucked at its tip; a bar is struck between its middle and an end.
        let contact = match (note.touch, bar.is_clamped()) {
            (Some(at), true) => at.clamp(0.3, 1.0),
            (Some(at), false) => at.clamp(0.02, 0.98),
            (None, true) => 0.5 + settings.position,
            (None, false) => settings.position,
        };
        // Hertz: a faster mallet is on the bar for less time, as the fifth root of its speed.
        // Softer mallets and longer bars keep it there longer.
        // A small bar throws the mallet back sooner, and no mallet a player would choose stays
        // on a bar for longer than it takes the bar to swing back.
        let seconds = (0.002_5
            * 0.06_f32.powf(settings.hardness)
            * note.velocity.max(0.05).powf(-0.2)
            * (REFERENCE_HZ / fundamental).clamp(0.1, 8.0).powf(0.6))
        .min(0.45 / fundamental);
        let amplitude = note.velocity;
        let limit = rate * 0.45;
        let budget = if quality == Quality::Draft { 4 } else { MODES };
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
        voice.bar = bar;
        voice.pitch = pitch;
        voice.contact = contact;
        voice.pan = note.pan;
        voice.pan_now = note.pan * pan_width;
        voice.held = true;
        voice.deferred = false;
        voice.damped = false;
        voice.sounding = true;
        voice.touching = (seconds * rate) as usize;
        voice.blow = (seconds, amplitude);
        voice.modes = 0;
        for mode in 0..budget {
            let hz = fundamental * bar.ratio(mode);
            if hz >= limit {
                break;
            }
            voice.hz[mode] = hz;
            voice.modes = mode + 1;
            let struck = amplitude
                * mode_shape(mode, contact, bar.is_clamped())
                * contact_spectrum(hz, seconds);
            voice.struck[mode] = struck;
            // Struck into the velocity half and heard on the other, a mode starts from rest.
            voice.rotors.place(mode, struck, 0.0);
        }
        voice.rotors.set_len(voice.modes);
        self.set_rates(index);
        let cents = self.performance.bend * 100.0;
        let gain = self.tube_gain(bar);
        let voice = &mut self.voices[index];
        voice.retune(cents, rate);
        for mode in 0..voice.modes {
            let (cosine, sine) = voice.rotors.pole(mode);
            voice.rotors.set(
                mode,
                Rotor {
                    cosine,
                    sine,
                    drive: 0.0,
                    listen_re: 0.0,
                    listen_im: if mode == 0 { gain } else { 1.0 },
                },
            );
        }
        voice.moving = voice.rotors.settle(0.0);
        voice.loudest = voice.moving.max(1.0e-9);
    }

    fn stop(&mut self, index: usize) {
        let pedal = self.performance.pedal;
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
            self.damp(index);
        }
    }

    fn silence(&mut self, index: usize) {
        let rate = self.rate;
        if let Some(voice) = self.voices.get_mut(index) {
            voice.held = false;
            voice.deferred = false;
            voice.mute.close(rate);
        }
    }

    fn perform(&mut self, performance: &Performance) {
        let bent = performance.bend != self.performance.bend;
        let lifted = self.performance.pedal && !performance.pedal;
        self.performance = *performance;
        if bent {
            let (cents, rate) = (performance.bend * 100.0, self.rate);
            for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
                voice.retune(cents, rate);
            }
        }
        if lifted {
            for index in 0..self.voices.len() {
                if self.voices[index].sounding && self.voices[index].deferred {
                    self.voices[index].deferred = false;
                    self.damp(index);
                }
            }
        }
    }

    fn render(&mut self, mid: &mut [f32], side: &mut [f32], tick: bool) {
        let frames = mid.len().min(side.len()).min(TICK);
        let width = self.performance.width;
        if tick {
            self.motor =
                (self.motor + self.settings.tremolo_rate * TICK as f32 / self.rate).fract();
            let turning = self.settings.tremolo > 0.0;
            for index in 0..self.voices.len() {
                if !self.voices[index].sounding {
                    continue;
                }
                let gain = self.tube_gain(self.voices[index].bar);
                let voice = &mut self.voices[index];
                voice.moving = voice.rotors.settle(FLOOR);
                if voice.moving == 0.0 || voice.mute.is_closed() {
                    voice.sounding = false;
                    voice.deferred = false;
                    voice.rotors.rest();
                    continue;
                }
                if turning && voice.bar.has_tubes() {
                    let (cosine, sine) = voice.rotors.pole(0);
                    voice.rotors.set(
                        0,
                        Rotor {
                            cosine,
                            sine,
                            drive: 0.0,
                            listen_re: 0.0,
                            listen_im: gain,
                        },
                    );
                }
            }
        }
        let mut bar = [0.0_f32; TICK];
        for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
            let bar = &mut bar[..frames];
            bar.fill(0.0);
            voice.rotors.ring(bar);
            voice.touching = voice.touching.saturating_sub(frames);
            let target = voice.pan * width;
            for ((sample, to_mid), to_side) in bar.iter().zip(mid.iter_mut()).zip(side.iter_mut()) {
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
        picture.excitation = if voice.touching > 0 { 1.0 } else { 0.0 };
        let clamped = voice.bar.is_clamped();
        let last = (picture.points.len() - 1) as f32;
        for (at, point) in picture.points.iter_mut().enumerate() {
            let x = at as f32 / last;
            *point = (0..voice.modes)
                .map(|mode| {
                    // Displacement is the turning phasor's other half over its frequency.
                    voice.rotors.at(mode).1 / voice.bar.ratio(mode).sqrt()
                        * mode_shape(mode, x, clamped)
                })
                .sum::<f32>()
                * DISPLAY;
        }
        for (mode, value) in picture.partials.iter_mut().enumerate() {
            let (re, im) = voice.rotors.at(mode);
            *value = if mode < voice.modes {
                re.hypot(im) * DISPLAY
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
    fn a_free_bar_is_still_at_its_two_supports_and_a_tine_at_its_clamp() {
        // The first mode's nodes are where a bar is hung.
        assert!(mode_shape(0, 0.224, false).abs() < 0.01);
        assert!(mode_shape(0, 0.776, false).abs() < 0.01);
        assert!(mode_shape(0, 0.5, false).abs() > 0.5);
        // The second mode is antisymmetric: a blow in the middle leaves it out.
        assert!(mode_shape(1, 0.5, false).abs() < 1.0e-3);
        assert!(mode_shape(0, 0.0, true).abs() < 1.0e-6);
        assert!((mode_shape(0, 1.0, true).abs() - 1.0).abs() < 0.01);
        assert!((mode_shape(0, 1.0, false).abs() - 1.0).abs() < 0.02);
    }

    #[test]
    fn each_bar_has_the_overtones_its_maker_gives_it() {
        assert_eq!(Bar::Marimba.ratio(1), 4.0);
        assert_eq!(Bar::Xylophone.ratio(1), 3.0);
        assert!((Bar::Glockenspiel.ratio(0) - 1.0).abs() < 1.0e-3);
        assert!((Bar::Glockenspiel.ratio(1) - 2.756).abs() < 0.01);
        assert!((Bar::Glockenspiel.ratio(2) - 5.404).abs() < 0.01);
        assert!((Bar::Tine.ratio(3) - 34.39).abs() < 0.2);
        for bar in [
            Bar::Marimba,
            Bar::Vibraphone,
            Bar::Xylophone,
            Bar::Glockenspiel,
            Bar::Tine,
        ] {
            let ratios: Vec<f32> = (0..MODES).map(|mode| bar.ratio(mode)).collect();
            assert!(
                ratios.windows(2).all(|pair| pair[0] < pair[1]),
                "{bar:?}: {ratios:?}"
            );
        }
    }

    #[test]
    fn a_longer_contact_leaves_out_the_high_modes() {
        assert!((contact_spectrum(10.0, 0.001) - 1.0).abs() < 1.0e-3);
        assert!(contact_spectrum(1_000.0, 0.000_3) > 0.8);
        assert!(contact_spectrum(4_000.0, 0.003) < 0.02);
        assert!(contact_spectrum(750.0, 0.001).is_finite());
    }
}
