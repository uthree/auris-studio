//! Whole-buffer loudness measurement.
//!
//! Written for two offline callers: the export dialog, which shows the level of what was just
//! rendered, and a future "normalise" command, which needs a gain that brings a rendered mix to
//! a target level. Both scan every sample of a finished render, so the work is the same shape as
//! the waveform reduction and lands on the same GPU path.
//!
//! **Neither exists.** The export dialog reports `AudioBuffer::peak` instead — the loudest
//! sample, found by a CPU scan — so nothing shipped reports the inter-sample peak below, and a
//! mix reading -0.3 dBFS in the dialog may be over full scale between its samples without
//! anything saying so. Reaching this from an export means handing
//! `auris_session::RenderJob` a [`GpuContext`], which it has no field for by design:
//! it is a self-contained copy of a render, made to be sent to a worker thread. That is the work,
//! and it has not been done.
//!
//! Use [`analyze_loudness`] — it tries the GPU and silently falls back to
//! [`analyze_loudness_cpu`].

use auris_core::AudioBuffer;
use auris_core::param::{db_to_gain, gain_to_db};

use crate::context::GpuContext;

/// Pipeline cache key and debug label for the loudness kernel.
const LABEL: &str = "auris-gpu.loudness";

/// WGSL source of the loudness kernel.
const SHADER: &str = include_str!("shaders/loudness.wgsl");

/// Samples reduced by one GPU invocation.
///
/// Large enough that the per-invocation overhead disappears, small enough that the shader's
/// f32 sum of squares stays accurate — the relative error of a naive f32 sum grows roughly
/// with the square root of the term count, so 4096 terms cost about 4e-6, and the host adds
/// the partial sums back together in f64.
const SAMPLES_PER_INVOCATION: usize = 4096;

/// Samples the interpolator reads past the sample it is working on.
const LOOK_AHEAD: usize = 2;

/// Uniform block handed to the loudness kernel. Must match `Params` in `loudness.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    sample_count: u32,
    first_active: u32,
    last_active: u32,
    span: u32,
}

/// Level measurements over a whole buffer.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LoudnessAnalysis {
    /// Largest absolute sample value across all channels.
    pub peak: f32,
    /// Root-mean-square level over every sample of every channel.
    pub rms: f32,
    /// Estimated largest value of the reconstructed continuous signal.
    ///
    /// A signal can exceed its highest *sample* between samples, which is what clips a
    /// downstream converter even though the file itself measures below 0 dBFS. This is an
    /// estimate, not a BS.1770-4 true-peak meter: it reconstructs with 4x Catmull-Rom
    /// interpolation rather than the standard's 4x FIR upsampler, so it tracks the direction
    /// and rough size of the overshoot while typically reading a little low. It is never less
    /// than [`Self::peak`].
    pub true_peak_estimate: f32,
}

impl LoudnessAnalysis {
    /// The measurement of a buffer with no samples.
    pub fn silent() -> Self {
        Self {
            peak: 0.0,
            rms: 0.0,
            true_peak_estimate: 0.0,
        }
    }

    /// Builds the analysis from accumulated totals.
    fn from_totals(peak: f32, sum_squares: f64, count: usize, true_peak: f32) -> Self {
        let rms = if count == 0 {
            0.0
        } else {
            (sum_squares / count as f64).sqrt() as f32
        };
        Self {
            peak,
            rms,
            true_peak_estimate: true_peak.max(peak),
        }
    }

    /// Sample peak in dBFS, floored at -120 dB.
    pub fn peak_db(&self) -> f32 {
        gain_to_db(self.peak)
    }

    /// RMS level in dBFS, floored at -120 dB.
    pub fn rms_db(&self) -> f32 {
        gain_to_db(self.rms)
    }

    /// Estimated true peak in dBFS, floored at -120 dB.
    pub fn true_peak_db(&self) -> f32 {
        gain_to_db(self.true_peak_estimate)
    }

    /// Linear gain that would bring the estimated true peak to `target_db`.
    ///
    /// Returns 1.0 for silence, where no gain reaches the target. Normalising against the true
    /// peak rather than the sample peak is what keeps the result from clipping a converter.
    pub fn normalization_gain(&self, target_db: f32) -> f32 {
        if self.true_peak_estimate <= 1e-9 {
            1.0
        } else {
            db_to_gain(target_db) / self.true_peak_estimate
        }
    }
}

