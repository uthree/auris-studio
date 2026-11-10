//! Whistles and flutes: a ribbon of air blown across the open end of a pipe.
//!
//! Nothing solid moves in a flute. The player's breath leaves a slit as a thin jet and crosses
//! the window to a sharp edge; the air in the pipe, sloshing in and out of that window, pushes
//! the jet to one side of the edge and then the other, and the jet, landing inside or outside,
//! feeds the sloshing that steers it. What makes this a loop that sounds a *note* is the time
//! the jet takes to cross: a push at the slit only arrives at the edge a moment later, and the
//! pipe sings where that moment is half its period.
//!
//! So the model is two delays and one curve. The pipe is open at both ends, a wave returning
//! the right way up after a whole period; the jet carries the returning wave across the window
//! for half of one; and what the jet delivers saturates — it cannot put more than all of
//! itself into the pipe — which is what sets the level and adds the overtones. An edge that
//! sits to one side of the jet's centre makes the two half-cycles unequal, and that is where
//! a flute's even harmonics come from. The turbulence of the jet is the breath in the tone.

use std::f32::consts::TAU;

use auris_core::motion::{MOTION_SCOPE, MotionVoice};
use auris_core::{ParamDescriptor, ParamUnit};
use auris_dsp::DcBlocker;

use super::waveguide::{Decimator, Line, Noise, lowpass_delay, lowpass_gain, lowpass_pole};
use super::{Engine, Mute, Note, Performance, Quality, TICK, harmonics, hertz};
use crate::ParamBank;

const P_PRESSURE: u32 = 0;
const P_JET: u32 = 1;
const P_NOISE: u32 = 2;
const P_ATTACK: u32 = 3;
const P_RELEASE: u32 = 4;
const P_VIBRATO: u32 = 5;
const P_LEGATO: u32 = 6;

const LOWEST_HZ: f32 = 20.0;
const DISPLAY: f32 = 0.03;
const DRIVE: f32 = 2.0;
/// How much of the returning wave the open mouth sends straight back down the pipe.
const MOUTH: f32 = 0.55;
/// Where the jet stops following what steers it, as a multiple of the note.
const JET_CORNER: f32 = 2.0;
/// How much of the breath a tongued note's first puff puts into the pipe at once.
const PUFF: f32 = 0.2;
/// How much of the reflection filter's delay the pipe is shortened by to stay on pitch.
const FILTER_SHARE: f32 = 0.94;

pub(super) fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::percent(P_PRESSURE, "pressure", "Breath Pressure", 0.5),
        ParamDescriptor::percent(P_JET, "jet_shape", "Jet Shape", 0.25),
        ParamDescriptor::percent(P_NOISE, "breath_noise", "Breath Noise", 0.12),
        ParamDescriptor::new(P_ATTACK, "attack", "Attack", 0.001, 0.3, 0.02)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 4.0, 0.1)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::percent(P_VIBRATO, "vibrato", "Vibrato", 0.2),
        ParamDescriptor::toggle(P_LEGATO, "legato", "Legato", false),
    ]
}

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    pressure: f32,
    jet: f32,
    noise: f32,
    attack: f32,
    release: f32,
    vibrato: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct Wind {
    /// How much air the jet carries, which is the most it can put into the pipe.
    breath: f32,
    ramp: f32,
    /// Where the edge sits in the jet, off its centre.
    offset: f32,
    /// How strongly the returning wave deflects the jet.
    gain: f32,
    noise: f32,
    /// How much of the way a stage of the jet's smoothing moves toward its input each sample.
    smooth: f32,
    /// The same for how the jet follows what steers it.
    sway: f32,
    /// What a tongued note's first puff keeps of itself each sample.
    fade: f32,
    pole: f32,
    reflection: f32,
    /// Output gain for this note's register: the air at the window is heard in proportion to
    /// how fast it changes, which favours high notes more than a listener does.
    loudness: f32,
}

