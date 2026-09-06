//! Offline time-frequency pictures of decoded audio sources.
//!
//! Every channel is analysed separately, then the loudest channel wins at each frequency.
//! Summing samples first would erase opposite-phase stereo. Overlapping windows cover the
//! entire source, and long sources pool their windows into a bounded number of time columns.

use auris_core::AudioBuffer;

use crate::{SILENCE_DB, SpectrumAnalyzer, bands_from_bins};

const WINDOW_FRAMES: usize = 4_096;
const HOP_FRAMES: usize = WINDOW_FRAMES / 2;
const MAX_COLUMNS: usize = 2_048;
const BAND_COUNT: usize = 96;
const LOW_HZ: f64 = 20.0;
const HIGH_HZ: f64 = 20_000.0;

/// A bounded, source-coordinate spectrogram with logarithmic frequency bands.
///
/// Levels are peak amplitudes in dBFS, with a full-scale, bin-centred sine at 0 dBFS and
/// silence at [`SILENCE_DB`]. Time columns divide the source duration evenly. Each column
/// takes the loudest analysis window overlapping it, so reducing the time
/// resolution of a long source keeps short events visible.
/// Bands narrower than one FFT bin interpolate neighbouring amplitudes for a continuous image;
/// this smooths the display without increasing the underlying frequency resolution.
#[derive(Clone, Debug)]
pub struct Spectrogram {
    columns: usize,
    frame_count: usize,
    sample_rate: f64,
    high_hz: f64,
    levels: Vec<f32>,
}

impl Spectrogram {
    /// Analyses all channels of `audio`; run this on a worker, never an audio or UI thread.
    ///
    /// Results use at most 2,048 time columns and 96 frequency bands between 20 Hz and the
    /// lower of 20 kHz and Nyquist. Empty audio or an invalid sample rate produces no columns.
    /// Non-finite input samples are treated as silence.
    pub fn analyse(audio: &AudioBuffer) -> Self {
        let rate = audio.sample_rate();
        let frames = audio.frame_count();
        let valid_rate = rate.is_finite() && rate > LOW_HZ * 2.0;
        let mut result = Self {
            columns: 0,
            frame_count: frames,
            sample_rate: if valid_rate { rate } else { 0.0 },
            high_hz: if valid_rate {
                (rate / 2.0).min(HIGH_HZ)
            } else {
                LOW_HZ
            },
            levels: Vec::new(),
        };
        if frames == 0 || !valid_rate {
            return result;
        }
        result.columns = frames.div_ceil(HOP_FRAMES).min(MAX_COLUMNS);
        result.levels = vec![SILENCE_DB; result.columns * BAND_COUNT];

        let mut analyzer = SpectrumAnalyzer::new(WINDOW_FRAMES);
        let mut samples = vec![0.0; WINDOW_FRAMES];
        let mut bins = vec![SILENCE_DB; analyzer.bin_count()];
        let interpolations = unresolved_bands(bins.len(), rate, result.high_hz);
        let mut bands = [SILENCE_DB; BAND_COUNT];
        let mut window_bands = [SILENCE_DB; BAND_COUNT];
        // Include both ends explicitly: a Hann window's zero at its edge must not erase an
        // event at the start or end of a source. All other windows overlap by half their size.
        let centres = (0..frames)
            .step_by(HOP_FRAMES)
            .chain((!(frames - 1).is_multiple_of(HOP_FRAMES)).then_some(frames - 1));
        for centre in centres {
            let start = centre.saturating_sub(WINDOW_FRAMES / 2);
            let end = centre.saturating_add(WINDOW_FRAMES / 2).min(frames);
            let padding = (WINDOW_FRAMES / 2).saturating_sub(centre);
            window_bands.fill(SILENCE_DB);
            for channel in audio.iter_channels() {
                samples.fill(0.0);
                for (destination, sample) in samples[padding..].iter_mut().zip(
                    channel
                        .get(start..end.min(channel.len()))
                        .unwrap_or_default(),
                ) {
                    *destination = if sample.is_finite() { *sample } else { 0.0 };
                }
                analyzer.reset();
                analyzer.push(&samples);
                analyzer.magnitudes(&mut bins);
                bands_from_bins(&bins, rate, LOW_HZ, result.high_hz, &mut bands);
                interpolate_bands(&bins, &interpolations, &mut bands);
                for (destination, level) in window_bands.iter_mut().zip(bands) {
                    // Corrupt samples can overflow the FFT even if individually finite. A
                    // finite floor keeps the image's colour conversion well-defined.
                    if level.is_finite() {
                        *destination = destination.max(level);
                    }
                }
            }
            let first = ((start as u128 * result.columns as u128) / frames as u128) as usize;
            let last = (((end - 1) as u128 * result.columns as u128) / frames as u128) as usize;
            for column in first..=last {
                let output = &mut result.levels[column * BAND_COUNT..(column + 1) * BAND_COUNT];
                for (destination, level) in output.iter_mut().zip(window_bands) {
                    *destination = destination.max(level);
                }
            }
        }
        result
    }

