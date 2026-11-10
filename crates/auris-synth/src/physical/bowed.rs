//! Bowed strings: a section of players, each a string under a bow, on the body of its register.
//!
//! A bow does not push a string; it catches it and lets it go. Rosin grips while the string
//! moves with the hair and gives way when the string is pulled back hard enough, and it grips
//! far better standing still than sliding — which is the whole mechanism. Once a period the
//! kink that travels round the string arrives at the bow, breaks the grip, and the string
//! flies back until the kink has been to the bridge and returned to catch it again. That is
//! Helmholtz's motion, and the bridge feels it as a sawtooth.
//!
//! The friction is solved exactly, sample by sample, against the two waves arriving at the bow
//! from either side: the string is stuck if the force that would hold it there is within what
//! rosin can give, and otherwise slides at the one speed where the falling friction curve and
//! the string's own impedance agree. Where both are possible the string keeps doing what it
//! was doing, which is the hysteresis that flattens a hard-pressed note.
//!
//! A bowed string does not have to move as Helmholtz said. Pressed too lightly it slips twice
//! or three times a period and whistles an octave up; too hard and it never lets go on time.
//! Between the two is the range a player works in, and three things here keep a note inside
//! it, each of them something a player does. The bow's force is set as a share of the most
//! the string takes at this bow speed and place, so the pressure control moves within the
//! range and not off its end. A note is caught firmly and eased to the pressure it is played
//! at, because a string that is already moving properly goes on doing so under a bow too
//! light to have started it. And each player listens: one whose string is slipping more than
//! once a period leans into it until it stops, and one whose note the bow has pulled flat —
//! which a firm bow does, more the higher the note — moves the finger until it is in tune.
//!
//! One string is a soloist. A section is several, and no two players have the same vibrato,
//! bow or tuning, so each player here is a string of their own, a few cents and a few
//! milliseconds from the next, a seat apart. And a section is not one instrument: a low note
//! is played on a cello or a bass and sounds through that body, whose resonances are a
//! violin's scaled down.

use std::borrow::Cow;
use std::f32::consts::TAU;

use auris_core::motion::{MOTION_BODY, MOTION_POINTS, MOTION_SCOPE, MotionVoice};
use auris_core::{ParamDescriptor, ParamUnit};
use auris_dsp::DcBlocker;

use super::body::{self, Radiator, seeded};
use super::waveguide::{Decimator, Line, Noise, lowpass_delay, lowpass_pole};
use super::{Engine, Mute, Note, Performance, Quality, TICK, harmonics, hertz, touch_position};
use crate::ParamBank;

const P_POSITION: u32 = 0;
const P_DAMPING: u32 = 1;
const P_BODY: u32 = 2;
const P_RELEASE: u32 = 3;
const P_PRESSURE: u32 = 4;
const P_SPEED: u32 = 5;
const P_RESPONSE: u32 = 6;
const P_LEGATO: u32 = 7;
const P_VIBRATO: u32 = 8;
const P_PLAYERS: u32 = 9;
const P_INSTRUMENT: u32 = 10;

/// Most players on one note.
const PLAYERS: usize = 4;
/// Friction of rosin at rest and in a fast slide, and the sliding speed at which it has fallen
/// half-way from one to the other.
const GRIP: f32 = 0.8;
const SLIDE: f32 = 0.3;
const KNEE: f32 = 0.05;
/// The lightest and the heaviest bow the pressure control asks for, as shares of the most a
/// string takes at the bow's speed and place. Measured on this string: under the first it
/// will not hold Helmholtz's motion anywhere on the instrument, and over the second the top
/// strings go a tenth of a semitone flat.
const LIGHTEST: f32 = 0.12;
const HEAVIEST: f32 = 0.6;
/// The share a note is caught at, however lightly it is then played.
const CATCH: f32 = 0.55;
/// Periods over which that first firmness eases to the pressure asked for.
const EASE_PERIODS: f32 = 16.0;
/// Periods a bowed string takes to lose sixty decibels of what it has: its own damping and a
/// stopping finger's, about what a plucked note on the same string lasts.
const RING_PERIODS: f32 = 310.0;
/// Fewest internal samples a kink may take from the bow to the bridge and back before the
/// string is run at twice the rate again: fewer, and whether it is stuck or sliding is known
/// too coarsely for the two to alternate cleanly.
const FINEST: f32 = 9.0;
/// Periods a player listens for before deciding the string is not moving as it should, and
/// the least time that takes, in seconds.
const LISTEN_PERIODS: f32 = 5.0;
const LISTEN_SECONDS: f32 = 0.025;
/// How much of the pitch error heard in one listening the finger takes out; the most it moves
/// at once, and ever, as shares of the string's length; and the error past which what was
/// heard is taken for a string not yet settled rather than a note out of tune.
const INTONATION: f32 = 0.4;
const NUDGE: f32 = 0.003;
const REACH: f32 = 0.012;
const MISHEARD: f32 = 0.02;
/// Lowest note a string is long enough for: just under a five-string bass's low B.
const LOWEST_HZ: f32 = 24.0;
/// How much of the loss filter's delay the loop is shortened by. A bowed string turns on a
/// corner, not a sinusoid, and feels the filter's delay at many frequencies at once; this is
/// what keeps the played pitch on the note across the range.
const FILTER_SHARE: f32 = 0.9;
const DISPLAY: f32 = 0.012;
/// Scale of a string's wave at the bridge before the body.
const DRIVE: f32 = 0.35;

