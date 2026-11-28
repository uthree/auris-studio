//! Brass: a player's lips buzzing into a tube that flares into a bell.
//!
//! The lips are a valve, as a reed is, but one that opens the other way. The breath pushes
//! them apart, toward the mouthpiece; the pressure in the mouthpiece pushes them back. They
//! are a mass on a spring with a resonance of their own, which the player tightens or slackens
//! to choose a note, and they swing open and shut once a period, letting the air through in
//! puffs. How wide they stand is solved as that mass on that spring; how much air gets through
//! the gap they leave is solved each sample, exactly, against the wave coming back up the tube.
//!
//! The tube is what decides the note. A cylinder closed at the lips would sound only the odd
//! harmonics, as a clarinet does; the mouthpiece and the flare pull a brass instrument's
//! resonances onto a whole harmonic series instead, and that series is what the lips are
//! playing against. So the bore here is a loop one period long, which the flare closes the
//! right way up for the low part of the wave. What the flare does not send back it lets out:
//! the high part. That is why a bell sounds brighter than the air inside it, and why each
//! instrument's bell — small on a trumpet, wide on a tuba, half stopped by a hand on a horn —
//! sets the band where its sound is loudest.
//!
//! What makes brass *brass* happens on the way to the bell. Sound travels a little faster where
//! the air is compressed than where it is rarefied, so over a tube's length the crest of a loud
//! wave catches up with the trough ahead of it and the wave steepens toward a shock. A quiet
//! note arrives as it left the lips; a loud one arrives nearly as a sawtooth, with its upper
//! harmonics many decibels stronger. Each sample here is carried to the bell in a time that
//! depends on its own pressure, and where a crest would overtake what is ahead of it, the two
//! arrive together: the shock. The brightness of a fortissimo is that, and not a filter opened
//! by velocity.
//!
//! Lips pull a note toward their own resonance, and a player hears it and lips the note back
//! in. The model does the same: it times each period of the buzz and moves the tube by what it
//! hears, so the note sounds where it is written at every dynamic.

use std::f32::consts::{PI, TAU};

use auris_core::motion::{MOTION_SCOPE, MotionVoice};
use auris_core::{ParamDescriptor, ParamUnit};
use auris_dsp::DcBlocker;

use super::calibration::dial;
use super::dials;
use super::waveguide::{Decimator, Ear, Line, Noise, lowpass_pole};
use super::{Engine, Mute, Note, Performance, Quality, TICK, harmonics, hertz};
use crate::ParamBank;

const P_INSTRUMENT: u32 = 0;
const P_PRESSURE: u32 = 1;
const P_BRASSINESS: u32 = 2;
const P_NOISE: u32 = 3;
const P_ATTACK: u32 = 4;
const P_RELEASE: u32 = 5;
const P_VIBRATO: u32 = 6;
const P_LEGATO: u32 = 7;
const P_TECHNIQUE: u32 = 8;

/// The instruments, in parameter-value order.
const INSTRUMENTS: &[std::borrow::Cow<'static, str>] = &[
    std::borrow::Cow::Borrowed("Trumpet"),
    std::borrow::Cow::Borrowed("Horn"),
    std::borrow::Cow::Borrowed("Trombone"),
    std::borrow::Cow::Borrowed("Tuba"),
];

/// Lowest note any of them is asked to play.
const LOWEST_HZ: f32 = 20.0;
/// Scale of bore pressure in the picture a frontend draws.
const DISPLAY: f32 = 0.15;
/// Scale of what the instrument radiates.
const DRIVE: f32 = 1.0;
/// Pitch at which the radiated level is left as it is.
const REGISTER_HZ: f32 = 300.0;
/// How far a unit of pressure across the lips opens them: the unit the pressures are in.
const LIP_GIVE: f32 = 1.0;
/// Most the measured pull of the lips is allowed to move the tube, in cents.
const TRIM_LIMIT: f32 = 150.0;
/// How much of a period's measured error is taken out at once.
const TRIM_GAIN: f32 = 0.35;
/// How loud the tongue's puff rings the tube at the start of a note, against the breath.
const SEED: f32 = 0.5;
/// How much more than the note is held with a player blows to start it, and the seconds
/// that eases over.
const KICK: f32 = 1.0;
const KICK_SECONDS: f32 = 0.03;
/// The most a sample's pressure can shorten its trip to the bell, in units of pressure: no
/// louder wave than this is told apart.
const STEEPEST: f32 = 1.5;
/// The room the trip to the bell is given, as a power of two.
const CARRY: usize = 2_048;

