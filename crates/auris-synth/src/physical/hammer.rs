//! A hammer thrown at a string, and the force between them for as long as they touch.
//!
//! A piano hammer is a mass on a nonlinear spring. Its felt is soft where it first meets the
//! string and hard deeper in, so the force rises as a power of how far it is compressed —
//! between the square and the cube. A harder blow drives further into the stiff part, the
//! contact ends sooner, and a shorter pulse holds more high partials: that is why a piano gets
//! brighter as it gets louder, and it is not something a level control can imitate.
//!
//! While they touch, the string is not a spring but a pair of waves leaving the striking point,
//! and the hammer feels it as a dashpot: velocity in proportion to force. The wave sent toward
//! the near end comes back inverted a fraction of a millisecond later and pushes the hammer
//! away, sometimes for a moment and then back again — the two or three humps in a bass note's
//! force pulse. So the string is carried here as what the hammer can feel of it: two lossless
//! delay lines, the two ways from the striking point, for the few milliseconds the contact
//! lasts. What the string then *does* with the force is the business of its partials.
//!
//! The contact is stiff, so it is stepped several times a sample. Hammer mass, felt stiffness
//! and exponent across the keyboard are the published measurements of Chaigne and Askenfelt
//! (1994) for a grand at C2, C4 and C7, joined geometrically between and beyond. They are one
//! string's share of a hammer — a third of its mass and of its felt, against one string of
//! three — and whoever throws one at a whole unison gives it that many times as much of both.

use auris_core::motion::MOTION_SCOPE;

use super::BLOW_WINDOW;

/// Longest way to an end of the string and back that is followed, in steps. A wave that would
/// take longer than this returns after the hammer has left, and is not waited for.
const REACH: usize = 2048;
/// Steps a second the contact is solved at, at least.
const STEP_RATE: f32 = 176_400.0;
/// Most steps to one output sample.
const MOST_STEPS: usize = 8;
/// Longest a hammer may stay near the string, in seconds.
const LONGEST: f32 = 0.014;
/// How far the hammer falls back before it is taken to have left for good, in metres.
const CLEAR: f32 = 2.0e-4;

/// What one blow needs to know about the hammer and the string it meets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Blow {
    /// Speed of the hammer as it reaches the string, in metres per second.
    pub speed: f32,
    /// Mass of the hammer, in kilograms.
    pub mass: f32,
    /// Stiffness of the felt: force in newtons at one metre of compression, were it to get there.
    pub stiffness: f32,
    /// Exponent of the felt's force law.
    pub exponent: f32,
    /// How much more force the felt gives while being compressed than while recovering, per
    /// metre per second of compression: its hysteresis.
    pub loss: f32,
    /// Wave impedance of everything the hammer strikes at once, in kilograms per second: the
    /// square root of tension times mass per length, times the number of strings.
    pub impedance: f32,
    /// Period of the string, in seconds.
    pub period: f32,
    /// Where along the string the hammer strikes, from the near end, as a fraction.
    pub position: f32,
}

/// One hammer in flight or at rest.
#[derive(Clone, Debug, Default)]
pub(crate) struct Hammer {
    blow: Option<Blow>,
    /// Hammer displacement past the string's rest line, and its velocity.
    place: f32,
    speed: f32,
    /// String displacement at the striking point.
    string: f32,
    /// Velocity waves on their way to the near end and to the bridge.
    near: Vec<f32>,
    far: Vec<f32>,
    cursor: usize,
    /// Round trips to each end, in steps.
    near_steps: f32,
    far_steps: f32,
    steps: usize,
    step: f32,
    elapsed: f32,
    /// Whether the hammer and string have touched yet.
    met: bool,
    /// Compression of the felt, for a frontend to draw.
    squeeze: f32,
    /// Largest force of this blow so far.
    peak: f32,
    /// The force over the blow for a frontend to draw, the largest in each of its windows;
    /// output samples since the hammer was thrown; and how many of them one window holds.
    trace: Vec<f32>,
    traced: usize,
    window: f32,
}

impl Hammer {
    /// Allocates the two delay lines.
    pub(crate) fn prepare(&mut self) {
        self.near = vec![0.0; REACH];
        self.far = vec![0.0; REACH];
        self.trace = vec![0.0; MOTION_SCOPE];
        self.rest();
    }

    /// Puts the hammer back on its rest.
    pub(crate) fn rest(&mut self) {
        self.blow = None;
        self.squeeze = 0.0;
    }

