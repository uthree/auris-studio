//! Turning a block of samples into a picture of what is in it.
//!
//! An equalizer draws the curve it is *going* to apply. What it cannot draw without this is the
//! signal it is applying it to, which is the half that tells you where to put the curve.
//!
//! # Where the work happens
//!
//! Splitting the job matters more than the arithmetic. The audio thread does the cheapest
//! possible thing — copy samples into a ring — and whoever is drawing does the transform, at
//! whatever rate it repaints. An FFT on the callback thread would be affordable at this size and
//! still wrong in principle: the analysis exists for a window that may not even be open, and the
//! render deadline is not the place to find out.
//!
use std::f32::consts::TAU;
use std::fmt;
use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use rustfft::FftPlanner;

/// Level reported for a bin with nothing in it.
///
/// Far enough down to read as silence on any scale a display might use, and finite, so a caller
/// can average or interpolate bins without special-casing negative infinity.
pub const SILENCE_DB: f32 = -120.0;

/// An in-place radix-2 FFT.
///
/// `real` and `imag` must be the same length and that length must be a power of two; anything
/// else is left untouched, because the alternative is a panic somewhere a caller cannot see.
///
/// The planned transform comes from `rustfft`, whose planner selects the best available SIMD
/// implementation for the machine. This compatibility entry point keeps separate real and
/// imaginary slices; [`SpectrumAnalyzer`] uses the cheaper real-input transform directly.
pub fn fft(real: &mut [f32], imag: &mut [f32]) {
    let n = real.len();
    if n != imag.len() || n < 2 || !n.is_power_of_two() {
        return;
    }
    let mut values: Vec<Complex<f32>> = real
        .iter()
        .copied()
        .zip(imag.iter().copied())
        .map(|(re, im)| Complex::new(re, im))
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut values);
    for ((real, imag), value) in real.iter_mut().zip(imag).zip(values) {
        *real = value.re;
        *imag = value.im;
    }
}

/// A rolling window of the signal, and the transform of it.
///
/// [`Self::push`] is the audio thread's half: it copies into a ring and does nothing else, so it
/// allocates nothing and takes time proportional to the block. [`Self::magnitudes`] is the
/// drawing thread's half, and does all the arithmetic.
#[derive(Clone)]
pub struct SpectrumAnalyzer {
    window: Vec<f32>,
    ring: Vec<f32>,
    write: usize,
    transform: Arc<dyn RealToComplex<f32>>,
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl fmt::Debug for SpectrumAnalyzer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpectrumAnalyzer")
            .field("size", &self.ring.len())
            .field("write", &self.write)
            .finish_non_exhaustive()
    }
}

impl SpectrumAnalyzer {
    /// An analyser over `size` samples, rounded up to a power of two and at least 64.
    ///
    /// A Hann window, because the alternative — no window at all — makes every tone in the signal
    /// smear across the whole display unless its frequency happens to land exactly on a bin,
    /// which no musical tone does.
    pub fn new(size: usize) -> Self {
        let size = size.max(64).next_power_of_two();
        let window = (0..size)
            .map(|index| 0.5 - 0.5 * (TAU * index as f32 / size as f32).cos())
            .collect();
        let transform = RealFftPlanner::<f32>::new().plan_fft_forward(size);
        let input = transform.make_input_vec();
        let spectrum = transform.make_output_vec();
        let scratch = transform.make_scratch_vec();
        Self {
            window,
            ring: vec![0.0; size],
            write: 0,
            transform,
            input,
            spectrum,
            scratch,
        }
    }

    /// How many samples one transform covers.
    pub fn size(&self) -> usize {
        self.ring.len()
    }

    /// How many bins [`Self::magnitudes`] fills.
    ///
    /// Half the window plus one: a real signal's spectrum is symmetric, so everything above the
    /// midpoint is the mirror of something below it and carries no further information.
    pub fn bin_count(&self) -> usize {
        self.ring.len() / 2 + 1
    }

    /// Centre frequency of a bin, at `sample_rate`.
    pub fn bin_frequency(&self, bin: usize, sample_rate: f64) -> f64 {
        bin as f64 * sample_rate / self.ring.len() as f64
    }

    /// Adds samples to the window, overwriting the oldest.
    ///
    /// Realtime-safe: no allocation, no locking, and a block longer than the window keeps only
    /// its tail, which is all the window could have held anyway.
    pub fn push(&mut self, samples: &[f32]) {
        let size = self.ring.len();
        let tail = samples.len().saturating_sub(size);
        for sample in &samples[tail..] {
            self.ring[self.write] = *sample;
            self.write = (self.write + 1) % size;
        }
    }

