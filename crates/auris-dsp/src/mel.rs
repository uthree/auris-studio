//! Bounded, fixed-reference mel power images for offline inspection.

use auris_core::AudioBuffer;
use std::sync::atomic::{AtomicBool, Ordering};

/// Mel bands from low to high, stored column-major. Values are power dB relative to 1.
pub struct MelSpectrum {
    /// Number of uniformly spaced time windows.
    pub columns: usize,
    /// HTK mel filter centre frequencies in hertz.
    pub frequencies: Vec<f64>,
    /// Column-major power levels, floored at -90 dB without per-image normalization.
    pub levels: Vec<f32>,
}

/// Analyze at most 512 centred Hann windows using 64 triangular HTK mel filters.
/// Channel powers are averaged, avoiding cancellation of opposite-phase stereo.
/// Returns `None` for cancellation or invalid/non-finite audio.
pub fn analyse(audio: &AudioBuffer, cancel: &AtomicBool) -> Option<MelSpectrum> {
    const SIZE: usize = 2048;
    const BANDS: usize = 64;
    let rate = audio.sample_rate();
    let frames = audio.frame_count();
    if frames == 0 || !rate.is_finite() || rate < 100.0 {
        return None;
    }
    let channels = audio.iter_channels().count();
    if channels == 0 {
        return None;
    }
    let mel = |hz: f64| 2595.0 * (1.0 + hz / 700.0).log10();
    let high = mel((rate / 2.0).min(20_000.0));
    let edges: Vec<_> = (0..BANDS + 2)
        .map(|i| 700.0 * (10.0_f64.powf(high * i as f64 / (BANDS + 1) as f64 / 2595.0) - 1.0))
        .collect();
    let filters: Vec<Vec<(usize, f64)>> = edges
        .windows(3)
        .map(|edge| {
            (0..=SIZE / 2)
                .filter_map(|bin| {
                    let hz = bin as f64 * rate / SIZE as f64;
                    let weight = ((hz - edge[0]) / (edge[1] - edge[0]))
                        .min((edge[2] - hz) / (edge[2] - edge[1]))
                        .clamp(0.0, 1.0);
                    (weight > 0.0).then_some((bin, weight))
                })
                .collect()
        })
        .collect();
    let columns = frames.div_ceil(512).min(512);
    let mut levels = Vec::with_capacity(columns * BANDS);
    let mut analyzer = crate::SpectrumAnalyzer::new(SIZE);
    let mut samples = vec![0.0; SIZE];
    let mut bins = vec![0.0; analyzer.bin_count()];
    for column in 0..columns {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let centre = ((column as f64 + 0.5) * frames as f64 / columns as f64) as i64;
        let mut power = [0.0; BANDS];
        for channel in audio.iter_channels() {
            for (i, sample) in samples.iter_mut().enumerate() {
                let frame = centre + i as i64 - SIZE as i64 / 2;
                *sample = usize::try_from(frame)
                    .ok()
                    .and_then(|frame| channel.get(frame))
                    .copied()
                    .unwrap_or(0.0);
                if !sample.is_finite() {
                    return None;
                }
            }
            analyzer.reset();
            analyzer.push(&samples);
            analyzer.magnitudes(&mut bins);
            for (band, filter) in filters.iter().enumerate() {
                power[band] += filter
                    .iter()
                    .map(|&(bin, weight)| weight * 10.0_f64.powf(f64::from(bins[bin]) / 10.0))
                    .sum::<f64>()
                    / channels as f64;
            }
        }
        levels.extend(power.map(|power| (10.0 * power.max(1e-9).log10()) as f32));
    }
    Some(MelSpectrum {
        columns,
        frequencies: edges[1..=BANDS].to_vec(),
        levels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gain_and_opposite_phase_stereo_keep_their_measured_power() {
        let make = |gain: f32| {
            let wave = crate::spectrum::sine(1000.0, 48000.0, gain, 48000);
            AudioBuffer::from_planar(
                vec![wave.clone(), wave.iter().map(|v| -v).collect()],
                48000.0,
            )
            .unwrap()
        };
        let cancel = AtomicBool::new(false);
        let loud = analyse(&make(0.8), &cancel).unwrap();
        let soft = analyse(&make(0.4), &cancel).unwrap();
        let band = loud
            .frequencies
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (*a - 1000.0).abs().total_cmp(&(*b - 1000.0).abs()))
            .unwrap()
            .0;
        let index = (loud.columns / 2) * 64 + band;
        assert!((loud.levels[index] - soft.levels[index] - 6.0206).abs() < 0.05);
        assert!(loud.levels[index] > -15.0);
        cancel.store(true, Ordering::Relaxed);
        assert!(analyse(&make(0.8), &cancel).is_none());
    }
}