#[derive(Clone, Debug)]
struct Voice {
    /// The wave on its way along the pipe and back.
    pipe: Line,
    /// The jet on its way across the window.
    jet: Line,
    low: f32,
    breath: f32,
    /// The last sample put into the pipe, for the radiated difference.
    last: f32,
    /// Two stages of the jet's own smoothing of its turbulence.
    hiss: [f32; 2],
    /// The jet as the edge meets it: what crossed the window, less its fastest wobbles.
    swing: f32,
    /// The first puff of a tongued note, not yet in the pipe.
    kick: f32,
    /// Output gain as it is now, on its way to the note's.
    loudness: f32,
    pipe_delay: f32,
    pipe_step: f32,
    jet_delay: f32,
    jet_step: f32,
    oversampling: usize,
    decimator: Decimator,
    blocker: DcBlocker,
    key: f32,
    pitch: f32,
    velocity: f32,
    sounding: bool,
    held: bool,
    pan: f32,
    pan_now: f32,
    mute: Mute,
    wind: Wind,
    vibrato: f32,
    vibrato_rate: f32,
    drift: f32,
    noise: Noise,
    age: usize,
    level: f32,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            pipe: Line::default(),
            jet: Line::default(),
            low: 0.0,
            breath: 0.0,
            last: 0.0,
            hiss: [0.0; 2],
            swing: 0.0,
            kick: 0.0,
            loudness: 0.0,
            pipe_delay: 0.0,
            pipe_step: 0.0,
            jet_delay: 0.0,
            jet_step: 0.0,
            oversampling: 1,
            decimator: Decimator::default(),
            blocker: DcBlocker::default(),
            key: 72.0,
            pitch: 72.0,
            velocity: 0.0,
            sounding: false,
            held: false,
            pan: 0.0,
            pan_now: 0.0,
            mute: Mute::default(),
            wind: Wind::default(),
            vibrato: 0.0,
            vibrato_rate: 5.0,
            drift: 0.0,
            noise: Noise::default(),
            age: 0,
            level: 0.0,
        }
    }
}

impl Voice {
    fn clear(&mut self) {
        self.pipe.clear();
        self.jet.clear();
        self.low = 0.0;
        self.breath = 0.0;
        self.last = 0.0;
        self.hiss = [0.0; 2];
        self.swing = 0.0;
        self.kick = 0.0;
        self.pipe_delay = 0.0;
        self.jet_delay = 0.0;
        self.drift = 0.0;
    }

    /// One internal sample. Returns what leaves the instrument.
    #[inline]
    fn step(&mut self) -> f32 {
        let wind = self.wind;
        self.pipe_delay += self.pipe_step;
        self.jet_delay += self.jet_step;
        let back = self.pipe.read(self.pipe_delay);
        self.low += (1.0 - wind.pole) * (back - self.low);
        // Two open ends, each turning the wave over: it comes back the right way up.
        let returning = wind.reflection * self.low;
        self.breath += (wind.breath - self.breath) * wind.ramp;
        // The jet as it reaches the edge: what left the slit half a period ago.
        let arriving = self.jet.read(self.jet_delay);
        if self.breath < 1.0e-6 {
            // No jet, no turbulence, nothing fed in: the pipe only rings down.
            self.jet.write(0.0);
            self.pipe.write(MOUTH * returning);
            let radiated = MOUTH * returning - self.last;
            self.last = MOUTH * returning;
            return radiated;
        }
        // The sloshing air pushes the jet where it leaves the slit; it gets to the edge later.
        // A jet carries its slow wobbles across the window and smooths its fast ones away, so
        // the breath in the tone is a band of air around the top of the instrument's range
        // and not a hiss.
        let [first, second] = &mut self.hiss;
        *first += wind.smooth * (self.noise.next() - *first);
        *second += wind.smooth * (*first - *second);
        let turbulence = wind.noise * self.breath * *second;
        self.jet.write(returning + turbulence);
        // A jet is steered by slow pushes and shrugs off quick ones. Without this the loop
        // would sing as well at three times its note as at once, and a pipe open at both
        // ends would sound like one closed at one.
        self.swing += wind.sway * (arriving - self.swing);
        let displaced = self.swing / self.breath;
        // A jet wholly inside or wholly outside can do no more: the share that goes in
        // saturates either way, measured from where the edge splits it at rest.
        let fed = self.breath * (wind.offset.tanh() - (wind.gain * displaced + wind.offset).tanh());
        // The tongue's puff: air let into the pipe all at once, dying away over a few
        // periods. A pipe handed a step rings at its own note, and that is the seed.
        self.kick *= wind.fade;
        let into = fed + MOUTH * returning + self.kick;
        self.pipe.write(into);
        // What is heard is the air moving in and out of the window: the change in what is there.
        let radiated = into - self.last;
        self.last = into;
        radiated
    }
}