    /// Throws the hammer. `rate` is the output sample rate.
    pub(crate) fn throw(&mut self, blow: Blow, rate: f32) {
        self.steps = ((STEP_RATE / rate).ceil() as usize).clamp(1, MOST_STEPS);
        let step_rate = rate * self.steps as f32;
        self.step = 1.0 / step_rate;
        let round_trip = blow.period * step_rate;
        self.near_steps = round_trip * blow.position.clamp(0.01, 0.5);
        self.far_steps = round_trip - self.near_steps;
        self.near.fill(0.0);
        self.far.fill(0.0);
        self.cursor = 0;
        self.place = 0.0;
        self.speed = blow.speed.max(0.0);
        self.string = 0.0;
        self.elapsed = 0.0;
        self.met = false;
        self.squeeze = 0.0;
        self.peak = 0.0;
        self.trace.fill(0.0);
        self.traced = 0;
        self.window = (BLOW_WINDOW * rate / MOTION_SCOPE as f32).max(1.0);
        self.blow = Some(blow);
    }

    /// The force over the last blow, in newtons, from the moment the hammer was thrown.
    pub(crate) fn trace(&self) -> &[f32] {
        &self.trace
    }

    /// Whether the hammer is still at the string.
    pub(crate) fn is_striking(&self) -> bool {
        self.blow.is_some()
    }

    /// How far the felt is compressed, relative to the deepest it has been this blow: one at
    /// the height of the blow, zero once the hammer has left.
    pub(crate) fn contact(&self) -> f32 {
        self.squeeze
    }

    fn arriving(line: &[f32], cursor: usize, steps: f32) -> f32 {
        // A wave that has further to go than the line is long has not come back yet.
        if steps >= (REACH - 2) as f32 || line.len() < REACH {
            return 0.0;
        }
        let whole = steps as usize;
        let part = steps - whole as f32;
        let newer = line[(cursor + REACH - whole) % REACH];
        let older = line[(cursor + REACH - whole - 1) % REACH];
        newer + part * (older - newer)
    }

    /// Fills `force` with the force on the string, in newtons, one value per output sample.
    /// Zero from the sample after the hammer leaves.
    pub(crate) fn strike(&mut self, force: &mut [f32]) {
        let Some(blow) = self.blow else {
            force.fill(0.0);
            return;
        };
        let admittance = 0.5 / blow.impedance.max(1.0e-3);
        for sample in force.iter_mut() {
            if self.blow.is_none() {
                *sample = 0.0;
                continue;
            }
            let mut total = 0.0;
            for _ in 0..self.steps {
                // Each end sends back, inverted, what was sent toward it.
                let from_near = -Self::arriving(&self.near, self.cursor, self.near_steps);
                let from_far = -Self::arriving(&self.far, self.cursor, self.far_steps);
                let carried = from_near + from_far;
                let squeeze = self.place - self.string;
                let pushing = if squeeze > 0.0 {
                    self.met = true;
                    let spring = blow.stiffness * squeeze.powf(blow.exponent);
                    // The felt's own loss makes the force depend on how fast it is being
                    // compressed, which in turn depends on the force through the string's
                    // dashpot. The two are linear in the force, and solved together.
                    let closing = self.speed - carried;
                    (spring * (1.0 + blow.loss * closing) / (1.0 + spring * blow.loss * admittance))
                        .max(0.0)
                } else {
                    0.0
                };
                let string_speed = carried + pushing * admittance;
                self.speed -= pushing / blow.mass * self.step;
                self.place += self.speed * self.step;
                self.string += string_speed * self.step;
                // What leaves toward each end is what arrived from the other, plus the push.
                let launched = pushing * admittance;
                self.near[self.cursor] = from_far + launched;
                self.far[self.cursor] = from_near + launched;
                self.cursor = (self.cursor + 1) % REACH;
                self.elapsed += self.step;
                self.peak = self.peak.max(pushing);
                self.squeeze = if self.peak > 0.0 {
                    pushing / self.peak
                } else {
                    0.0
                };
                total += pushing;
                let gone = self.met && self.speed < 0.0 && squeeze < -CLEAR;
                if gone || self.elapsed > LONGEST {
                    self.blow = None;
                    self.squeeze = 0.0;
                    break;
                }
            }
            *sample = total / self.steps as f32;
            let window = (self.traced as f32 / self.window) as usize;
            if let Some(slot) = self.trace.get_mut(window) {
                *slot = slot.max(*sample);
            }
            self.traced += 1;
        }
    }
}

