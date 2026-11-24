//! Slapping a bass: a string thrown at its frets.
//!
//! A fingered note moves along the fingerboard and never meets it. A slapped one is driven
//! straight at it. The thumb comes down on the string near the end of the neck and knocks it
//! onto the last fret; a popped string is hooked by a finger, pulled away from the board and
//! let go, and comes back down on every fret at once half a period later. Either way the
//! sound is the collision: a string stopped dead where it touches metal, the corner that
//! leaves that place in both directions, and the rattle while the string is still swinging
//! further than the frets allow.
//!
//! None of that is linear, so none of it can be put into the partials as a pluck is. It is
//! solved against them instead, sample by sample, for as long as the string can reach a fret:
//!
//! * A **fret** is a place the string may not pass. Whenever the string is on it and still
//!   going down, it is given the impulse that stops it there and sends it back with a
//!   quarter of its speed. The impulse can only take energy from the string, never give it
//!   any, so nothing a player does can make the string blow up against its frets.
//! * The **thumb** is a mass on a lossy spring, its flesh, thrown at the string. It pushes the
//!   string down until the board under it takes the load, is thrown back by its own flesh,
//!   and leaves.
//!
//! Lengths here are metres and forces newtons, because the frets are where an instrument
//! maker puts them: a string stopped at a fret runs straight from it to the saddle, so the
//! clearance over every fret above follows from one number, the height of the saddle. The
//! phasors carry no units of their own, and one more number ties them to metres: the force
//! of a pluck at full velocity.
//!
//! Five numbers here are not a maker's but were set by measurement, against single notes of
//! three recorded basses each played finger-style, thumbed and popped (the IDMT-SMT-Bass
//! set): the force of a full pluck, what a fret gives back, the thumb's speed and how much
//! of it the string feels, and how hard a pop pulls. What was matched is what slapping does to a note — how much louder the blow is
//! than the note it leaves, how far the peak stands over it, how much of the fundamental
//! goes into the partials above, how fast the note then falls — each as the difference from
//! the same note plucked. `tools/eval/bass_slap.py` is the instrument.

use std::borrow::Cow;
use std::f32::consts::PI;

use super::calibration::dial;
use super::dials;
use super::rotors::{Point, Rotors};
use crate::params::finite_or;

/// Scale length of the instrument, nut to saddle, in metres: thirty-four inches.
const SCALE: f32 = 0.864;
/// Tension of a string, in newtons. The four of a set are within a tenth of one another.
const TENSION: f32 = 180.0;
/// Force of a pluck at full velocity, in newtons. With the tension, this is what says how
/// many metres a phasor's unit is: a full pluck a sixth of the way along an open string
/// holds it about four millimetres aside.
const FULL_PLUCK: f32 = 6.5;
/// Height of the strings over the line of the fret tops, at the saddle, in metres. A string
/// stopped at a fret is straight from there to the saddle, which makes this two millimetres
/// over the fret an octave up: a medium action.
const SADDLE: f32 = 4.0e-3;
/// Height of an open string over the same line at the nut, in metres: a nut is cut to hold
/// its string a little higher than a fret would, so that an open string clears the first.
const NUT: f32 = 3.0e-4;
/// Frets on the neck.
const FRETS: f32 = 20.0;
/// Frets a string is held to, of those between the stopping finger and the end of the neck:
/// the last, the first, and the rest spread evenly between. A string comes down on a stretch
/// of the neck at a time, and the frets left out lie between ones that stop it. Held to
/// three, what is measured of a slapped note moves by a decibel or so; held to all twenty, a
/// popped note comes out two decibels quieter than it does at six, where the numbers above
/// were set.
const HELD_FRETS: usize = 6;

/// Frets `quality` holds a string to: half as many in a draft.
pub(super) fn frets(draft: bool) -> usize {
    let held = dial(dials::SLAP_FRETS, HELD_FRETS as f32) as usize;
    if draft { held / 2 } else { held }
}

