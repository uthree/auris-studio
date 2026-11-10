//! Reed woodwinds: a clarinet's cylinder of air, closed at the reed.
//!
//! A reed is a valve the player's breath holds nearly shut. The pressure in the mouth pushes it
//! toward the mouthpiece, the pressure inside the instrument pushes it back, and the air that
//! gets through the gap between them goes as the square root of the difference. So a little
//! more pressure inside lets *more* air in, up to the point where the reed slams shut — a valve
//! with negative resistance, which is what keeps the air column sounding. The reed's gap and
//! the flow through it are solved together each sample, exactly, against the wave coming back
//! up the bore.
//!
//! The reed is taken to follow the pressure at once. A real one has a resonance of its own a
//! couple of kilohertz up and lags a little, and that lag is part of why a clarinet goes flat
//! as it is blown harder; an instrument played from a keyboard has to stay on its note, so the
//! lag is left out and the pitch is the bore's alone.
//!
//! The bore is a tube closed at the reed and open at the other end, so the wave comes back
//! upside down and the note is the one whose *quarter* wavelength fits: a clarinet plays an
//! octave below a flute of its length and has no even harmonics to speak of. It does not come
//! back whole. Above about 1.5 kHz the row of open tone holes stops reflecting and lets the
//! sound out instead, and that — what the reflection leaves behind — is what is heard.

use std::f32::consts::TAU;

use auris_core::motion::{MOTION_SCOPE, MotionVoice};
use auris_core::{ParamDescriptor, ParamUnit};
use auris_dsp::DcBlocker;

use super::waveguide::{Decimator, Line, Noise, lowpass_delay, lowpass_gain, lowpass_pole};
use super::{Engine, Mute, Note, Performance, Quality, TICK, harmonics, hertz};
use crate::ParamBank;

const P_PRESSURE: u32 = 0;
const P_REED: u32 = 1;
const P_NOISE: u32 = 2;
const P_ATTACK: u32 = 3;
const P_RELEASE: u32 = 4;
const P_VIBRATO: u32 = 5;
const P_LEGATO: u32 = 6;

/// Lowest note the bore is long enough for.
const LOWEST_HZ: f32 = 20.0;
/// Scale of bore pressure in the picture a frontend draws.
const DISPLAY: f32 = 0.03;
/// Scale of what the instrument radiates.
const DRIVE: f32 = 2.2;
/// Pitch at which the radiated level is left as it is. Sound leaves a small hole in proportion
/// to its frequency, so low notes come out weaker than the pressure inside them; a player's
/// low register is not weak, and part of the difference is given back.
const REGISTER_HZ: f32 = 440.0;

/// How much of the pressure a note first sounds at is already behind the tongue as it lets go.
const TONGUED: f32 = 0.2;

pub(super) fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::percent(P_PRESSURE, "pressure", "Breath Pressure", 0.5),
        ParamDescriptor::percent(P_REED, "reed_stiffness", "Reed Stiffness", 0.5),
        ParamDescriptor::percent(P_NOISE, "breath_noise", "Breath Noise", 0.15),
        ParamDescriptor::new(P_ATTACK, "attack", "Attack", 0.001, 0.3, 0.02)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 4.0, 0.12)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::percent(P_VIBRATO, "vibrato", "Vibrato", 0.12),
        ParamDescriptor::toggle(P_LEGATO, "legato", "Legato", false),
    ]
}

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    pressure: f32,
    reed: f32,
    noise: f32,
    attack: f32,
    release: f32,
    vibrato: f32,
}

/// What a voice needs for one control tick.
#[derive(Clone, Copy, Debug, Default)]
struct Wind {
    /// Mouth pressure the player is heading for, as a fraction of what shuts the reed.
    mouth: f32,
    ramp: f32,
    /// How wide the reed's gap is at rest, against the bore's impedance.
    embouchure: f32,
    noise: f32,
    pole: f32,
    reflection: f32,
    /// Pole of the gentle low-pass on what is radiated.
    air: f32,
    /// Output gain for this note's register.
    gain: f32,
}