/// What makes one brass instrument sound unlike another.
///
/// Fitted, not chosen: each instrument's numbers are the ones that bring the first ten
/// harmonics of its notes nearest those of the University of Iowa recordings of it, at
/// *pianissimo*, *mezzo forte* and *fortissimo* — every note the fit was not shown included,
/// to within a few decibels a harmonic. `docs/physical-instruments.md` has the account.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bore {
    /// Where the bell stops sending the wave back and lets it out, and how sharply its
    /// radiation rises to that frequency: a bell reflects most just below it.
    bell_hz: f32,
    bell_q: f32,
    /// What is left of the highest harmonics as the sound leaves the bell and the room takes
    /// it; a horn's hand in its bell takes most.
    air_hz: f32,
    /// Seconds a unit of pressure shortens the trip to the bell by: how far down the tube
    /// a loud wave has to steepen.
    steepening: f32,
    /// How much of a round trip the walls take, at the bottom of the instrument's range.
    loss: f32,
    /// Mouth pressure at the softest breath and at the hardest, in units of what opens the
    /// lips by their own width.
    softest: f32,
    hardest: f32,
    /// The lips' own resonance as a share of the note — under it, as lips stand for every note
    /// a brass instrument is in tune on — how sharply they resonate, and how much air a unit
    /// of their opening lets through under a unit of pressure, against the tube.
    lip_tuning: f32,
    lip_q: f32,
    lip_area: f32,
    /// Gain that brings the instrument to the trumpet's loudness in its own register.
    level: f32,
}

impl Bore {
    fn of(instrument: usize) -> Self {
        match instrument {
            // A horn's bell is wide and its player's hand half fills it: its sound is low and
            // round, and brightens little. A trombone's long cylinder is where a wave steepens
            // most.
            1 => Self {
                bell_hz: 405.0,
                bell_q: 2.59,
                air_hz: 3_955.0,
                steepening: 2.3e-4,
                loss: 0.03,
                softest: 0.076,
                hardest: 0.738,
                lip_tuning: 0.685,
                lip_q: 7.43,
                lip_area: 0.648,
                level: 0.452,
            },
            2 => Self {
                bell_hz: 504.0,
                bell_q: 1.24,
                air_hz: 5_331.0,
                steepening: 5.5e-3,
                loss: 0.03,
                softest: 0.059,
                hardest: 0.595,
                lip_tuning: 0.715,
                lip_q: 9.71,
                lip_area: 0.54,
                level: 1.422,
            },
            3 => Self {
                bell_hz: 469.0,
                bell_q: 1.57,
                air_hz: 8_297.0,
                steepening: 4.9e-4,
                loss: 0.035,
                softest: 0.063,
                hardest: 1.043,
                lip_tuning: 0.687,
                lip_q: 5.37,
                lip_area: 0.886,
                level: 1.096,
            },
            _ => Self {
                bell_hz: 1_209.0,
                bell_q: 2.76,
                air_hz: 1_726.0,
                steepening: 3.5e-4,
                loss: 0.03,
                softest: 0.057,
                hardest: 0.847,
                lip_tuning: 0.779,
                lip_q: 3.27,
                lip_area: 0.871,
                level: 1.0,
            },
        }
    }
}

