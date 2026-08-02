//! One-pole parameter smoothing.
//!
//! A fader or knob delivers a new value once per block. Applying it as a step produces an
//! audible click at the block boundary ("zipper noise"), so continuous parameters are moved
//! towards their target one sample at a time instead.

/// Per-sample coefficient of a one-pole filter with the given exponential time constant.
///
/// `exp(-1 / (time * fs))` is the pole of a first-order low-pass whose step response covers
/// `1 - 1/e` (about 63 %) of the distance to the target after `time` seconds. A non-positive
/// or non-finite time constant yields `0.0`, which makes the filter follow its input exactly.
pub fn one_pole_coefficient(time_seconds: f32, sample_rate: f32) -> f32 {
    if !time_seconds.is_finite()
        || !sample_rate.is_finite()
        || time_seconds <= 0.0
        || sample_rate <= 0.0
    {
        return 0.0;
    }
    (-1.0 / (time_seconds * sample_rate)).exp()
}

/// A value that glides towards its target instead of jumping to it.
///
/// The type is `Copy` on purpose: an effect that has to run the same ramp over several
/// channels snapshots the smoother before the first channel and restores that snapshot for
/// each of the others, so every channel sees an identical gain curve.
/// The remaining *distance* is what decays, not the value itself.
///
/// Iterating `current = target + (current - target) * c` in `f32` stalls: once the gap is small
/// relative to `target`, `target + gap * c` rounds straight back to `target + gap` and the ramp
/// never finishes. For a 50 ms ramp that happens over a thousand ULPs short of the target.
/// Decaying the gap in its own variable keeps every step in the gap's own exponent range, so it
/// converges geometrically all the way down however slow the ramp is.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SmoothedValue {
    target: f32,
    difference: f32,
    coefficient: f32,
}

/// Gap at which the ramp is declared finished and snapped shut.
///
/// Roughly -140 dBFS, i.e. below the resolution of 24-bit audio, so the snap is inaudible and
/// the pole never grinds its way down into denormals.
const SETTLED_EPSILON: f32 = 1.0e-7;

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

impl SmoothedValue {
    /// A smoother sitting on `value` with the given ramp time.
    pub fn new(value: f32, time_seconds: f32, sample_rate: f32) -> Self {
        Self {
            target: finite_or_zero(value),
            difference: 0.0,
            coefficient: one_pole_coefficient(time_seconds, sample_rate),
        }
    }

    /// Changes the ramp time. Does not disturb the value or the target.
    pub fn set_time(&mut self, time_seconds: f32, sample_rate: f32) {
        self.coefficient = one_pole_coefficient(time_seconds, sample_rate);
    }

    /// Sets the value the smoother is heading for, keeping the current value where it is.
    pub fn set_target(&mut self, target: f32) {
        let target = finite_or_zero(target);
        self.difference = self.current() - target;
        self.target = target;
    }

    /// Jumps straight to `value`, cancelling any ramp in progress.
    ///
    /// Call this from `prepare`/`reset`, never mid-block.
    pub fn snap_to(&mut self, value: f32) {
        self.target = finite_or_zero(value);
        self.difference = 0.0;
    }

    /// Jumps straight to the current target.
    pub fn snap_to_target(&mut self) {
        self.difference = 0.0;
    }

    /// Advances one sample and returns the new value.
    pub fn next_value(&mut self) -> f32 {
        self.difference *= self.coefficient;
        if self.difference.abs() < SETTLED_EPSILON {
            self.difference = 0.0;
        }
        self.current()
    }

    /// The value without advancing.
    pub fn current(&self) -> f32 {
        self.target + self.difference
    }

    /// The value being approached.
    pub fn target(&self) -> f32 {
        self.target
    }

    /// `true` once the ramp has reached its target exactly.
    pub fn is_settled(&self) -> bool {
        self.difference == 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefficient_matches_the_one_over_e_definition() {
        // 10 ms at 48 kHz is 480 samples; after that many steps a one-pole covers 1 - 1/e.
        let coefficient = one_pole_coefficient(0.010, 48_000.0);
        assert!((coefficient - (-1.0f32 / 480.0).exp()).abs() < 1e-9);

        let mut value = SmoothedValue::new(0.0, 0.010, 48_000.0);
        value.set_target(1.0);
        for _ in 0..480 {
            value.next_value();
        }
        assert!((value.current() - (1.0 - 1.0 / std::f32::consts::E)).abs() < 1e-3);
    }

    #[test]
    fn zero_time_constant_follows_the_target_immediately() {
        let mut value = SmoothedValue::new(0.0, 0.0, 48_000.0);
        value.set_target(0.5);
        assert_eq!(value.next_value(), 0.5);
    }

    #[test]
    fn ramp_settles_exactly_on_the_target() {
        let mut value = SmoothedValue::new(0.0, 0.001, 48_000.0);
        value.set_target(-3.25);
        for _ in 0..4_000 {
            value.next_value();
        }
        assert!(value.is_settled());
        assert_eq!(value.current(), -3.25);
    }

    #[test]
    fn snapshots_replay_the_same_curve() {
        let mut value = SmoothedValue::new(0.0, 0.005, 48_000.0);
        value.set_target(1.0);
        let snapshot = value;
        let first: Vec<f32> = (0..16).map(|_| value.next_value()).collect();
        let mut replay = snapshot;
        let second: Vec<f32> = (0..16).map(|_| replay.next_value()).collect();
        assert_eq!(first, second);
    }
}
