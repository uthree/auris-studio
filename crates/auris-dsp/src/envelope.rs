//! Level detection for dynamics processing and metering.

use crate::smooth::one_pole_coefficient;

/// How the follower rectifies its input before smoothing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EnvelopeMode {
    /// Tracks `|x|`. Catches transients, which is what a limiter needs.
    Peak,
    /// Tracks `sqrt(mean(x^2))`. Closer to perceived loudness, which is what a meter wants.
    Rms,
}

/// A one-pole envelope follower with independent attack and release times.
///
/// Both coefficients are `exp(-1 / (time * fs))`, so `time` is the classic 1/e time constant.
/// Peak mode covers about 63% of an amplitude step in that time. RMS mode smooths power before
/// taking its square root, so its reported amplitude covers about 79.5% instead.
#[derive(Copy, Clone, Debug)]
pub struct EnvelopeFollower {
    mode: EnvelopeMode,
    sample_rate: f32,
    attack_seconds: f32,
    release_seconds: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
    state: f32,
}

impl EnvelopeFollower {
    /// A follower cleared to silence.
    pub fn new(
        sample_rate: f32,
        attack_seconds: f32,
        release_seconds: f32,
        mode: EnvelopeMode,
    ) -> Self {
        let mut follower = Self {
            mode,
            sample_rate,
            attack_seconds,
            release_seconds,
            attack_coefficient: 0.0,
            release_coefficient: 0.0,
            state: 0.0,
        };
        follower.recompute();
        follower
    }

    fn recompute(&mut self) {
        self.attack_coefficient = one_pole_coefficient(self.attack_seconds, self.sample_rate);
        self.release_coefficient = one_pole_coefficient(self.release_seconds, self.sample_rate);
    }

    /// Changes the sample rate and rescales both coefficients.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    /// Sets the attack time constant in seconds.
    pub fn set_attack(&mut self, seconds: f32) {
        self.attack_seconds = seconds;
        self.recompute();
    }

    /// Sets the release time constant in seconds.
    pub fn set_release(&mut self, seconds: f32) {
        self.release_seconds = seconds;
        self.recompute();
    }

    /// Switches between peak and RMS detection, clearing the state so the units stay coherent.
    pub fn set_mode(&mut self, mode: EnvelopeMode) {
        if self.mode != mode {
            self.mode = mode;
            self.state = 0.0;
        }
    }

    /// The detection mode in use.
    pub fn mode(&self) -> EnvelopeMode {
        self.mode
    }

    /// Clears the envelope to silence.
    pub fn reset(&mut self) {
        self.state = 0.0;
    }

    /// Feeds one sample and returns the new envelope, in the same units as the input.
    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        // Silence, not arithmetic: a single NaN would latch, because every later comparison
        // against a NaN state is false and the release path re-injects it for ever — long
        // after the audio itself has recovered. The same answer `DelayLine::read` gives a
        // non-finite sample.
        let input = if input.is_finite() { input } else { 0.0 };
        // RMS smooths the *square* of the signal, so the pole sees energy rather than
        // amplitude; the square root is taken only on the way out.
        let rectified = match self.mode {
            EnvelopeMode::Peak => input.abs(),
            EnvelopeMode::Rms => input * input,
        };
        let coefficient = if rectified > self.state {
            self.attack_coefficient
        } else {
            self.release_coefficient
        };
        self.state = crate::settled(rectified + (self.state - rectified) * coefficient);
        self.value()
    }

    /// The current envelope without advancing it.
    pub fn value(&self) -> f32 {
        match self.mode {
            EnvelopeMode::Peak => self.state,
            EnvelopeMode::Rms => self.state.max(0.0).sqrt(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn a_nan_sample_does_not_latch_the_follower() {
        // Unguarded, one NaN made the state NaN, every later comparison false, and the
        // release path re-injected it for ever: the follower reported NaN long after the
        // audio had recovered.
        for mode in [EnvelopeMode::Peak, EnvelopeMode::Rms] {
            let mut follower = EnvelopeFollower::new(SR, 0.001, 0.001, mode);
            follower.process(0.5);
            follower.process(f32::NAN);
            for _ in 0..SR as usize {
                follower.process(0.5);
            }
            assert!(
                (follower.value() - 0.5).abs() < 1e-4,
                "{mode:?} never recovered: {}",
                follower.value()
            );
        }
    }

    #[test]
    fn peak_mode_converges_to_the_input_amplitude() {
        let mut follower = EnvelopeFollower::new(SR, 0.001, 0.001, EnvelopeMode::Peak);
        for _ in 0..SR as usize {
            follower.process(0.5);
        }
        assert!((follower.value() - 0.5).abs() < 1e-4);
    }

    #[test]
    fn rms_of_a_sine_is_amplitude_over_root_two() {
        let mut follower = EnvelopeFollower::new(SR, 0.100, 0.100, EnvelopeMode::Rms);
        let step = std::f32::consts::TAU * 1_000.0 / SR;
        for n in 0..SR as usize {
            follower.process((step * n as f32).sin());
        }
        assert!(
            (follower.value() - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01,
            "rms was {}",
            follower.value()
        );
    }

    #[test]
    fn attack_reaches_one_over_e_after_its_time_constant() {
        let mut follower = EnvelopeFollower::new(SR, 0.010, 1.0, EnvelopeMode::Peak);
        // 10 ms at 48 kHz is 480 samples.
        for _ in 0..480 {
            follower.process(1.0);
        }
        let expected = 1.0 - 1.0 / std::f32::consts::E;
        assert!(
            (follower.value() - expected).abs() < 1e-3,
            "envelope was {}",
            follower.value()
        );
    }

    #[test]
    fn release_is_slower_than_attack_when_asked() {
        let mut follower = EnvelopeFollower::new(SR, 0.0001, 0.500, EnvelopeMode::Peak);
        for _ in 0..1_000 {
            follower.process(1.0);
        }
        let peak = follower.value();
        assert!(peak > 0.99);
        // 10 ms of silence with a 500 ms release must barely move.
        for _ in 0..480 {
            follower.process(0.0);
        }
        assert!(follower.value() > 0.97, "decayed to {}", follower.value());
    }

    #[test]
    fn a_release_reaches_exact_silence_before_entering_the_subnormal_range() {
        let mut follower = EnvelopeFollower::new(SR, 0.001, 0.001, EnvelopeMode::Peak);
        follower.process(1.0);
        for _ in 0..20_000 {
            follower.process(0.0);
        }
        assert_eq!(follower.value(), 0.0);
    }
}
