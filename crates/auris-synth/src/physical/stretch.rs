//! A string's longitudinal motion: what its transverse motion does to its tension, and the
//! metallic ring that comes of it.
//!
//! A string that swings sideways is stretched, and stretched the more the further it swings,
//! so its tension carries the products of its transverse partials: their sums and differences
//! in frequency. The string has modes along its length too, a few kilohertz apart in a plain
//! steel wire and lower in a wound one, whose winding adds mass without adding stiffness. Each
//! of them is driven by the products of the transverse partials whose orders differ by its own
//! number, and those whose orders add to it, and it pulls on the bridge lengthways. Driven far
//! below its resonance it passes the products on as a quiet comb a little flat of the
//! transverse partials — the *phantom partials* — and near its resonance it rings, an
//! inharmonic clang that no transverse partial explains. That clang is much of what makes a
//! grand's bass sound like wire rather than like a coil spring: without it a low note's
//! partials march on in step, and the dispersion of a stiff string sweeps through every period
//! of the note as it does through a spring.
//!
//! The forces are worked out from the transverse phasors' displacements sample by sample, and
//! each longitudinal mode is a two-pole resonator. Nothing here allocates.

use std::f32::consts::{PI, TAU};

use super::rotors::LANES;

/// Longitudinal modes followed.
pub(crate) const MODES: usize = 3;
/// Transverse partials whose products are taken: enough to reach the first longitudinal mode
/// of the lowest strings.
pub(crate) const PARTIALS: usize = 64;

/// The longitudinal motion of one note's strings.
#[derive(Clone, Debug)]
pub(crate) struct Stretch {
    /// Longitudinal modes in use; none for a note that has no longitudinal ring to speak of.
    modes: usize,
    /// Transverse partials taken into the products.
    partials: usize,
    /// Strings of the note, which lie side by side in the phasor pool.
    strings: usize,
    /// Each resonator's feedback: `2 r cos θ` and `r²`.
    feedback: [[f32; 2]; MODES],
    /// Each resonator's last two outputs.
    state: [[f32; 2]; MODES],
    /// What one unit of each resonator's motion pulls on the bridge.
    pull: [f32; MODES],
    /// Each partial's slope: its displacement times its order, taken afresh every sample.
    slope: [f32; PARTIALS + 1],
}

impl Default for Stretch {
    fn default() -> Self {
        Self {
            modes: 0,
            partials: 0,
            strings: 1,
            feedback: [[0.0; 2]; MODES],
            state: [[0.0; 2]; MODES],
            pull: [0.0; MODES],
            slope: [0.0; PARTIALS + 1],
        }
    }
}

impl Stretch {
    /// Lays out the longitudinal modes of a note whose first is at `first` hertz, over
    /// `partials` transverse partials of `strings` strings each, the modes `quality` sharp,
    /// heard at `gain`. A `gain` of zero, or a first mode above what `rate` can carry, leaves
    /// the note without one.
    pub(crate) fn set(
        &mut self,
        first: f32,
        quality: f32,
        gain: f32,
        partials: usize,
        strings: usize,
        rate: f32,
    ) {
        self.partials = partials.min(PARTIALS);
        self.strings = strings.max(1);
        self.modes = 0;
        let positive = |value: f32| value.is_finite() && value > 0.0;
        if !positive(gain) || !positive(first) || self.partials < 2 {
            return;
        }
        for mode in 0..MODES {
            let hz = first * (mode + 1) as f32;
            if hz >= rate * 0.45 {
                break;
            }
            let theta = TAU * hz / rate;
            let radius = (-PI * hz / (quality.max(1.0) * rate)).exp();
            self.feedback[mode] = [2.0 * radius * theta.cos(), radius * radius];
            // The strain at the bridge goes as the mode's number, and alternates in sign from
            // one mode to the next; the resonator's gain below its peak, `1 / θ²`, is taken out
            // so that `gain` is the size of the quiet comb the products make.
            let sign = if mode % 2 == 0 { -1.0 } else { 1.0 };
            self.pull[mode] = sign * gain * (mode + 1) as f32 * theta * theta;
            self.modes = mode + 1;
        }
    }

    /// Stops the motion.
    pub(crate) fn reset(&mut self) {
        self.state = [[0.0; 2]; MODES];
    }

