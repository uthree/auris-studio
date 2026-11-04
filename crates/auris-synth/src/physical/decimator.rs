//! Two-to-one decimation for a delay-line string run at twice the host rate.
//!
//! A 63-tap Kaiser-windowed (β = 8) half-band lowpass. Half its taps are zero, so an output
//! costs seventeen multiplies. Measured at the doubled rate, it is flat within 0.003 dB to 0.21
//! of that rate (20.2 kHz at a 48 kHz host) and at least 70 dB down from 0.29 (27.8 kHz), so
//! what the oversampled string produces above the host's band edge is removed rather than
//! folded back onto it. Its delay is 15.5 host samples, the same for every note.

/// Taps of the half-band filter at odd distances 1, 3, 5, … 31 from its centre, whose own tap
/// is one half. Every even distance but zero is zero.
const ODD_TAPS: [f32; 16] = [
    3.170_729e-1,
    -1.024_425_2e-1,
    5.772_404_8e-2,
    -3.748_938_4e-2,
    2.563_768_7e-2,
    -1.780_47e-2,
    1.231_558_4e-2,
    -8.376_211e-3,
    5.543_647_4e-3,
    -3.534_414_5e-3,
    2.145_908_7e-3,
    -1.222_076_1e-3,
    6.381_064e-4,
    -2.935_601e-4,
    1.090_362_4e-4,
    -2.401_525_4e-5,
];
const TAPS: usize = 63;
const CENTRE: usize = TAPS / 2;
const RING: usize = 64;

/// History of one oversampled voice. Fixed-size: resetting it for a new note never allocates.
#[derive(Clone, Debug)]
pub(super) struct Decimator {
    /// Every input written twice, `RING` apart, so the newest `TAPS` are always contiguous.
    history: [f32; 2 * RING],
    cursor: usize,
}

impl Default for Decimator {
    fn default() -> Self {
        Self {
            history: [0.0; 2 * RING],
            cursor: 0,
        }
    }
}

impl Decimator {
    pub(super) fn reset(&mut self) {
        self.history.fill(0.0);
        self.cursor = 0;
    }

    fn push(&mut self, sample: f32) {
        self.history[self.cursor] = sample;
        self.history[self.cursor + RING] = sample;
        self.cursor = (self.cursor + 1) % RING;
    }

    /// Takes two internal samples, oldest first, and returns one host sample.
    pub(super) fn process(&mut self, first: f32, second: f32) -> f32 {
        self.push(first);
        self.push(second);
        // The newest `TAPS` samples, oldest first. The filter is symmetric, so the order in
        // which taps meet them does not matter.
        let start = self.cursor + RING - TAPS;
        let window = &self.history[start..start + TAPS];
        let mut sum = 0.5 * window[CENTRE];
        for (index, tap) in ODD_TAPS.iter().enumerate() {
            let distance = 2 * index + 1;
            sum += tap * (window[CENTRE - distance] + window[CENTRE + distance]);
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    /// Steady-state amplitude of a sine at `cycles` per internal sample through the decimator.
    fn gain(cycles: f64) -> f32 {
        let mut decimator = Decimator::default();
        let mut peak = 0.0_f32;
        for frame in 0..4_000 {
            // Generated in f64: an f32 phase this far into the signal is itself noise at -55 dB.
            let at = |n: usize| (TAU * cycles * n as f64).sin() as f32;
            let out = decimator.process(at(2 * frame), at(2 * frame + 1));
            if frame > 100 {
                peak = peak.max(out.abs());
            }
        }
        peak
    }

    #[test]
    fn passes_the_host_band_and_rejects_what_would_fold_into_it() {
        // At a 48 kHz host the internal rate is 96 kHz: 1 kHz, 18 kHz, then 30 and 40 kHz.
        assert!((gain(1.0 / 96.0) - 1.0).abs() < 1e-3);
        assert!((gain(18.0 / 96.0) - 1.0).abs() < 1e-3);
        assert!(gain(30.0 / 96.0) < 10.0_f32.powf(-70.0 / 20.0));
        assert!(gain(40.0 / 96.0) < 10.0_f32.powf(-70.0 / 20.0));
    }

    #[test]
    fn unit_dc_comes_out_whole() {
        let mut decimator = Decimator::default();
        let mut out = 0.0;
        for _ in 0..64 {
            out = decimator.process(1.0, 1.0);
        }
        assert!((out - 1.0).abs() < 1e-5, "{out}");
    }
}