#[derive(Clone, Debug)]
struct Voice {
    /// The wave on its way down the bore and back.
    bore: Line,
    /// Two stages of the open end's reflection filter, as the reed meets what it sent back.
    low: [f32; 2],
    /// The same two stages where the wave meets the open end, half a trip sooner.
    out: [f32; 2],
    /// Output gain as it is now, on its way to the note's.
    gain: f32,
    /// Signed square root of the pressure across the reed at the last sample, in units of
    /// the pressure that shuts it: where the next sample's solution is looked for.
    across: f32,
    mouth: f32,
    air: f32,
    delay: f32,
    delay_step: f32,
    oversampling: usize,
    decimator: Decimator,
    blocker: DcBlocker,
    key: f32,
    pitch: f32,
    velocity: f32,
    sounding: bool,
    held: bool,
    /// Whether this note took over a voice that was still sounding, and so was not tongued.
    slurred: bool,
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
            bore: Line::default(),
            low: [0.0; 2],
            out: [0.0; 2],
            gain: 0.0,
            across: 0.0,
            mouth: 0.0,
            air: 0.0,
            delay: 0.0,
            delay_step: 0.0,
            oversampling: 1,
            decimator: Decimator::default(),
            blocker: DcBlocker::default(),
            key: 60.0,
            pitch: 60.0,
            velocity: 0.0,
            sounding: false,
            held: false,
            slurred: false,
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

/// The air through a reed, given how wide its gap is at rest and what the pressure across it
/// would be with no air moving at all.
///
/// With a pressure `x` across it, in units of the pressure that shuts it, the gap is `1 - x`
/// of its width at rest and the air through it goes as the square root of `x`:
/// `flow = embouchure * (1 - x) * sqrt(x)`, and nothing once `x` reaches one. The pressure
/// across is what there would be without flow, less the flow itself — the bore answers air
/// let in with pressure — so `x + flow(x) = unloaded`. Written in `s = sqrt(x)` that is a
/// cubic with one root, and Newton's method lands on it from the nearer of two places:
/// `guess`, the last sample's root, which is nearly always within a step or two, or the root
/// there would be with no air moving, which is what catches the reed as it snaps open.
/// Returns the flow and the new `s`.
fn reed_flow(embouchure: f32, unloaded: f32, guess: f32) -> (f32, f32) {
    if unloaded >= 1.0 {
        // Shut: nothing gets through, and the whole of the pressure is across the reed.
        return (0.0, 1.0);
    }
    let miss = |s: f32| {
        let across = s * s.abs();
        across + embouchure * (1.0 - across) * s - unloaded
    };
    let still = unloaded.abs().sqrt().copysign(unloaded);
    let guess = guess.clamp(-4.0, 1.0);
    let mut s = if miss(guess).abs() < miss(still).abs() {
        guess
    } else {
        still
    };
    for _ in 0..8 {
        let off = miss(s);
        if off.abs() < 1.0e-6 {
            break;
        }
        let slope = 2.0 * s.abs() + embouchure * (1.0 - 3.0 * s * s.abs());
        s = (s - off / slope.max(1.0e-3)).clamp(-4.0, 1.0);
    }
    (embouchure * (1.0 - s * s.abs()) * s, s)
}

impl Voice {
    fn clear(&mut self) {
        self.bore.clear();
        self.low = [0.0; 2];
        self.out = [0.0; 2];
        self.across = 0.0;
        self.mouth = 0.0;
        self.air = 0.0;
        self.delay = 0.0;
        self.drift = 0.0;
    }

