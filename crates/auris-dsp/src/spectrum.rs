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

/// Level reported for a bin with nothing in it.
///
/// Far enough down to read as silence on any scale a display might use, and finite, so a caller
/// can average or interpolate bins without special-casing negative infinity.
pub const SILENCE_DB: f32 = -120.0;

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

/// [`bands_from_bins`], with a band too narrow to hold a bin of its own read off the straight
/// line between the two bins either side of its centre.
///
/// For a curve, and for a spectrogram drawn finer than the window resolves. At the bottom of the
/// range a twenty-fourth of an octave is a hertz or two and a bin is tens of them, so several
/// bands share every gap between bins. Repeating the nearest bin across them draws a staircase
/// whose risers are an artefact of where the bins happen to fall; joining the bins with straight
/// lines draws what the window measured, at the resolution it measured it, without the steps.
/// The interpolation is in decibels, as the display is, and it adds no level that neither
/// neighbour had.
pub fn bands_from_bins_interpolated(
    bins: &[f32],
    sample_rate: f64,
    low_hz: f64,
    high_hz: f64,
    out: &mut [f32],
) {
    bands_from_bins(bins, sample_rate, low_hz, high_hz, out);
    if bins.len() < 2 || out.is_empty() || sample_rate <= 0.0 || low_hz <= 0.0 || high_hz <= low_hz
    {
        return;
    }
    let span = (high_hz / low_hz).log2();
    let hz_per_bin = sample_rate / 2.0 / (bins.len() - 1) as f64;
    let count = out.len();
    let band_of = |frequency: f64| {
        let position = (frequency / low_hz).log2() / span;
        ((position * count as f64) as usize).min(count - 1)
    };
    for (band, level) in out.iter_mut().enumerate() {
        let from = low_hz * 2f64.powf(span * band as f64 / count as f64);
        let to = low_hz * 2f64.powf(span * (band + 1) as f64 / count as f64);
        let first = (from / hz_per_bin).ceil() as usize;
        let holds_a_bin = (first..bins.len())
            .take_while(|&bin| bin as f64 * hz_per_bin < to)
            .any(|bin| {
                bin as f64 * hz_per_bin >= low_hz && band_of(bin as f64 * hz_per_bin) == band
            });
        if holds_a_bin {
            continue;
        }
        let at = (from * to).sqrt() / hz_per_bin;
        let below = (at.floor() as usize).min(bins.len() - 1);
        let above = (below + 1).min(bins.len() - 1);
        let fraction = (at - below as f64).clamp(0.0, 1.0) as f32;
        *level = bins[below] + (bins[above] - bins[below]) * fraction;
    }
}

/// Frequency of A above middle C, which names every other pitch class.
const CONCERT_A_HZ: f64 = 440.0;