    /// Empties the window.
    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.write = 0;
    }

    /// Fills `out` with the level of each bin, in dBFS.
    ///
    /// `out` is filled up to whichever is shorter, so a caller drawing fewer bins than the window
    /// holds costs itself nothing.
    pub fn magnitudes(&mut self, out: &mut [f32]) {
        let size = self.ring.len();
        // Read the ring oldest-first, so the window function lands on the window rather than on
        // whatever rotation the write cursor happens to be at.
        for index in 0..size {
            let sample = self.ring[(self.write + index) % size];
            self.input[index] = sample * self.window[index];
        }
        if self
            .transform
            .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch)
            .is_err()
        {
            out.fill(SILENCE_DB);
            return;
        }

        // A Hann window sums to half its length, and the transform of a real signal splits each
        // tone between a bin and its mirror — so the scale that puts a full-scale sine at 0 dBFS
        // is 4/size rather than 1/size.
        let scale = 4.0 / size as f32;
        let bins = self.bin_count().min(out.len());
        for (bin, slot) in out.iter_mut().enumerate().take(bins) {
            let power = self.spectrum[bin].norm_sqr();
            // DC and Nyquist mirror onto themselves, so unlike an interior real-signal bin they
            // have no negative-frequency partner whose half of the amplitude must be recovered.
            let mirror = if bin == 0 || bin == size / 2 {
                0.5
            } else {
                1.0
            };
            let amplitude = power.sqrt() * scale * mirror;
            *slot = if amplitude > 0.0 {
                (20.0 * amplitude.log10()).max(SILENCE_DB)
            } else {
                SILENCE_DB
            };
        }
        for slot in out.iter_mut().skip(bins) {
            *slot = SILENCE_DB;
        }
    }
}

/// Gathers bins into bands spaced by octave between `low_hz` and `high_hz`.
///
/// Logarithmic, because pitch is: linear bins put nine tenths of a display above 2 kHz, where
/// almost nothing a person is equalising actually lives. Each band takes the loudest bin that
/// falls in it rather than the mean, so one tone stays a peak instead of being flattened by the
/// empty space either side of it.
///
/// Bands with no bin in them are left at [`SILENCE_DB`], which at the bottom of a display is
/// indistinguishable from a band that is genuinely quiet — the alternative, interpolating, would
/// invent energy at frequencies the window cannot resolve.
pub fn bands_from_bins(bins: &[f32], sample_rate: f64, low_hz: f64, high_hz: f64, out: &mut [f32]) {
    out.fill(SILENCE_DB);
    if bins.len() < 2 || out.is_empty() || sample_rate <= 0.0 || low_hz <= 0.0 || high_hz <= low_hz
    {
        return;
    }
    let span = (high_hz / low_hz).log2();
    // `bins` covers 0..=nyquist in `bins.len() - 1` steps, so one step is this wide.
    let hz_per_bin = sample_rate / 2.0 / (bins.len() - 1) as f64;
    for (index, level) in bins.iter().enumerate() {
        let frequency = index as f64 * hz_per_bin;
        if frequency < low_hz || frequency > high_hz {
            continue;
        }
        let position = (frequency / low_hz).log2() / span;
        let band = ((position * out.len() as f64) as usize).min(out.len() - 1);
        out[band] = out[band].max(*level);
    }
}