const INSTRUMENTS: &[Cow<'static, str>] = &[
    Cow::Borrowed("Section"),
    Cow::Borrowed("Violin"),
    Cow::Borrowed("Viola"),
    Cow::Borrowed("Cello"),
    Cow::Borrowed("Contrabass"),
];

/// Each body's resonances as a multiple of a violin's.
const SCALES: [f32; 4] = [1.0, 0.84, 0.375, 0.23];

/// The open strings of a violin, a viola, a cello and a bass, in hertz.
const OPEN_STRINGS: [[f32; 4]; 4] = [
    [196.0, 293.66, 440.0, 659.26],
    [130.81, 196.0, 293.66, 440.0],
    [65.41, 98.0, 146.83, 220.0],
    [41.2, 55.0, 73.42, 98.0],
];

/// How much further along the sounding string a bow is than along the open one.
///
/// The bow stays where it is while a finger shortens the string, so the same bow is a larger
/// share of what is left; a player moves it toward the bridge in the high positions, and
/// makes up about half of that. A note is taken on the highest string that reaches it.
fn stopped(body: usize, hz: f32) -> f32 {
    let strings = OPEN_STRINGS[body.min(3)];
    let open = strings
        .iter()
        .rev()
        .find(|open| **open <= hz * 1.03)
        .unwrap_or(&strings[0]);
    (hz / open).max(1.0).sqrt()
}

/// How far a player goes toward evening out a note the body favours or refuses, as the power
/// of how much it does: they bow such a note more gently, or more firmly, by ear.
const EVENING: f32 = 0.7;

pub(super) fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::new(P_POSITION, "position", "Bow Position", 0.03, 0.25, 0.09)
            .with_unit(ParamUnit::Percent),
        ParamDescriptor::percent(P_DAMPING, "damping", "Damping", 0.3),
        ParamDescriptor::percent(P_BODY, "body", "Body Resonance", 1.0),
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 3.0, 0.35)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::percent(P_PRESSURE, "bow_pressure", "Bow Pressure", 0.3),
        ParamDescriptor::percent(P_SPEED, "bow_speed", "Bow speed", 0.6),
        ParamDescriptor::new(
            P_RESPONSE,
            "bow_response",
            "Bow response",
            0.002,
            0.12,
            0.08,
        )
        .with_unit(ParamUnit::Seconds),
        ParamDescriptor::toggle(P_LEGATO, "legato", "Legato", false),
        ParamDescriptor::percent(P_VIBRATO, "vibrato", "Vibrato", 0.35),
        ParamDescriptor::new(P_PLAYERS, "players", "Players", 1.0, PLAYERS as f32, 3.0)
            .with_steps(PLAYERS as u32),
        ParamDescriptor::new(P_INSTRUMENT, "instrument", "Instrument", 0.0, 4.0, 0.0)
            .with_choices(INSTRUMENTS),
    ]
}

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    position: f32,
    damping: f32,
    body: f32,
    release: f32,
    pressure: f32,
    speed: f32,
    response: f32,
    vibrato: f32,
    players: usize,
    /// Zero for a section seated by register; otherwise one more than the body's index.
    instrument: usize,
}