    /// Whether the note has longitudinal modes at all.
    pub(crate) fn is_active(&self) -> bool {
        self.modes > 0
    }

    /// One sample: takes the transverse displacements from `im`, the displacement halves of
    /// the note's phasors, and returns what the longitudinal modes pull on the bridge.
    #[inline]
    pub(crate) fn step(&mut self, im: &[[f32; LANES]]) -> f32 {
        let strings = self.strings;
        let partials = self.partials;
        for (order, slope) in self.slope.iter_mut().enumerate().take(partials + 1).skip(1) {
            let mut moved = 0.0;
            for string in 0..strings {
                let at = (order - 1) * strings + string;
                moved += im.get(at / LANES).map_or(0.0, |lanes| lanes[at % LANES]);
            }
            *slope = moved * order as f32;
        }
        let mut pulled = 0.0;
        for mode in 0..self.modes {
            let number = mode + 1;
            // Orders that differ by the mode's number, and orders that add to it.
            let mut force = 0.0;
            let reach = partials.saturating_sub(number);
            for (low, high) in self.slope[1..=reach].iter().zip(&self.slope[1 + number..]) {
                force += low * high;
            }
            for order in 1..number {
                force -= 0.5 * self.slope[order] * self.slope[number - order];
            }
            let [a, b] = self.feedback[mode];
            let [last, before] = self.state[mode];
            let now = force + a * last - b * before;
            self.state[mode] = [now, last];
            pulled += self.pull[mode] * now;
        }
        if pulled.is_finite() {
            pulled
        } else {
            self.reset();
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Amplitude of `samples` at `hz`, by a Hann-windowed single-bin transform.
    fn at(samples: &[f32], hz: f32, rate: f32) -> f32 {
        let n = samples.len() as f32;
        let (mut re, mut im) = (0.0_f32, 0.0_f32);
        for (i, sample) in samples.iter().enumerate() {
            let w = 0.5 - 0.5 * (TAU * i as f32 / n).cos();
            let phase = TAU * hz * i as f32 / rate;
            re += sample * w * phase.cos();
            im += sample * w * phase.sin();
        }
        (re * re + im * im).sqrt() * 4.0 / n
    }

    /// What the longitudinal modes make of two transverse partials of one string, `low` and
    /// `high` in order, swinging at `f` hertz each.
    fn pulled(orders: (usize, usize), f: (f32, f32), first: f32) -> Vec<f32> {
        let rate = 48_000.0;
        let mut stretch = Stretch::default();
        stretch.set(first, 40.0, 1.0, 32, 1, rate);
        let mut lanes = vec![[0.0_f32; LANES]; 32 / LANES];
        (0..24_000)
            .map(|i| {
                let t = i as f32 / rate;
                for (order, hz) in [(orders.0, f.0), (orders.1, f.1)] {
                    let at = order - 1;
                    lanes[at / LANES][at % LANES] = (TAU * hz * t).sin();
                }
                stretch.step(&lanes)
            })
            .collect()
    }

    #[test]
    fn two_partials_pull_at_their_sum_and_most_on_a_longitudinal_mode() {
        let rate = 48_000.0;
        // Partials 10 and 11 of a string at 100 Hz: their orders differ by one, so the first
        // longitudinal mode carries their sum.
        let near = pulled((10, 11), (1_000.0, 1_100.0), 2_100.0);
        let far = pulled((10, 11), (1_000.0, 1_100.0), 6_000.0);
        let sum = at(&near[4_800..], 2_100.0, rate);
        let beside = at(&near[4_800..], 1_900.0, rate);
        assert!(sum > 100.0 * beside, "sum {sum}, beside {beside}");
        // On its resonance the mode passes the product on some forty times as loud as far
        // below it, where it only follows.
        let resonant = sum / at(&far[4_800..], 2_100.0, rate);
        assert!((20.0..80.0).contains(&resonant), "resonance {resonant}");
    }

    #[test]
    fn partials_whose_orders_do_not_meet_a_mode_pull_nothing() {
        // Orders 4 and 9 differ by five and add to thirteen, and each with itself adds to an
        // order past three: no mode of the three followed is driven.
        let pulled = pulled((4, 9), (400.0, 900.0), 1_300.0);
        assert!(pulled.iter().all(|sample| sample.abs() < 1.0e-6));
    }
}
