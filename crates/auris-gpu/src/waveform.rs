//! Waveform overview reduction — the main kernel of this crate.
//!
//! Drawing an audio clip means answering, for each horizontal pixel, "what did the signal do
//! during these N samples?". The answer is three numbers: the lowest and highest sample, which
//! draw the outline, and the RMS level, which draws the darker body inside it. A four-minute
//! stereo file at 48 kHz is 23 million samples per channel. The reduction runs once when a
//! decoded source is installed and its fixed-size result is cached; zooming only re-bins that
//! cached overview on the CPU.
//!
//! Buckets are independent, which is what makes this a good GPU job: one invocation per bucket,
//! no shared state, no synchronisation. Use [`compute_peaks`] — it tries the GPU and silently
//! falls back to [`compute_peaks_cpu`].

use auris_core::AudioBuffer;

use crate::context::GpuContext;

/// Pipeline cache key and debug label for the bucket kernel.
const LABEL: &str = "auris-gpu.waveform_peaks";

/// WGSL source of the bucket kernel.
const SHADER: &str = include_str!("shaders/waveform.wgsl");

/// Uniform block handed to the bucket kernel. Must match `Params` in `waveform.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    sample_count: u32,
    samples_per_bucket: u32,
    bucket_count: u32,
    padding: u32,
}

/// The three values that describe one horizontal pixel of a waveform.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct WaveformBucket {
    /// Lowest sample in the bucket.
    pub min: f32,
    /// Highest sample in the bucket.
    pub max: f32,
    /// Root-mean-square level of the bucket.
    pub rms: f32,
}

/// A downsampled overview of an [`AudioBuffer`], one entry per bucket per channel.
///
/// The three vectors are parallel and laid out channel-major: bucket `b` of channel `c` lives
/// at index `c * bucket_count() + b`. Use [`WaveformPeaks::bucket`] rather than indexing by
/// hand.
#[derive(Clone, Debug, PartialEq)]
pub struct WaveformPeaks {
    /// Lowest sample per bucket.
    pub min: Vec<f32>,
    /// Highest sample per bucket.
    pub max: Vec<f32>,
    /// RMS level per bucket.
    pub rms: Vec<f32>,
    /// Samples reduced into one bucket. The final bucket of a channel may hold fewer.
    pub samples_per_bucket: u32,
    /// Number of channels described.
    pub channel_count: usize,
}

impl WaveformPeaks {
    /// An overview of a buffer that has no frames.
    fn empty(channel_count: usize, samples_per_bucket: u32) -> Self {
        Self {
            min: Vec::new(),
            max: Vec::new(),
            rms: Vec::new(),
            samples_per_bucket,
            channel_count,
        }
    }

    /// Buckets per channel.
    pub fn bucket_count(&self) -> usize {
        self.min.len().checked_div(self.channel_count).unwrap_or(0)
    }

    /// `true` when there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.min.is_empty()
    }

    /// One bucket, or `None` when either coordinate is out of range.
    ///
    /// The three vectors are public, so a caller can hand back a value whose lengths disagree;
    /// that yields `None` here rather than a panic.
    pub fn bucket(&self, channel: usize, bucket: usize) -> Option<WaveformBucket> {
        let buckets = self.bucket_count();
        if channel >= self.channel_count || bucket >= buckets {
            return None;
        }
        let index = channel * buckets + bucket;
        Some(WaveformBucket {
            min: *self.min.get(index)?,
            max: *self.max.get(index)?,
            rms: *self.rms.get(index)?,
        })
    }

    /// First sample of a bucket, for mapping a pixel back to a position in the source.
    pub fn bucket_start_frame(&self, bucket: usize) -> u64 {
        bucket as u64 * self.samples_per_bucket.max(1) as u64
    }
}

