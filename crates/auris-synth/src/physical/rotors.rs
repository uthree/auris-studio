//! A bank of damped rotating phasors: the one linear element every body here is made of.
//!
//! A string's partial, a bar's bending mode, a bell's hum and a soundboard's resonance are all
//! the same thing once the instrument is linear: a complex number that turns by a fixed angle
//! every sample and shrinks by a fixed ratio as it goes. Its real half is the mode's velocity and
//! its imaginary half its displacement, scaled so both have the same size. Everything that makes
//! one instrument different from another is in how the phasors are set going and how they are
//! listened to; advancing them is one loop, written once, here.
//!
//! The loop keeps the phasors in groups of [`LANES`] so that a compiler which may use vector
//! instructions turns each group into a handful of them. Unused lanes of the last group hold
//! zero and stay there.

use std::f32::consts::TAU;

/// Phasors advanced together.
pub(crate) const LANES: usize = 8;

/// Below this a group of phasors is taken to have stopped, and is put to exact rest before its
/// arithmetic can reach the denormal range.
const REST: f32 = 1.0e-18;

/// Nepers per decibel: a level falling `x` dB/s is an amplitude falling `x * NEPERS_PER_DB`
/// nepers a second.
pub(crate) const NEPERS_PER_DB: f32 = 0.115_129_25;

/// The amplitude decay rate, in nepers per second, of something that falls 60 dB in `seconds`.
pub(crate) fn decay_rate(seconds: f32) -> f32 {
    6.907_755 / seconds.max(1.0e-4)
}

/// One sample's turn and shrink for a mode at `hz` decaying at `nepers` a second: the real and
/// imaginary halves of its pole.
pub(crate) fn pole(hz: f32, nepers: f32, rate: f32) -> (f32, f32) {
    let radius = (-f64::from(nepers.max(0.0)) / f64::from(rate)).exp();
    let (sine, cosine) = (f64::from(TAU) * f64::from(hz) / f64::from(rate)).sin_cos();
    ((radius * cosine) as f32, (radius * sine) as f32)
}

/// How one phasor is set going and listened to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Rotor {
    /// Real half of the pole, from [`pole`].
    pub cosine: f32,
    /// Imaginary half of the pole.
    pub sine: f32,
    /// What a unit of the driving signal adds to the real half each sample.
    pub drive: f32,
    /// Weight of the real half in the output.
    pub listen_re: f32,
    /// Weight of the imaginary half in the output.
    pub listen_im: f32,
}

type Lane = [f32; LANES];

/// A fixed pool of phasors with one driving signal and one output.
#[derive(Clone, Debug, Default)]
pub(crate) struct Rotors {
    re: Vec<Lane>,
    im: Vec<Lane>,
    cosine: Vec<Lane>,
    sine: Vec<Lane>,
    drive: Vec<Lane>,
    listen_re: Vec<Lane>,
    listen_im: Vec<Lane>,
    /// Groups configured for the sounding note.
    groups: usize,
    /// Groups still moving; the rest have been put to rest and are skipped.
    live: usize,
}

impl Rotors {
    /// Allocates room for `capacity` phasors. The only method that allocates.
    pub(crate) fn prepare(&mut self, capacity: usize) {
        let groups = capacity.div_ceil(LANES).max(1);
        for lanes in [
            &mut self.re,
            &mut self.im,
            &mut self.cosine,
            &mut self.sine,
            &mut self.drive,
            &mut self.listen_re,
            &mut self.listen_im,
        ] {
            lanes.clear();
            lanes.resize(groups, [0.0; LANES]);
        }
        self.groups = 0;
        self.live = 0;
    }

    /// Phasors this pool has room for.
    pub(crate) fn capacity(&self) -> usize {
        self.re.len() * LANES
    }

    /// Stops every phasor and forgets how they were configured.
    pub(crate) fn clear(&mut self) {
        for lanes in [
            &mut self.re,
            &mut self.im,
            &mut self.cosine,
            &mut self.sine,
            &mut self.drive,
            &mut self.listen_re,
            &mut self.listen_im,
        ] {
            lanes.fill([0.0; LANES]);
        }
        self.groups = 0;
        self.live = 0;
    }

    /// Stops every phasor where it is, keeping the configuration.
    pub(crate) fn rest(&mut self) {
        self.re.fill([0.0; LANES]);
        self.im.fill([0.0; LANES]);
        self.live = self.groups;
    }

