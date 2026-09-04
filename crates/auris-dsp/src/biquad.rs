//! Second-order IIR filter sections.
//!
//! Designs follow Robert Bristow-Johnson's *Audio EQ Cookbook*, which derives every classic
//! filter shape from the same bilinear-transformed analogue prototype. Coefficients are
//! computed in `f64` and stored in `f32`: the design step involves catastrophic cancellation
//! near DC, but running the difference equation in `f32` is fine once the coefficients are
//! accurate.
//!
//! [`Biquad`] runs the **transposed direct form II** difference equation. Of the four classic
//! forms it is the one with the best round-off behaviour in single precision at low cutoff
//! frequencies, which is exactly where an EQ's high-pass and low-shelf bands live.

use std::f64::consts::PI;

/// Smallest design frequency accepted, in Hz. Below this the bilinear prewarping term
/// `tan(pi f / fs)` underflows to zero and the filter degenerates.
const MIN_FREQUENCY_HZ: f64 = 1.0;

/// Largest design frequency as a fraction of the sample rate.
///
/// `tan(pi f / fs)` diverges at Nyquist (`f = fs / 2`), so a user sweeping an EQ to 20 kHz at
/// 44.1 kHz would otherwise produce infinite coefficients. Stopping at `0.4995 fs` keeps the
/// full 20 kHz audio band reachable at 44.1 kHz while bounding the prewarping.
const MAX_FREQUENCY_RATIO: f64 = 0.4995;

/// Highest stable design frequency for `sample_rate`.
pub(crate) fn max_frequency(sample_rate: f64) -> f32 {
    (sample_rate * MAX_FREQUENCY_RATIO) as f32
}

/// Smallest accepted Q. Zero would divide by zero when forming `alpha`.
const MIN_Q: f64 = 1.0e-3;

/// Normalised biquad coefficients: `a0` has already been divided out.
///
/// The difference equation is
/// `y[n] = b0 x[n] + b1 x[n-1] + b2 x[n-2] - a1 y[n-1] - a2 y[n-2]`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BiquadCoefficients {
    /// Feed-forward coefficient for `x[n]`.
    pub b0: f32,
    /// Feed-forward coefficient for `x[n-1]`.
    pub b1: f32,
    /// Feed-forward coefficient for `x[n-2]`.
    pub b2: f32,
    /// Feedback coefficient for `y[n-1]`.
    pub a1: f32,
    /// Feedback coefficient for `y[n-2]`.
    pub a2: f32,
}

impl Default for BiquadCoefficients {
    fn default() -> Self {
        Self::identity()
    }
}

/// Intermediate quantities shared by every cookbook design.
struct Design {
    cos_w0: f64,
    alpha: f64,
}

fn design(sample_rate: f64, frequency: f64, q: f64) -> Option<Design> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return None;
    }
    let frequency = if frequency.is_finite() {
        frequency
    } else {
        0.0
    };
    let max = f64::from(max_frequency(sample_rate));
    let frequency = frequency.clamp(MIN_FREQUENCY_HZ.min(max), max);
    let q = if q.is_finite() { q.max(MIN_Q) } else { MIN_Q };

    let w0 = 2.0 * PI * frequency / sample_rate;
    Some(Design {
        cos_w0: w0.cos(),
        alpha: w0.sin() / (2.0 * q),
    })
}