/// The whistles and flutes.
pub(super) struct Flutes {
    rate: f32,
    settings: Settings,
    performance: Performance,
    voices: Vec<Voice>,
    pan_step: f32,
    expression: f32,
}

impl Flutes {
    pub(super) fn new() -> Self {
        Self {
            rate: 48_000.0,
            settings: Settings::default(),
            performance: Performance::default(),
            voices: Vec::new(),
            pan_step: 1.0,
            expression: 1.0,
        }
    }

    fn oversampling(&self, quality: Quality) -> usize {
        if quality == Quality::Draft || self.rate >= 88_200.0 {
            1
        } else {
            2
        }
    }

    fn conduct(&mut self, index: usize, start: bool) {
        let (settings, rate, performance) = (self.settings, self.rate, self.performance);
        let expression = self.expression;
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        let internal = rate * voice.oversampling as f32;
        let ticks_per_second = rate / TICK as f32;
        let target = voice.key + performance.bend;
        if start {
            voice.pitch = target;
        } else {
            voice.pitch +=
                (target - voice.pitch) * (1.0 - (-1.0 / (0.015 * ticks_per_second)).exp());
        }
        let seconds = voice.age as f32 / ticks_per_second;
        let settled = ((seconds - 0.2) / 0.3).clamp(0.0, 1.0);
        let depth = (settings.vibrato * 18.0 + performance.modulation * 35.0) * settled;
        voice.vibrato = (voice.vibrato + voice.vibrato_rate / ticks_per_second).fract();
        voice.drift += (voice.noise.next() * 2.5 - voice.drift) * 0.02;
        let cents = voice.drift + depth * (TAU * voice.vibrato).sin();
        let fundamental = hertz(voice.pitch + cents / 100.0).clamp(LOWEST_HZ, internal * 0.2);
        let corner = (4_500.0 * (fundamental / 600.0).powf(0.3)).max(2.5 * fundamental);
        let pole = lowpass_pole(corner, internal);
        let breath = settings.pressure * (0.1 + 0.9 * voice.velocity) * (0.25 + 0.75 * expression);
        let time = if voice.held {
            settings.attack
        } else {
            (settings.release * 0.3).max(0.004)
        };
        let turn = TAU * fundamental / internal;
        let offset = 1.2 * settings.jet;
        let reflection = 0.93;
        // How strongly the jet must be deflected for the pipe to sound at all: enough to make
        // up what a round trip loses, through whatever part of the jet's profile the edge
        // meets. The loop is kept only a little over that, which is what leaves a flute's
        // tone so nearly a sine — the jet is barely driven into saturation — and a faster jet
        // is deflected further by the same wave, so blowing harder is brighter.
        let profile = (1.0 - offset.tanh().powi(2)).max(0.2);
        let jet_pole = lowpass_pole(JET_CORNER * fundamental, internal);
        let threshold = (1.0 / (reflection * lowpass_gain(pole, turn)) - MOUTH).max(0.05)
            / (profile * lowpass_gain(jet_pole, turn));
        // A tongued start: the first moments of a note are blown harder, which is the chiff.
        let chiff = 1.0 + 1.2 * (-seconds / 0.03).exp();
        voice.wind = Wind {
            breath: if voice.held { breath } else { 0.0 },
            ramp: 1.0 - (-1.0 / (time.max(0.001) * internal)).exp(),
            offset,
            gain: threshold * (1.05 + 0.25 * breath * breath) * chiff,
            // No jet is perfectly smooth, and it is the roughness that starts a pipe sounding.
            // Smoothing takes most of white noise away, and the gain here puts back what is
            // left of it at the level the control names.
            noise: 0.006 + settings.noise * 0.18,
            smooth: 1.0 - lowpass_pole(3_500.0, internal),
            sway: 1.0 - jet_pole,
            fade: (-fundamental / (4.0 * internal)).exp(),
            pole,
            reflection,
            loudness: (880.0 / fundamental).powf(0.5).clamp(0.4, 2.5),
        };
        // The pipe sounds the note whose whole period is one trip along it and back, less
        // what the end's filter keeps; the jet crosses the window in half of one.
        let period = internal / fundamental;
        let pipe =
            (period - lowpass_delay(pole, turn) * FILTER_SHARE).clamp(3.0, voice.pipe.reach());
        // The jet's sluggishness is part of the half period it takes to answer.
        let jet = (period * 0.5 - lowpass_delay(jet_pole, turn)).clamp(2.0, voice.jet.reach());
        if start || voice.pipe_delay == 0.0 {
            voice.pipe_delay = pipe;
            voice.jet_delay = jet;
            voice.pipe_step = 0.0;
            voice.jet_step = 0.0;
        } else {
            let steps = (TICK * voice.oversampling) as f32;
            voice.pipe_step = (pipe - voice.pipe_delay) / steps;
            voice.jet_step = (jet - voice.jet_delay) / steps;
        }
    }
}