    /// Number of equally spaced time columns, or zero for unusable audio.
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// Number of logarithmic frequency bands in each column.
    pub fn bands(&self) -> usize {
        BAND_COUNT
    }

    /// Length of the decoded source in frames, before clip offsets or stretching.
    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Source sample rate, or zero when the source's rate was invalid.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Lower edge of the frequency range, in hertz.
    pub fn low_hz(&self) -> f64 {
        LOW_HZ
    }

    /// Upper edge of the frequency range, in hertz; never above Nyquist for valid audio.
    pub fn high_hz(&self) -> f64 {
        self.high_hz
    }

    /// One time column's dBFS levels, ordered from low to high frequency.
    pub fn column(&self, index: usize) -> Option<&[f32]> {
        if index >= self.columns {
            return None;
        }
        Some(&self.levels[index * BAND_COUNT..(index + 1) * BAND_COUNT])
    }

    /// Maps a frame position within the source to its time column.
    ///
    /// Negative, non-finite and out-of-source positions return `None`; callers can therefore
    /// leave silent clip padding blank instead of extending the source's final column.
    pub fn column_at_frame(&self, frame: f64) -> Option<usize> {
        if self.columns == 0
            || !frame.is_finite()
            || frame < 0.0
            || frame >= self.frame_count as f64
        {
            return None;
        }
        Some(
            ((frame / self.frame_count as f64 * self.columns as f64) as usize)
                .min(self.columns - 1),
        )
    }
}

/// Bands narrower than the FFT's frequency resolution still need a continuous image. Keep
/// peak pooling where a bin exists, and sample between adjacent bin amplitudes elsewhere.
/// The mapping depends only on the source rate, so build it once rather than per window.
fn unresolved_bands(bin_count: usize, rate: f64, high_hz: f64) -> Vec<(usize, usize, f64)> {
    let span = (high_hz / LOW_HZ).log2();
    let hz_per_bin = rate / 2.0 / (bin_count - 1) as f64;
    let mut occupied = [false; BAND_COUNT];
    for bin in 0..bin_count {
        let frequency = bin as f64 * hz_per_bin;
        if (LOW_HZ..=high_hz).contains(&frequency) {
            let position = (frequency / LOW_HZ).log2() / span;
            occupied[((position * BAND_COUNT as f64) as usize).min(BAND_COUNT - 1)] = true;
        }
    }
    occupied
        .iter()
        .enumerate()
        .filter_map(|(band, occupied)| {
            if *occupied {
                return None;
            }
            let frequency =
                LOW_HZ * (high_hz / LOW_HZ).powf((band as f64 + 0.5) / BAND_COUNT as f64);
            let position = (frequency / hz_per_bin).min((bin_count - 1) as f64);
            Some((band, position as usize, position.fract()))
        })
        .collect()
}