/// The force rosin puts on a string, in the string's own units of velocity.
///
/// `wanted` is how much faster the bow is moving than the string would be without it, and
/// `force` the bow's normal force over twice the string's impedance. Sticking, the friction is
/// exactly what brings the string up to the bow, `wanted` itself, for as long as that is within
/// `GRIP * force`. Sliding, it is where the friction curve — falling from `GRIP` toward `SLIDE`
/// as the slide quickens — meets the string, which is the root of a quadratic. Between the two
/// thresholds both are possible, and the string goes on doing whichever it was.
fn friction(wanted: f32, force: f32, stuck: &mut bool, rough: f32) -> f32 {
    if force <= 0.0 {
        *stuck = false;
        return 0.0;
    }
    let (size, sign) = (wanted.abs(), wanted.signum());
    let hold = GRIP * force;
    let b = KNEE - size + force * SLIDE;
    let c = KNEE * (hold - size);
    if size <= hold {
        if *stuck {
            return wanted;
        }
        // Still sliding, if a slide is still a solution.
        let discriminant = b * b - 4.0 * c;
        if b < 0.0 && discriminant >= 0.0 {
            let slide = 0.5 * (-b + discriminant.sqrt());
            return sign * (size - slide) * (1.0 + rough);
        }
        *stuck = true;
        return wanted;
    }
    *stuck = false;
    let slide = 0.5 * (-b + (b * b - 4.0 * c).max(0.0).sqrt());
    sign * ((size - slide) * (1.0 + rough)).clamp(0.0, size)
}

/// One player: a string in two pieces either side of the bow.
#[derive(Clone, Debug, Default)]
struct Player {
    /// Waves on their way to the bridge and back, and to the nut and back.
    bridge: Line,
    nut: Line,
    /// State of the bridge's loss filter.
    low: f32,
    stuck: bool,
    /// How much harder than asked this player has found the string needs.
    press: f32,
    /// How far the finger has moved to bring the note into tune, as a share of the string.
    trim: f32,
    /// What the player has heard since last making up their mind: internal samples listened
    /// to; times the string let go, and when it first and last did; and the periods the note
    /// was meant to have, summed over the control ticks counted beside them.
    watched: u32,
    slips: u32,
    first: u32,
    last: u32,
    meant: f32,
    ticks: u32,
    /// Samples the string has now been sliding, and gripped, without a break worth the name;
    /// how long each has to last to count as a slip or to end one; and how long ago the last
    /// slip began, with how soon after it another counts as a slip of its own.
    run: u32,
    gap: u32,
    real: u32,
    chatter: u32,
    since: u32,
    apart: u32,
    /// Listenings still to sit out after leaning into the string, while it settles.
    patience: u8,
    /// How fast this player's bow is moving now.
    speed: f32,
    /// Vibrato phase in turns, its rate in hertz and its depth relative to the section's.
    vibrato: f32,
    rate: f32,
    depth: f32,
    /// How far this player sits from the section's pitch, in cents, and wanders from it.
    detune: f32,
    drift: f32,
    /// Seat, as an offset to the note's place; and how hard this player bows.
    seat: f32,
    drive: f32,
    /// Internal samples before this player comes in.
    wait: usize,
    bridge_delay: usize,
    nut_delay: f32,
    nut_step: f32,
    noise: Noise,
}

impl Player {
    fn clear(&mut self) {
        self.bridge.clear();
        self.nut.clear();
        self.low = 0.0;
        self.stuck = false;
        self.press = 1.0;
        self.trim = 0.0;
        self.forget();
        self.run = 0;
        self.gap = 0;
        self.since = 0;
        self.patience = 0;
        self.speed = 0.0;
        self.drift = 0.0;
        self.nut_delay = 0.0;
    }

    /// Starts listening afresh.
    fn forget(&mut self) {
        self.watched = 0;
        self.slips = 0;
        self.first = 0;
        self.last = 0;
        self.meant = 0.0;
        self.ticks = 0;
    }

    /// One internal sample. Returns the wave reaching the bridge.
    #[inline]
    fn step(&mut self, bow: &Bow) -> f32 {
        self.nut_delay += self.nut_step;
        let from_bridge = self.bridge.tap(self.bridge_delay);
        let from_nut = self.nut.read(self.nut_delay);
        let (target, force) = if self.wait > 0 {
            self.wait -= 1;
            (0.0, 0.0)
        } else {
            (bow.speed * self.drive, bow.force)
        };
        self.speed += (target - self.speed) * bow.ramp;
        let wanted = self.speed - (from_bridge + from_nut);
        let rough = bow.rough * self.noise.next();
        let push = friction(wanted, force * self.press, &mut self.stuck, rough);
        // A slip is the string sliding for a fair part of the time a kink takes to reach the
        // bridge and return. The hair catching for a sample or two in the middle of one does
        // not make it two, and neither does a slide that comes in pieces, as it does under
        // a heavy bow low on the instrument: what makes two is a second one well into the
        // period, which is the string sounding its octave.
        self.watched += 1;
        self.since = self.since.saturating_add(1);
        if self.stuck {
            self.gap = self.gap.saturating_add(1);
            if self.gap > self.chatter {
                self.run = 0;
            }
        } else {
            self.gap = 0;
            self.run += 1;
            if self.run == self.real && self.since > self.apart {
                if self.slips == 0 {
                    self.first = self.watched;
                }
                self.last = self.watched;
                self.slips += 1;
                self.since = 0;
            }
        }
        let to_bridge = from_nut + push;
        let to_nut = from_bridge + push;
        // Both ends send a wave back upside down; the bridge takes the top off it as well.
        self.low += (1.0 - bow.pole) * (to_bridge - self.low);
        self.bridge.write(-bow.bridge * self.low);
        self.nut.write(-bow.nut * to_nut);
        to_bridge
    }
}