pub(super) fn descriptors() -> Vec<ParamDescriptor> {
    vec![
        ParamDescriptor::new(P_INSTRUMENT, "instrument", "Instrument", 0.0, 3.0, 0.0)
            .with_choices(INSTRUMENTS),
        ParamDescriptor::percent(P_PRESSURE, "pressure", "Breath Pressure", 0.5),
        ParamDescriptor::percent(P_BRASSINESS, "brassiness", "Brassiness", 0.5),
        ParamDescriptor::percent(P_NOISE, "breath_noise", "Breath Noise", 0.1),
        ParamDescriptor::new(P_ATTACK, "attack", "Attack", 0.001, 0.3, 0.03)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::new(P_RELEASE, "release", "Release", 0.01, 4.0, 0.15)
            .with_unit(ParamUnit::Seconds),
        ParamDescriptor::percent(P_VIBRATO, "vibrato", "Vibrato", 0.0),
        ParamDescriptor::toggle(P_LEGATO, "legato", "Legato", true),
        ParamDescriptor::new(P_TECHNIQUE, "technique", "Technique", 0.0, 1.0, 0.0)
            .with_choices(super::TONGUINGS),
    ]
}

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    instrument: usize,
    pressure: f32,
    brassiness: f32,
    noise: f32,
    attack: f32,
    release: f32,
    vibrato: f32,
    flutter: bool,
}

impl Settings {
    fn bore(&self) -> Bore {
        Bore::of(self.instrument)
    }
}

/// How deep a rolled tongue cuts the breath at each roll.
const FLUTTER: f32 = 0.85;

/// What a voice needs for one control tick.
#[derive(Clone, Copy, Debug, Default)]
struct Wind {
    /// Mouth pressure the player is heading for.
    mouth: f32,
    ramp: f32,
    /// How much of what the breath carries is turbulence.
    noise: f32,
    /// Pole of the two-stage low-pass the bell sends back through, and of the slow high-pass
    /// that keeps the mouth's steady pressure out of the tube.
    pole: f32,
    blocking: f32,
    reflection: f32,
    /// The lips' resonance and damping, per internal sample, and how much air a unit of
    /// their opening lets through.
    lip_turn: f32,
    lip_damping: f32,
    area: f32,
    /// How quickly the steady pressure in the tube is followed.
    settle: f32,
    /// The high-pass the sound leaves the bell through: a bell radiates little below the
    /// frequency at which it stops reflecting, and most just above it.
    bell: Biquad,
    /// Samples a unit of pressure shortens the trip to the bell by.
    steepening: f32,
    /// Pole of the gentle low-pass on what is radiated.
    air: f32,
    /// Output gain for this note's register.
    gain: f32,
}

/// Coefficients of a second-order section, normalised so its first feedback term is one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
}

impl Biquad {
    /// A high-pass with its corner at `hz` and resonance `q`.
    fn highpass(hz: f32, q: f32, rate: f32) -> Self {
        let turn = TAU * hz.min(rate * 0.45) / rate;
        let (sine, cosine) = turn.sin_cos();
        let alpha = sine / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha;
        let pass = (1.0 + cosine) / 2.0 / a0;
        Self {
            b: [pass, -2.0 * pass, pass],
            a: [-2.0 * cosine / a0, (1.0 - alpha) / a0],
        }
    }

    /// One sample through the section, whose memory is `state`.
    #[inline]
    fn run(&self, state: &mut [f32; 2], input: f32) -> f32 {
        // Transposed direct form II.
        let output = self.b[0] * input + state[0];
        state[0] = self.b[1] * input - self.a[0] * output + state[1];
        state[1] = self.b[2] * input - self.a[1] * output;
        output
    }
}

/// A wave carried to the bell, each sample in a time of its own.
#[derive(Clone, Debug, Default)]
struct Carry {
    /// When each sample arrives and what it is, in order of arrival.
    arrival: Vec<f64>,
    value: Vec<f32>,
    head: usize,
    count: usize,
    /// The last arrival queued: none may come before it.
    last: f64,
    /// Samples since the note began.
    now: f64,
}