/// How much of its speed the string keeps, turned round, where it meets a fret. With none
/// the rattle is over in a period or two and the note is left dull; with all of it the frets
/// take nothing, and a hard slap goes on rattling long after a real one has rung clear.
const BOUNCE: f32 = 0.25;

/// Mass of the thumb as the string feels it, in kilograms: its last joint, which the flesh
/// and the joint behind it let go on ahead of the hand.
const THUMB_MASS: f32 = 0.004;
/// Speed of the thumb at full velocity, in metres per second.
const THUMB_SPEED: f32 = 3.5;
/// How long the softest and the hardest thumb would stay on something that did not give, in
/// seconds: the pad of the thumb, and the bone at the side of its joint.
const SOFTEST_THUMB: f32 = 2.5e-3;
const HARDEST_THUMB: f32 = 0.8e-3;
/// Damping ratio of the thumb's flesh, which gives back about a third of the speed it met
/// the string with.
const FLESH: f32 = 0.35;
/// Longest the thumb stays down, in seconds. A player turns the wrist back whether or not
/// the string has thrown the thumb off.
const LONGEST: f32 = 0.02;

/// How hard a popping finger pulls the string before it slips, against a plucking one at the
/// same velocity: a little less. What makes a pop is where the string is pulled, not how far.
const PULL: f32 = 0.9;

/// [`PULL`], as a calibration run has it.
pub(super) fn pull() -> f32 {
    dial(dials::SLAP_PULL, PULL)
}

/// The pitch from which the slap technique pops a note instead of thumbing it: E2, an octave
/// over the lowest string. A slapped line is roots under the thumb and their octaves under
/// the finger, and every root on the two low strings has its octave at or above this.
const POP_FROM: f32 = 40.0;

/// How much of its clearance over a fret a string must keep to count as clear of it.
const NEAR: f32 = 0.5;
/// Periods of the note a string must stay clear of its frets before it is left alone.
const CALM_PERIODS: f32 = 3.0;

/// How a bass is played.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Technique {
    /// With the fingers or a pick, along the board.
    #[default]
    Pluck,
    /// Slapped: low notes under the thumb, the ones from E2 up popped.
    Slap,
    /// Every note under the thumb.
    Thumb,
    /// Every note popped.
    Pop,
    /// Every note a harmonic.
    Harmonics,
}

impl Technique {
    /// Choice labels in parameter-value order.
    pub(crate) const CHOICES: &'static [Cow<'static, str>] = &[
        Cow::Borrowed("Pluck"),
        Cow::Borrowed("Slap"),
        Cow::Borrowed("Thumb"),
        Cow::Borrowed("Pop"),
        Cow::Borrowed("Harmonics"),
    ];

    /// The technique a choice parameter's value names, rounding to the nearest step.
    pub(crate) fn from_value(value: f32) -> Self {
        match finite_or(value, 0.0).round() as i32 {
            i32::MIN..=0 => Self::Pluck,
            1 => Self::Slap,
            2 => Self::Thumb,
            3 => Self::Pop,
            _ => Self::Harmonics,
        }
    }

    /// What the hand does to a note at `pitch`.
    pub(crate) fn stroke(self, pitch: f32) -> Stroke {
        match self {
            Self::Pluck => Stroke::Pluck,
            Self::Thumb => Stroke::Thumb,
            Self::Pop => Stroke::Pop,
            Self::Harmonics => Stroke::Harmonic,
            Self::Slap if pitch < POP_FROM => Stroke::Thumb,
            Self::Slap => Stroke::Pop,
        }
    }
}

/// What sets one note going.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Stroke {
    /// Held aside and let go, along the board.
    #[default]
    Pluck,
    /// Struck with the side of the thumb, toward the board.
    Thumb,
    /// Pulled away from the board and let go.
    Pop,
    /// Plucked with a finger of the other hand laid lightly on the middle of the string.
    Harmonic,
}