/// What every player of a note shares for one control tick.
#[derive(Clone, Copy, Debug, Default)]
struct Bow {
    speed: f32,
    force: f32,
    ramp: f32,
    rough: f32,
    pole: f32,
    bridge: f32,
    nut: f32,
}

#[derive(Clone, Debug)]
struct Voice {
    players: [Player; PLAYERS],
    count: usize,
    /// Internal samples to one output sample.
    oversampling: usize,
    decimators: [Decimator; 2],
    /// The stage before those, for a string run at four times the rate.
    halvers: [Decimator; 2],
    blockers: [DcBlocker; 2],
    key: f32,
    /// The pitch being played now, which glides to the key under a slur.
    pitch: f32,
    velocity: f32,
    /// What the bow's speed is multiplied by to even this note out against its neighbours, and
    /// what it is multiplied by now, on its way there.
    evened: f32,
    evening: f32,
    position: f32,
    body: usize,
    sounding: bool,
    held: bool,
    pan: f32,
    pan_now: f32,
    mute: Mute,
    bow: Bow,
    /// Control ticks since the note began.
    age: usize,
    level: f32,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            players: Default::default(),
            count: 1,
            oversampling: 1,
            decimators: Default::default(),
            halvers: Default::default(),
            blockers: Default::default(),
            key: 60.0,
            pitch: 60.0,
            velocity: 0.0,
            evened: 1.0,
            evening: 1.0,
            position: 0.1,
            body: 0,
            sounding: false,
            held: false,
            pan: 0.0,
            pan_now: 0.0,
            mute: Mute::default(),
            bow: Bow::default(),
            age: 0,
            level: 0.0,
        }
    }
}

/// The body a note at `pitch` sounds through when the section seats itself.
fn register(pitch: f32) -> usize {
    match pitch as i32 {
        55.. => 0,
        48..=54 => 1,
        36..=47 => 2,
        _ => 3,
    }
}

/// The bowed strings.
pub(super) struct Bowed {
    rate: f32,
    settings: Settings,
    performance: Performance,
    voices: Vec<Voice>,
    bodies: [Radiator; 4],
    pan_step: f32,
    /// Expression as the bows follow it.
    expression: f32,
}

impl Bowed {
    pub(super) fn new() -> Self {
        Self {
            rate: 48_000.0,
            settings: Settings::default(),
            performance: Performance::default(),
            voices: Vec::new(),
            bodies: Default::default(),
            pan_step: 1.0,
            expression: 1.0,
        }
    }

    /// How many times the host's rate a string is run at.
    ///
    /// Stick and slip turn into one another at an instant, and what that does to a sampled
    /// string is put out of hearing by running the string at twice an ordinary rate. A note
    /// high on the instrument and bowed near the bridge needs twice that again: its kink is
    /// back from the bridge within a handful of samples.
    fn oversampling(&self, quality: Quality, hz: f32, position: f32) -> usize {
        if quality == Quality::Draft {
            return 1;
        }
        let base = if self.rate >= 88_200.0 { 1 } else { 2 };
        if position * base as f32 * self.rate / hz < FINEST {
            base * 2
        } else {
            base
        }
    }