impl Engine for Flutes {
    fn prepare(&mut self, rate: f32, voices: usize) {
        self.rate = rate;
        let internal = if rate >= 88_200.0 { rate } else { rate * 2.0 };
        let longest = (internal / LOWEST_HZ).ceil() as usize;
        self.voices = (0..voices)
            .map(|_| {
                let mut voice = Voice::default();
                voice.pipe.prepare(longest);
                voice.jet.prepare(longest / 2);
                voice
            })
            .collect();
        self.pan_step = 1.0 - (-1.0 / (0.010 * rate)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.clear();
            voice.sounding = false;
            voice.held = false;
            voice.mute.open();
            voice.level = 0.0;
            voice.noise = Noise::default();
        }
        self.expression = self.performance.expression;
    }

    fn configure(&mut self, params: &ParamBank) {
        self.settings = Settings {
            pressure: params.at(P_PRESSURE),
            jet: params.at(P_JET),
            noise: params.at(P_NOISE),
            attack: params.at(P_ATTACK),
            release: params.at(P_RELEASE),
            vibrato: params.at(P_VIBRATO),
        };
    }

    fn start(&mut self, index: usize, note: &Note, quality: Quality) {
        let rate = self.rate;
        let oversampling = self.oversampling(quality);
        let width = self.performance.width;
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        if !note.stolen {
            voice.clear();
        }
        voice.key = f32::from(note.pitch);
        voice.velocity = note.velocity;
        voice.oversampling = oversampling;
        voice.sounding = true;
        voice.held = true;
        voice.pan = note.pan;
        voice.pan_now = note.pan * width;
        voice.mute.open();
        voice.age = 0;
        voice.level = 1.0;
        voice.decimator.reset();
        voice.blocker = DcBlocker::new(12.0, rate);
        voice.noise.seed(note.seed);
        voice.vibrato = 0.0;
        voice.vibrato_rate = 4.6 + 0.8 * voice.noise.next().abs();
        let stolen = note.stolen;
        self.conduct(index, true);
        if let Some(voice) = self.voices.get_mut(index).filter(|_| !stolen) {
            voice.loudness = voice.wind.loudness;
            // The tongue lets the breath go all at once, and the first of it into the pipe is
            // what the note grows from: the chiff.
            voice.kick = PUFF * voice.wind.breath;
        }
    }