    /// One internal sample. Returns what leaves the instrument.
    #[inline]
    fn step(&mut self) -> f32 {
        let wind = self.wind;
        self.delay += self.delay_step;
        let back = self.bore.read(self.delay);
        // The same wave half a trip sooner, as it reaches the open end.
        let arriving = self.bore.read(self.delay * 0.5);
        let [first, second] = &mut self.low;
        *first += (1.0 - wind.pole) * (back - *first);
        *second += (1.0 - wind.pole) * (*first - *second);
        // The open end sends the low part of the wave back upside down.
        let returning = -wind.reflection * *second;
        self.mouth += (wind.mouth - self.mouth) * wind.ramp;
        let unloaded = self.mouth - 2.0 * returning;
        // Air through a narrow gap is turbulent: the breath in the tone.
        let hiss = 1.0 + wind.noise * self.noise.next();
        let (flow, across) = reed_flow(wind.embouchure * hiss, unloaded, self.across);
        self.across = across;
        self.bore.write(returning + flow);
        // What the tone holes do not send back they let out, there and then: a note is heard
        // a quarter of its period after the reed first moves, the time sound takes to get
        // down the instrument.
        let [first, second] = &mut self.out;
        *first += (1.0 - wind.pole) * (arriving - *first);
        *second += (1.0 - wind.pole) * (*first - *second);
        let radiated = arriving - *second;
        self.air += (1.0 - wind.air) * (radiated - self.air);
        self.air
    }
}

/// The reed instruments.
pub(super) struct Reeds {
    rate: f32,
    settings: Settings,
    performance: Performance,
    voices: Vec<Voice>,
    pan_step: f32,
    expression: f32,
}

impl Reeds {
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
                (target - voice.pitch) * (1.0 - (-1.0 / (0.02 * ticks_per_second)).exp());
        }
        let seconds = voice.age as f32 / ticks_per_second;
        let settled = ((seconds - 0.25) / 0.35).clamp(0.0, 1.0);
        let depth = (settings.vibrato * 20.0 + performance.modulation * 35.0) * settled;
        voice.vibrato = (voice.vibrato + voice.vibrato_rate / ticks_per_second).fract();
        voice.drift += (voice.noise.next() * 2.0 - voice.drift) * 0.02;
        let cents = voice.drift + depth * (TAU * voice.vibrato).sin();
        let fundamental = hertz(voice.pitch + cents / 100.0).max(LOWEST_HZ);
        // The tone holes stop sending sound back near 1.5 kHz, and no lower than a note's own
        // third harmonic, or the top of the instrument would not speak at all.
        let corner = 2_400.0_f32.max(3.2 * fundamental);
        let pole = lowpass_pole(corner, internal);
        let breath = settings.pressure * (0.15 + 0.85 * voice.velocity) * (0.4 + 0.6 * expression);
        let time = if voice.held {
            settings.attack
        } else {
            (settings.release * 0.3).max(0.004)
        };
        let turn = TAU * fundamental / internal;
        // A stiff reed stands closer to the lay and lets less through.
        let embouchure = 0.48 - 0.26 * settings.reed;
        // The walls take a little of every round trip, more of it from a long narrow one.
        let reflection = 0.975 - 0.02 * (150.0 / fundamental).sqrt().min(2.0);
        // Where this note begins to sound at all: the mouth pressure at which the reed gives
        // back just what a round trip of the bore loses. A lossless pipe starts at a third of
        // the pressure that shuts the reed; a real one needs a little more, and more the
        // higher the note. Playing is measured from there, so the softest breath still speaks
        // on every note, as it does for a player. Just over the threshold the wave is small
        // and nearly a sine; it grows as the root of how far over the breath is, and half-way
        // to shutting the reed it is nearly square.
        let returned = reflection * lowpass_gain(pole, turn).powi(2);
        let needed = (1.0 - returned) / (1.0 + returned);
        let root = (2.0 * needed + (4.0 * needed * needed + 12.0 * embouchure * embouchure).sqrt())
            / (6.0 * embouchure);
        let threshold = root * root;
        let over = 0.006 + 0.32 * breath * breath;
        if start && !voice.slurred {
            // The tongue holds the reed shut while the breath gathers behind it, and lets go
            // with a little of it already there: the note grows from the first wave that
            // sends down the bore, quickly but from nearly nothing. Measured against a
            // recorded clarinet, that is how its notes begin at every dynamic; a tongue that
            // let go at the full playing pressure began each one with a bark.
            voice.mouth = TONGUED * threshold;
        }
        voice.wind = Wind {
            mouth: if voice.held { threshold + over } else { 0.0 },
            ramp: 1.0 - (-1.0 / (time.max(0.001) * internal)).exp(),
            embouchure,
            noise: settings.noise * 0.5,
            pole,
            reflection,
            air: lowpass_pole(7_000.0, internal),
            gain: (REGISTER_HZ / fundamental).powf(0.35).clamp(0.5, 2.5),
        };
        // A closed pipe sounds the note whose half period is one trip down the bore and back.
        // The open end's two filter stages take a little of that time themselves.
        let wanted = (internal / (2.0 * fundamental) - 2.0 * lowpass_delay(pole, turn))
            .clamp(3.0, voice.bore.reach());
        if start || voice.delay == 0.0 {
            voice.delay = wanted;
            voice.delay_step = 0.0;
        } else {
            voice.delay_step = (wanted - voice.delay) / (TICK * voice.oversampling) as f32;
        }
    }
}