/// Reduces `buffer` to one min/max/RMS bucket per `samples_per_bucket` samples, on the CPU.
///
/// This is the reference implementation: the GPU kernel is tested against it, and it is what
/// runs whenever no GPU is available. A `samples_per_bucket` of 0 is treated as 1.
pub fn compute_peaks_cpu(buffer: &AudioBuffer, samples_per_bucket: u32) -> WaveformPeaks {
    let stride = samples_per_bucket.max(1);
    let channel_count = buffer.channel_count();
    let frames = waveform_frames(buffer);
    if frames == 0 {
        return WaveformPeaks::empty(channel_count, stride);
    }

    let buckets = frames.div_ceil(stride as usize);
    let total = buckets * channel_count;
    let mut peaks = WaveformPeaks {
        min: Vec::with_capacity(total),
        max: Vec::with_capacity(total),
        rms: Vec::with_capacity(total),
        samples_per_bucket: stride,
        channel_count,
    };

    let stride = stride as usize;
    for channel in 0..channel_count {
        let samples = buffer.channel(channel);
        // `frames` describes the first channel. `AudioBuffer::channels_mut` lets a caller leave
        // the others a different length, and the channel-major layout only holds if every
        // channel contributes exactly `buckets` entries — so drive the loop from `buckets` and
        // clamp the reads, instead of letting each channel's own length decide how many buckets
        // it produces. The GPU path is already rectangular for the same reason.
        let usable = samples.len().min(frames);
        for bucket in 0..buckets {
            let start = bucket * stride;
            let end = (start + stride).min(usable);
            let chunk = if start < end {
                &samples[start..end]
            } else {
                &[]
            };
            let (min, max, sum_squares) = reduce(chunk);
            peaks.min.push(min);
            peaks.max.push(max);
            peaks
                .rms
                .push((sum_squares / chunk.len().max(1) as f32).sqrt());
        }
    }
    peaks
}

fn waveform_frames(buffer: &AudioBuffer) -> usize {
    let first = buffer.frame_count();
    if first == 0 {
        buffer.channels().iter().map(Vec::len).max().unwrap_or(0)
    } else {
        first
    }
}

/// Minimum, maximum and sum of squares of a non-empty slice, in the same order as the shader.
fn reduce(samples: &[f32]) -> (f32, f32, f32) {
    let mut iter = samples.iter().copied();
    let Some(first) = iter.next() else {
        return (0.0, 0.0, 0.0);
    };
    let mut min = first;
    let mut max = first;
    let mut sum_squares = first * first;
    for value in iter {
        min = min.min(value);
        max = max.max(value);
        sum_squares += value * value;
    }
    (min, max, sum_squares)
}