/// Measures peak, RMS and estimated true peak on the CPU.
///
/// This is the reference implementation the GPU kernel is tested against, and what runs when
/// no GPU is available. The sum of squares is accumulated in `f64` so that a long render does
/// not lose precision.
pub fn analyze_loudness_cpu(buffer: &AudioBuffer) -> LoudnessAnalysis {
    let mut peak = 0.0f32;
    let mut true_peak = 0.0f32;
    let mut sum_squares = 0.0f64;
    let mut count = 0usize;

    for samples in buffer.channels() {
        for (index, &value) in samples.iter().enumerate() {
            peak = peak.max(value.abs());
            sum_squares += (value * value) as f64;

            let previous = sample_clamped(samples, index.saturating_sub(1));
            let next = sample_clamped(samples, index + 1);
            let after = sample_clamped(samples, index + 2);
            for step in [0.25, 0.5, 0.75] {
                true_peak = true_peak.max(catmull_rom(previous, value, next, after, step).abs());
            }
        }
        count += samples.len();
    }

    LoudnessAnalysis::from_totals(peak, sum_squares, count, true_peak)
}

/// Reads a sample, clamping to the last one. Matches `sample_at` in `loudness.wgsl`.
fn sample_clamped(samples: &[f32], index: usize) -> f32 {
    match samples.last() {
        Some(&last) => samples.get(index).copied().unwrap_or(last),
        None => 0.0,
    }
}

/// Catmull-Rom interpolation between `p1` and `p2`, with `p0` and `p3` as the outer control
/// points. Matches `catmull_rom` in `loudness.wgsl` exactly, including the order of operations,
/// so the two implementations agree to within f32 rounding.
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a = 2.0 * p1;
    let b = p2 - p0;
    let c = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
    let d = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    0.5 * (a + b * t + c * t * t + d * t * t * t)
}

impl GpuContext {
    /// Measures peak, RMS and estimated true peak with a WGSL compute shader.
    ///
    /// Each invocation reduces a fixed span of samples to a partial
    /// (min, max, sum of squares, true peak); the host folds the partials together, summing in
    /// `f64`. Channels longer than the device's storage-binding limit are split into chunks
    /// that overlap by a sample on each side so the interpolator never sees a false edge.
    ///
    /// Returns `None` on any wgpu failure; use [`analyze_loudness`] to fall back automatically.
    pub fn analyze_loudness(&self, buffer: &AudioBuffer) -> Option<LoudnessAnalysis> {
        // `frame_count()` only describes the first channel, and `AudioBuffer::channels_mut`
        // lets the rest differ. The CPU reference scans every channel, so take the shortcut
        // only when there is genuinely nothing to scan.
        if buffer.channels().iter().all(|samples| samples.is_empty()) {
            return Some(LoudnessAnalysis::silent());
        }

        // Whole invocations only, and leave room for the samples the overlap adds.
        let headroom = self.max_input_samples().saturating_sub(LOOK_AHEAD + 1);
        let per_chunk = (headroom / SAMPLES_PER_INVOCATION).min(self.max_output_slots())
            * SAMPLES_PER_INVOCATION;
        if per_chunk == 0 {
            log::debug!("auris-gpu: device limits leave no room for a loudness dispatch");
            return None;
        }

        let mut peak = 0.0f32;
        let mut true_peak = 0.0f32;
        let mut sum_squares = 0.0f64;
        let mut count = 0usize;

        for samples in buffer.channels() {
            let mut start = 0;
            while start < samples.len() {
                let active = per_chunk.min(samples.len() - start);
                // Overlap by one sample behind and two ahead so that every interpolation this
                // dispatch performs sees the same neighbours the CPU reference would.
                let lead = usize::from(start > 0);
                let tail = LOOK_AHEAD.min(samples.len() - start - active);
                let chunk = samples.get(start - lead..start + active + tail)?;

                let params = Params {
                    sample_count: chunk.len() as u32,
                    first_active: lead as u32,
                    last_active: (lead + active) as u32,
                    span: SAMPLES_PER_INVOCATION as u32,
                };
                let slots = active.div_ceil(SAMPLES_PER_INVOCATION);
                for slot in self.run_reduction(LABEL, SHADER, chunk, params, slots)? {
                    peak = peak.max(slot[0].abs()).max(slot[1].abs());
                    sum_squares += slot[2] as f64;
                    true_peak = true_peak.max(slot[3]);
                }
                start += active;
            }
            count += samples.len();
        }

        Some(LoudnessAnalysis::from_totals(
            peak,
            sum_squares,
            count,
            true_peak,
        ))
    }
}

