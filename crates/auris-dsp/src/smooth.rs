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

/// Removes a signal's steady component without touching the band it plays in.
///
/// `y[n] = x[n] - x[n-1] + r * y[n-1]`, the ordinary one-pole DC blocker: a zero exactly at DC
/// and a pole just inside it. A [`Biquad`](crate::Biquad) highpass would do the same job and is
/// the wrong tool for a corner this low — at five hertz against a 192 kHz rate the normalised
/// frequency is 2.6e-5, and a direct-form biquad spends most of its `f32` mantissa on the
/// difference between coefficients that are all but equal. This form has one coefficient and
/// stays well conditioned wherever the corner is.
///
/// What it is for: an excitation with a mean. A bow pushes a string one way for as long as it is
/// drawn, and a bridge does not pass that to the air; a struck or plucked string has no mean to
/// remove and wants none of this.
#[derive(Clone, Copy, Debug)]
pub struct DcBlocker {
    previous_input: f32,
    previous_output: f32,
    retention: f32,
}

/// Transparent until [`DcBlocker::new`] gives it a corner.
///
/// The derived default would be a retention of zero, which is `y[n] = x[n] - x[n-1]`: a
/// differentiator, not a blocker. A value that has not been prepared yet belongs to a voice that
/// has not sounded yet, and it must pass audio through untouched rather than shape it.
impl Default for DcBlocker {
    fn default() -> Self {
        Self {
            previous_input: 0.0,
            previous_output: 0.0,
            retention: 1.0,
        }
    }
}

impl DcBlocker {
    /// A blocker whose -3 dB corner is `corner_hz` at `sample_rate`.
    ///
    /// Choose the corner below every fundamental the signal carries: the attenuation at a
    /// frequency `f` is `f / hypot(f, corner_hz)`, so five hertz costs a double bass's open E
    /// (41 Hz) seven hundredths of a decibel and a bowed cello's open C rather less.
    ///
    /// A corner that is not finite, not positive, or at or above half the rate leaves the signal
    /// alone rather than ringing: a nonsensical setting should be inaudible, not destructive.
    pub fn new(corner_hz: f32, sample_rate: f32) -> Self {
        let usable = corner_hz.is_finite()
            && sample_rate.is_finite()
            && corner_hz > 0.0
            && sample_rate > 0.0
            && corner_hz < sample_rate * 0.5;
        Self {
            previous_input: 0.0,
            previous_output: 0.0,
            // `exp` rather than the `1 - 2*pi*f/rate` approximation, which is only close while
            // the corner is a small fraction of the rate and goes negative when it is not.
            retention: if usable {
                (-std::f32::consts::TAU * corner_hz / sample_rate).exp()
            } else {
                // A pole at one is a zero and a pole on top of each other: the pair cancel and
                // the sample passes through untouched.
                1.0
            },
        }
    }

    /// The next sample, with the steady component taken out.
    pub fn next(&mut self, input: f32) -> f32 {
        let output = input - self.previous_input + self.retention * self.previous_output;
        self.previous_input = input;
        self.previous_output = output;
        output
    }

    /// Forgets the signal so far, for a voice starting again.
    pub fn reset(&mut self) {
        self.previous_input = 0.0;
        self.previous_output = 0.0;
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
    fn a_blocker_removes_a_steady_offset_and_keeps_the_note() {
        // Numbers rather than "it runs": the offset has to go and the fundamental has to stay,
        // at every rate a project can be set to.
        for rate in [44_100.0_f32, 48_000.0, 96_000.0, 192_000.0] {
            let mut blocker = DcBlocker::new(5.0, rate);
            let frequency = 196.0_f32; // A violin's open G, the lowest note the bow plays here.
            let step = std::f32::consts::TAU * frequency / rate;
            let frames = rate as usize;
            let mut out = Vec::with_capacity(frames);
            for n in 0..frames {
                out.push(blocker.next((step * n as f32).sin() + 0.25));
            }
            // Past the first few corner periods, where the blocker is still converging.
            let steady = &out[frames / 2..];
            let mean = steady.iter().sum::<f32>() / steady.len() as f32;
            let peak = steady.iter().fold(0.0_f32, |high, s| high.max(s.abs()));
            assert!(
                mean.abs() < 1.0e-3,
                "{rate}: offset left behind, mean {mean}"
            );
            assert!(
                (0.97..=1.03).contains(&peak),
                "{rate}: the note was attenuated, peak {peak}"
            );
        }
    }

    #[test]
    fn a_blocker_leaves_the_lowest_notes_alone() {
        // The corner is only useful if it is below the band. A first-order response is
        // `f / hypot(f, corner)`, so check the two lowest open strings in the pack against it.
        let mut blocker = DcBlocker::new(5.0, 48_000.0);
        let frequency = 41.2_f32; // A double bass's open E.
        let step = std::f32::consts::TAU * frequency / 48_000.0;
        let mut peak = 0.0_f32;
        for n in 0..48_000 {
            let value = blocker.next((step * n as f32).sin());
            if n > 24_000 {
                peak = peak.max(value.abs());
            }
        }
        let expected = frequency / frequency.hypot(5.0);
        assert!(
            (peak - expected).abs() < 0.01,
            "peak {peak} against the first-order response {expected}"
        );
        assert!(peak > 0.99, "a bass note lost more than 0.1 dB: {peak}");
    }

    #[test]
    fn an_unprepared_blocker_is_transparent_rather_than_a_differentiator() {
        // A derived `Default` would leave the retention at zero, which is `x[n] - x[n-1]`. A
        // voice built but not yet prepared must pass audio through, not differentiate it.
        let mut blocker = DcBlocker::default();
        let input = [0.0, 1.0, -0.5, 0.25, 0.25];
        let out: Vec<f32> = input.iter().map(|s| blocker.next(*s)).collect();
        assert_eq!(out, input);
    }

    #[test]
    fn a_nonsensical_corner_passes_the_signal_through() {
        // A corner at or above Nyquist, or a rate of nothing, must be inaudible rather than
        // destructive: a project opened with a corrupt rate should still make a sound.
        for (corner, rate) in [
            (0.0_f32, 48_000.0_f32),
            (-5.0, 48_000.0),
            (30_000.0, 48_000.0),
        ] {
            let mut blocker = DcBlocker::new(corner, rate);
            let input = [0.0, 1.0, -0.5, 0.25, 0.25];
            let out: Vec<f32> = input.iter().map(|s| blocker.next(*s)).collect();
            assert_eq!(out, input, "corner {corner} at {rate} changed the signal");
        }
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