impl GpuContext {
    /// Reduces `buffer` to waveform buckets with a WGSL compute shader.
    ///
    /// One invocation reduces one bucket. Channels are dispatched one at a time, and a channel
    /// longer than the device's storage-binding limit is split into chunks on bucket
    /// boundaries, so a bucket is never spread across two dispatches.
    ///
    /// Returns `None` on any wgpu failure; use [`compute_peaks`] to fall back automatically.
    pub fn compute_peaks(
        &self,
        buffer: &AudioBuffer,
        samples_per_bucket: u32,
    ) -> Option<WaveformPeaks> {
        let stride = samples_per_bucket.max(1) as usize;
        let channel_count = buffer.channel_count();
        let frames = waveform_frames(buffer);
        if frames == 0 {
            return Some(WaveformPeaks::empty(channel_count, stride as u32));
        }

        // A bucket must fit in one dispatch, so an absurd bucket size has no GPU path at all.
        let buckets_per_chunk = (self.max_input_samples() / stride).min(self.max_output_slots());
        if buckets_per_chunk == 0 {
            log::debug!("auris-gpu: {stride} samples per bucket exceeds the device limits");
            return None;
        }

        let buckets = frames.div_ceil(stride);
        let total = buckets * channel_count;
        let mut peaks = WaveformPeaks {
            min: Vec::with_capacity(total),
            max: Vec::with_capacity(total),
            rms: Vec::with_capacity(total),
            samples_per_bucket: stride as u32,
            channel_count,
        };

        for channel in 0..channel_count {
            let samples = buffer.channel(channel);
            // Clamped to `frames` as well as to the channel's own length: the CPU reference
            // reads no further, and a secondary channel longer than the first would otherwise
            // fold its samples past `frame_count` into the last bucket — when the crate's
            // whole contract is that the two paths differ only in speed.
            let usable = samples.len().min(frames);
            let mut first_bucket = 0;
            while first_bucket < buckets {
                let count = buckets_per_chunk.min(buckets - first_bucket);
                // Both ends clamped, not only one: a chunk boundary past a short channel's end
                // used to leave `start > end`, and the failed slice read as "the GPU failed" —
                // sending every large ragged buffer to the CPU for no reason a log would show.
                let start = (first_bucket * stride).min(usable);
                let end = ((first_bucket + count) * stride).min(usable);
                let chunk = samples.get(start..end)?;
                if chunk.is_empty() {
                    // Nothing left in this channel: the remaining buckets are the silence the
                    // CPU reference writes, answered here rather than dispatched — a zero-length
                    // storage binding is not a thing a device accepts.
                    for _ in 0..count {
                        peaks.min.push(0.0);
                        peaks.max.push(0.0);
                        peaks.rms.push(0.0);
                    }
                    first_bucket += count;
                    continue;
                }

                let params = Params {
                    sample_count: chunk.len() as u32,
                    samples_per_bucket: stride as u32,
                    bucket_count: count as u32,
                    padding: 0,
                };
                let slots = self.run_reduction(LABEL, SHADER, chunk, params, count)?;
                for slot in slots {
                    peaks.min.push(slot[0]);
                    peaks.max.push(slot[1]);
                    peaks.rms.push(slot[2]);
                }
                first_bucket += count;
            }
        }
        Some(peaks)
    }
}