impl Carry {
    fn prepare(&mut self) {
        self.arrival = vec![0.0; CARRY];
        self.value = vec![0.0; CARRY];
        self.clear();
    }

    fn clear(&mut self) {
        self.head = 0;
        self.count = 0;
        self.last = f64::NEG_INFINITY;
        self.now = 0.0;
    }

    /// Sends `value` off now, to arrive `trip` samples later, and returns what reaches the
    /// bell this sample.
    fn step(&mut self, value: f32, trip: f32) -> f32 {
        let mask = CARRY - 1;
        // A crest that would overtake what is ahead of it arrives with it: the shock front.
        let arrival = (self.now + f64::from(trip)).max(self.last + 1.0e-3);
        if self.count == CARRY {
            // Never with the trips this is asked for; dropping the oldest keeps it bounded.
            self.head = (self.head + 1) & mask;
            self.count -= 1;
        }
        let at = (self.head + self.count) & mask;
        self.arrival[at] = arrival;
        self.value[at] = value;
        self.count += 1;
        self.last = arrival;
        let now = self.now;
        self.now += 1.0;
        while self.count >= 2 && self.arrival[(self.head + 1) & mask] <= now {
            self.head = (self.head + 1) & mask;
            self.count -= 1;
        }
        if self.count < 2 || self.arrival[self.head] > now {
            return 0.0;
        }
        let next = (self.head + 1) & mask;
        let (from, to) = (self.arrival[self.head], self.arrival[next]);
        let along = ((now - from) / (to - from).max(1.0e-9)) as f32;
        self.value[self.head] + (self.value[next] - self.value[self.head]) * along
    }
}

#[derive(Clone, Debug)]
struct Voice {
    /// The wave on its way down the tube and back.
    bore: Line,
    /// The same wave, steepening on its way to the bell.
    carry: Carry,
    /// How far apart the lips have stood, sample by sample, for the picture.
    trace: Line,
    /// Two stages of the bell's reflection, and the high-pass after them.
    low: [f32; 2],
    block: [f32; 2],
    /// The same two stages on the wave as it leaves the bell.
    out: [f32; 2],
    /// How far the lips stand apart, and how fast they are moving.
    lips: f32,
    lip_speed: f32,
    /// Pressure in the mouthpiece at the last sample.
    inside: f32,
    /// The steady part of the pressure leaving the mouthpiece.
    steady: f32,
    gain: f32,
    mouth: f32,
    air: f32,
    hiss: f32,
    delay: f32,
    delay_step: f32,
    /// Cents the lips have been heard to pull the note sharp by, which the tube is moved by.
    trim: f32,
    ear: Ear,
    oversampling: usize,
    decimator: Decimator,
    blocker: DcBlocker,
    key: f32,
    slid: f32,
    pitch: f32,
    /// The frequency the note is meant to sound at, as last conducted.
    aim: f32,
    velocity: f32,
    sounding: bool,
    held: bool,
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
            carry: Carry::default(),
            trace: Line::default(),
            low: [0.0; 2],
            block: [0.0; 2],
            out: [0.0; 2],
            lips: 0.0,
            lip_speed: 0.0,
            inside: 0.0,
            steady: 0.0,
            gain: 0.0,
            mouth: 0.0,
            air: 0.0,
            hiss: 0.0,
            delay: 0.0,
            delay_step: 0.0,
            trim: 0.0,
            ear: Ear::default(),
            oversampling: 1,
            decimator: Decimator::default(),
            blocker: DcBlocker::default(),
            key: 60.0,
            slid: 60.0,
            pitch: 60.0,
            aim: 261.6,
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

/// The air through the lips, given how far apart they stand and what the pressure across them
/// would be with no air moving at all.
///
/// Air through a gap goes as the square root of the pressure across it, and the tube answers
/// air let in with pressure of its own: `flow = gap * sqrt(unloaded - flow)`. That is a
/// quadratic in the flow with one root of the right sign, taken here directly.
fn lip_flow(gap: f32, unloaded: f32) -> f32 {
    let squared = gap * gap;
    let flow = 0.5 * (-squared + (squared * squared + 4.0 * squared * unloaded.abs()).sqrt());
    flow.copysign(unloaded)
}

impl Voice {
    fn clear(&mut self) {
        self.bore.clear();
        self.carry.clear();
        self.trace.clear();
        self.low = [0.0; 2];
        self.block = [0.0; 2];
        self.out = [0.0; 2];
        self.lips = 0.0;
        self.lip_speed = 0.0;
        self.inside = 0.0;
        self.steady = 0.0;
        self.mouth = 0.0;
        self.air = 0.0;
        self.hiss = 0.0;
        self.delay = 0.0;
        self.trim = 0.0;
        self.ear.clear();
        self.drift = 0.0;
    }

