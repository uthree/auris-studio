//! Exact comparison against the serial implementation before Rayon was introduced.

use super::*;
use std::{hint::black_box, time::Instant};

fn serial_reference(buffer: &AudioBuffer, samples_per_bucket: u32) -> WaveformPeaks {
    let stride = samples_per_bucket.max(1);
    let channel_count = buffer.channel_count();
    let frames = if buffer.frame_count() == 0 {
        buffer.channels().iter().map(Vec::len).max().unwrap_or(0)
    } else {
        buffer.frame_count()
    };
    let buckets = frames.div_ceil(stride as usize);
    let total = buckets * channel_count;
    let mut peaks = WaveformPeaks {
        min: Vec::with_capacity(total),
        max: Vec::with_capacity(total),
        rms: Vec::with_capacity(total),
        samples_per_bucket: stride,
        channel_count,
    };
    for channel in 0..channel_count {
        let samples = buffer.channel(channel);
        let usable = samples.len().min(frames);
        for bucket in 0..buckets {
            let start = bucket * stride as usize;
            let end = (start + stride as usize).min(usable);
            let chunk = if start < end {
                &samples[start..end]
            } else {
                &[]
            };
            let mut iter = chunk.iter().copied();
            let (min, max, sum_squares) = if let Some(first) = iter.next() {
                let mut min = first;
                let mut max = first;
                let mut sum_squares = first * first;
                for value in iter {
                    min = min.min(value);
                    max = max.max(value);
                    sum_squares += value * value;
                }
                (min, max, sum_squares)
            } else {
                (0.0, 0.0, 0.0)
            };
            peaks.min.push(min);
            peaks.max.push(max);
            peaks
                .rms
                .push((sum_squares / chunk.len().max(1) as f32).sqrt());
        }
    }
    peaks
}

fn fixture(frames: usize, channels: usize) -> AudioBuffer {
    AudioBuffer::from_planar(
        (0..channels)
            .map(|channel| {
                let mut state = 12345u32 + channel as u32;
                (0..frames)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        (state as f32 / u32::MAX as f32 - 0.5) * 1.5
                    })
                    .collect()
            })
            .collect(),
        48_000.0,
    )
    .unwrap()
}

fn assert_same_bits(actual: &WaveformPeaks, expected: &WaveformPeaks) {
    assert_eq!(actual.channel_count, expected.channel_count);
    assert_eq!(actual.samples_per_bucket, expected.samples_per_bucket);
    for (name, got, want) in [
        ("min", &actual.min, &expected.min),
        ("max", &actual.max, &expected.max),
        ("rms", &actual.rms, &expected.rms),
    ] {
        assert_eq!(got.len(), want.len(), "{name}");
        for (index, (got, want)) in got.iter().zip(want).enumerate() {
            assert_eq!(got.to_bits(), want.to_bits(), "{name}[{index}]");
        }
    }
}

#[test]
fn waveform_matches_serial_bits_across_pool_sizes_and_bucket_boundaries() {
    for threads in [1, 2, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        for frames in [0, 1, 127, 8192, 87_381, 87_382, 131_075] {
            let buffer = fixture(frames, 3);
            for stride in [0, 1, 31, 256, 512, 4096, u32::MAX] {
                let expected = serial_reference(&buffer, stride);
                let actual = pool.install(|| compute_peaks_cpu(&buffer, stride));
                assert_same_bits(&actual, &expected);
            }
        }
    }
}

#[test]
fn ragged_channels_and_nonfinite_samples_keep_the_serial_bits() {
    let silence = AudioBuffer::stereo(PARALLEL_SAMPLES, 48_000.0);
    assert_same_bits(
        &compute_peaks_cpu(&silence, 256),
        &serial_reference(&silence, 256),
    );
    let mut buffer = fixture(131_075, 3);
    buffer.channels_mut()[1].truncate(1001);
    buffer.channels_mut()[2].extend_from_slice(&[9.0; 19]);
    buffer.channels_mut()[0][..7].copy_from_slice(&[
        0.0,
        -0.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::from_bits(0x7fc0_1234),
        f32::MIN_POSITIVE,
        f32::from_bits(1),
    ]);
    for threads in [1, 2, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        for stride in [1, 31, 512, u32::MAX] {
            let expected = serial_reference(&buffer, stride);
            let actual = pool.install(|| compute_peaks_cpu(&buffer, stride));
            assert_same_bits(&actual, &expected);
        }
    }
    buffer.channels_mut()[0].clear();
    for stride in [0, 333, 4096] {
        assert_same_bits(
            &compute_peaks_cpu(&buffer, stride),
            &serial_reference(&buffer, stride),
        );
    }
}

#[test]
#[ignore = "manual release benchmark; run with --release --ignored --nocapture"]
fn measure_waveform_cpu() {
    for (frames, channels, stride) in [
        (4096, 2, 512),
        (48_000 * 180, 1, 512),
        (48_000 * 180, 2, 256),
        (48_000 * 180, 2, 512),
        (48_000 * 180, 2, 4096),
        (48_000 * 1800, 2, 256),
    ] {
        let buffer = fixture(frames, channels);
        let reference = serial_reference(&buffer, stride);
        assert_same_bits(&compute_peaks_cpu(&buffer, stride), &reference);
        let mut serial = Vec::new();
        let mut current = Vec::new();
        for iteration in 0..9 {
            // Alternate order so cache warmth does not always favour one implementation.
            for parallel in [iteration % 2 == 0, iteration % 2 != 0] {
                let start = Instant::now();
                let result = if parallel {
                    compute_peaks_cpu(black_box(&buffer), stride)
                } else {
                    serial_reference(black_box(&buffer), stride)
                };
                black_box(result);
                if parallel {
                    current.push(start.elapsed());
                } else {
                    serial.push(start.elapsed());
                }
            }
        }
        serial.sort();
        current.sort();
        println!(
            "frames={frames} channels={channels} stride={stride} serial_ms={:.3} current_ms={:.3} speedup={:.2}",
            serial[4].as_secs_f64() * 1000.0,
            current[4].as_secs_f64() * 1000.0,
            serial[4].as_secs_f64() / current[4].as_secs_f64()
        );
    }
}