    /// Declares that phasors `0..count` are configured; the ones beyond are silenced.
    pub(crate) fn set_len(&mut self, count: usize) {
        let count = count.min(self.capacity());
        let groups = count.div_ceil(LANES);
        for index in count..(groups * LANES).min(self.capacity()) {
            self.set(index, Rotor::default());
            self.place(index, 0.0, 0.0);
        }
        for group in groups..self.re.len() {
            for lanes in [
                &mut self.re,
                &mut self.im,
                &mut self.cosine,
                &mut self.sine,
                &mut self.drive,
                &mut self.listen_re,
                &mut self.listen_im,
            ] {
                if let Some(lane) = lanes.get_mut(group) {
                    *lane = [0.0; LANES];
                }
            }
        }
        self.groups = groups;
        self.live = groups;
    }

    /// Phasors configured, rounded up to whole groups.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.groups * LANES
    }

    /// Configures phasor `index`, leaving where it currently is alone.
    pub(crate) fn set(&mut self, index: usize, rotor: Rotor) {
        let (group, lane) = (index / LANES, index % LANES);
        let slots = [
            (&mut self.cosine, rotor.cosine),
            (&mut self.sine, rotor.sine),
            (&mut self.drive, rotor.drive),
            (&mut self.listen_re, rotor.listen_re),
            (&mut self.listen_im, rotor.listen_im),
        ];
        for (lanes, value) in slots {
            if let Some(slot) = lanes.get_mut(group).and_then(|lanes| lanes.get_mut(lane)) {
                *slot = value;
            }
        }
    }

    /// Replaces only the pole of phasor `index`: a retune or a change of damping.
    pub(crate) fn turn(&mut self, index: usize, cosine: f32, sine: f32) {
        let (group, lane) = (index / LANES, index % LANES);
        if let Some(slot) = self
            .cosine
            .get_mut(group)
            .and_then(|lanes| lanes.get_mut(lane))
        {
            *slot = cosine;
        }
        if let Some(slot) = self
            .sine
            .get_mut(group)
            .and_then(|lanes| lanes.get_mut(lane))
        {
            *slot = sine;
        }
    }

    /// The pole of phasor `index`.
    pub(crate) fn pole(&self, index: usize) -> (f32, f32) {
        let (group, lane) = (index / LANES, index % LANES);
        (
            self.cosine.get(group).map_or(0.0, |lanes| lanes[lane]),
            self.sine.get(group).map_or(0.0, |lanes| lanes[lane]),
        )
    }

    /// Multiplies every pole's radius by `factor`: a damper coming down, or lifting again.
    pub(crate) fn shrink(&mut self, factor: f32) {
        let groups = self.groups;
        let poles = self.cosine.iter_mut().zip(self.sine.iter_mut());
        for (cosine, sine) in poles.take(groups) {
            for value in cosine.iter_mut().chain(sine.iter_mut()) {
                *value *= factor;
            }
        }
    }

    /// Puts phasor `index` at `re + i·im`.
    pub(crate) fn place(&mut self, index: usize, re: f32, im: f32) {
        let (group, lane) = (index / LANES, index % LANES);
        if let Some(slot) = self.re.get_mut(group).and_then(|lanes| lanes.get_mut(lane)) {
            *slot = re;
        }
        if let Some(slot) = self.im.get_mut(group).and_then(|lanes| lanes.get_mut(lane)) {
            *slot = im;
        }
        self.live = self.live.max((group + 1).min(self.groups));
    }

    /// Where phasor `index` is.
    pub(crate) fn at(&self, index: usize) -> (f32, f32) {
        let (group, lane) = (index / LANES, index % LANES);
        (
            self.re.get(group).map_or(0.0, |lanes| lanes[lane]),
            self.im.get(group).map_or(0.0, |lanes| lanes[lane]),
        )
    }

    /// What phasor `index` contributes to the output right now.
    pub(crate) fn heard(&self, index: usize) -> f32 {
        let (group, lane) = (index / LANES, index % LANES);
        let (re, im) = self.at(index);
        let listen_re = self.listen_re.get(group).map_or(0.0, |lanes| lanes[lane]);
        let listen_im = self.listen_im.get(group).map_or(0.0, |lanes| lanes[lane]);
        re * listen_re + im * listen_im
    }

    /// Whether anything is still moving.
    #[cfg(test)]
    pub(crate) fn is_ringing(&self) -> bool {
        self.live > 0
    }

    /// Advances every live phasor through `out.len()` samples, adding what is heard to `out`.
    pub(crate) fn ring(&mut self, out: &mut [f32]) {
        let live = self.live;
        for sample in out {
            let mut heard = [0.0_f32; LANES];
            let state = self.re.iter_mut().zip(self.im.iter_mut());
            let poles = self.cosine.iter().zip(self.sine.iter());
            let ears = self.listen_re.iter().zip(self.listen_im.iter());
            for (((re, im), (cosine, sine)), (listen_re, listen_im)) in
                state.zip(poles).zip(ears).take(live)
            {
                for lane in 0..LANES {
                    let turned = cosine[lane] * re[lane] - sine[lane] * im[lane];
                    im[lane] = sine[lane] * re[lane] + cosine[lane] * im[lane];
                    re[lane] = turned;
                    heard[lane] += listen_re[lane] * turned + listen_im[lane] * im[lane];
                }
            }
            *sample += heard.iter().sum::<f32>();
        }
    }

    /// Like [`Rotors::ring`], with `force` pushing every phasor by its own drive as it goes.
    /// `force` and `out` are the same length; the shorter decides otherwise.
    pub(crate) fn ring_driven(&mut self, force: &[f32], out: &mut [f32]) {
        let live = self.groups;
        self.live = live;
        for (push, sample) in force.iter().zip(out) {
            let mut heard = [0.0_f32; LANES];
            let state = self.re.iter_mut().zip(self.im.iter_mut());
            let poles = self.cosine.iter().zip(self.sine.iter());
            let ears = self.listen_re.iter().zip(self.listen_im.iter());
            for ((((re, im), (cosine, sine)), (listen_re, listen_im)), drive) in
                state.zip(poles).zip(ears).zip(self.drive.iter()).take(live)
            {
                for lane in 0..LANES {
                    let turned =
                        cosine[lane] * re[lane] - sine[lane] * im[lane] + drive[lane] * push;
                    im[lane] = sine[lane] * re[lane] + cosine[lane] * im[lane];
                    re[lane] = turned;
                    heard[lane] += listen_re[lane] * turned + listen_im[lane] * im[lane];
                }
            }
            *sample += heard.iter().sum::<f32>();
        }
    }

    /// Puts to rest the groups that have died away below `floor` and stops advancing the ones
    /// at the end of the pool, which is where a body's shortest-lived modes are kept. Returns
    /// the sum of what is left, a measure of how much is still moving.
    pub(crate) fn settle(&mut self, floor: f32) -> f32 {
        let floor = floor.max(REST);
        let mut total = 0.0;
        let mut live = 0;
        for (index, (re, im)) in self
            .re
            .iter_mut()
            .zip(self.im.iter_mut())
            .take(self.live)
            .enumerate()
        {
            let moving: f32 = re.iter().chain(im.iter()).map(|value| value.abs()).sum();
            if moving.is_finite() && moving >= floor {
                live = index + 1;
                total += moving;
            } else {
                // Not finite is a fault upstream; resting the group keeps it from spreading.
                *re = [0.0; LANES];
                *im = [0.0; LANES];
            }
        }
        self.live = live;
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{goertzel, rms};

    const RATE: f32 = 48_000.0;

    fn bank(rotors: &[Rotor]) -> Rotors {
        let mut bank = Rotors::default();
        bank.prepare(rotors.len());
        for (index, rotor) in rotors.iter().enumerate() {
            bank.set(index, *rotor);
        }
        bank.set_len(rotors.len());
        bank
    }

    fn mode(hz: f32, seconds: f32) -> Rotor {
        let (cosine, sine) = pole(hz, decay_rate(seconds), RATE);
        Rotor {
            cosine,
            sine,
            drive: 1.0,
            listen_re: 0.0,
            listen_im: 1.0,
        }
    }

    #[test]
    fn a_phasor_rings_at_its_frequency_and_falls_sixty_decibels_in_its_decay_time() {
        let mut rotors = bank(&[mode(440.0, 1.0)]);
        rotors.place(0, 0.0, 1.0);
        let mut out = vec![0.0; 48_000];
        rotors.ring(&mut out);
        let strongest = (430..451)
            .max_by(|a, b| {
                goertzel(&out[..24_000], 48_000.0, f64::from(*a)).total_cmp(&goertzel(
                    &out[..24_000],
                    48_000.0,
                    f64::from(*b),
                ))
            })
            .unwrap();
        assert_eq!(strongest, 440);
        let early = rms(&out[..480]);
        let late = rms(&out[24_000..24_480]);
        let fall = 20.0 * (early / late).log10();
        assert!((fall - 30.0).abs() < 0.3, "fell {fall} dB in half a second");
    }

    #[test]
    fn low_modes_keep_their_pitch_in_single_precision_at_a_high_rate() {
        // A0 at 192 kHz turns by under a thousandth of a radian a sample.
        let (cosine, sine) = pole(27.5, 0.1, 192_000.0);
        let mut rotors = bank(&[Rotor {
            cosine,
            sine,
            drive: 0.0,
            listen_re: 0.0,
            listen_im: 1.0,
        }]);
        rotors.place(0, 1.0, 0.0);
        let mut out = vec![0.0; 192_000 * 4];
        rotors.ring(&mut out);
        let strength = |hz: f64| goertzel(&out, 192_000.0, hz);
        let best = (-40..=40)
            .map(|tenths| 27.5 * (f64::from(tenths) / 12_000.0).exp2())
            .max_by(|a, b| strength(*a).total_cmp(&strength(*b)))
            .unwrap();
        let cents = 1200.0 * (best / 27.5).log2();
        assert!(cents.abs() < 1.0, "A0 rang {cents} cents off");
    }

    #[test]
    fn a_driven_phasor_answers_most_at_its_own_frequency() {
        let response = |hz: f32| {
            let mut rotors = bank(&[Rotor {
                listen_re: 1.0,
                listen_im: 0.0,
                ..mode(1_000.0, 0.05)
            }]);
            let force: Vec<f32> = (0..24_000)
                .map(|n| (TAU * hz * n as f32 / RATE).sin())
                .collect();
            let mut out = vec![0.0; force.len()];
            rotors.ring_driven(&force, &mut out);
            rms(&out[12_000..])
        };
        let at = response(1_000.0);
        assert!(at > response(900.0) * 3.0 && at > response(1_100.0) * 3.0);
        // Driven on its real half and heard there, a mode is a band-pass: it passes neither a
        // steady push nor the top of the band at anything like its resonant gain.
        assert!(at > response(20.0) * 30.0);
    }

    #[test]
    fn padding_lanes_stay_silent_and_dead_groups_stop_being_advanced() {
        let mut rotors = bank(&[mode(200.0, 2.0), mode(3_000.0, 0.01)]);
        assert_eq!(rotors.len(), LANES);
        let mut long = Rotors::default();
        long.prepare(20);
        for index in 0..20 {
            // The first group rings on; the two after it die at once.
            let seconds = if index < LANES { 3.0 } else { 0.005 };
            long.set(index, mode(300.0 + index as f32 * 50.0, seconds));
            long.place(index, 0.0, 0.1);
        }
        long.set_len(20);
        for index in 0..20 {
            long.place(index, 0.0, 0.1);
        }
        let mut out = vec![0.0; 4_800];
        long.ring(&mut out);
        assert!(long.settle(1.0e-9) > 0.0);
        assert!(long.is_ringing());
        // Only the long-lived first group is left to advance.
        assert_eq!(long.live, 1);
        rotors.place(0, 0.0, 1.0);
        let mut out = vec![0.0; 64];
        rotors.ring(&mut out);
        assert!(out.iter().all(|sample| sample.is_finite()));
        rotors.rest();
        assert_eq!(rotors.settle(0.0), 0.0);
        assert!(!rotors.is_ringing());
    }

    #[test]
    fn shrinking_the_poles_shortens_a_ringing_note() {
        let mut rotors = bank(&[mode(440.0, 10.0)]);
        rotors.place(0, 0.0, 1.0);
        let mut free = vec![0.0; 4_800];
        rotors.ring(&mut free);
        // A damper worth 60 dB in a tenth of a second.
        rotors.shrink((-decay_rate(0.1) / RATE).exp());
        let mut damped = vec![0.0; 4_800];
        rotors.ring(&mut damped);
        let fall = 20.0 * (rms(&free[4_320..]) / rms(&damped[4_320..])).log10();
        assert!((fall - 57.0).abs() < 3.0, "the damper took {fall} dB");
    }
}