    /// One internal sample. Returns what leaves the bell.
    #[inline]
    fn step(&mut self) -> f32 {
        let wind = self.wind;
        self.delay += self.delay_step;
        let back = self.bore.read(self.delay);
        let [first, second] = &mut self.low;
        *first += (1.0 - wind.pole) * (back - *first);
        *second += (1.0 - wind.pole) * (*first - *second);
        // The flare sends the low part of the wave back the right way up; the mouth's steady
        // pressure has nowhere to stand in a tube open at its far end.
        let [input, held] = &mut self.block;
        *held = 0.5 * (1.0 + wind.blocking) * (*second - *input) + wind.blocking * *held;
        *input = *second;
        let returning = wind.reflection * *held;
        self.mouth += (wind.mouth - self.mouth) * wind.ramp;
        self.hiss += 0.3 * (self.noise.next() - self.hiss);
        let unloaded = self.mouth * (1.0 + wind.noise * self.hiss) - 2.0 * returning;
        let flow = lip_flow(wind.area * self.lips.max(0.0), unloaded);
        let across = unloaded - flow;
        // The lips: a mass on a spring, pushed apart by the pressure across them.
        let (turn, damping) = (wind.lip_turn, wind.lip_damping);
        self.lip_speed += turn * turn * (LIP_GIVE * across - self.lips) - damping * self.lip_speed;
        self.lips += self.lip_speed;
        self.trace.write(self.lips.max(0.0));
        let leaving = returning + flow;
        self.inside = leaving + returning;
        self.ear.hear(self.inside);
        self.bore.write(leaving);
        // Carried to the bell, crests ahead of troughs. What hastens a sample is how far it
        // stands above the tube's steady pressure, not the steady pressure itself.
        self.steady += wind.settle * (leaving - self.steady);
        let swing = (leaving - self.steady).clamp(-STEEPEST, STEEPEST);
        let trip = 0.5 * self.delay + wind.steepening * STEEPEST;
        let arriving = self.carry.step(leaving, trip - wind.steepening * swing);
        // The bell lets out what it does not send back: little of the low part of the wave,
        // and less the lower it is, as a mouth smaller than the wavelength radiates.
        let radiated = wind.bell.run(&mut self.out, arriving);
        self.air += (1.0 - wind.air) * (radiated - self.air);
        self.air
    }
}

/// The brass instruments.
pub(super) struct Brass {
    rate: f32,
    settings: Settings,
    performance: Performance,
    voices: Vec<Voice>,
    pan_step: f32,
    expression: f32,
}

/// Samples by which the bell's two low-pass stages and the blocking high-pass, together,
/// delay a sinusoid turning `turn` radians a sample.
fn loop_delay(pole: f32, blocking: f32, turn: f32) -> f32 {
    let turn = turn.max(1.0e-4);
    let (sine, cosine) = turn.sin_cos();
    // Each low-pass stage lags by the angle of 1 - pole·e^{-iω}.
    let low = (pole * sine).atan2(1.0 - pole * cosine);
    // The high-pass (1 - e^{-iω}) / (1 - b·e^{-iω}) leads by a quarter turn less half the
    // step, less the lag of its pole.
    let lead = 0.5 * (PI - turn) - (blocking * sine).atan2(1.0 - blocking * cosine);
    (2.0 * low - lead) / turn
}

impl Brass {
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
        let bore = settings.bore();
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        let internal = rate * voice.oversampling as f32;
        let ticks_per_second = rate / TICK as f32;
        if start {
            voice.slid = voice.key;
        } else {
            super::slide(&mut voice.slid, voice.key, &performance, ticks_per_second);
        }
        let target = voice.slid + performance.bend;
        if start {
            voice.pitch = target;
        } else {
            voice.pitch +=
                (target - voice.pitch) * (1.0 - (-1.0 / (0.02 * ticks_per_second)).exp());
        }
        let seconds = voice.age as f32 / ticks_per_second;
        let settled = ((seconds - 0.3) / 0.4).clamp(0.0, 1.0);
        let depth = (settings.vibrato * 20.0 + performance.modulation * 30.0) * settled;
        voice.vibrato = (voice.vibrato + voice.vibrato_rate / ticks_per_second).fract();
        voice.drift += (voice.noise.next() * 1.5 - voice.drift) * 0.02;
        let cents = voice.drift + depth * (TAU * voice.vibrato).sin();
        let fundamental = hertz(voice.pitch + cents / 100.0).clamp(LOWEST_HZ, internal * 0.1);
        // The lips pull the note; the player hears by how much and lips it back.
        if voice.level > 1.0e-4 {
            let gain = dial(dials::BRASS_TRIM_GAIN, TRIM_GAIN);
            voice
                .ear
                .correct(&mut voice.trim, voice.aim, gain, TRIM_LIMIT, internal);
        }
        voice.aim = fundamental;
        voice.ear.tune(fundamental, 0.5, internal);
        // The Breath Pressure control's middle plays a full velocity at the breath the
        // instrument is voiced for; its top blows past that.
        let breath = 2.0
            * settings.pressure
            * (0.15 + 0.85 * voice.velocity)
            * (0.4 + 0.6 * expression)
            * performance.breath.clamp(0.0, 1.0);
        let (roll, attack) = if settings.flutter {
            let rolls = super::FLUTTER_HZ + 2.0 * (voice.vibrato_rate - 5.0);
            (
                super::flutter(seconds, rolls, FLUTTER),
                settings.attack.min(super::FLUTTER_TURN),
            )
        } else {
            (1.0, settings.attack)
        };
        let time = if voice.held {
            attack
        } else {
            (settings.release * 0.3).max(0.004)
        };
        let bell_hz = dial(dials::BRASS_BELL_HZ, bore.bell_hz);
        let corner = bell_hz.max(3.0 * fundamental);
        let pole = lowpass_pole(corner, internal);
        let blocking = lowpass_pole((fundamental * 0.125).min(20.0), internal);
        // The walls take more of a long narrow tube's round trip than of a short wide one's.
        let reflection = 1.0 - bore.loss * (110.0 / fundamental).sqrt().clamp(0.5, 2.0);
        // Breath is heard in decibels: each step of it is a like step of loudness.
        let (softest, hardest) = (
            dial(dials::BRASS_SOFTEST, bore.softest),
            dial(dials::BRASS_HARDEST, bore.hardest),
        );
        let mouth = softest * (hardest / softest).powf(breath.clamp(0.0, 1.25));
        // A player starts a note with more breath than it is held with, and lets it settle:
        // lips barely over the pressure that sets them going take a long time to get there.
        let kick = 1.0
            + dial(dials::BRASS_KICK, KICK)
                * (-seconds / dial(dials::BRASS_KICK_SECONDS, KICK_SECONDS)).exp();
        let tuning = dial(dials::BRASS_LIP_TUNING, bore.lip_tuning);
        let lip_q = dial(dials::BRASS_LIP_Q, bore.lip_q);
        let speaking = performance.speaking();
        voice.wind = Wind {
            mouth: if voice.held {
                mouth * kick * roll * speaking
            } else {
                0.0
            },
            ramp: 1.0 - (-1.0 / (time.max(0.001) * internal)).exp(),
            noise: settings.noise * 0.6,
            pole,
            blocking,
            reflection,
            lip_turn: TAU * tuning * fundamental / internal,
            lip_damping: TAU * tuning * fundamental / internal / lip_q,
            area: dial(dials::BRASS_LIP_AREA, bore.lip_area),
            settle: 1.0 - lowpass_pole(fundamental * 0.25, internal),
            bell: Biquad::highpass(bell_hz, dial(dials::BRASS_BELL_Q, bore.bell_q), internal),
            steepening: dial(dials::BRASS_STEEPENING, bore.steepening)
                * settings.brassiness
                * 2.0
                * internal,
            air: lowpass_pole(dial(dials::BRASS_AIR_HZ, bore.air_hz), internal),
            gain: bore.level * (REGISTER_HZ / fundamental).powf(0.3).clamp(0.4, 2.5),
        };
        // The loop is one period of the note, less what its filters take, moved by what the
        // lips were heard to pull.
        let played = fundamental * (-voice.trim / 1200.0).exp2();
        let wanted = (internal / played - loop_delay(pole, blocking, TAU * played / internal))
            .clamp(4.0, voice.bore.reach());
        if start || voice.delay == 0.0 {
            voice.delay = wanted;
            voice.delay_step = 0.0;
        } else {
            voice.delay_step = (wanted - voice.delay) / (TICK * voice.oversampling) as f32;
        }
    }
}

