//! Compact, reference-derived radiation coloration, designed at the host sample rate.
//!
//! These regularized spectral priors are fitted with temporal mel loss against real
//! University of Iowa recordings. They include recording and excitation coloration;
//! they are not measured isolated bridge admittances. See docs/physical-copy-synthesis.md.

use auris_dsp::{Biquad, BiquadCoefficients};

use super::Model;

const BANDS: usize = 12;

#[derive(Clone, Debug, Default)]
pub(super) struct Body {
    sections: [Biquad; BANDS],
    legacy: [Biquad; 3],
    fitted: bool,
}

impl Body {
    pub(super) fn prepare(&mut self, model: Model, rate: f32) {
        let gains = match model {
            Model::Piano => Some([
                -8.998, -4.394, 9.000, 9.000, 0.001, -0.445, 9.000, 9.000, 8.190, 5.707, 2.783,
                0.311,
            ]),
            Model::Guitar => Some([
                -1.461, 5.898, 8.876, 7.226, 1.867, -0.267, -0.352, 0.458, 1.530, 1.882, 0.962,
                -0.275,
            ]),
            Model::Violin => Some([
                -6.352, -6.253, -1.862, 6.249, 4.245, -2.720, -3.706, 0.414, 3.636, 4.732, 3.702,
                3.117,
            ]),
            _ => None,
        };
        self.fitted = gains.is_some();
        if let Some(gains) = gains {
            for (index, (section, gain)) in self.sections.iter_mut().zip(gains).enumerate() {
                let hz = 90.0_f32 * (10_000.0_f32 / 90.0).powf(index as f32 / 11.0);
                section.set_coefficients(if hz < rate * 0.45 {
                    BiquadCoefficients::peaking(f64::from(rate), hz, 0.9, gain)
                } else {
                    BiquadCoefficients::identity()
                });
            }
        }
        let frequencies = if model == Model::Bass {
            [70.0, 180.0, 430.0]
        } else {
            [350.0, 900.0, 2400.0]
        };
        for (section, frequency) in self.legacy.iter_mut().zip(frequencies) {
            section.set_coefficients(BiquadCoefficients::bandpass(
                f64::from(rate),
                frequency,
                1.8,
            ));
        }
        self.reset();
    }

    pub(super) fn next(&mut self, input: f32, amount: f32) -> f32 {
        if self.fitted {
            let wet = self
                .sections
                .iter_mut()
                .fold(input, |sample, section| section.process_sample(sample));
            input + amount * (wet - input)
        } else {
            input
                + amount
                    * self
                        .legacy
                        .iter_mut()
                        .map(|filter| filter.process_sample(input))
                        .sum::<f32>()
        }
    }

    pub(super) fn reset(&mut self) {
        for section in self.sections.iter_mut().chain(&mut self.legacy) {
            section.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{goertzel, peak};

    #[test]
    fn radiation_changes_spectral_shape_without_unstable_or_dc_tails() {
        for model in [Model::Piano, Model::Guitar, Model::Violin] {
            for rate in [8000.0, 44100.0, 48000.0, 96000.0, 192000.0] {
                let mut body = Body::default();
                body.prepare(model, rate);
                let impulse: Vec<_> = (0..rate as usize)
                    .map(|i| body.next(if i == 0 { 1.0 } else { 0.0 }, 1.0))
                    .collect();
                assert!(impulse.iter().all(|sample| sample.is_finite()));
                assert!(peak(&impulse) < 8.0);
                assert!(peak(&impulse[(rate * 0.5) as usize..]) < 1e-5);
                let dc: f32 = impulse.iter().sum();
                assert!((dc - 1.0).abs() < 0.03, "{model:?} at {rate}: DC {dc}");
                let low = goertzel(&impulse, f64::from(rate), 200.0);
                let upper = goertzel(&impulse, f64::from(rate), 2000.0);
                assert!((upper / low - 1.0).abs() > 0.05, "no coloration: {model:?}");
            }
        }
    }
}