/// One note as the slapping hand meets it.
#[derive(Clone, Copy, Debug)]
pub(super) struct Blow {
    pub stroke: Stroke,
    /// Frequency of the note, in hertz.
    pub fundamental: f32,
    /// Sounding length of the open string over the note's: one on an open string.
    pub fret: f32,
    /// Where the thumb lands, along the sounding length from the bridge.
    pub contact: f32,
    /// How hard, above zero and at most one.
    pub velocity: f32,
    /// How hard a part of the thumb, from its pad at zero to the bone at one.
    pub hardness: f32,
    /// Frets the string is held to, from [`frets`].
    pub frets: usize,
    pub rate: f32,
}

/// A fret under the sounding string.
#[derive(Clone, Debug, Default)]
struct Fret {
    point: Point,
    /// Where it is, along the sounding length from the bridge.
    at: f32,
    /// How far under the resting string its top is, in metres.
    gap: f32,
}

/// The thumb on its way to the string, on it, or gone.
#[derive(Clone, Debug, Default)]
struct Thumb {
    point: Point,
    at: f32,
    /// Where its surface is, in metres from the string's rest line, and how fast it moves:
    /// both negative toward the board.
    place: f32,
    speed: f32,
    mass: f32,
    /// Stiffness and loss of its flesh, in newtons per metre and per metre a second.
    stiffness: f32,
    loss: f32,
    /// Seconds since it was thrown.
    elapsed: f32,
    met: bool,
    down: bool,
}

/// The slapping hand and the frets, for one voice.
#[derive(Clone, Debug, Default)]
pub(super) struct Slap {
    frets: Vec<Fret>,
    count: usize,
    thumb: Thumb,
    armed: bool,
    /// Metres, and metres a second, to one unit of a [`Point`]'s displacement and velocity.
    metres: f32,
    /// Mass of each of the string's modes, in kilograms.
    mass: f32,
    step: f32,
    bounce: f32,
    /// The least clearance the string has left over any fret since the last tick, as a share
    /// of that fret's own.
    nearest: f32,
    /// Seconds the string has stayed clear of every fret, and how many make it calm.
    clear: f32,
    calm: f32,
}

impl Slap {
    /// Allocates the frets and the thumb for a pool of `capacity` phasors. The only method
    /// that allocates.
    pub(super) fn prepare(&mut self, capacity: usize) {
        self.frets = vec![Fret::default(); FRETS as usize + 1];
        for fret in &mut self.frets {
            fret.point.prepare(capacity);
        }
        self.thumb.point.prepare(capacity);
        self.rest();
    }

    /// Takes the hand away and stops watching the frets.
    pub(super) fn rest(&mut self) {
        self.armed = false;
        self.thumb.down = false;
        self.count = 0;
    }

    /// Whether the string is being held to its frets, and must be rung through
    /// [`Slap::ring`].
    pub(super) fn is_armed(&self) -> bool {
        self.armed
    }

    /// Whether the thumb is still on its way or on the string: a note that has not moved yet
    /// has not died away.
    pub(super) fn is_striking(&self) -> bool {
        self.armed && self.thumb.down
    }