impl Engine for Brass {
    fn prepare(&mut self, rate: f32, voices: usize) {
        self.rate = rate;
        let internal = if rate >= 88_200.0 { rate } else { rate * 2.0 };
        let longest = (internal / LOWEST_HZ).ceil() as usize;
        self.voices = (0..voices)
            .map(|_| {
                let mut voice = Voice::default();
                voice.bore.prepare(longest);
                voice.trace.prepare(longest);
                voice.carry.prepare();
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
            instrument: params.at(P_INSTRUMENT).round().clamp(0.0, 3.0) as usize,
            pressure: params.at(P_PRESSURE),
            brassiness: params.at(P_BRASSINESS),
            noise: params.at(P_NOISE),
            attack: params.at(P_ATTACK),
            release: params.at(P_RELEASE),
            vibrato: params.at(P_VIBRATO),
            flutter: params.at(P_TECHNIQUE) >= 0.5,
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
        voice.key = note.pitch;
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
            // The tongue lets the breath at the lips all at once, and the tube rings with that
            // puff at its own note before the lips have learnt it: the buzz grows from that
            // rather than from nothing, as a tongued note does.
            let seed = dial(dials::BRASS_SEED, SEED) * voice.wind.mouth;
            let period = voice.delay.max(4.0);
            let samples = (period.ceil() as usize + 4).min(voice.bore.reach() as usize);
            for n in 0..samples {
                voice.bore.write(seed * (TAU * n as f32 / period).sin());
            }
        }
    }

    fn glide(&mut self, index: usize, note: &Note) -> bool {
        let Some(voice) = self.voices.get_mut(index) else {
            return false;
        };
        if !voice.sounding {
            return false;
        }
        voice.key = note.pitch;
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
        // How far apart the lips stand.
        picture.excitation = voice.lips.clamp(0.0, 2.0);
        // Pressure along the tube is the wave going down plus the wave coming back, which at
        // a place `x` of the way along left the mouthpiece `x` and `2 - x` half-trips ago.
        let delay = voice.delay.max(4.0);
        let count = picture.points.len();
        for (at, point) in picture.points.iter_mut().enumerate() {
            let x = at as f32 / (count - 1) as f32;
            let down = voice.bore.read(2.0 + x * delay * 0.5);
            let back = voice.bore.read(delay - x * delay * 0.5);
            *point = (down + back) * DISPLAY;
        }
        picture.modes = [0.0; auris_core::motion::MOTION_MODES];
        // The lips through one period: shut, pushed apart, swinging back to touch.
        let reach = voice.bore.reach() - 2.0;
        let period = delay.min(reach).max(4.0);
        let leaving = |back: f32| voice.bore.read(back.max(2.0));
        for (index, sample) in picture.scope.iter_mut().enumerate() {
            let back = period * (1.0 - index as f32 / MOTION_SCOPE as f32);
            *sample = voice.trace.read(back.max(2.0)).clamp(0.0, 4.0);
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
    fn the_air_through_the_lips_obeys_its_law_at_the_pressure_it_leaves() {
        for unloaded in [-0.8_f32, -0.05, 0.0, 0.02, 0.4, 1.3] {
            for gap in [0.0_f32, 0.2, 1.0, 3.0] {
                let flow = lip_flow(gap, unloaded);
                let across = unloaded - flow;
                let law = gap * across.abs().sqrt() * across.signum();
                assert!(
                    (flow - law).abs() < 1.0e-4,
                    "gap {gap} unloaded {unloaded}: {flow} against {law}"
                );
            }
        }
    }

    #[test]
    fn a_rising_wave_steepens_into_a_front_on_its_way_to_the_bell() {
        // A pressure rising over twenty samples, each carried faster the higher it stands:
        // the top catches up with the foot and they arrive nearly together.
        let rise = |steepening: f32| {
            let mut carry = Carry::default();
            carry.prepare();
            let heard: Vec<f32> = (0..200)
                .map(|n| {
                    let value = ((n as f32 - 50.0) / 20.0).clamp(0.0, 1.0);
                    carry.step(value, 40.0 - steepening * value)
                })
                .collect();
            let first = heard.iter().position(|v| *v > 0.1).unwrap();
            let last = heard.iter().position(|v| *v > 0.9).unwrap();
            last - first
        };
        assert!((15..=17).contains(&rise(0.0)), "{}", rise(0.0));
        assert!(rise(18.0) <= 3, "{}", rise(18.0));
        // A crest can catch what is ahead of it but never pass it: a wave that would have
        // folded over arrives as one jump.
        assert!(rise(30.0) <= 1, "{}", rise(30.0));
    }

    #[test]
    fn the_loop_delay_of_its_filters_is_what_they_lag_by() {
        // Against the filters themselves, run on a sinusoid until it settles.
        let internal = 96_000.0_f32;
        for hz in [60.0_f32, 233.0, 700.0] {
            let pole = lowpass_pole(1_200.0_f32.max(3.0 * hz), internal);
            let blocking = lowpass_pole((hz * 0.125).min(20.0), internal);
            let turn = TAU * hz / internal;
            let (mut a, mut b, mut input, mut held) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
            let (mut re, mut im, mut in_re, mut in_im) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
            let total = (internal * 4.0) as usize;
            for n in 0..total {
                let x = (turn * n as f32).sin();
                a += (1.0 - pole) * (x - a);
                b += (1.0 - pole) * (a - b);
                held = 0.5 * (1.0 + blocking) * (b - input) + blocking * held;
                input = b;
                if n > total / 2 {
                    let phase = f64::from(turn) * n as f64;
                    re += f64::from(held) * phase.cos();
                    im += f64::from(held) * phase.sin();
                    in_re += f64::from(x) * phase.cos();
                    in_im += f64::from(x) * phase.sin();
                }
            }
            // A sinusoid lagging by `d` reads as an angle `d` greater against sines and cosines.
            let lag = (im.atan2(re) - in_im.atan2(in_re)).rem_euclid(std::f64::consts::TAU);
            let lag = if lag > std::f64::consts::PI {
                lag - std::f64::consts::TAU
            } else {
                lag
            };
            let measured = lag / f64::from(turn);
            let predicted = f64::from(loop_delay(pole, blocking, turn));
            assert!(
                (measured - predicted).abs() < 0.05,
                "{hz} Hz: measured {measured} predicted {predicted}"
            );
        }
    }
}