/// Measures a buffer's loudness, preferring the GPU.
///
/// This is what callers use. A `None` context, or any GPU failure, transparently runs
/// [`analyze_loudness_cpu`] instead.
pub fn analyze_loudness(gpu: Option<&GpuContext>, buffer: &AudioBuffer) -> LoudnessAnalysis {
    if let Some(gpu) = gpu {
        if let Some(analysis) = gpu.analyze_loudness(buffer) {
            return analysis;
        }
        log::debug!("auris-gpu: loudness analysis fell back to the CPU");
    }
    analyze_loudness_cpu(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `frames` samples of a sine at `period` samples per cycle, scaled to `amplitude`.
    fn sine(frames: usize, period: f32, amplitude: f32, phase: f32) -> AudioBuffer {
        let samples = (0..frames)
            .map(|i| amplitude * (std::f32::consts::TAU * i as f32 / period + phase).sin())
            .collect();
        AudioBuffer::from_planar(vec![samples], 48_000.0).unwrap()
    }

    #[test]
    fn full_scale_sine_measures_minus_three_db_rms() {
        // 48 samples per period lands exactly on the crest and the trough.
        let analysis = analyze_loudness_cpu(&sine(480, 48.0, 1.0, 0.0));
        assert!((analysis.peak - 1.0).abs() < 1e-6, "{analysis:?}");
        assert!(
            (analysis.rms - 1.0 / 2.0f32.sqrt()).abs() < 1e-5,
            "{analysis:?}"
        );
        // A sine's RMS is 3.0103 dB below its peak.
        assert!((analysis.rms_db() + 3.0103).abs() < 1e-2, "{analysis:?}");
        assert!((analysis.peak_db()).abs() < 1e-3, "{analysis:?}");
    }

    #[test]
    fn silence_measures_as_the_floor() {
        let analysis = analyze_loudness_cpu(&AudioBuffer::stereo(256, 48_000.0));
        assert_eq!(analysis, LoudnessAnalysis::silent());
        assert_eq!(analysis.rms_db(), -120.0);
        assert_eq!(analysis.normalization_gain(-1.0), 1.0);
    }

    #[test]
    fn empty_buffer_measures_as_silence() {
        let analysis = analyze_loudness_cpu(&AudioBuffer::stereo(0, 48_000.0));
        assert_eq!(analysis, LoudnessAnalysis::silent());
    }

    #[test]
    fn constant_signal_has_no_inter_sample_overshoot() {
        // Catmull-Rom through four equal control points is that constant, so a DC buffer must
        // not invent an overshoot.
        let buffer = AudioBuffer::from_planar(vec![vec![0.5; 64]], 48_000.0).unwrap();
        let analysis = analyze_loudness_cpu(&buffer);
        assert_eq!(analysis.peak, 0.5);
        assert_eq!(analysis.rms, 0.5);
        assert_eq!(analysis.true_peak_estimate, 0.5);
        // 0.5 -> 1.0 is exactly +6.0206 dB, so normalising to 0 dBFS doubles the signal.
        assert!((analysis.normalization_gain(0.0) - 2.0).abs() < 1e-4);
        assert!((analysis.normalization_gain(-6.0206) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn inter_sample_peak_exceeds_the_sample_peak() {
        // A quarter-rate sine offset by 45 degrees samples at +-0.70711 while the underlying
        // waveform reaches 1.0, the classic inter-sample peak case. Catmull-Rom recovers
        // 0.883883 of that: 0.5 * (2*p1 + (p2-p0)/2 - (2*p0-5*p1+4*p2-p3)/4) at t = 0.5.
        let buffer = sine(64, 4.0, 1.0, std::f32::consts::FRAC_PI_4);
        let analysis = analyze_loudness_cpu(&buffer);
        assert!(
            (analysis.peak - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
            "{analysis:?}"
        );
        assert!(
            (analysis.true_peak_estimate - 0.883_883_5).abs() < 1e-4,
            "{analysis:?}"
        );
        assert!(analysis.true_peak_estimate > analysis.peak);
        // The overshoot is about 1.94 dB above the sample peak.
        assert!(
            (analysis.true_peak_db() - analysis.peak_db() - 1.938).abs() < 1e-2,
            "{analysis:?}"
        );
    }

    #[test]
    fn rms_covers_every_channel() {
        // One full-scale channel and one silent channel average to -3 dB of power, i.e. an
        // overall RMS of 1/sqrt(2) of the loud channel's.
        let buffer =
            AudioBuffer::from_planar(vec![vec![1.0; 128], vec![0.0; 128]], 48_000.0).unwrap();
        let analysis = analyze_loudness_cpu(&buffer);
        assert_eq!(analysis.peak, 1.0);
        assert!(
            (analysis.rms - 1.0 / 2.0f32.sqrt()).abs() < 1e-6,
            "{analysis:?}"
        );
    }

    #[test]
    fn without_a_context_the_wrapper_is_the_cpu_path() {
        let buffer = sine(1000, 37.0, 0.6, 0.3);
        assert_eq!(
            analyze_loudness(None, &buffer),
            analyze_loudness_cpu(&buffer)
        );
    }

    #[test]
    fn gpu_matches_the_cpu_reference() {
        let Some(gpu) = GpuContext::new() else {
            return;
        };
        // Long enough to need several invocations, and not a multiple of the span.
        let frames = 12_345;
        let left: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 0.017).sin() * 0.8)
            .collect();
        let right: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 1.5).sin() * 0.55 + 0.05)
            .collect();
        let buffer = AudioBuffer::from_planar(vec![left, right], 48_000.0).unwrap();

        let expected = analyze_loudness_cpu(&buffer);
        let actual = gpu
            .analyze_loudness(&buffer)
            .expect("a working adapter should complete the reduction");

        assert!(
            (expected.peak - actual.peak).abs() < 1e-4,
            "cpu {expected:?} gpu {actual:?}"
        );
        assert!(
            (expected.rms - actual.rms).abs() < 1e-4,
            "cpu {expected:?} gpu {actual:?}"
        );
        assert!(
            (expected.true_peak_estimate - actual.true_peak_estimate).abs() < 1e-4,
            "cpu {expected:?} gpu {actual:?}"
        );
    }

    #[test]
    fn gpu_matches_the_cpu_across_chunk_boundaries() {
        let Some(mut gpu) = GpuContext::new() else {
            return;
        };
        // A real adapter swallows a whole channel in one dispatch, so the overlap logic — one
        // sample behind and two ahead, so the interpolator never meets a false edge — is dead
        // code in the other GPU test. One output slot per dispatch forces a chunk every
        // SAMPLES_PER_INVOCATION samples instead.
        gpu.constrain_for_test(usize::MAX, 1);

        // Deliberately full of inter-sample peaks: a quarter-rate sine at 45 degrees overshoots
        // by 1.94 dB. The length is not a multiple of the chunk size. The second channel stays
        // quieter than the first channel's *sample* peak of 1/sqrt(2), so the overshoot is what
        // sets `true_peak_estimate` for the whole buffer.
        let frames = 3 * SAMPLES_PER_INVOCATION + 1_234;
        let left: Vec<f32> = (0..frames)
            .map(|i| (std::f32::consts::TAU * i as f32 / 4.0 + std::f32::consts::FRAC_PI_4).sin())
            .collect();
        let right: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * 0.017).sin() * 0.5)
            .collect();
        let buffer = AudioBuffer::from_planar(vec![left, right], 48_000.0).unwrap();

        let expected = analyze_loudness_cpu(&buffer);
        let actual = gpu
            .analyze_loudness(&buffer)
            .expect("the chunked path should still complete");

        // The overshoot must actually be present, or the test proves nothing about the overlap.
        assert!(
            expected.true_peak_estimate > expected.peak * 1.2,
            "{expected:?}"
        );
        assert!(
            (expected.peak - actual.peak).abs() < 1e-4,
            "cpu {expected:?} gpu {actual:?}"
        );
        assert!(
            (expected.rms - actual.rms).abs() < 1e-4,
            "cpu {expected:?} gpu {actual:?}"
        );
        assert!(
            (expected.true_peak_estimate - actual.true_peak_estimate).abs() < 1e-4,
            "cpu {expected:?} gpu {actual:?}"
        );
    }

    #[test]
    fn chunk_overlap_preserves_an_overshoot_that_straddles_the_boundary() {
        let Some(mut gpu) = GpuContext::new() else {
            return;
        };
        // One output slot per dispatch puts a chunk boundary every SAMPLES_PER_INVOCATION
        // samples: chunk 0 reduces [0, 4096), chunk 1 reduces [4096, 8192).
        gpu.constrain_for_test(usize::MAX, 1);
        let edge = SAMPLES_PER_INVOCATION;

        // A periodic signal hides a broken overlap, because the same overshoot recurs
        // everywhere and the buffer maximum survives losing one sample's worth of it. So plant
        // exactly one overshoot in an otherwise silent buffer and put it on the seam.
        //
        // `-A, A, A, -A` is the maximal Catmull-Rom case: it reaches 1.25 * A halfway between
        // the two middle samples. Placed at `edge - 2`, the peak is evaluated at `edge - 1` —
        // the last sample chunk 0 reduces — and needs `edge` and `edge + 1`, which only exist
        // in that dispatch because of the two-sample look-ahead. Placed at `edge - 1`, the peak
        // is evaluated at `edge`, the first sample chunk 1 reduces, and needs `edge - 1`, which
        // is there only because of the one-sample look-behind.
        const A: f32 = 0.8;
        for (name, first) in [("look-ahead", edge - 2), ("look-behind", edge - 1)] {
            let mut samples = vec![0.0f32; 2 * SAMPLES_PER_INVOCATION + 500];
            samples[first] = -A;
            samples[first + 1] = A;
            samples[first + 2] = A;
            samples[first + 3] = -A;
            let buffer = AudioBuffer::from_planar(vec![samples], 48_000.0).unwrap();

            let expected = analyze_loudness_cpu(&buffer);
            // The whole point of the buffer: its true peak is that one interpolated overshoot,
            // 25% above the highest sample. Dropping a neighbour drops it to 1.1406 * A, which
            // is far outside the 1e-4 tolerance below.
            assert!(
                (expected.peak - A).abs() < 1e-6,
                "{name}: {expected:?} should peak at the sample value"
            );
            assert!(
                (expected.true_peak_estimate - 1.25 * A).abs() < 1e-5,
                "{name}: {expected:?} should overshoot to 1.25 * A"
            );

            let actual = gpu
                .analyze_loudness(&buffer)
                .expect("the chunked path should still complete");
            assert!(
                (expected.true_peak_estimate - actual.true_peak_estimate).abs() < 1e-4,
                "{name}: cpu {expected:?} gpu {actual:?}"
            );
            assert!(
                (expected.peak - actual.peak).abs() < 1e-4
                    && (expected.rms - actual.rms).abs() < 1e-4,
                "{name}: cpu {expected:?} gpu {actual:?}"
            );
        }
    }

    #[test]
    fn mono_and_ragged_buffers_measure_the_same_on_both_paths() {
        let mut buffer =
            AudioBuffer::from_planar(vec![vec![0.25; 500], vec![-0.5; 500]], 48_000.0).unwrap();
        buffer.channels_mut()[1].truncate(100);

        let cpu = analyze_loudness_cpu(&buffer);
        // 500 samples at 0.25 and 100 at -0.5: peak is 0.5 and the mean square is
        // (500 * 0.0625 + 100 * 0.25) / 600.
        assert_eq!(cpu.peak, 0.5);
        let expected_rms = ((500.0 * 0.0625 + 100.0 * 0.25) / 600.0f32).sqrt();
        assert!((cpu.rms - expected_rms).abs() < 1e-6, "{cpu:?}");

        if let Some(gpu) = GpuContext::new() {
            let actual = gpu
                .analyze_loudness(&buffer)
                .expect("a working adapter should complete the reduction");
            assert!(
                (cpu.peak - actual.peak).abs() < 1e-4,
                "cpu {cpu:?} gpu {actual:?}"
            );
            assert!(
                (cpu.rms - actual.rms).abs() < 1e-4,
                "cpu {cpu:?} gpu {actual:?}"
            );
            assert!(
                (cpu.true_peak_estimate - actual.true_peak_estimate).abs() < 1e-4,
                "cpu {cpu:?} gpu {actual:?}"
            );
        }
    }
}