/// A sine at `frequency`, for tests and for anyone measuring a filter.
pub fn sine(frequency: f64, sample_rate: f64, amplitude: f32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|frame| {
            let phase = TAU as f64 * frequency * frame as f64 / sample_rate;
            amplitude * phase.sin() as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which bin holds the most energy.
    fn loudest(bins: &[f32]) -> usize {
        bins.iter()
            .enumerate()
            .fold((0usize, f32::NEG_INFINITY), |best, (bin, level)| {
                if *level > best.1 { (bin, *level) } else { best }
            })
            .0
    }

    #[test]
    fn the_transform_of_a_constant_is_all_in_the_first_bin() {
        let mut real = vec![1.0f32; 8];
        let mut imag = vec![0.0f32; 8];
        fft(&mut real, &mut imag);
        assert!((real[0] - 8.0).abs() < 1e-4, "got {}", real[0]);
        for bin in 1..8 {
            assert!(real[bin].abs() < 1e-4, "bin {bin} is {}", real[bin]);
            assert!(imag[bin].abs() < 1e-4);
        }
    }

    #[test]
    fn a_sine_on_a_bin_centre_lands_entirely_in_that_bin() {
        // Three cycles across eight samples is exactly bin 3, so there is nothing to smear.
        let size = 8;
        let mut real: Vec<f32> = (0..size)
            .map(|n| (TAU * 3.0 * n as f32 / size as f32).sin())
            .collect();
        let mut imag = vec![0.0f32; size];
        fft(&mut real, &mut imag);
        let power: Vec<f32> = (0..size / 2 + 1)
            .map(|bin| real[bin] * real[bin] + imag[bin] * imag[bin])
            .collect();
        assert_eq!(loudest(&power), 3);
    }

    #[test]
    fn a_length_that_is_not_a_power_of_two_is_left_alone() {
        // A panic here would be on whichever thread happened to be drawing.
        let mut real = vec![1.0f32, 2.0, 3.0];
        let mut imag = vec![0.0f32; 3];
        fft(&mut real, &mut imag);
        assert_eq!(real, vec![1.0, 2.0, 3.0]);

        let mut mismatched_real = vec![1.0f32; 4];
        let mut short_imag = vec![0.0f32; 2];
        fft(&mut mismatched_real, &mut short_imag);
        assert_eq!(mismatched_real, vec![1.0; 4]);
    }

    #[test]
    fn a_full_scale_sine_reads_about_zero_decibels_at_its_own_frequency() {
        let rate = 48_000.0;
        let mut analyzer = SpectrumAnalyzer::new(1024);
        // A frequency that lands on a bin centre, so windowing does not spread the peak.
        let bin = 64;
        let frequency = analyzer.bin_frequency(bin, rate);
        analyzer.push(&sine(frequency, rate, 1.0, analyzer.size()));

        let mut bins = vec![0.0; analyzer.bin_count()];
        analyzer.magnitudes(&mut bins);
        assert_eq!(loudest(&bins), bin);
        assert!(
            (bins[bin] - 0.0).abs() < 1.0,
            "a full-scale sine read {} dBFS",
            bins[bin]
        );
        // And well away from it there is nothing.
        assert!(bins[bin * 4] < -60.0, "leakage at {} dBFS", bins[bin * 4]);
    }

    #[test]
    fn self_mirrored_bins_are_not_counted_twice() {
        let mut analyzer = SpectrumAnalyzer::new(1_024);
        analyzer.push(&vec![1.0; analyzer.size()]);
        let mut bins = vec![0.0; analyzer.bin_count()];
        analyzer.magnitudes(&mut bins);
        assert!(bins[0].abs() < 0.1, "DC read {} dBFS", bins[0]);

        analyzer.reset();
        let nyquist: Vec<f32> = (0..analyzer.size())
            .map(|frame| if frame % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        analyzer.push(&nyquist);
        analyzer.magnitudes(&mut bins);
        let last = bins.len() - 1;
        assert!(bins[last].abs() < 0.1, "Nyquist read {} dBFS", bins[last]);
    }

    #[test]
    fn halving_the_amplitude_costs_six_decibels() {
        let rate = 48_000.0;
        let mut analyzer = SpectrumAnalyzer::new(1024);
        let bin = 32;
        let frequency = analyzer.bin_frequency(bin, rate);
        let mut bins = vec![0.0; analyzer.bin_count()];

        analyzer.push(&sine(frequency, rate, 1.0, analyzer.size()));
        analyzer.magnitudes(&mut bins);
        let loud = bins[bin];

        analyzer.reset();
        analyzer.push(&sine(frequency, rate, 0.5, analyzer.size()));
        analyzer.magnitudes(&mut bins);
        assert!(
            (loud - bins[bin] - 6.02).abs() < 0.2,
            "{loud} then {}",
            bins[bin]
        );
    }

    #[test]
    fn silence_reads_as_silence_rather_than_as_negative_infinity() {
        let mut analyzer = SpectrumAnalyzer::new(256);
        let mut bins = vec![0.0; analyzer.bin_count()];
        analyzer.magnitudes(&mut bins);
        assert!(bins.iter().all(|level| *level == SILENCE_DB));
        assert!(bins.iter().all(|level| level.is_finite()));
    }

    #[test]
    fn pushing_keeps_the_most_recent_window() {
        // A block longer than the window keeps its tail: the oldest samples are the ones the
        // window could never have shown anyway.
        let mut analyzer = SpectrumAnalyzer::new(64);
        let long: Vec<f32> = (0..200).map(|n| n as f32).collect();
        analyzer.push(&long);
        assert_eq!(analyzer.ring[analyzer.write.wrapping_sub(1) % 64], 199.0);
    }

    #[test]
    fn a_window_smaller_than_the_floor_is_rounded_up() {
        let analyzer = SpectrumAnalyzer::new(3);
        assert_eq!(analyzer.size(), 64);
        assert_eq!(analyzer.bin_count(), 33);
        // And a size that is not a power of two goes up to the next one.
        assert_eq!(SpectrumAnalyzer::new(1000).size(), 1024);
    }

    #[test]
    fn bin_frequencies_run_from_nothing_to_half_the_rate() {
        let analyzer = SpectrumAnalyzer::new(1024);
        assert_eq!(analyzer.bin_frequency(0, 48_000.0), 0.0);
        assert_eq!(analyzer.bin_frequency(512, 48_000.0), 24_000.0);
    }

    #[test]
    fn pushing_never_allocates_however_long_the_block() {
        // The audio thread's half of the split. `push` is the only part of this module it runs.
        let mut analyzer = SpectrumAnalyzer::new(1024);
        let block = vec![0.25f32; 4096];
        let before = analyzer.ring.capacity();
        analyzer.push(&block);
        assert_eq!(analyzer.ring.capacity(), before);
    }

    #[test]
    fn the_window_is_a_hann_and_ends_at_zero() {
        let analyzer = SpectrumAnalyzer::new(64);
        assert!(analyzer.window[0].abs() < 1e-6);
        assert!((analyzer.window[32] - 1.0).abs() < 1e-6);
        // Symmetric about the middle, which is what stops a window from tilting the spectrum.
        for index in 1..32 {
            let mirrored = analyzer.window[64 - index];
            assert!((analyzer.window[index] - mirrored).abs() < 1e-6);
        }
    }
}