    fn glide(&mut self, index: usize, note: &Note) -> bool {
        let Some(voice) = self.voices.get_mut(index) else {
            return false;
        };
        if !voice.sounding {
            return false;
        }
        voice.key = f32::from(note.pitch);
        voice.velocity = note.velocity;
        voice.held = true;
        true
    }

    fn stop(&mut self, index: usize) {
        if let Some(voice) = self.voices.get_mut(index) {
            voice.held = false;
        }
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
        if tick {
            let follow = 1.0 - (-1.0 / (0.02 * self.rate / TICK as f32)).exp();
            self.expression += (self.performance.expression - self.expression) * follow;
            for index in 0..self.voices.len() {
                if !self.voices[index].sounding {
                    continue;
                }
                let voice = &mut self.voices[index];
                voice.age += 1;
                if voice.mute.is_closed() || (!voice.held && voice.level < 2.0e-5) {
                    voice.sounding = false;
                    continue;
                }
                voice.level *= 0.9;
                self.conduct(index, false);
            }
        }
        for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
            let target = voice.pan * width;
            // The change from one internal sample to the next is twice as large at twice the
            // rate's half: the same air, measured over half the time.
            let scale = DRIVE * voice.oversampling as f32;
            for (to_mid, to_side) in mid.iter_mut().zip(side.iter_mut()).take(frames) {
                let heard = if voice.oversampling == 2 {
                    let first = voice.step();
                    let second = voice.step();
                    voice.decimator.process(first, second)
                } else {
                    voice.step()
                };
                voice.loudness += (voice.wind.loudness - voice.loudness) * self.pan_step;
                let heard = voice.blocker.next(heard) * voice.mute.next() * scale * voice.loudness;
                voice.pan_now += (target - voice.pan_now) * self.pan_step;
                voice.level = voice.level.max(heard.abs());
                *to_mid += heard;
                *to_side += heard * voice.pan_now;
            }
        }
    }

    fn level(&self, index: usize) -> f32 {
        self.voices
            .get(index)
            .filter(|voice| voice.sounding)
            .map_or(0.0, |voice| voice.level.clamp(1.0e-6, 1.0))
    }

    fn is_held(&self, index: usize) -> bool {
        self.voices.get(index).is_some_and(|voice| voice.held)
    }

    fn pressure(&self) -> f32 {
        self.settings.pressure
    }

    fn observe(&self, index: usize, picture: &mut MotionVoice) {
        let Some(voice) = self.voices.get(index) else {
            return;
        };
        picture.level = self.level(index);
        picture.contact = 0.0;
        picture.excitation = voice.breath;
        // Pressure along an open pipe: the wave going out less the wave coming back, which
        // makes both ends the nodes they are.
        let delay = voice.pipe_delay.max(4.0);
        let count = picture.points.len();
        for (at, point) in picture.points.iter_mut().enumerate() {
            let x = at as f32 / (count - 1) as f32;
            let out = voice.pipe.read(2.0 + x * delay * 0.5);
            let back = voice.pipe.read(delay - x * delay * 0.5);
            *point = (out - back) * DISPLAY;
        }
        picture.modes = [0.0; auris_core::motion::MOTION_MODES];
        // What steered the jet through the last period is still crossing the window behind
        // what has arrived: where each part of it met the edge is where it will.
        let reach = voice.jet.reach() - voice.jet_delay - 2.0;
        let period = delay.min(reach).max(4.0);
        let breath = voice.breath.max(1.0e-4);
        for (index, sample) in picture.scope.iter_mut().enumerate() {
            let back = period * (1.0 - index as f32 / MOTION_SCOPE as f32);
            let steered = voice.jet.read(voice.jet_delay + back) / breath;
            *sample = -(voice.wind.gain * steered + voice.wind.offset).tanh();
        }
        harmonics(
            |along| voice.pipe.read((delay * (1.0 - along)).max(2.0)) * DISPLAY,
            &mut picture.partials,
        );
    }
}