/// How much of each of the twelve pitch classes is in a spectrum: a chromagram's column.
///
/// `bins` are levels in dBFS, as [`SpectrumAnalyzer::magnitudes`] fills them, and the result is
/// power, linear, with C first and B last, in twelve-tone equal temperament from A = 440 Hz.
/// Octaves are folded together, which is the point: a chord reads the same in any voicing, and
/// the bass's root and the melody's note land in the same row.
///
/// A bin belongs to the semitone nearest it and is weighted by a raised cosine of how far from
/// that semitone's centre it sits, so a tone in tune counts fully and the skirt of its window
/// halfway to the next semitone counts for nothing — the alternative, sharing every bin between
/// its two neighbours, gives every pure tone a ghost a semitone away.
///
/// Only bins between `low_hz` and `high_hz` count, and of those only the ones narrower than a
/// semitone: below that a bin spans two pitch classes and assigning it to either is a guess. How
/// low that is depends on the window, which is why a chromagram wants a longer one than a
/// spectrum does.
pub fn chroma_from_bins(bins: &[f32], sample_rate: f64, low_hz: f64, high_hz: f64) -> [f32; 12] {
    let mut chroma = [0.0f32; 12];
    if bins.len() < 2 || sample_rate <= 0.0 || low_hz <= 0.0 || high_hz <= low_hz {
        return chroma;
    }
    let hz_per_bin = sample_rate / 2.0 / (bins.len() - 1) as f64;
    // A semitone at `f` is `f * (2^(1/12) - 1)` wide; below this frequency a bin is wider.
    let resolvable_from = hz_per_bin / (2f64.powf(1.0 / 12.0) - 1.0);
    let from = low_hz.max(resolvable_from);
    let first = (from / hz_per_bin).ceil() as usize;
    for (index, &level) in bins.iter().enumerate().skip(first.max(1)) {
        let frequency = index as f64 * hz_per_bin;
        if frequency > high_hz {
            break;
        }
        if level <= SILENCE_DB {
            continue;
        }
        // MIDI note 69 is A4; 60 is C4, so the remainder mod 12 counts from C.
        let note = 69.0 + 12.0 * (frequency / CONCERT_A_HZ).log2();
        let nearest = note.round();
        let distance = (note - nearest) as f32;
        let weight = (std::f32::consts::PI * distance).cos().powi(2);
        let class = (nearest as i64).rem_euclid(12) as usize;
        chroma[class] += weight * 10.0f32.powf(level / 10.0);
    }
    chroma
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

    #[test]
    fn narrow_bands_are_joined_by_straight_lines_and_wide_ones_keep_their_own() {
        // Bins 100 Hz apart, and 240 bands across 20 Hz to 20 kHz: below a few kilohertz a band
        // is narrower than a bin.
        let bins: Vec<f32> = (0..=240).map(|bin| -0.1 * bin as f32).collect();
        let mut sparse = [0.0; 240];
        let mut joined = [0.0; 240];
        bands_from_bins(&bins, 48_000.0, 20.0, 20_480.0, &mut sparse);
        bands_from_bins_interpolated(&bins, 48_000.0, 20.0, 20_480.0, &mut joined);
        assert!(sparse.iter().take(80).any(|&level| level == SILENCE_DB));
        assert!(joined.iter().all(|&level| level > SILENCE_DB));
        for (band, (&sparse, &joined)) in sparse.iter().zip(&joined).enumerate() {
            if sparse > SILENCE_DB {
                assert_eq!(sparse, joined, "band {band} held a bin and was changed");
            }
        }
        // The bins fall a tenth of a decibel per bin, so an interpolated band reads its centre
        // frequency's fraction of the way down: no steps, and nothing the bins did not say.
        let centre = |band: usize| 20.0 * 1024f64.powf((band as f64 + 0.5) / 240.0);
        for (band, &level) in joined.iter().enumerate().take(60) {
            let expected = -0.1 * (centre(band) / 100.0) as f32;
            assert!(
                (level - expected).abs() < 1e-3,
                "band {band}: {level} against {expected}"
            );
        }
        // Monotonic bins give a monotonic curve: the line has no risers in it.
        assert!(joined.windows(2).all(|pair| pair[1] <= pair[0] + 1e-6));
    }

    fn chroma_of(frequencies: &[f64]) -> [f32; 12] {
        let rate = 48_000.0;
        let mut analyzer = SpectrumAnalyzer::new(8192);
        let mut signal = vec![0.0f32; 8192];
        for &frequency in frequencies {
            for (sample, tone) in signal.iter_mut().zip(sine(frequency, rate, 0.25, 8192)) {
                *sample += tone;
            }
        }
        analyzer.push(&signal);
        let mut bins = vec![0.0; analyzer.bin_count()];
        analyzer.magnitudes(&mut bins);
        chroma_from_bins(&bins, rate, 100.0, 5_000.0)
    }

    fn loudest_class(chroma: &[f32; 12]) -> usize {
        loudest(chroma)
    }

    #[test]
    fn a_tone_lands_in_its_own_pitch_class_in_any_octave() {
        // A2, A4 and A6, then C3, middle C and C6.
        for frequency in [110.0, 440.0, 1760.0] {
            assert_eq!(loudest_class(&chroma_of(&[frequency])), 9, "{frequency} Hz");
        }
        for frequency in [130.81, 261.63, 1046.5] {
            assert_eq!(loudest_class(&chroma_of(&[frequency])), 0, "{frequency} Hz");
        }
    }

    #[test]
    fn a_pure_tone_leaves_its_neighbours_nearly_empty() {
        let chroma = chroma_of(&[440.0]);
        let a = chroma[9];
        for (class, &power) in chroma.iter().enumerate() {
            if class != 9 {
                // Twenty decibels down at least: no ghost a semitone away.
                assert!(power < a * 0.01, "class {class}: {power} against A's {a}");
            }
        }
    }

    #[test]
    fn a_major_triad_lights_its_three_classes_above_the_rest() {
        // C4, E4, G4.
        let chroma = chroma_of(&[261.63, 329.63, 392.0]);
        let quietest_of_chord = [0, 4, 7]
            .iter()
            .map(|&class| chroma[class])
            .fold(f32::INFINITY, f32::min);
        for class in [1, 2, 3, 5, 6, 8, 9, 10, 11] {
            assert!(
                chroma[class] < quietest_of_chord * 0.1,
                "class {class}: {} against {quietest_of_chord}",
                chroma[class]
            );
        }
    }

    #[test]
    fn silence_and_tones_outside_the_range_have_no_chroma() {
        assert!(chroma_of(&[]).iter().all(|&power| power == 0.0));
        // 60 Hz is below the 100 Hz floor, 8 kHz above the 5 kHz ceiling.
        let outside = chroma_of(&[60.0, 8_000.0]);
        let inside = chroma_of(&[440.0]);
        assert!(outside.iter().sum::<f32>() < inside[9] * 1e-3);
    }

    #[test]
    fn bins_wider_than_a_semitone_are_left_out() {
        // At 48 kHz a 1024-sample window's bins are 47 Hz apart, wider than a semitone below
        // about 800 Hz: A4 cannot be told from its neighbours there and so is not counted.
        let rate = 48_000.0;
        let mut analyzer = SpectrumAnalyzer::new(1024);
        analyzer.push(&sine(440.0, rate, 0.25, 1024));
        let mut bins = vec![0.0; analyzer.bin_count()];
        analyzer.magnitudes(&mut bins);
        let chroma = chroma_from_bins(&bins, rate, 100.0, 5_000.0);
        assert!(chroma.iter().all(|&power| power < 1e-6), "{chroma:?}");
    }
}
