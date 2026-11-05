use super::*;
use std::hint::black_box;
use std::time::Instant;

fn reference_analyse(audio: &AudioBuffer) -> Spectrogram {
    let rate = audio.sample_rate();
    let frames = audio.frame_count();
    let valid_rate = rate.is_finite() && rate > LOW_HZ * 2.0;
    let mut result = Spectrogram {
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
            crate::bands_from_bins(&bins, rate, LOW_HZ, result.high_hz, &mut bands);
            interpolate_bands(&bins, &interpolations, &mut bands);
            for (destination, level) in window_bands.iter_mut().zip(bands) {
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

fn assert_same(actual: &Spectrogram, expected: &Spectrogram) {
    assert_eq!(actual.columns, expected.columns);
    assert_eq!(actual.frame_count, expected.frame_count);
    assert_eq!(actual.sample_rate, expected.sample_rate);
    assert_eq!(actual.high_hz, expected.high_hz);
    assert_eq!(actual.levels, expected.levels);
}

fn fixture(channels: usize, frames: usize, rate: f64, nonfinite: bool) -> AudioBuffer {
    let channels: Vec<Vec<f32>> = (0..channels)
        .map(|channel| {
            (0..frames)
                .map(|frame| {
                    if nonfinite && frame % 97 == 0 {
                        if frame % 194 == 0 {
                            f32::NAN
                        } else {
                            f32::INFINITY
                        }
                    } else {
                        let t = frame as f64 / rate.max(1.0);
                        (0.2 * (std::f64::consts::TAU * (220.0 + channel as f64 * 31.0) * t).sin())
                            as f32
                    }
                })
                .collect()
        })
        .collect();
    AudioBuffer::from_planar(channels, rate).expect("fixture")
}

#[test]
fn cached_analysis_matches_the_original_for_shapes_rates_and_nonfinite_input() {
    for channels in [1, 2, 8] {
        for rate in [8_000.0, 44_100.0, 48_000.0, 192_000.0] {
            for frames in [0, 1, 2_047, 2_048, 4_097] {
                let audio = fixture(channels, frames, rate, false);
                assert_same(&Spectrogram::analyse(&audio), &reference_analyse(&audio));
            }
            let audio = fixture(channels, 4_097, rate, true);
            assert_same(&Spectrogram::analyse(&audio), &reference_analyse(&audio));
        }
    }
}

#[test]
fn cached_analysis_matches_for_a_source_beyond_the_column_bound() {
    let audio = fixture(1, MAX_COLUMNS * HOP_FRAMES + 1, 48_000.0, false);
    assert_same(&Spectrogram::analyse(&audio), &reference_analyse(&audio));
}

#[test]
#[ignore = "release-only spectrogram benchmark"]
fn benchmark_original_and_cached_analysis_nine_alternating_trials() {
    fn median(values: &mut [u128]) -> u128 {
        values.sort_unstable();
        values[values.len() / 2]
    }
    for seconds in [1, 10, 60] {
        let audio = fixture(2, seconds * 48_000, 48_000.0, false);
        assert_same(&Spectrogram::analyse(&audio), &reference_analyse(&audio));
        black_box(Spectrogram::analyse(black_box(&audio)));
        black_box(reference_analyse(black_box(&audio)));
        let mut original = Vec::with_capacity(9);
        let mut cached = Vec::with_capacity(9);
        for trial in 0..9 {
            if trial % 2 == 0 {
                let start = Instant::now();
                black_box(reference_analyse(black_box(&audio)));
                original.push(start.elapsed().as_micros());
                let start = Instant::now();
                black_box(Spectrogram::analyse(black_box(&audio)));
                cached.push(start.elapsed().as_micros());
            } else {
                let start = Instant::now();
                black_box(Spectrogram::analyse(black_box(&audio)));
                cached.push(start.elapsed().as_micros());
                let start = Instant::now();
                black_box(reference_analyse(black_box(&audio)));
                original.push(start.elapsed().as_micros());
            }
        }
        eprintln!(
            "spectrogram {seconds}s: original={}us cached={}us",
            median(&mut original),
            median(&mut cached)
        );
    }
}