fn interpolate_bands(bins: &[f32], interpolations: &[(usize, usize, f64)], bands: &mut [f32]) {
    for &(band, lower, fraction) in interpolations {
        let upper = (lower + 1).min(bins.len() - 1);
        let amplitude = |index: usize| {
            let level = bins[index];
            if level.is_finite() && level > SILENCE_DB {
                10.0_f64.powf(f64::from(level) / 20.0)
            } else {
                0.0
            }
        };
        let amplitude = amplitude(lower) * (1.0 - fraction) + amplitude(upper) * fraction;
        bands[band] = if amplitude > 0.0 {
            (20.0 * amplitude.log10()).max(f64::from(SILENCE_DB)) as f32
        } else {
            SILENCE_DB
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;
    const TONE_HZ: f64 = 3_000.0;

    fn tone(amplitude: f32, frames: usize) -> Vec<f32> {
        crate::spectrum::sine(TONE_HZ, RATE, amplitude, frames)
    }

    fn audio(channels: Vec<Vec<f32>>) -> AudioBuffer {
        AudioBuffer::from_planar(channels, RATE).unwrap()
    }

    fn loudest(column: &[f32]) -> (usize, f32) {
        column
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap()
    }

    #[test]
    fn a_bin_centred_sine_has_the_right_frequency_and_level() {
        let result = Spectrogram::analyse(&audio(vec![tone(1.0, WINDOW_FRAMES * 4)]));
        let (band, level) = loudest(result.column(3).unwrap());
        let expected = ((TONE_HZ / result.low_hz()).ln()
            / (result.high_hz() / result.low_hz()).ln()
            * result.bands() as f64) as usize;
        assert_eq!(band, expected);
        assert!(level.abs() < 0.1, "full-scale sine: {level} dBFS");
        assert!(result.column(3).unwrap()[band / 2] < -60.0);
    }

    #[test]
    fn half_amplitude_is_six_decibels_quieter() {
        let loud = Spectrogram::analyse(&audio(vec![tone(1.0, WINDOW_FRAMES * 4)]));
        let quiet = Spectrogram::analyse(&audio(vec![tone(0.5, WINDOW_FRAMES * 4)]));
        let difference = loudest(loud.column(3).unwrap()).1 - loudest(quiet.column(3).unwrap()).1;
        assert!(
            (difference - 6.0206).abs() < 0.01,
            "difference: {difference}"
        );
    }

    #[test]
    fn low_tones_do_not_acquire_silent_stripes_between_fft_bins() {
        let frequency = RATE * 2.0 / WINDOW_FRAMES as f64;
        let result = Spectrogram::analyse(&audio(vec![crate::spectrum::sine(
            frequency,
            RATE,
            1.0,
            WINDOW_FRAMES * 4,
        )]));
        let column = result.column(3).unwrap();
        // The first four bands are all around the 23.4 Hz tone, but only one contains an FFT
        // bin. Their amplitude must follow the resolved peak rather than drop to silence.
        assert!(
            column[..4].iter().all(|level| *level > -6.1),
            "low bands: {:?}",
            &column[..4]
        );
        assert!(loudest(column).1.abs() < 0.1);
    }

    #[test]
    fn stereo_phase_and_channel_choice_do_not_hide_a_tone() {
        let samples = tone(0.5, WINDOW_FRAMES * 4);
        let mono = Spectrogram::analyse(&audio(vec![samples.clone()]));
        let opposite = Spectrogram::analyse(&audio(vec![
            samples.clone(),
            samples.iter().map(|x| -*x).collect(),
        ]));
        let right_only = Spectrogram::analyse(&audio(vec![vec![0.0; samples.len()], samples]));
        assert_eq!(mono.levels, opposite.levels);
        assert_eq!(mono.levels, right_only.levels);
    }

    #[test]
    fn time_columns_follow_the_source_and_preserve_a_short_event_when_pooled() {
        let frames = HOP_FRAMES * (MAX_COLUMNS + 4);
        let mut samples = vec![0.0; frames];
        let start = frames / 2;
        samples[start..start + WINDOW_FRAMES].copy_from_slice(&tone(1.0, WINDOW_FRAMES));
        let result = Spectrogram::analyse(&audio(vec![samples]));
        assert_eq!(result.columns(), MAX_COLUMNS);
        assert_eq!(result.bands(), BAND_COUNT);
        assert_eq!(result.frame_count(), frames);
        assert_eq!(result.sample_rate(), RATE);
        assert!(
            result
                .column(0)
                .unwrap()
                .iter()
                .all(|level| *level == SILENCE_DB)
        );
        let event_column = result
            .column_at_frame((start + WINDOW_FRAMES / 2) as f64)
            .unwrap();
        let event_peak = (event_column - 1..=event_column + 1)
            .map(|column| loudest(result.column(column).unwrap()).1)
            .fold(SILENCE_DB, f32::max);
        assert!(event_peak > -1.0, "short event: {event_peak} dBFS");
        assert!(
            result
                .column(MAX_COLUMNS - 1)
                .unwrap()
                .iter()
                .all(|level| *level == SILENCE_DB)
        );
        assert_eq!(result.column_at_frame(0.0), Some(0));
        assert_eq!(
            result.column_at_frame((frames - 1) as f64),
            Some(MAX_COLUMNS - 1)
        );
        for frame in [-1.0, frames as f64, f64::NAN, f64::INFINITY] {
            assert_eq!(result.column_at_frame(frame), None);
        }
        assert_eq!(result.column(MAX_COLUMNS), None);
    }

    #[test]
    fn silence_short_sources_and_invalid_data_remain_finite() {
        let silence = Spectrogram::analyse(&AudioBuffer::new(2, WINDOW_FRAMES, RATE));
        assert!(silence.levels.iter().all(|level| *level == SILENCE_DB));
        let short = Spectrogram::analyse(&audio(vec![vec![1.0]]));
        assert_eq!(short.columns(), 1);
        assert!(short.levels.iter().all(|level| level.is_finite()));
        assert!(loudest(short.column(0).unwrap()).1 > SILENCE_DB);
        let invalid = Spectrogram::analyse(&audio(vec![vec![
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ]]));
        assert!(invalid.levels.iter().all(|level| *level == SILENCE_DB));
        assert_eq!(
            Spectrogram::analyse(&AudioBuffer::new(1, 0, RATE)).columns(),
            0
        );
        for rate in [0.0, -1.0, 40.0, f64::NAN, f64::INFINITY] {
            let result = Spectrogram::analyse(&AudioBuffer::new(1, 4, rate));
            assert_eq!(result.columns(), 0);
            assert_eq!(result.column_at_frame(0.0), None);
        }
        let low_rate = Spectrogram::analyse(&AudioBuffer::new(1, 100, 8_000.0));
        assert_eq!(low_rate.high_hz(), 4_000.0);
        let mut ragged = AudioBuffer::new(2, 100, RATE);
        ragged.channels_mut()[1].clear();
        assert!(
            Spectrogram::analyse(&ragged)
                .levels
                .iter()
                .all(|level| *level == SILENCE_DB)
        );
    }

    #[test]
    fn transients_at_both_source_edges_survive_windowing() {
        let mut samples = vec![0.0; WINDOW_FRAMES * 4];
        samples[0] = 1.0;
        *samples.last_mut().unwrap() = 1.0;
        let result = Spectrogram::analyse(&audio(vec![samples]));
        for column in [0, result.columns() - 1] {
            let level = loudest(result.column(column).unwrap()).1;
            assert!((level - 20.0 * (4.0 / WINDOW_FRAMES as f32).log10()).abs() < 0.01);
        }
        assert!(
            result
                .column(result.columns() / 2)
                .unwrap()
                .iter()
                .all(|level| *level == SILENCE_DB)
        );
    }
}