    /// Lays the frets out under a string and throws the thumb at it. Each of the string's
    /// partials is then described with [`Slap::partial`].
    pub(super) fn begin(&mut self, blow: &Blow) {
        if self.frets.is_empty() || !matches!(blow.stroke, Stroke::Thumb | Stroke::Pop) {
            self.rest();
            return;
        }
        let fret = blow.fret.max(1.0);
        let open = blow.fundamental / fret;
        // A pluck of force F at a fraction p of a string of length L and tension T holds it
        // aside by F L p (1 - p) / T; the same pluck is an amplitude of one here, whose
        // displacement a point reads as π p (1 - p) / (4 f), f being the note's frequency.
        let pluck = dial(dials::SLAP_PLUCK, FULL_PLUCK);
        self.metres = 4.0 * open * pluck * SCALE / (PI * TENSION);
        // Mass per length from the tension and the open string's pitch; half the sounding
        // string's mass is what each of its modes carries.
        let density = TENSION / (2.0 * SCALE * open).powi(2);
        self.mass = density * SCALE / fret * 0.5;
        self.step = 1.0 / blow.rate;
        // Which fret the finger is at, and the ones spread from the neck's end down to it.
        let stopped = 12.0 * fret.log2();
        let far_end = if stopped < 0.5 { NUT } else { 0.0 };
        let saddle = dial(dials::SLAP_SADDLE, SADDLE);
        let free = (FRETS - stopped).floor().max(0.0) as usize;
        self.count = free.min(blow.frets).min(self.frets.len());
        for (index, slot) in self.frets.iter_mut().take(self.count).enumerate() {
            let below = if self.count > 1 {
                (index * (free - 1)) as f32 / (self.count - 1) as f32
            } else {
                0.0
            };
            let number = FRETS - below.round();
            slot.at = (-(number - stopped) / 12.0).exp2();
            slot.gap = saddle * (1.0 - slot.at) + far_end * slot.at;
            slot.point.clear();
        }
        // A thumb that lands over the neck has the board under it: the string can go no
        // further down there than the frets either side of the thumb let it.
        let over_the_neck = self.count > 0 && blow.contact >= self.frets[0].at;
        if blow.stroke == Stroke::Thumb && over_the_neck && self.count < self.frets.len() {
            let slot = &mut self.frets[self.count];
            slot.at = blow.contact;
            slot.gap = saddle * (1.0 - slot.at) + far_end * slot.at;
            slot.point.clear();
            self.count += 1;
        }
        let hardness = blow.hardness.clamp(0.0, 1.0);
        let dwell = SOFTEST_THUMB * (HARDEST_THUMB / SOFTEST_THUMB).powf(hardness);
        let turn = PI / dwell;
        self.thumb.mass = dial(dials::SLAP_THUMB_MASS, THUMB_MASS);
        self.thumb.at = blow.contact;
        self.thumb.place = 0.0;
        self.thumb.speed = -dial(dials::SLAP_THUMB_SPEED, THUMB_SPEED) * blow.velocity;
        self.thumb.stiffness = self.thumb.mass * turn * turn;
        self.thumb.loss = 2.0 * dial(dials::SLAP_FLESH, FLESH) * self.thumb.mass * turn;
        self.bounce = dial(dials::SLAP_BOUNCE, BOUNCE);
        self.thumb.elapsed = 0.0;
        self.thumb.met = false;
        self.thumb.down = blow.stroke == Stroke::Thumb;
        self.thumb.point.clear();
        self.armed = true;
        self.nearest = 1.0;
        self.clear = 0.0;
        self.calm = CALM_PERIODS / blow.fundamental.max(1.0);
    }

    /// Says that partial `order` of the string is phasor `index`, turning at `hz`.
    pub(super) fn partial(&mut self, index: usize, order: f32, hz: f32) {
        if !self.armed {
            return;
        }
        for fret in self.frets.iter_mut().take(self.count) {
            fret.point.set(index, (PI * order * fret.at).sin(), hz);
        }
        if self.thumb.down {
            let thumb = &mut self.thumb;
            thumb.point.set(index, (PI * order * thumb.at).sin(), hz);
        }
    }

    /// Advances `rotors` through `out.len()` samples as [`Rotors::ring`] does, with the thumb
    /// on the string and the frets under it.
    pub(super) fn ring(&mut self, rotors: &mut Rotors, out: &mut [f32]) {
        let (metres, mass, step, bounce) = (self.metres, self.mass, self.step, self.bounce);
        for sample in out.iter_mut() {
            rotors.ring(std::slice::from_mut(sample));
            if self.thumb.down {
                self.thumb.press(rotors, metres, mass, step);
            }
            for fret in self.frets.iter().take(self.count) {
                let clearance = rotors.displacement(&fret.point) * metres + fret.gap;
                self.nearest = self.nearest.min(clearance / fret.gap);
                if clearance >= 0.0 {
                    continue;
                }
                let speed = rotors.velocity(&fret.point);
                if speed < 0.0 {
                    let stop = -(1.0 + bounce) * speed / fret.point.mobility().max(1.0e-6);
                    rotors.knock(&fret.point, stop);
                }
            }
        }
    }