fn normalize(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> BiquadCoefficients {
    if !a0.is_finite() || a0.abs() < f64::EPSILON {
        return BiquadCoefficients::identity();
    }
    let coefficients = BiquadCoefficients {
        b0: (b0 / a0) as f32,
        b1: (b1 / a0) as f32,
        b2: (b2 / a0) as f32,
        a1: (a1 / a0) as f32,
        a2: (a2 / a0) as f32,
    };
    if coefficients.is_finite() {
        coefficients
    } else {
        BiquadCoefficients::identity()
    }
}

impl BiquadCoefficients {
    /// A pass-through section: `y[n] = x[n]`.
    pub fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    /// `true` when every coefficient is a finite number.
    pub fn is_finite(&self) -> bool {
        self.b0.is_finite()
            && self.b1.is_finite()
            && self.b2.is_finite()
            && self.a1.is_finite()
            && self.a2.is_finite()
    }

    /// Two-pole low-pass, -12 dB per octave above `frequency`.
    pub fn lowpass(sample_rate: f64, frequency: f32, q: f32) -> Self {
        let Some(d) = design(sample_rate, frequency as f64, q as f64) else {
            return Self::identity();
        };
        let (cos_w0, alpha) = (d.cos_w0, d.alpha);
        normalize(
            (1.0 - cos_w0) * 0.5,
            1.0 - cos_w0,
            (1.0 - cos_w0) * 0.5,
            1.0 + alpha,
            -2.0 * cos_w0,
            1.0 - alpha,
        )
    }

    /// Two-pole high-pass, -12 dB per octave below `frequency`.
    pub fn highpass(sample_rate: f64, frequency: f32, q: f32) -> Self {
        let Some(d) = design(sample_rate, frequency as f64, q as f64) else {
            return Self::identity();
        };
        let (cos_w0, alpha) = (d.cos_w0, d.alpha);
        normalize(
            (1.0 + cos_w0) * 0.5,
            -(1.0 + cos_w0),
            (1.0 + cos_w0) * 0.5,
            1.0 + alpha,
            -2.0 * cos_w0,
            1.0 - alpha,
        )
    }

    /// Band-pass with 0 dB gain at `frequency` (the cookbook's "constant peak gain" variant).
    pub fn bandpass(sample_rate: f64, frequency: f32, q: f32) -> Self {
        let Some(d) = design(sample_rate, frequency as f64, q as f64) else {
            return Self::identity();
        };
        let (cos_w0, alpha) = (d.cos_w0, d.alpha);
        normalize(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha)
    }

    /// Band-reject notch centred on `frequency`.
    pub fn notch(sample_rate: f64, frequency: f32, q: f32) -> Self {
        let Some(d) = design(sample_rate, frequency as f64, q as f64) else {
            return Self::identity();
        };
        let (cos_w0, alpha) = (d.cos_w0, d.alpha);
        normalize(
            1.0,
            -2.0 * cos_w0,
            1.0,
            1.0 + alpha,
            -2.0 * cos_w0,
            1.0 - alpha,
        )
    }

    /// Unity-magnitude all-pass; only the phase changes, by 180 degrees at `frequency`.
    pub fn allpass(sample_rate: f64, frequency: f32, q: f32) -> Self {
        let Some(d) = design(sample_rate, frequency as f64, q as f64) else {
            return Self::identity();
        };
        let (cos_w0, alpha) = (d.cos_w0, d.alpha);
        normalize(
            1.0 - alpha,
            -2.0 * cos_w0,
            1.0 + alpha,
            1.0 + alpha,
            -2.0 * cos_w0,
            1.0 - alpha,
        )
    }

    /// Bell filter: `gain_db` of boost or cut at `frequency`, unity gain far from it.
    pub fn peaking(sample_rate: f64, frequency: f32, q: f32, gain_db: f32) -> Self {
        let Some(d) = design(sample_rate, frequency as f64, q as f64) else {
            return Self::identity();
        };
        let (cos_w0, alpha) = (d.cos_w0, d.alpha);
        // The cookbook works with the *amplitude* square root of the gain, so that a peaking
        // section's skirt gain is the geometric mean of unity and the peak gain.
        let a = 10f64.powf(gain_db as f64 / 40.0);
        normalize(
            1.0 + alpha * a,
            -2.0 * cos_w0,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos_w0,
            1.0 - alpha / a,
        )
    }

    /// Low shelf: `gain_db` applied below `frequency`, unity above it.
    pub fn low_shelf(sample_rate: f64, frequency: f32, q: f32, gain_db: f32) -> Self {
        let Some(d) = design(sample_rate, frequency as f64, q as f64) else {
            return Self::identity();
        };
        let (cos_w0, alpha) = (d.cos_w0, d.alpha);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let sqrt_a2alpha = 2.0 * a.sqrt() * alpha;
        normalize(
            a * ((a + 1.0) - (a - 1.0) * cos_w0 + sqrt_a2alpha),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
            a * ((a + 1.0) - (a - 1.0) * cos_w0 - sqrt_a2alpha),
            (a + 1.0) + (a - 1.0) * cos_w0 + sqrt_a2alpha,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
            (a + 1.0) + (a - 1.0) * cos_w0 - sqrt_a2alpha,
        )
    }

    /// High shelf: `gain_db` applied above `frequency`, unity below it.
    pub fn high_shelf(sample_rate: f64, frequency: f32, q: f32, gain_db: f32) -> Self {
        let Some(d) = design(sample_rate, frequency as f64, q as f64) else {
            return Self::identity();
        };
        let (cos_w0, alpha) = (d.cos_w0, d.alpha);
        let a = 10f64.powf(gain_db as f64 / 40.0);
        let sqrt_a2alpha = 2.0 * a.sqrt() * alpha;
        normalize(
            a * ((a + 1.0) + (a - 1.0) * cos_w0 + sqrt_a2alpha),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
            a * ((a + 1.0) + (a - 1.0) * cos_w0 - sqrt_a2alpha),
            (a + 1.0) - (a - 1.0) * cos_w0 + sqrt_a2alpha,
            2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
            (a + 1.0) - (a - 1.0) * cos_w0 - sqrt_a2alpha,
        )
    }

    /// Magnitude of the frequency response at `frequency_hz`, in dB.
    ///
    /// Evaluates `|H(e^jw)|` directly, which lets the UI draw an EQ curve without pushing audio
    /// through the filter. The result is clamped to +/-120 dB so a pole sitting on the unit
    /// circle cannot produce an infinite plot value.
    pub fn magnitude_db(&self, frequency_hz: f32, sample_rate: f64) -> f32 {
        if !sample_rate.is_finite() || sample_rate <= 0.0 || !frequency_hz.is_finite() {
            return 0.0;
        }
        let nyquist = sample_rate * 0.5;
        let frequency = (frequency_hz as f64).clamp(0.0, nyquist);
        let w = 2.0 * PI * frequency / sample_rate;
        let (sin_w, cos_w) = w.sin_cos();
        let (sin_2w, cos_2w) = (2.0 * w).sin_cos();

        // H(e^jw) = (b0 + b1 e^-jw + b2 e^-2jw) / (1 + a1 e^-jw + a2 e^-2jw)
        let num_re = self.b0 as f64 + self.b1 as f64 * cos_w + self.b2 as f64 * cos_2w;
        let num_im = -(self.b1 as f64 * sin_w + self.b2 as f64 * sin_2w);
        let den_re = 1.0 + self.a1 as f64 * cos_w + self.a2 as f64 * cos_2w;
        let den_im = -(self.a1 as f64 * sin_w + self.a2 as f64 * sin_2w);

        let num = num_re * num_re + num_im * num_im;
        let den = den_re * den_re + den_im * den_im;
        if den <= f64::MIN_POSITIVE {
            return 120.0;
        }
        let db = 10.0 * (num / den).max(1.0e-12).log10();
        db.clamp(-120.0, 120.0) as f32
    }
}

/// A biquad section in transposed direct form II.
#[derive(Copy, Clone, Debug)]
pub struct Biquad {
    coefficients: BiquadCoefficients,
    s1: f32,
    s2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new(BiquadCoefficients::identity())
    }
}