/// A value joined geometrically between `(key, value)` corners, and held beyond the ends.
pub(crate) fn across_keys(corners: &[(f32, f32)], key: f32) -> f32 {
    let Some(first) = corners.first() else {
        return 0.0;
    };
    if key <= first.0 {
        return first.1;
    }
    for pair in corners.windows(2) {
        let (low, high) = (pair[0], pair[1]);
        if key <= high.0 {
            let t = (key - low.0) / (high.0 - low.0);
            return low.1 * (high.1 / low.1).powf(t);
        }
    }
    corners.last().map_or(0.0, |corner| corner.1)
}

/// A grand piano's hammers and strings across the keyboard, by MIDI key.
pub(crate) mod grand {
    /// Hammer mass for each string it strikes, in kilograms.
    pub(crate) const MASS: &[(f32, f32)] = &[
        (21.0, 6.0e-3),
        (36.0, 4.9e-3),
        (60.0, 2.97e-3),
        (96.0, 2.2e-3),
        (108.0, 1.2e-3),
    ];
    /// Felt stiffness for each string it strikes.
    pub(crate) const STIFFNESS: &[(f32, f32)] = &[
        (21.0, 1.5e8),
        (36.0, 4.0e8),
        (60.0, 4.5e9),
        (96.0, 1.0e12),
        (108.0, 6.0e12),
    ];
    /// Exponent of the felt's force law.
    pub(crate) const EXPONENT: &[(f32, f32)] = &[
        (21.0, 2.2),
        (36.0, 2.3),
        (60.0, 2.5),
        (96.0, 3.0),
        (108.0, 3.1),
    ];
    /// Wave impedance of one string, in kilograms per second.
    pub(crate) const IMPEDANCE: &[(f32, f32)] = &[
        (21.0, 12.0),
        (28.0, 7.0),
        (36.0, 3.72),
        (48.0, 2.6),
        (60.0, 2.06),
        (96.0, 1.97),
        (108.0, 1.9),
    ];
    /// How far along the string the hammers strike, relative to the bass: a seventh or an
    /// eighth of the way through most of the compass, and nearer the end at the top.
    pub(crate) const STRIKE: &[(f32, f32)] =
        &[(21.0, 1.0), (72.0, 1.0), (96.0, 0.55), (108.0, 0.5)];
    /// Inharmonicity coefficient, drawn through the stiffness read off the partials of a
    /// recorded Steinway B at twenty-six keys: `tools/eval/piano_stretch.py` reads them, and
    /// `docs/piano-stretch-tuning.md` lists them. The lowest strings are the stiffest in the
    /// bass, those from G1 to C3 are all much alike, and from there the stiffness climbs to
    /// the top. Above A6 the recording shows no second partial to read a string by, and the
    /// curve goes on as it rose through the two octaves below.
    pub(crate) const INHARMONICITY: &[(f32, f32)] = &[
        (21.0, 2.6e-4),
        (24.0, 2.47e-4),
        (28.0, 1.38e-4),
        (31.0, 1.2e-4),
        (48.0, 1.2e-4),
        (60.0, 3.15e-4),
        (72.0, 1.03e-3),
        (84.0, 2.2e-3),
        (96.0, 5.0e-3),
        (108.0, 1.1e-2),
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn blow(key: f32, speed: f32, strings: f32) -> Blow {
        Blow {
            speed,
            mass: across_keys(grand::MASS, key) * strings,
            stiffness: across_keys(grand::STIFFNESS, key) * strings,
            exponent: across_keys(grand::EXPONENT, key),
            loss: 0.1,
            impedance: across_keys(grand::IMPEDANCE, key) * strings,
            period: 1.0 / (440.0 * ((key - 69.0) / 12.0).exp2()),
            position: 0.12 * across_keys(grand::STRIKE, key),
        }
    }

    fn pulse(blow: Blow) -> Vec<f32> {
        let mut hammer = Hammer::default();
        hammer.prepare();
        hammer.throw(blow, RATE);
        let mut force = vec![0.0; 960];
        hammer.strike(&mut force);
        assert!(!hammer.is_striking(), "the hammer never left");
        force
    }

    /// Milliseconds from the first to the last sample with any force in it.
    fn duration(force: &[f32]) -> f32 {
        let first = force.iter().position(|sample| *sample > 0.0).unwrap();
        let last = force.iter().rposition(|sample| *sample > 0.0).unwrap();
        (last - first + 1) as f32 / RATE * 1_000.0
    }

    #[test]
    fn middle_c_is_struck_for_a_couple_of_milliseconds_and_the_treble_for_less() {
        let middle = pulse(blow(60.0, 2.0, 3.0));
        let treble = pulse(blow(96.0, 2.0, 3.0));
        let bass = pulse(blow(36.0, 2.0, 2.0));
        assert!(
            (1.2..3.2).contains(&duration(&middle)),
            "{} ms",
            duration(&middle)
        );
        assert!(duration(&treble) < 1.0, "{} ms", duration(&treble));
        assert!(
            duration(&bass) > duration(&middle),
            "{} ms",
            duration(&bass)
        );
        assert!(
            middle
                .iter()
                .all(|sample| sample.is_finite() && *sample >= 0.0)
        );
    }

    #[test]
    fn a_harder_blow_is_shorter_and_more_than_proportionally_stronger() {
        let soft = pulse(blow(60.0, 1.0, 3.0));
        let hard = pulse(blow(60.0, 4.0, 3.0));
        assert!(duration(&hard) < duration(&soft) * 0.92);
        let peak = |force: &[f32]| force.iter().fold(0.0_f32, |a, b| a.max(*b));
        let ratio = peak(&hard) / peak(&soft);
        assert!(
            ratio > 4.4,
            "four times the speed gave {ratio} times the force"
        );
        // Forces of the size the measurements report: tens of newtons at forte.
        assert!((15.0..120.0).contains(&peak(&hard)), "{} N", peak(&hard));
    }

    #[test]
    fn the_hammer_gives_up_about_its_momentum_change_and_no_more_than_its_energy() {
        let blow = blow(60.0, 2.0, 3.0);
        let force = pulse(blow);
        let impulse: f32 = force.iter().sum::<f32>() / RATE;
        // It arrives at 2 m/s and leaves slower, backwards: between one and two momenta.
        let momentum = blow.mass * blow.speed;
        assert!(
            (momentum..2.0 * momentum).contains(&impulse),
            "impulse {impulse}, momentum {momentum}"
        );
    }

    #[test]
    fn three_strings_are_struck_as_one_is_by_a_third_of_the_hammer() {
        // The published hammers are one string's share. A blow on the whole unison lasts as
        // long as that share's blow on one string, with three times the force.
        let one = pulse(blow(60.0, 2.0, 1.0));
        let three = pulse(blow(60.0, 2.0, 3.0));
        assert!((duration(&one) - duration(&three)).abs() < 0.05);
        let total = |force: &[f32]| force.iter().sum::<f32>();
        assert!((total(&three) / total(&one) - 3.0).abs() < 0.03);
    }

    #[test]
    fn the_wave_from_the_near_end_dents_a_bass_blow() {
        // On C2 the reflection from the agraffe is back in under a millisecond, well inside
        // the contact, and the force shows it: more than one hump.
        let force = pulse(blow(36.0, 3.0, 2.0));
        let humps = force
            .windows(3)
            .filter(|w| w[1] > w[0] && w[1] >= w[2] && w[1] > 0.5)
            .count();
        assert!(humps >= 2, "{humps} hump(s)");
    }

    #[test]
    fn the_blow_is_the_same_at_every_sample_rate() {
        let reference: f32 = pulse(blow(60.0, 2.0, 3.0)).iter().sum::<f32>() / RATE;
        for rate in [44_100.0_f32, 96_000.0, 192_000.0] {
            let mut hammer = Hammer::default();
            hammer.prepare();
            hammer.throw(blow(60.0, 2.0, 3.0), rate);
            let mut force = vec![0.0; (rate * 0.02) as usize];
            hammer.strike(&mut force);
            let impulse: f32 = force.iter().sum::<f32>() / rate;
            assert!(
                (impulse / reference - 1.0).abs() < 0.03,
                "{rate}: impulse {impulse} against {reference}"
            );
        }
    }

    #[test]
    fn values_are_joined_across_the_keyboard_and_held_past_its_ends() {
        assert_eq!(across_keys(grand::MASS, 0.0), 6.0e-3);
        assert_eq!(across_keys(grand::MASS, 127.0), 1.2e-3);
        let between = across_keys(grand::STIFFNESS, 48.0);
        assert!((between / (4.0e8_f32 * 4.5e9).sqrt() - 1.0).abs() < 1.0e-3);
    }
}