impl Engine for Reeds {
    fn prepare(&mut self, rate: f32, voices: usize) {
        self.rate = rate;
        let internal = if rate >= 88_200.0 { rate } else { rate * 2.0 };
        let longest = (internal / (2.0 * LOWEST_HZ)).ceil() as usize;
        self.voices = (0..voices)
            .map(|_| {
                let mut voice = Voice::default();
                voice.bore.prepare(longest);
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
            reed: params.at(P_REED),
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
        voice.slurred = note.stolen;
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
        voice.vibrato_rate = 4.8 + 0.6 * voice.noise.next().abs();
        let stolen = note.stolen;
        self.conduct(index, true);
        if let Some(voice) = self.voices.get_mut(index).filter(|_| !stolen) {
            voice.gain = voice.wind.gain;
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
            // Breath follows the expression pedal over a couple of hundredths of a second.
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
            for (to_mid, to_side) in mid.iter_mut().zip(side.iter_mut()).take(frames) {
                let heard = if voice.oversampling == 2 {
                    let first = voice.step();
                    let second = voice.step();
                    voice.decimator.process(first, second)
                } else {
                    voice.step()
                };
                voice.gain += (voice.wind.gain - voice.gain) * self.pan_step;
                let heard = voice.blocker.next(heard) * voice.mute.next() * DRIVE * voice.gain;
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
        // How far the reed is from shut.
        picture.excitation = (1.0 - voice.across * voice.across.abs()).clamp(0.0, 2.0);
        // Pressure along the bore is the wave going down plus the wave coming back, which at
        // a place `x` of the way along left the mouthpiece `x` and `2 - x` half-trips ago. The
        // one coming back has been turned upside down by the open end.
        let delay = voice.delay.max(4.0);
        let count = picture.points.len();
        for (at, point) in picture.points.iter_mut().enumerate() {
            let x = at as f32 / (count - 1) as f32;
            let down = voice.bore.read(2.0 + x * delay * 0.5);
            let back = voice.bore.read(delay - x * delay * 0.5);
            *point = (down - back) * DISPLAY;
        }
        picture.modes = [0.0; auris_core::motion::MOTION_MODES];
        // The pressure in the mouthpiece is the wave leaving it and the wave coming back,
        // and both are still in the bore. Less the mouth's, that is what stands across the
        // reed, and the reed stands as far open as it leaves room for.
        let reach = voice.bore.reach() - delay - 2.0;
        let period = (2.0 * delay).min(reach).max(4.0);
        let leaving = |back: f32| voice.bore.read(back.max(2.0));
        let returning = |back: f32| -voice.wind.reflection * voice.bore.read(delay + back);
        for (index, sample) in picture.scope.iter_mut().enumerate() {
            let back = period * (1.0 - index as f32 / MOTION_SCOPE as f32);
            *sample = (1.0 - voice.mouth + leaving(back) + returning(back)).clamp(0.0, 2.0);
        }
        harmonics(
            |along| leaving(period * (1.0 - along)) * DISPLAY,
            &mut picture.partials,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reed_lets_more_air_in_as_the_bore_pushes_back_until_it_shuts() {
        // With the mouth at half the closing pressure, raising the pressure inside from below
        // opens the reed faster than it slows the air: the flow rises, then falls to nothing.
        let flow_at = |inside: f32| {
            let across = 0.5 - inside;
            let opening = 0.4 * (1.0 - across).max(0.0);
            opening * across.max(0.0).sqrt()
        };
        assert!(flow_at(0.1) > flow_at(-0.2));
        assert_eq!(flow_at(-0.6), 0.0);
        // The solved flow agrees with that law at the pressure it ends up with, from any guess.
        for unloaded in [-0.8_f32, -0.1, 0.0, 0.05, 0.4, 0.95, 1.5] {
            for embouchure in [0.1_f32, 0.3, 0.55] {
                for guess in [-1.0_f32, 0.0, 0.6, 1.0] {
                    let (flow, _) = reed_flow(embouchure, unloaded, guess);
                    let across = unloaded - flow;
                    let law = if across >= 1.0 {
                        0.0
                    } else {
                        embouchure * (1.0 - across) * across.abs().sqrt() * across.signum()
                    };
                    assert!(
                        (flow - law).abs() < 2.0e-3,
                        "embouchure {embouchure} unloaded {unloaded} guess {guess}: {flow} against {law}"
                    );
                }
            }
        }
    }
}