    /// Notes, once a control tick, whether the string still comes near its frets, and stops
    /// watching them once it has stayed clear for a few periods. `seconds` is the tick's
    /// length.
    pub(super) fn tick(&mut self, seconds: f32) {
        if !self.armed {
            return;
        }
        if self.thumb.down || self.nearest < NEAR {
            self.clear = 0.0;
        } else {
            self.clear += seconds;
        }
        self.nearest = 1.0;
        if self.clear >= self.calm {
            self.armed = false;
        }
    }
}

impl Thumb {
    /// One sample of the thumb against the string under it.
    fn press(&mut self, rotors: &mut Rotors, metres: f32, mass: f32, step: f32) {
        let string = rotors.displacement(&self.point) * metres;
        let squeeze = string - self.place;
        if squeeze > 0.0 {
            self.met = true;
            let closing = rotors.velocity(&self.point) * metres - self.speed;
            // The flesh's force depends on how fast it is being squeezed, and that on the
            // force, through how easily the string and the thumb each give way. The two are
            // linear in the force and solved together, so a stiff thumb on a light string
            // is as stable as a soft one.
            let gives = self.point.mobility() / mass + 1.0 / self.mass;
            let rising = self.stiffness * step + self.loss;
            let force = ((self.stiffness * squeeze + rising * closing)
                / (1.0 + rising * step * gives))
                .max(0.0);
            rotors.knock(&self.point, -force * step / (mass * metres));
            self.speed += force * step / self.mass;
        }
        self.place += self.speed * step;
        self.elapsed += step;
        let thrown_off = self.met && squeeze < 0.0 && self.speed > 0.0;
        if thrown_off || self.elapsed > LONGEST {
            self.down = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::rotors::{Rotor, pole};
    use super::*;

    #[test]
    fn the_slap_technique_thumbs_the_roots_and_pops_their_octaves() {
        assert_eq!(Technique::Pluck.stroke(28.0), Stroke::Pluck);
        assert_eq!(Technique::Pluck.stroke(52.0), Stroke::Pluck);
        // Every root on the two low strings, and the octave over each.
        for root in 28..40 {
            assert_eq!(Technique::Slap.stroke(root as f32), Stroke::Thumb);
            assert_eq!(Technique::Slap.stroke((root + 12) as f32), Stroke::Pop);
        }
        assert_eq!(Technique::Thumb.stroke(55.0), Stroke::Thumb);
        assert_eq!(Technique::Pop.stroke(28.0), Stroke::Pop);
        for (value, technique) in [
            (0.0, Technique::Pluck),
            (1.2, Technique::Slap),
            (2.0, Technique::Thumb),
            (3.0, Technique::Pop),
            (4.0, Technique::Harmonics),
            (9.0, Technique::Harmonics),
            (f32::NAN, Technique::Pluck),
        ] {
            assert_eq!(Technique::from_value(value), technique);
        }
        assert_eq!(Technique::Harmonics.stroke(40.0), Stroke::Harmonic);
        assert_eq!(Technique::CHOICES.len(), 5);
    }

    fn blow(stroke: Stroke, fret: f32, frets: usize) -> Blow {
        Blow {
            stroke,
            fundamental: 41.2 * fret,
            fret,
            contact: 0.32 * fret,
            velocity: 0.8,
            hardness: 0.5,
            frets,
            rate: 48_000.0,
        }
    }

    #[test]
    fn the_frets_lie_where_a_maker_puts_them_and_clear_the_string_by_its_action() {
        let mut slap = Slap::default();
        slap.prepare(64);
        slap.begin(&blow(Stroke::Pop, 1.0, HELD_FRETS));
        assert!(slap.is_armed() && !slap.is_striking());
        assert_eq!(slap.count, HELD_FRETS);
        // The twentieth fret of an open string, and the first.
        let last = &slap.frets[0];
        assert!((last.at - 0.315).abs() < 1.0e-3, "{}", last.at);
        assert!((last.gap - 2.83e-3).abs() < 2.0e-5, "{} m", last.gap);
        let first = &slap.frets[HELD_FRETS - 1];
        assert!((first.at - 0.944).abs() < 1.0e-3, "{}", first.at);
        // Stopped at the twelfth fret there are eight left, and the last is the same fret:
        // further along a string half as long. A thumb that lands over the neck has the
        // board under it as well.
        slap.begin(&blow(Stroke::Thumb, 2.0, HELD_FRETS));
        assert!(slap.is_striking());
        assert_eq!(slap.count, HELD_FRETS + 1);
        assert!((slap.frets[0].at - 0.63).abs() < 1.0e-3);
        assert_eq!(slap.frets[HELD_FRETS].at, 0.64);
        assert!(slap.frets.iter().all(|fret| fret.at < 1.0));
        // One that lands past the end of the neck has nothing under it but the string.
        let mut past = blow(Stroke::Thumb, 1.0, HELD_FRETS);
        past.contact = 0.25;
        slap.begin(&past);
        assert_eq!(slap.count, HELD_FRETS);
        // Past the end of the neck there is nothing left to hit.
        slap.begin(&blow(Stroke::Pop, 3.5, HELD_FRETS));
        assert_eq!(slap.count, 0);
        // A plucked note has no business here at all.
        slap.begin(&blow(Stroke::Pluck, 1.0, HELD_FRETS));
        assert!(!slap.is_armed());
    }

    const PARTIALS: usize = 96;

    /// An open low E that loses nothing, with its frets under it: at rest for a thumb, or
    /// held `amplitude` aside at three tenths of its length for a pop.
    fn string(stroke: Stroke, amplitude: f32, velocity: f32) -> (Rotors, Slap) {
        let hz = 41.2_f32;
        let mut rotors = Rotors::default();
        rotors.prepare(PARTIALS);
        let mut slap = Slap::default();
        slap.prepare(PARTIALS);
        slap.begin(&Blow {
            velocity,
            ..blow(stroke, 1.0, HELD_FRETS)
        });
        for n in 0..PARTIALS {
            let order = (n + 1) as f32;
            let (cosine, sine) = pole(hz * order, 0.0, 48_000.0);
            rotors.set(
                n,
                Rotor {
                    cosine,
                    sine,
                    listen_im: 1.0,
                    ..Rotor::default()
                },
            );
            slap.partial(n, order, hz * order);
        }
        rotors.set_len(PARTIALS);
        for n in 0..PARTIALS {
            let order = (n + 1) as f32;
            rotors.place(n, 0.0, amplitude * (PI * order * 0.3).sin() / order);
        }
        (rotors, slap)
    }

    /// Energy of the string, in joules.
    fn energy(rotors: &Rotors, slap: &Slap) -> f32 {
        let moving: f32 = (0..PARTIALS)
            .map(|n| {
                let (re, im) = rotors.at(n);
                re * re + im * im
            })
            .sum();
        0.5 * slap.mass * slap.metres * slap.metres * moving
    }

    /// How far under the top of any fret the string is, in metres.
    fn sunk(rotors: &Rotors, slap: &Slap) -> f32 {
        slap.frets
            .iter()
            .take(slap.count)
            .map(|fret| -(rotors.displacement(&fret.point) * slap.metres + fret.gap))
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_popped_string_is_stopped_by_its_frets_which_only_ever_take_energy_from_it() {
        let (mut rotors, mut slap) = string(Stroke::Pop, 1.0, 1.0);
        let pulled = energy(&rotors, &slap);
        let (mut before, mut deepest, mut stops) = (pulled, 0.0_f32, 0);
        let mut out = [0.0_f32];
        // Four periods of the note.
        for _ in 0..4_660 {
            slap.ring(&mut rotors, &mut out);
            let now = energy(&rotors, &slap);
            assert!(now <= before * 1.000_01, "the frets gave the string energy");
            if now < before * 0.999_9 {
                stops += 1;
            }
            before = now;
            deepest = deepest.max(sunk(&rotors, &slap));
        }
        assert!(stops > 100, "the string met its frets {stops} times");
        // What is left is a string that swings about as far as its frets let it.
        let kept = before / pulled;
        assert!((0.15..0.7).contains(&kept), "kept {kept} of its energy");
        // A fret is not a wall to a string of ninety-six partials, which cannot bend more
        // sharply than its shortest one; it gives by a fraction of the action.
        assert!(deepest < 0.1 * SADDLE, "sank {deepest} m into a fret");
    }

    #[test]
    fn a_string_that_stays_clear_of_its_frets_is_left_alone() {
        // Pulled a few tenths of a millimetre aside, the string comes back down a third of
        // the way to the frets under its middle.
        let (mut rotors, mut slap) = string(Stroke::Pop, 0.05, 0.05);
        let pulled = energy(&rotors, &slap);
        let mut out = [0.0_f32; 32];
        let mut ticks = 0;
        while slap.is_armed() {
            slap.ring(&mut rotors, &mut out);
            slap.tick(32.0 / 48_000.0);
            ticks += 1;
            assert!(ticks < 1_000, "the frets were never let go of");
        }
        assert!((energy(&rotors, &slap) / pulled - 1.0).abs() < 1.0e-3);
        // Three periods of 41.2 Hz, in ticks of two thirds of a millisecond.
        assert!((105..=115).contains(&ticks), "{ticks} ticks");
    }

    #[test]
    fn the_thumb_knocks_the_string_onto_the_fret_and_is_thrown_off() {
        let (mut rotors, mut slap) = string(Stroke::Thumb, 0.0, 1.0);
        assert!(slap.is_striking());
        assert_eq!(energy(&rotors, &slap), 0.0);
        let thrown = 0.5 * THUMB_MASS * THUMB_SPEED * THUMB_SPEED;
        let mut out = [0.0_f32];
        let (mut down, mut reached) = (0, false);
        while slap.is_striking() {
            slap.ring(&mut rotors, &mut out);
            down += 1;
            reached |= sunk(&rotors, &slap) > 0.0;
            // Between them the thumb and the string never have more than the thumb came with.
            let thumb = 0.5 * THUMB_MASS * slap.thumb.speed * slap.thumb.speed;
            let both = thumb + energy(&rotors, &slap);
            assert!(both <= thrown * 1.001, "{both} J of {thrown} J");
        }
        let milliseconds = down as f32 / 48.0;
        assert!((2.0..8.0).contains(&milliseconds), "down {milliseconds} ms");
        assert!(slap.thumb.speed > 0.0, "the thumb was not thrown back");
        assert!(reached, "the string never came down as far as a fret");
        let given = energy(&rotors, &slap) / thrown;
        assert!((0.4..0.95).contains(&given), "the string got {given}");
    }

    #[test]
    fn a_full_pluck_holds_an_open_string_about_four_millimetres_aside() {
        // The triangle a pluck of amplitude one at a sixth of the string leaves, read back
        // at the same place through a point, in metres.
        let (hz, at) = (41.2_f32, 1.0 / 6.0);
        let mut rotors = Rotors::default();
        rotors.prepare(64);
        let mut slap = Slap::default();
        slap.prepare(64);
        let mut struck = blow(Stroke::Thumb, 1.0, 1);
        struck.contact = at;
        slap.begin(&struck);
        for n in 0..64 {
            let order = (n + 1) as f32;
            slap.partial(n, order, hz * order);
            rotors.place(n, 0.0, (PI * order * at).sin() / order);
        }
        rotors.set_len(64);
        let aside = rotors.displacement(&slap.thumb.point) * slap.metres;
        let expected = FULL_PLUCK * SCALE * at * (1.0 - at) / TENSION;
        assert!((aside / expected - 1.0).abs() < 0.03, "{aside} m");
        assert!((3.5e-3..5.0e-3).contains(&aside), "{aside} m");
    }
}