    /// Works out what the bows of `voice` do for the coming tick, and each player's string
    /// length.
    fn conduct(&mut self, index: usize, start: bool) {
        let (settings, rate, performance) = (self.settings, self.rate, self.performance);
        let expression = self.expression;
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        let internal = rate * voice.oversampling as f32;
        let ticks_per_second = rate / TICK as f32;
        // A slur moves the finger, not the bow: the pitch arrives over a few hundredths of a
        // second.
        let target = voice.key + performance.bend;
        if start {
            voice.pitch = target;
        } else {
            voice.pitch +=
                (target - voice.pitch) * (1.0 - (-1.0 / (0.03 * ticks_per_second)).exp());
        }
        let seconds = voice.age as f32 / ticks_per_second;
        // Vibrato comes in once a note has settled, never on its attack.
        let settled = ((seconds - 0.18) / 0.3).clamp(0.0, 1.0);
        let settled = settled * settled * (3.0 - 2.0 * settled);
        let depth = (settings.vibrato * 32.0 + performance.modulation * 45.0) * settled;
        let fundamental = hertz(voice.pitch).max(LOWEST_HZ);
        // A thin string keeps its top longer than a thick one: the upper strings are brighter
        // than the pitch alone would make them.
        // This rounds the kink each time it meets the bridge, and the bow sharpens it again
        // each time it passes, the more the harder it presses: which is why pressing harder
        // is brighter.
        let register = (fundamental / 440.0).powf(if fundamental > 440.0 { 0.45 } else { 0.25 });
        let corner = 4_500.0 * 0.5_f32.powf(settings.damping) * register;
        let pole = lowpass_pole(corner.clamp(800.0, 16_000.0), internal);
        let position = (voice.position * stopped(voice.body, fundamental)).min(0.3);
        // The bow comes round to what evens the note out over a few hundredths of a second.
        voice.evening += 0.2 * (voice.evened - voice.evening);
        let speed = if voice.held {
            (0.05 + 0.55 * settings.speed)
                * (0.3 + 0.7 * voice.velocity)
                * expression
                * voice.evening
        } else {
            0.0
        };
        // The most force a string takes at this speed and place before the bow crushes the
        // note, by Schelleng's account. A harder key presses harder as well as bowing faster.
        let limit = speed.max(0.02) / (position * (GRIP - SLIDE));
        let pressure = (settings.pressure + 0.2 * (voice.velocity - 0.5)).clamp(0.0, 1.0);
        let asked = LIGHTEST * (HEAVIEST / LIGHTEST).powf(pressure);
        let ease = (EASE_PERIODS / fundamental).clamp(0.05, 0.4);
        let share = asked + (CATCH - asked).max(0.0) * (-seconds / ease).exp();
        // Bowed, a string loses the same part of itself each period wherever it is stopped;
        // let go, it rings for as long as the release says.
        let ring = if voice.held {
            RING_PERIODS
        } else {
            settings.release.max(0.01) * fundamental
        };
        voice.bow = Bow {
            speed,
            force: if voice.held { limit * share } else { 0.0 },
            ramp: 1.0 - (-1.0 / (settings.response.max(0.001) * internal)).exp(),
            rough: 0.12,
            pole,
            bridge: (-6.907_755 / ring).exp(),
            nut: 0.997,
        };
        let (held, steps) = (voice.held, (TICK * voice.oversampling) as f32);
        for player in voice.players.iter_mut().take(voice.count) {
            player.vibrato = (player.vibrato + player.rate / ticks_per_second).fract();
            // No hand holds a pitch still: a cent or two of slow wander.
            player.drift += (player.noise.next() * 1.8 - player.drift) * 0.03;
            let cents =
                player.detune + player.drift + depth * player.depth * (TAU * player.vibrato).sin();
            let hz = hertz(voice.pitch + cents / 100.0).max(LOWEST_HZ);
            let period = internal / hz - lowpass_delay(pole, TAU * hz / internal) * FILTER_SHARE;
            if start {
                player.bridge_delay = ((position * period).round() as usize).max(2);
                player.real = ((0.4 * player.bridge_delay as f32) as u32).max(2);
                player.chatter = (0.16 * player.bridge_delay as f32) as u32;
            }
            player.meant += internal / hz;
            player.ticks += 1;
            // Pieces of one slide come a trip to the bridge and back apart, or two.
            player.apart = (2.6 * player.bridge_delay as f32).min(0.42 * period) as u32;
            let listened = (LISTEN_PERIODS * period).max(LISTEN_SECONDS * internal);
            if held && player.watched as f32 >= listened {
                let meant = player.meant / player.ticks as f32;
                let slips = player.slips as f32 * meant / player.watched as f32;
                if player.patience > 0 {
                    player.patience -= 1;
                } else if slips > 1.4 {
                    // Slipping more than once a period: a firmer bow, a little at a time,
                    // kept once it is found, and given a moment to take.
                    player.press = (player.press * 1.25).min((HEAVIEST / share).max(1.0));
                    player.patience = 1;
                } else if slips > 0.7 && player.slips >= 4 {
                    // Moving as it should. The time from one letting-go to the next is the
                    // period being played, and the finger takes out part of how far that is
                    // from the period meant.
                    let heard = (player.last - player.first) as f32 / (player.slips - 1) as f32;
                    let flat = heard / meant - 1.0;
                    if flat.abs() < MISHEARD {
                        let nudge = (INTONATION * flat).clamp(-NUDGE, NUDGE);
                        player.trim = (player.trim - nudge).clamp(-REACH, REACH);
                    }
                }
                player.forget();
            }
            let wanted = (period * (1.0 + player.trim) - player.bridge_delay as f32)
                .clamp(3.0, player.nut.reach());
            if start || player.nut_delay == 0.0 {
                player.nut_delay = wanted;
                player.nut_step = 0.0;
            } else {
                player.nut_step = (wanted - player.nut_delay) / steps;
            }
        }
    }
}