/// Reduces `buffer` to waveform buckets, preferring the GPU.
///
/// This is what callers use. Pass the application's [`GpuContext`] when it has one; a `None`
/// context, or any GPU failure, transparently runs [`compute_peaks_cpu`] instead.
pub fn compute_peaks(
    gpu: Option<&GpuContext>,
    buffer: &AudioBuffer,
    samples_per_bucket: u32,
) -> WaveformPeaks {
    if let Some(gpu) = gpu {
        if let Some(peaks) = gpu.compute_peaks(buffer, samples_per_bucket) {
            return peaks;
        }
        log::debug!("auris-gpu: waveform reduction fell back to the CPU");
    }
    compute_peaks_cpu(buffer, samples_per_bucket)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mono ramp 0, 1, 2, ... so every bucket has a hand-checkable min, max and RMS.
    fn ramp(frames: usize) -> AudioBuffer {
        AudioBuffer::from_planar(vec![(0..frames).map(|i| i as f32).collect()], 48_000.0).unwrap()
    }

    #[test]
    fn ramp_buckets_hold_exact_values() {
        let peaks = compute_peaks_cpu(&ramp(8), 4);
        assert_eq!(peaks.bucket_count(), 2);
        assert_eq!(peaks.channel_count, 1);

        let first = peaks.bucket(0, 0).unwrap();
        assert_eq!(first.min, 0.0);
        assert_eq!(first.max, 3.0);
        // sqrt((0 + 1 + 4 + 9) / 4) = sqrt(3.5)
        assert!((first.rms - 3.5f32.sqrt()).abs() < 1e-6, "{first:?}");

        let second = peaks.bucket(0, 1).unwrap();
        assert_eq!(second.min, 4.0);
        assert_eq!(second.max, 7.0);
        // sqrt((16 + 25 + 36 + 49) / 4) = sqrt(31.5)
        assert!((second.rms - 31.5f32.sqrt()).abs() < 1e-6, "{second:?}");

        assert_eq!(peaks.bucket(0, 2), None);
        assert_eq!(peaks.bucket(1, 0), None);
    }

    #[test]
    fn trailing_partial_bucket_uses_its_real_length() {
        // 10 frames in buckets of 4 leaves a final bucket holding only samples 8 and 9.
        let peaks = compute_peaks_cpu(&ramp(10), 4);
        assert_eq!(peaks.bucket_count(), 3);

        let last = peaks.bucket(0, 2).unwrap();
        assert_eq!(last.min, 8.0);
        assert_eq!(last.max, 9.0);
        // sqrt((64 + 81) / 2) = sqrt(72.5); dividing by 4 instead would give 4.26.
        assert!((last.rms - 72.5f32.sqrt()).abs() < 1e-6, "{last:?}");
        assert_eq!(peaks.bucket_start_frame(2), 8);
    }

    #[test]
    fn one_period_of_a_sine_has_rms_of_amplitude_over_root_two() {
        // 1 kHz at 48 kHz is exactly 48 samples per period, and 48 samples hit the crest and
        // the trough exactly, so min and max are the amplitude to the last bit.
        let amplitude = 0.5f32;
        let samples: Vec<f32> = (0..96)
            .map(|i| amplitude * (std::f32::consts::TAU * i as f32 / 48.0).sin())
            .collect();
        let buffer = AudioBuffer::from_planar(vec![samples], 48_000.0).unwrap();
        let peaks = compute_peaks_cpu(&buffer, 48);
        assert_eq!(peaks.bucket_count(), 2);

        for bucket in 0..2 {
            let values = peaks.bucket(0, bucket).unwrap();
            assert!((values.max - amplitude).abs() < 1e-6, "{values:?}");
            assert!((values.min + amplitude).abs() < 1e-6, "{values:?}");
            let expected = amplitude / 2.0f32.sqrt();
            assert!((values.rms - expected).abs() < 1e-6, "{values:?}");
        }
    }

    #[test]
    fn channels_are_laid_out_channel_major() {
        let buffer = AudioBuffer::from_planar(
            vec![vec![1.0, 2.0, 3.0, 4.0], vec![-1.0, -2.0, -3.0, -4.0]],
            48_000.0,
        )
        .unwrap();
        let peaks = compute_peaks_cpu(&buffer, 2);
        assert_eq!(peaks.bucket_count(), 2);
        assert_eq!(peaks.min.len(), 4);

        assert_eq!(peaks.bucket(0, 0).unwrap().max, 2.0);
        assert_eq!(peaks.bucket(0, 1).unwrap().max, 4.0);
        assert_eq!(peaks.bucket(1, 0).unwrap().min, -2.0);
        assert_eq!(peaks.bucket(1, 1).unwrap().min, -4.0);
    }

    #[test]
    fn empty_buffer_produces_no_buckets() {
        let peaks = compute_peaks_cpu(&AudioBuffer::stereo(0, 48_000.0), 512);
        assert_eq!(peaks.bucket_count(), 0);
        assert!(peaks.is_empty());
        assert_eq!(peaks.bucket(0, 0), None);
    }

    #[test]
    fn zero_bucket_size_becomes_one_sample_per_bucket() {
        let peaks = compute_peaks_cpu(&ramp(3), 0);
        assert_eq!(peaks.samples_per_bucket, 1);
        assert_eq!(peaks.bucket_count(), 3);
        assert_eq!(peaks.bucket(0, 2).unwrap().rms, 2.0);
    }

    #[test]
    fn without_a_context_the_wrapper_is_the_cpu_path() {
        let buffer = ramp(1000);
        assert_eq!(
            compute_peaks(None, &buffer, 64),
            compute_peaks_cpu(&buffer, 64)
        );
    }

    #[test]
    fn gpu_matches_the_cpu_reference() {
        let Some(gpu) = GpuContext::new() else {
            return;
        };
        // Two channels, a length that is not a multiple of the bucket size, and content that
        // exercises both signs.
        let frames = 10_000;
        let left: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 0.01).sin() * 0.75)
            .collect();
        let right: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 0.003).cos() * 0.4 - 0.1)
            .collect();
        let buffer = AudioBuffer::from_planar(vec![left, right], 48_000.0).unwrap();

        let expected = compute_peaks_cpu(&buffer, 333);
        let actual = gpu
            .compute_peaks(&buffer, 333)
            .expect("a working adapter should complete the reduction");

        assert_eq!(actual.bucket_count(), expected.bucket_count());
        assert_eq!(actual.channel_count, expected.channel_count);
        for channel in 0..expected.channel_count {
            for bucket in 0..expected.bucket_count() {
                let want = expected.bucket(channel, bucket).unwrap();
                let got = actual.bucket(channel, bucket).unwrap();
                assert!(
                    (want.min - got.min).abs() < 1e-4
                        && (want.max - got.max).abs() < 1e-4
                        && (want.rms - got.rms).abs() < 1e-4,
                    "channel {channel} bucket {bucket}: cpu {want:?} gpu {got:?}"
                );
            }
        }
    }

    #[test]
    fn output_stays_rectangular_for_a_ragged_buffer() {
        // `AudioBuffer::channels_mut` can leave the channels different lengths, and
        // `frame_count()` only reports the first one. The layout contract says bucket `b` of
        // channel `c` is at `c * bucket_count() + b`, which only holds if every channel emits
        // the same number of buckets whatever its own length.
        let mut buffer =
            AudioBuffer::from_planar(vec![vec![1.0; 8], vec![2.0; 8], vec![3.0; 8]], 48_000.0)
                .unwrap();
        buffer.channels_mut()[1].truncate(3);
        buffer.channels_mut()[2].extend_from_slice(&[9.0; 8]);
        assert_eq!(buffer.frame_count(), 8);

        let peaks = compute_peaks_cpu(&buffer, 4);
        assert_eq!(peaks.channel_count, 3);
        assert_eq!(peaks.bucket_count(), 2);
        assert_eq!(peaks.min.len(), 6);
        assert_eq!(peaks.max.len(), 6);
        assert_eq!(peaks.rms.len(), 6);

        // The short channel keeps its real samples in bucket 0 and reads as silence in the
        // bucket it has no samples for.
        assert_eq!(peaks.bucket(1, 0).unwrap().max, 2.0);
        assert_eq!(
            peaks.bucket(1, 1).unwrap(),
            WaveformBucket {
                min: 0.0,
                max: 0.0,
                rms: 0.0
            }
        );
        // The long channel is cut off at `frame_count()`, so the 9.0s never appear.
        assert_eq!(peaks.bucket(2, 0).unwrap().max, 3.0);
        assert_eq!(peaks.bucket(2, 1).unwrap().max, 3.0);
        assert_eq!(peaks.bucket(2, 2), None);
    }

    #[test]
    fn an_empty_first_channel_does_not_hide_later_channels() {
        let mut buffer =
            AudioBuffer::from_planar(vec![vec![0.0; 4], vec![0.5; 4]], 48_000.0).unwrap();
        buffer.channels_mut()[0].clear();

        let peaks = compute_peaks_cpu(&buffer, 4);

        assert_eq!(peaks.bucket_count(), 1);
        assert_eq!(peaks.bucket(0, 0).unwrap().max, 0.0);
        assert_eq!(peaks.bucket(1, 0).unwrap().max, 0.5);
    }

    #[test]
    fn bucket_rejects_inconsistent_parallel_vectors() {
        // The three vectors are public; a caller can build a value whose lengths disagree, and
        // reading it must not panic.
        let peaks = WaveformPeaks {
            min: vec![0.0; 4],
            max: vec![1.0; 4],
            rms: Vec::new(),
            samples_per_bucket: 2,
            channel_count: 2,
        };
        assert_eq!(peaks.bucket_count(), 2);
        assert_eq!(peaks.bucket(0, 0), None);
        assert_eq!(peaks.bucket(1, 1), None);
    }

    #[test]
    fn gpu_handles_a_partial_trailing_bucket() {
        let Some(gpu) = GpuContext::new() else {
            return;
        };
        let peaks = gpu.compute_peaks(&ramp(10), 4).unwrap();
        assert_eq!(peaks.bucket_count(), 3);
        let last = peaks.bucket(0, 2).unwrap();
        assert_eq!(last.min, 8.0);
        assert_eq!(last.max, 9.0);
        assert!((last.rms - 72.5f32.sqrt()).abs() < 1e-4, "{last:?}");
    }

    #[test]
    fn gpu_keeps_the_chunked_path_for_a_ragged_buffer() {
        // The two conditions together: a channel shorter than the first *and* a buffer large
        // enough to chunk. A chunk boundary past the short channel's end used to make an
        // inverted slice, read as a GPU failure, and silently send exactly the buffers that
        // wanted the GPU most to the CPU. The short channel's missing buckets are silence, as
        // the CPU reference writes them.
        let Some(mut gpu) = GpuContext::new() else {
            return;
        };
        gpu.constrain_for_test(1000, 3);

        let frames = 10_000usize;
        let stride = 333u32;
        let long: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 0.01).sin() * 0.75)
            .collect();
        let mut buffer = AudioBuffer::from_planar(vec![long.clone(), long], 48_000.0).unwrap();
        buffer.channels_mut()[1].truncate(500);

        let expected = compute_peaks_cpu(&buffer, stride);
        let actual = gpu
            .compute_peaks(&buffer, stride)
            .expect("a ragged buffer must not read as a GPU failure");

        assert_eq!(actual.bucket_count(), expected.bucket_count());
        for channel in 0..2 {
            for bucket in 0..expected.bucket_count() {
                let want = expected.bucket(channel, bucket).unwrap();
                let got = actual.bucket(channel, bucket).unwrap();
                assert!(
                    (want.min - got.min).abs() < 1e-4
                        && (want.max - got.max).abs() < 1e-4
                        && (want.rms - got.rms).abs() < 1e-4,
                    "channel {channel} bucket {bucket}: {want:?} vs {got:?}"
                );
            }
        }
    }

    #[test]
    fn gpu_splits_long_channels_on_bucket_boundaries() {
        let Some(mut gpu) = GpuContext::new() else {
            return;
        };
        // A real adapter takes a gigabyte in one dispatch, so the chunking loop never runs for
        // a test-sized buffer. Shrink the limits until it does: with a 1000-sample ceiling and
        // 3 slots per dispatch, 333 samples per bucket gives 3 buckets per chunk, so 10_000
        // frames need 11 dispatches per channel and the last one carries a partial bucket that
        // is also the last bucket of the channel.
        gpu.constrain_for_test(1000, 3);

        let frames = 10_000usize;
        let stride = 333u32;
        let left: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 0.01).sin() * 0.75)
            .collect();
        let right: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 0.003).cos() * 0.4 - 0.1)
            .collect();
        let buffer = AudioBuffer::from_planar(vec![left, right], 48_000.0).unwrap();

        let expected = compute_peaks_cpu(&buffer, stride);
        let actual = gpu
            .compute_peaks(&buffer, stride)
            .expect("the chunked path should still complete");

        assert_eq!(actual.bucket_count(), expected.bucket_count());
        assert_eq!(actual.bucket_count(), frames.div_ceil(stride as usize));
        assert_eq!(actual.channel_count, 2);
        for channel in 0..2 {
            for bucket in 0..expected.bucket_count() {
                let want = expected.bucket(channel, bucket).unwrap();
                let got = actual.bucket(channel, bucket).unwrap();
                assert!(
                    (want.min - got.min).abs() < 1e-4
                        && (want.max - got.max).abs() < 1e-4
                        && (want.rms - got.rms).abs() < 1e-4,
                    "channel {channel} bucket {bucket}: cpu {want:?} gpu {got:?}"
                );
            }
        }
    }
}