impl Biquad {
    /// A filter with the given coefficients and cleared state.
    pub fn new(coefficients: BiquadCoefficients) -> Self {
        Self {
            coefficients,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// Replaces the coefficients, keeping the filter state so the change is click-free.
    pub fn set_coefficients(&mut self, coefficients: BiquadCoefficients) {
        self.coefficients = coefficients;
    }

    /// The coefficients currently in use.
    pub fn coefficients(&self) -> BiquadCoefficients {
        self.coefficients
    }

    /// Clears the filter memory.
    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }

    /// Filters one sample.
    #[inline]
    pub fn process_sample(&mut self, input: f32) -> f32 {
        // Silence, not arithmetic: both state taps recirculate through every later output, so
        // one non-finite input would latch the filter — see `crate::settled`, which also
        // flushes the taps' denormal tails on the way through.
        let input = if input.is_finite() { input } else { 0.0 };
        let c = &self.coefficients;
        let output = c.b0 * input + self.s1;
        self.s1 = crate::settled(c.b1 * input - c.a1 * output + self.s2);
        self.s2 = crate::settled(c.b2 * input - c.a2 * output);
        output
    }

    /// Filters a block in place.
    pub fn process_block(&mut self, samples: &mut [f32]) {
        for sample in samples.iter_mut() {
            *sample = self.process_sample(*sample);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    /// RMS of `frequency` after `filter`, relative to the input, in dB.
    fn measured_response_db(mut filter: Biquad, frequency: f32, sample_rate: f64) -> f32 {
        let frames = (sample_rate * 0.5) as usize;
        let step = std::f32::consts::TAU * frequency / sample_rate as f32;
        let mut sum_in = 0.0f64;
        let mut sum_out = 0.0f64;
        // Discard the first 20 % so the transient does not pollute the measurement.
        let settle = frames / 5;
        for n in 0..frames {
            let x = (step * n as f32).sin();
            let y = filter.process_sample(x);
            if n >= settle {
                sum_in += (x * x) as f64;
                sum_out += (y * y) as f64;
            }
        }
        20.0 * (sum_out.sqrt() / sum_in.sqrt()).log10() as f32
    }

    #[test]
    fn one_khz_passes_a_hundred_hertz_high_pass() {
        let coefficients = BiquadCoefficients::highpass(SR, 100.0, 0.707);
        let predicted = coefficients.magnitude_db(1_000.0, SR);
        let measured = measured_response_db(Biquad::new(coefficients), 1_000.0, SR);
        // Two decades above a 2-pole corner the response is flat to a small fraction of a dB.
        assert!(predicted > -0.1, "predicted {predicted} dB");
        assert!((measured - predicted).abs() < 0.1, "measured {measured} dB");
    }

    #[test]
    fn a_hundred_hertz_is_attenuated_by_a_one_khz_high_pass() {
        let coefficients = BiquadCoefficients::highpass(SR, 1_000.0, 0.707);
        let predicted = coefficients.magnitude_db(100.0, SR);
        let measured = measured_response_db(Biquad::new(coefficients), 100.0, SR);
        // One decade below the corner a 2-pole high-pass is 40 dB down.
        assert!(
            (predicted + 40.0).abs() < 1.0,
            "predicted {predicted} dB, expected about -40"
        );
        assert!((measured - predicted).abs() < 0.2, "measured {measured} dB");
    }

    #[test]
    fn one_khz_passes_a_ten_khz_low_pass() {
        let coefficients = BiquadCoefficients::lowpass(SR, 10_000.0, 0.707);
        let predicted = coefficients.magnitude_db(1_000.0, SR);
        let measured = measured_response_db(Biquad::new(coefficients), 1_000.0, SR);
        assert!(predicted > -0.1, "predicted {predicted} dB");
        assert!((measured - predicted).abs() < 0.1, "measured {measured} dB");
    }

    #[test]
    fn peaking_hits_its_gain_at_the_centre_frequency() {
        for gain_db in [-12.0, -6.0, 3.0, 9.0, 18.0] {
            let coefficients = BiquadCoefficients::peaking(SR, 1_000.0, 1.0, gain_db);
            let predicted = coefficients.magnitude_db(1_000.0, SR);
            assert!(
                (predicted - gain_db).abs() < 0.01,
                "{gain_db} dB band predicted {predicted} dB"
            );
            let measured = measured_response_db(Biquad::new(coefficients), 1_000.0, SR);
            assert!(
                (measured - gain_db).abs() < 0.1,
                "{gain_db} dB band measured {measured} dB"
            );
        }
    }

    #[test]
    fn shelves_reach_their_gain_at_the_extremes() {
        let low = BiquadCoefficients::low_shelf(SR, 200.0, 0.707, 6.0);
        assert!((low.magnitude_db(10.0, SR) - 6.0).abs() < 0.1);
        assert!(low.magnitude_db(20_000.0, SR).abs() < 0.1);

        let high = BiquadCoefficients::high_shelf(SR, 4_000.0, 0.707, -6.0);
        assert!((high.magnitude_db(20_000.0, SR) + 6.0).abs() < 0.1);
        assert!(high.magnitude_db(20.0, SR).abs() < 0.1);
    }

    #[test]
    fn zero_gain_bands_are_exactly_transparent() {
        // At 0 dB the numerator and denominator polynomials coincide, so H(z) is exactly 1
        // even though the coefficients are not the identity section's.
        let peak = BiquadCoefficients::peaking(SR, 1_000.0, 1.0, 0.0);
        assert_eq!(peak.b0, 1.0);
        assert_eq!(peak.b1, peak.a1);
        assert_eq!(peak.b2, peak.a2);

        for frequency in [20.0, 1_000.0, 20_000.0] {
            assert!(peak.magnitude_db(frequency, SR).abs() < 1e-4);
            let shelf = BiquadCoefficients::low_shelf(SR, 120.0, 0.707, 0.0);
            assert!(shelf.magnitude_db(frequency, SR).abs() < 1e-4);
            let shelf = BiquadCoefficients::high_shelf(SR, 8_000.0, 0.707, 0.0);
            assert!(shelf.magnitude_db(frequency, SR).abs() < 1e-4);
        }
    }

    #[test]
    fn frequencies_past_nyquist_stay_stable() {
        // 20 kHz at 44.1 kHz is legal; 30 kHz is not, and must not produce infinities.
        for frequency in [20_000.0f32, 22_050.0, 30_000.0, 1.0e9] {
            let coefficients = BiquadCoefficients::lowpass(44_100.0, frequency, 0.707);
            assert!(
                coefficients.is_finite(),
                "{frequency} Hz produced {coefficients:?}"
            );
            let mut filter = Biquad::new(coefficients);
            let mut peak = 0.0f32;
            for n in 0..4_800 {
                let x = (0.1 * n as f32).sin();
                peak = peak.max(filter.process_sample(x).abs());
            }
            assert!(
                peak.is_finite() && peak < 10.0,
                "{frequency} Hz peaked at {peak}"
            );
        }
    }

    #[test]
    fn allpass_keeps_unity_magnitude() {
        let coefficients = BiquadCoefficients::allpass(SR, 1_000.0, 0.707);
        for frequency in [50.0, 500.0, 1_000.0, 5_000.0, 15_000.0] {
            assert!(coefficients.magnitude_db(frequency, SR).abs() < 1e-4);
        }
    }

    #[test]
    fn notch_rejects_its_centre_frequency() {
        let coefficients = BiquadCoefficients::notch(SR, 1_000.0, 4.0);
        assert!(coefficients.magnitude_db(1_000.0, SR) < -80.0);
        assert!(coefficients.magnitude_db(100.0, SR).abs() < 0.5);
    }

    #[test]
    fn bandpass_is_unity_at_its_centre_and_rolls_off_both_ways() {
        for q in [0.5f32, 1.0, 4.0, 10.0] {
            let coefficients = BiquadCoefficients::bandpass(SR, 1_000.0, q);
            // The cookbook's constant-peak-gain variant is exactly 0 dB at w0 by construction.
            let at_centre = coefficients.magnitude_db(1_000.0, SR);
            assert!(at_centre.abs() < 1e-4, "q {q} peaked at {at_centre} dB");
            let measured = measured_response_db(Biquad::new(coefficients), 1_000.0, SR);
            assert!((measured).abs() < 0.1, "q {q} measured {measured} dB");
            assert!(coefficients.magnitude_db(50.0, SR) < -10.0);
            assert!(coefficients.magnitude_db(18_000.0, SR) < -10.0);
        }
    }

    #[test]
    fn butterworth_q_puts_the_corner_at_minus_three_db() {
        // The defining property of a Q of 1/sqrt(2): the response at the design frequency is
        // exactly half power. Getting `alpha` wrong by any factor moves this.
        for frequency in [100.0f32, 1_000.0, 8_000.0] {
            let low = BiquadCoefficients::lowpass(SR, frequency, std::f32::consts::FRAC_1_SQRT_2);
            let at_corner = low.magnitude_db(frequency, SR);
            assert!(
                (at_corner + 3.0103).abs() < 0.05,
                "low-pass at {frequency} Hz was {at_corner} dB"
            );
            let high = BiquadCoefficients::highpass(SR, frequency, std::f32::consts::FRAC_1_SQRT_2);
            let at_corner = high.magnitude_db(frequency, SR);
            assert!(
                (at_corner + 3.0103).abs() < 0.05,
                "high-pass at {frequency} Hz was {at_corner} dB"
            );
        }
    }

    #[test]
    fn every_design_agrees_with_audio_pushed_through_it() {
        // `magnitude_db` and the difference equation are separate pieces of code reading the same
        // coefficients; this pins them to each other across the whole design family.
        let designs: [(&str, BiquadCoefficients); 8] = [
            ("lowpass q4", BiquadCoefficients::lowpass(SR, 1_000.0, 4.0)),
            ("highpass", BiquadCoefficients::highpass(SR, 1_000.0, 0.707)),
            ("bandpass", BiquadCoefficients::bandpass(SR, 1_000.0, 2.0)),
            ("notch", BiquadCoefficients::notch(SR, 1_000.0, 2.0)),
            ("allpass", BiquadCoefficients::allpass(SR, 1_000.0, 0.707)),
            (
                "peaking",
                BiquadCoefficients::peaking(SR, 1_000.0, 1.5, 9.0),
            ),
            (
                "low_shelf",
                BiquadCoefficients::low_shelf(SR, 300.0, 0.707, 8.0),
            ),
            (
                "high_shelf",
                BiquadCoefficients::high_shelf(SR, 4_000.0, 0.707, -8.0),
            ),
        ];
        for (name, coefficients) in designs {
            for frequency in [40.0f32, 200.0, 700.0, 1_500.0, 4_000.0, 12_000.0] {
                let predicted = coefficients.magnitude_db(frequency, SR);
                if predicted < -40.0 {
                    // A deep null needs an impractically long average to measure accurately.
                    continue;
                }
                let measured = measured_response_db(Biquad::new(coefficients), frequency, SR);
                assert!(
                    (predicted - measured).abs() < 0.2,
                    "{name} at {frequency} Hz: curve {predicted} dB, audio {measured} dB"
                );
            }
        }
    }

    #[test]
    fn a_non_finite_sample_does_not_latch_the_filter() {
        // One NaN used to sit in the state taps for ever — every later output NaN, and the
        // denormal flush blind to it, a NaN failing every comparison — so one bad sample
        // silenced the band until reset. The filter has to keep answering afterwards.
        for poison in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut filter = Biquad::new(BiquadCoefficients::peaking(SR, 1_000.0, 1.0, 6.0));
            filter.process_sample(0.5);
            filter.process_sample(poison);

            let step = std::f32::consts::TAU * 1_000.0 / SR as f32;
            let mut peak = 0.0f32;
            for n in 0..1_000 {
                let output = filter.process_sample((step * n as f32).sin());
                assert!(output.is_finite(), "{poison}: output went non-finite");
                peak = peak.max(output.abs());
            }
            assert!(peak > 0.5, "{poison}: the filter never recovered ({peak})");
        }
    }
}