impl Engine for Bowed {
    fn prepare(&mut self, rate: f32, voices: usize) {
        self.rate = rate;
        let internal = if rate >= 88_200.0 { rate } else { rate * 2.0 };
        let longest = (internal / LOWEST_HZ).ceil() as usize;
        self.voices = (0..voices)
            .map(|_| {
                let mut voice = Voice::default();
                for player in &mut voice.players {
                    player.nut.prepare(longest);
                    player.bridge.prepare(longest / 2);
                }
                voice
            })
            .collect();
        for (body, scale) in self.bodies.iter_mut().zip(SCALES) {
            body.prepare(&body::VIOLIN, scale, rate);
        }
        self.pan_step = 1.0 - (-1.0 / (0.010 * rate)).exp();
        self.reset();
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            for player in &mut voice.players {
                player.clear();
            }
            voice.sounding = false;
            voice.held = false;
            voice.mute.open();
            voice.level = 0.0;
        }
        for body in &mut self.bodies {
            body.reset();
        }
        self.expression = self.performance.expression;
    }

    fn configure(&mut self, params: &ParamBank) {
        self.settings = Settings {
            position: params.at(P_POSITION),
            damping: params.at(P_DAMPING),
            body: params.at(P_BODY),
            release: params.at(P_RELEASE),
            pressure: params.at(P_PRESSURE),
            speed: params.at(P_SPEED),
            response: params.at(P_RESPONSE),
            vibrato: params.at(P_VIBRATO),
            players: (params.at(P_PLAYERS).round() as usize).clamp(1, PLAYERS),
            instrument: params.at(P_INSTRUMENT).round().clamp(0.0, 4.0) as usize,
        };
        for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
            // The bow moves along the string at once; everything else is read every tick.
            voice.position = self.settings.position;
        }
    }

    fn start(&mut self, index: usize, note: &Note, quality: Quality) {
        let (rate, settings) = (self.rate, self.settings);
        let width = self.performance.width;
        let key = f32::from(note.pitch);
        let position = match note.touch {
            Some(at) => touch_position(at).clamp(0.03, 0.4),
            None => settings.position,
        };
        let body = match settings.instrument {
            0 => register(key),
            chosen => chosen - 1,
        };
        let sounding = hertz(key + self.performance.bend).max(LOWEST_HZ);
        let reach = (position * stopped(body, sounding)).min(0.3);
        let oversampling = self.oversampling(quality, sounding, reach);
        let evened = self.bodies.get(body).map_or(1.0, |body| {
            body.favour(sounding).powf(-EVENING).clamp(0.6, 1.5)
        });
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        voice.key = key;
        voice.evened = evened;
        voice.evening = evened;
        voice.velocity = note.velocity;
        voice.position = position;
        voice.body = body;
        voice.oversampling = oversampling;
        voice.count = settings.players;
        voice.sounding = true;
        voice.held = true;
        voice.pan = note.pan;
        voice.pan_now = note.pan * width;
        voice.mute.open();
        voice.age = 0;
        voice.level = 1.0;
        for decimator in voice.decimators.iter_mut().chain(&mut voice.halvers) {
            decimator.reset();
        }
        voice.blockers = [DcBlocker::new(8.0, rate), DcBlocker::new(8.0, rate)];
        let count = voice.count;
        let internal = rate * oversampling as f32;
        for (number, player) in voice.players.iter_mut().enumerate().take(count) {
            let seat = number as u32 * 8;
            player.clear();
            player
                .noise
                .seed(note.seed ^ (0x51ED_270B_u32.wrapping_mul(number as u32 + 1)));
            // The first desk plays the note; the others are near it.
            let spread = if count > 1 { 1.0 } else { 0.0 };
            let near = |salt: u32| 2.0 * seeded(note.seed, seat + salt) - 1.0;
            player.detune = if number == 0 { 0.0 } else { 7.0 * near(0) };
            player.rate = 5.6 * (1.0 + 0.12 * near(1));
            player.depth = 1.0 + 0.25 * near(2);
            player.vibrato = seeded(note.seed, seat + 3);
            player.drive = 1.0 + 0.1 * near(4) * spread;
            player.seat = spread * 0.3 * (2.0 * number as f32 / (count - 1).max(1) as f32 - 1.0);
            player.wait = if number == 0 {
                0
            } else {
                (0.022 * seeded(note.seed, seat + 5) * internal) as usize
            };
        }
        self.conduct(index, true);
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
        let (body, sounding) = (voice.body, hertz(voice.key).max(LOWEST_HZ));
        let evened = self.bodies.get(body).map_or(1.0, |body| {
            body.favour(sounding).powf(-EVENING).clamp(0.6, 1.5)
        });
        if let Some(voice) = self.voices.get_mut(index) {
            voice.evened = evened;
        }
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
            // The bow arm follows the expression pedal in its own time.
            let follow =
                1.0 - (-1.0 / (self.settings.response.max(0.004) * self.rate / TICK as f32)).exp();
            self.expression += (self.performance.expression - self.expression) * follow;
            for body in &mut self.bodies {
                body.tick();
            }
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
                // Falls a tenth of the way to nothing each tick unless the string refills it.
                voice.level *= 0.9;
                self.conduct(index, false);
            }
        }
        let mut buses = [[[0.0_f32; TICK]; 2]; 4];
        let mut used = [false; 4];
        for voice in self.voices.iter_mut().filter(|voice| voice.sounding) {
            let target = voice.pan * width;
            let scale = DRIVE * (voice.position / 0.09).sqrt() / (voice.count as f32).sqrt();
            let bow = voice.bow;
            used[voice.body] = true;
            let [bus_mid, bus_side] = &mut buses[voice.body];
            for frame in 0..frames {
                let mut subs = [[0.0_f32; 2]; 4];
                for sub in subs.iter_mut().take(voice.oversampling) {
                    for player in voice.players.iter_mut().take(voice.count) {
                        let wave = player.step(&bow);
                        sub[0] += wave;
                        sub[1] += wave * player.seat;
                    }
                }
                if voice.oversampling == 4 {
                    for (channel, halver) in voice.halvers.iter_mut().enumerate() {
                        let first = halver.process(subs[0][channel], subs[1][channel]);
                        let second = halver.process(subs[2][channel], subs[3][channel]);
                        subs[0][channel] = first;
                        subs[1][channel] = second;
                    }
                }
                let (centre, seated) = if voice.oversampling >= 2 {
                    (
                        voice.decimators[0].process(subs[0][0], subs[1][0]),
                        voice.decimators[1].process(subs[0][1], subs[1][1]),
                    )
                } else {
                    (subs[0][0], subs[0][1])
                };
                // A bow pushes one way for as long as it is drawn; a bridge passes none of it.
                let gate = voice.mute.next() * scale;
                let centre = voice.blockers[0].next(centre) * gate;
                let seated = voice.blockers[1].next(seated) * gate;
                voice.pan_now += (target - voice.pan_now) * self.pan_step;
                voice.level = voice.level.max(centre.abs());
                bus_mid[frame] += centre;
                bus_side[frame] += centre * voice.pan_now + seated * width;
            }
        }
        for ((body, bus), used) in self.bodies.iter_mut().zip(buses.iter_mut()).zip(used) {
            let [bus_mid, bus_side] = bus;
            let (bus_mid, bus_side) = (&mut bus_mid[..frames], &mut bus_side[..frames]);
            // A body nobody is playing still rings out what it was last given.
            let _ = used;
            body.process(bus_mid, bus_side, self.settings.body, width);
            for frame in 0..frames {
                mid[frame] += bus_mid[frame];
                side[frame] += bus_side[frame];
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
        let Some(player) = voice.players.first() else {
            return;
        };
        picture.level = self.level(index);
        picture.contact = voice.position;
        picture.excitation = player.speed;
        // The string's slope is the wave going one way less the wave going the other; summed
        // along the string that is its shape, the kink of Helmholtz's motion.
        let bridge = player.bridge_delay as f32;
        let nut = player.nut_delay.max(4.0);
        let position = (bridge / (bridge + nut)).clamp(0.01, 0.9);
        let count = picture.points.len();
        let mut height = 0.0;
        for (at, point) in picture.points.iter_mut().enumerate() {
            let x = at as f32 / (count - 1) as f32;
            let slope = if x <= position {
                let along = x / position;
                let to_bridge = -player
                    .bridge
                    .tap((bridge * 0.5 * (1.0 - along)) as usize + 1);
                let to_bow = player
                    .bridge
                    .tap((bridge * 0.5 * (1.0 + along)) as usize + 1);
                to_bridge - to_bow
            } else {
                let along = (1.0 - x) / (1.0 - position);
                let to_nut = -player.nut.read(nut * 0.5 * (1.0 - along) + 2.0);
                let to_bow = player.nut.read(nut * 0.5 * (1.0 + along));
                to_bow - to_nut
            };
            height += slope;
            *point = height;
        }
        // Both ends are fixed: whatever the sum has drifted by is taken out along the length.
        let drift = height;
        let scale = DISPLAY * voice.position.max(0.03) * 64.0 / count as f32;
        for (at, point) in picture.points.iter_mut().enumerate() {
            let x = at as f32 / (count - 1) as f32;
            *point = (*point - drift * x) * scale;
        }
        picture.points[0] = 0.0;
        picture.points[MOTION_POINTS - 1] = 0.0;
        picture.modes = [0.0; auris_core::motion::MOTION_MODES];
        // What the string did under the hair through the last period is still on its way
        // round: the wave it sent toward the nut, and the wave that arrived from there to
        // meet it. Together they are the string's speed at the bow, which in Helmholtz's
        // motion is the bow's own for most of the period and a quick flight back for the
        // rest.
        let period = (bridge + nut).min(player.nut.reach() - nut - 2.0).max(4.0);
        let kept = voice.bow.nut.max(0.5);
        let under_the_hair = |back: f32| {
            let sent = -player.nut.read(back.max(2.0)) / kept;
            sent + player.nut.read(nut + back)
        };
        let moving = player.speed.abs().max(0.02);
        for (index, sample) in picture.scope.iter_mut().enumerate() {
            let back = period * (1.0 - index as f32 / MOTION_SCOPE as f32);
            *sample = (under_the_hair(back) / moving).clamp(-24.0, 4.0);
        }
        harmonics(
            |along| under_the_hair(period * (1.0 - along)) * DISPLAY,
            &mut picture.partials,
        );
    }

    fn observe_body(&self, body: &mut [f32; MOTION_BODY]) {
        // Whichever bodies are being played, together: a section is all of them at once.
        let mut each = [0.0_f32; MOTION_BODY];
        for instrument in &self.bodies {
            instrument.observe(&mut each);
            for (mode, part) in body.iter_mut().zip(each) {
                *mode += part * self.settings.body * super::BODY_DISPLAY;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rosin_lets_go_gently_under_a_light_bow_and_all_at_once_under_a_heavy_one() {
        let release = |force: f32| {
            let hold = GRIP * force;
            let mut stuck = true;
            let before = friction(hold - 1.0e-5, force, &mut stuck, 0.0);
            assert!(stuck);
            let after = friction(hold + 1.0e-5, force, &mut stuck, 0.0);
            assert!(!stuck, "the string should have let go");
            after / before
        };
        // Under a light bow friction falls more gently than the string's own line rises, the
        // two only ever meet once, and a slide begins from nothing.
        assert!((release(0.05) - 1.0).abs() < 0.02, "{}", release(0.05));
        // Under a heavy one they meet three times, and a string that lets go has nowhere to
        // go but the far one: friction drops at once to near its sliding value. That jump is
        // the corner of Helmholtz's motion.
        for force in [0.5, 2.0, 4.0] {
            let kept = release(force);
            assert!((SLIDE / GRIP..0.6).contains(&kept), "force {force}: {kept}");
        }
    }

    #[test]
    fn friction_never_pushes_a_string_past_the_bow_or_against_it() {
        for force in [0.0, 0.3, 2.0] {
            for wanted in [-3.0, -0.4, -0.01, 0.0, 0.01, 0.4, 3.0] {
                for was in [true, false] {
                    let mut stuck = was;
                    let push = friction(wanted, force, &mut stuck, 0.0);
                    assert!(push * wanted >= 0.0, "{push} against {wanted}");
                    assert!(push.abs() <= wanted.abs() + 1.0e-6);
                    assert!(push.abs() <= GRIP * force + 1.0e-6);
                }
            }
        }
    }

    #[test]
    fn a_sliding_string_is_not_caught_until_it_has_slowed_well_below_the_grip() {
        // Friction falls as the slide quickens, so a string already sliding keeps sliding at
        // forces that would have held it had it been stuck: the hysteresis of the bow.
        let force = 2.0;
        let hold = GRIP * force;
        let mut stuck = false;
        let sliding = friction(hold * 0.9, force, &mut stuck, 0.0);
        assert!(!stuck);
        assert!(sliding < hold * 0.9 * 0.8, "{sliding}");
        let mut stuck = true;
        assert_eq!(friction(hold * 0.9, force, &mut stuck, 0.0), hold * 0.9);
        // Slowed far enough, it is caught.
        let mut stuck = false;
        friction(hold * 0.2, force, &mut stuck, 0.0);
        assert!(stuck);
    }

    #[test]
    fn each_register_has_the_body_that_plays_it() {
        assert_eq!(register(76.0), 0);
        assert_eq!(register(55.0), 0);
        assert_eq!(register(50.0), 1);
        assert_eq!(register(40.0), 2);
        assert_eq!(register(30.0), 3);
    }
}
