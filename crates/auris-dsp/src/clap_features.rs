//! Deterministic, offline log-mel input for the exported HTSAT unfused CLAP model.
//!
//! Coefficients come from the model bundle's pinned Transformers feature extractor. This is
//! analysis only: waveform gain is preserved and no synthesis or playback constants change.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use rustfft::{Fft, FftPlanner, num_complex::Complex};

/// One ten-second model window at 48 kHz.
pub const CLAP_SAMPLES: usize = 480_000;
/// Centered frames, including the frame at the ten-second boundary.
pub const CLAP_FRAMES: usize = 1001;
/// Slaney mel bands expected by the unfused model.
pub const CLAP_MELS: usize = 64;
const FFT_SIZE: usize = 1024;
const BINS: usize = FFT_SIZE / 2 + 1;
const HOP: usize = 480;

/// Reusable FFT plan and sparse mel filters for worker-thread CLAP preprocessing.
pub struct ClapFrontend {
    window: Vec<f64>,
    filters: Vec<Vec<(usize, f64)>>,
    fft: Arc<dyn Fft<f64>>,
}

impl ClapFrontend {
    /// Loads a periodic Hann window followed by a row-major 513 by 64 mel matrix.
    pub fn new(coefficients: &[f64]) -> Result<Self, &'static str> {
        if coefficients.len() != FFT_SIZE + BINS * CLAP_MELS
            || coefficients.iter().any(|x| !x.is_finite() || *x < 0.0)
            || !coefficients[..FFT_SIZE].iter().any(|x| *x > 0.0)
        {
            return Err("invalid CLAP preprocessing coefficients");
        }
        let filters: Vec<Vec<_>> = (0..CLAP_MELS)
            .map(|band| {
                (0..BINS)
                    .filter_map(|bin| {
                        let weight = coefficients[FFT_SIZE + bin * CLAP_MELS + band];
                        (weight > 0.0).then_some((bin, weight))
                    })
                    .collect()
            })
            .collect();
        if filters.iter().any(Vec::is_empty) {
            return Err("CLAP mel filters contain an empty band");
        }
        Ok(Self {
            window: coefficients[..FFT_SIZE].to_vec(),
            filters,
            fft: FftPlanner::<f64>::new().plan_fft_forward(FFT_SIZE),
        })
    }

    /// Produces time-major 1001 by 64 decibel features from up to ten seconds of 48 kHz mono.
    ///
    /// Short inputs repeat a whole number of times, then zero-pad the remainder, matching
    /// the model's `repeatpad` policy. Center padding reflects without repeating edge samples.
    /// No random cropping occurs. Cancellation is checked once per STFT frame.
    pub fn extract(&self, mono: &[f32], cancel: &AtomicBool) -> Result<Vec<f32>, &'static str> {
        if mono.is_empty() || mono.len() > CLAP_SAMPLES || mono.iter().any(|x| !x.is_finite()) {
            return Err("CLAP expects 1 to 480000 finite mono samples at 48 kHz");
        }
        let mut buffer = vec![Complex::default(); FFT_SIZE];
        let mut scratch = vec![Complex::default(); self.fft.get_inplace_scratch_len()];
        let mut power = vec![0.0; BINS];
        let mut features = Vec::with_capacity(CLAP_FRAMES * CLAP_MELS);
        for frame in 0..CLAP_FRAMES {
            if cancel.load(Ordering::Relaxed) {
                return Err("CLAP evaluation cancelled");
            }
            for (offset, (value, window)) in buffer.iter_mut().zip(&self.window).enumerate() {
                let index = frame * HOP + offset;
                let reflected = if index < FFT_SIZE / 2 {
                    FFT_SIZE / 2 - index
                } else if index - FFT_SIZE / 2 >= CLAP_SAMPLES {
                    2 * CLAP_SAMPLES - 2 - (index - FFT_SIZE / 2)
                } else {
                    index - FFT_SIZE / 2
                };
                *value = Complex::new(repeatpad_sample(mono, reflected) as f64 * window, 0.0);
            }
            self.fft.process_with_scratch(&mut buffer, &mut scratch);
            for (power, complex) in power.iter_mut().zip(&buffer) {
                // The reference FFT runs in f64, stores complex64, then computes f64 power.
                let re = complex.re as f32 as f64;
                let im = complex.im as f32 as f64;
                *power = re * re + im * im;
            }
            for filter in &self.filters {
                let energy: f64 = filter
                    .iter()
                    .map(|&(bin, weight)| power[bin] * weight)
                    .sum();
                features.push((10.0 * energy.max(1e-10).log10()) as f32);
            }
        }
        Ok(features)
    }
}

fn repeatpad_sample(mono: &[f32], index: usize) -> f32 {
    if index < (CLAP_SAMPLES / mono.len()) * mono.len() {
        mono[index % mono.len()]
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coefficients() -> Vec<f64> {
        let mut coefficients = vec![0.0; FFT_SIZE + BINS * CLAP_MELS];
        coefficients[..FFT_SIZE].fill(1.0);
        // Each band selects the same exact FFT bin; expected powers have closed forms.
        coefficients[FFT_SIZE..FFT_SIZE + CLAP_MELS].fill(1.0);
        coefficients
    }

    #[test]
    fn centered_constant_and_silence_have_exact_power_and_shape() {
        let frontend = ClapFrontend::new(&coefficients()).unwrap();
        let cancel = AtomicBool::new(false);
        let features = frontend.extract(&[0.25], &cancel).unwrap();
        assert_eq!(features.len(), CLAP_FRAMES * CLAP_MELS);
        let expected = (10.0 * (256.0_f64 * 256.0).log10()) as f32;
        assert!(features.iter().all(|x| (*x - expected).abs() < 1e-5));
        assert!(
            frontend
                .extract(&[0.0], &cancel)
                .unwrap()
                .iter()
                .all(|x| *x == -100.0)
        );
    }

    #[test]
    fn repeatpad_preserves_whole_repeats_then_zeros_instead_of_cycling_to_end() {
        let mono = vec![0.25; 170_000];
        assert_eq!(repeatpad_sample(&mono, 339_999), 0.25);
        assert_eq!(repeatpad_sample(&mono, 340_000), 0.0);
        assert_eq!(repeatpad_sample(&mono, CLAP_SAMPLES - 1), 0.0);
        let frontend = ClapFrontend::new(&coefficients()).unwrap();
        let features = frontend.extract(&mono, &AtomicBool::new(false)).unwrap();
        assert_eq!(*features.last().unwrap(), -100.0);
        assert!(features[0] > 40.0);
    }

    #[test]
    fn rejects_malformed_coefficients_pcm_and_cancellation() {
        assert!(ClapFrontend::new(&[]).is_err());
        assert!(ClapFrontend::new(&vec![0.0; coefficients().len()]).is_err());
        let mut bad = coefficients();
        bad[10] = f64::NAN;
        assert!(ClapFrontend::new(&bad).is_err());
        let frontend = ClapFrontend::new(&coefficients()).unwrap();
        for samples in [vec![], vec![f32::INFINITY], vec![0.0; CLAP_SAMPLES + 1]] {
            assert!(frontend.extract(&samples, &AtomicBool::new(false)).is_err());
        }
        assert_eq!(
            frontend
                .extract(&[0.1], &AtomicBool::new(true))
                .unwrap_err(),
            "CLAP evaluation cancelled"
        );
    }
}
