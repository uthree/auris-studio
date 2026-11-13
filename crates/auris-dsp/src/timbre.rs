//! Offline timbre descriptors and deterministic grouping; no trained weights or audio-thread work.

use auris_core::AudioBuffer;
use rustfft::{FftPlanner, num_complex::Complex};

use crate::drum_analysis::{AcousticCharacter, analyze_drum_audio};

#[path = "timbre_character.rs"]
mod character;
pub use character::{TimbreCharacter, timbre_character};

/// Measures gain-independent MFCC statistics, spectra and envelope over attack, body and release.
///
/// `release_seconds` is the known note-off time in this recording. Stereo powers are summed,
/// never waveforms, so opposite-phase channels remain audible to the analysis. Returns `None`
/// for silence. Inputs obey the same finite PCM, duration and rate bounds as drum analysis.
pub fn timbre_features(
    audio: &AudioBuffer,
    release_seconds: f64,
) -> Result<Option<Vec<f64>>, &'static str> {
    if !release_seconds.is_finite()
        || release_seconds <= 0.0
        || release_seconds >= audio.frame_count() as f64 / audio.sample_rate()
    {
        return Err("timbre analysis requires note-off inside the recording");
    }
    let measured = analyze_drum_audio(audio)?;
    if measured.character == AcousticCharacter::Silent {
        return Ok(None);
    }
    let rate = audio.sample_rate();
    let n = ((rate * 0.046).round() as usize).next_power_of_two();
    let hop = n / 2;
    let fft = FftPlanner::<f64>::new().plan_fft_forward(n);
    let mut bins = vec![Complex::default(); n];
    let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
    let mut power = vec![0.0; n / 2 + 1];
    let window: Vec<_> = (0..n)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        .collect();
    let mel = |hz: f64| 2595.0 * (1.0 + hz / 700.0).log10();
    let hz = |m: f64| 700.0 * (10.0f64.powf(m / 2595.0) - 1.0);
    let edges: Vec<_> = (0..34)
        .map(|i| hz(mel(40.0) + (mel((rate / 2.0).min(10_000.0)) - mel(40.0)) * i as f64 / 33.0))
        .collect();
    let filters: Vec<Vec<_>> = edges
        .windows(3)
        .map(|e| {
            (0..power.len())
                .map(|i| {
                    let f = i as f64 * rate / n as f64;
                    ((f - e[0]) / (e[1] - e[0]))
                        .min((e[2] - f) / (e[2] - e[1]))
                        .clamp(0.0, 1.0)
                })
                .collect()
        })
        .collect();
    let mut regions: [Vec<[f64; 12]>; 3] = Default::default();
    for start in (0..audio.frame_count()).step_by(hop) {
        power.fill(0.0);
        for channel in audio.channels() {
            for (i, bin) in bins.iter_mut().enumerate() {
                *bin = Complex::new(
                    f64::from(channel.get(start + i).copied().unwrap_or(0.0)) * window[i],
                    0.0,
                );
            }
            fft.process_with_scratch(&mut bins, &mut scratch);
            for (p, b) in power.iter_mut().zip(&bins) {
                *p += b.norm_sqr();
            }
        }
        let total: f64 = power.iter().sum();
        if total <= measured.peak.powi(2) * 1e-8 {
            continue;
        }
        let logs: Vec<f64> = filters
            .iter()
            .map(|filter| {
                (filter.iter().zip(&power).map(|(w, p)| w * p).sum::<f64>() / total)
                    .max(1e-10)
                    .ln()
            })
            .collect();
        let coefficients = std::array::from_fn(|k| {
            // Omit coefficient zero: absolute spectral energy is not a timbre coordinate.
            logs.iter()
                .enumerate()
                .map(|(m, value)| {
                    value * (std::f64::consts::PI * (k + 1) as f64 * (m as f64 + 0.5) / 32.0).cos()
                })
                .sum::<f64>()
                / 32.0f64.sqrt()
        });
        let time = start as f64 / rate;
        let region = if time >= release_seconds {
            2
        } else if time < measured.onset_seconds + 0.1 {
            0
        } else {
            1
        };
        regions[region].push(coefficients);
    }
    let mut features = Vec::new();
    for region in &regions {
        for k in 0..12 {
            let mean =
                region.iter().map(|frame| frame[k]).sum::<f64>() / region.len().max(1) as f64;
            let variance = region
                .iter()
                .map(|frame| (frame[k] - mean).powi(2))
                .sum::<f64>()
                / region.len().max(1) as f64;
            features.extend([mean, variance.sqrt()]);
        }
    }
    for spectrum in [&measured.attack, &measured.body, &measured.tail] {
        features.extend([
            spectrum.low,
            spectrum.body,
            spectrum.high,
            spectrum.centroid_hz.ln_1p(),
            spectrum.flatness,
            spectrum.concentration,
        ]);
    }
    features.extend([
        measured.onset_seconds,
        measured.energy_duration_seconds,
        measured.sustained_energy,
    ]);
    Ok(Some(features))
}

/// Standardized features and deterministic k-means memberships fitted in that space.
#[derive(Clone, Debug)]
pub struct TimbreGroups {
    /// Coordinates centered and scaled per feature; use these for nearest-neighbor queries.
    pub standardized: Vec<Vec<f64>>,
    /// Zero-based k-means groups fitted in the full feature space.
    pub clusters: Vec<usize>,
}

#[path = "timbre_clusters.rs"]
mod clustering;

/// Maximum number of groups in a refined library timbre map.
pub const MAX_TIMBRE_CLUSTERS: usize = 24;

/// Groups a library catalogue and refines populous groups.
///
/// Starts with up to twelve coarse groups, then bisects groups larger than twice the average
/// coarse population (at least twelve sounds). Splits need two children of at least three sounds
/// and a five-percent reduction in within-group squared error. At most 24 groups are returned;
/// identical or weakly separable sounds may stay together above the population target.
pub fn group_timbres_refined(
    rows: &[Vec<f64>],
    initial_clusters: usize,
) -> Result<TimbreGroups, &'static str> {
    let mut groups = group_timbres(rows, initial_clusters.clamp(1, 12))?;
    groups.clusters =
        clustering::refine(&groups.standardized, groups.clusters, MAX_TIMBRE_CLUSTERS);
    Ok(groups)
}

/// Centers and scales the full feature space for acoustic nearest-neighbor search.
/// Constant features become zero; malformed or excessive matrices are rejected.
pub fn standardize_timbres(rows: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, &'static str> {
    let dims = rows.first().map_or(0, Vec::len);
    if rows.len() > 100_000
        || dims > 2048
        || rows
            .iter()
            .any(|r| r.len() != dims || r.iter().any(|v| !v.is_finite()))
    {
        return Err("invalid or excessive timbre matrix");
    }
    let mut data = rows.to_vec();
    for d in 0..dims {
        let mean = rows.iter().map(|r| r[d]).sum::<f64>() / rows.len().max(1) as f64;
        let sd = (rows.iter().map(|r| (r[d] - mean).powi(2)).sum::<f64>()
            / rows.len().max(1) as f64)
            .sqrt();
        for row in &mut data {
            row[d] = if sd > 1e-7 { (row[d] - mean) / sd } else { 0.0 };
        }
    }

    Ok(data)
}

/// Standardizes a bounded catalogue and groups it reproducibly. Empty input has no groups.
///
/// Constant features become zero. k-means starts from farthest-first seeds with stable ties,
/// so the same catalogue always gives the same groups.
pub fn group_timbres(rows: &[Vec<f64>], clusters: usize) -> Result<TimbreGroups, &'static str> {
    if rows.len() > 512 {
        return Err("invalid or excessive timbre matrix");
    }
    let data = standardize_timbres(rows)?;
    let references: Vec<_> = data.iter().map(Vec::as_slice).collect();
    let groups = kmeans(&references, clusters.clamp(1, rows.len().max(1)));
    Ok(TimbreGroups {
        standardized: data,
        clusters: groups,
    })
}

fn distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}
fn kmeans(rows: &[&[f64]], k: usize) -> Vec<usize> {
    let Some(first) = rows.first() else {
        return Vec::new();
    };
    let mut centers = vec![first.to_vec()];
    while centers.len() < k {
        let farthest = rows
            .iter()
            .max_by(|a, b| {
                let nearest = |r: &[f64]| {
                    centers
                        .iter()
                        .map(|c| distance(r, c))
                        .fold(f64::INFINITY, f64::min)
                };
                nearest(a).total_cmp(&nearest(b))
            })
            .unwrap();
        if centers.iter().any(|c| distance(c, farthest) < 1e-12) {
            break;
        }
        centers.push(farthest.to_vec());
    }
    lloyd(rows, centers)
}

fn lloyd(rows: &[&[f64]], mut centers: Vec<Vec<f64>>) -> Vec<usize> {
    let Some(first) = rows.first() else {
        return Vec::new();
    };
    let mut labels = vec![usize::MAX; rows.len()];
    for _ in 0..60 {
        let next: Vec<_> = rows
            .iter()
            .map(|r| {
                (0..centers.len())
                    .min_by(|&a, &b| distance(r, &centers[a]).total_cmp(&distance(r, &centers[b])))
                    .unwrap()
            })
            .collect();
        if next == labels {
            break;
        }
        labels = next;
        let mut sums = vec![vec![0.0; first.len()]; centers.len()];
        let mut counts = vec![0; centers.len()];
        for (row, &label) in rows.iter().zip(&labels) {
            counts[label] += 1;
            for (sum, value) in sums[label].iter_mut().zip(row.iter()) {
                *sum += value;
            }
        }
        for i in 0..centers.len() {
            if counts[i] > 0 {
                for v in &mut sums[i] {
                    *v /= counts[i] as f64;
                }
                centers[i] = sums[i].clone();
            }
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acoustic_standardization_scales_beyond_the_grouping_limit_and_zeroes_constants() {
        let rows = (0..1024)
            .map(|i| vec![f64::from(i), 7.0])
            .collect::<Vec<_>>();
        let normalized = standardize_timbres(&rows).unwrap();
        let mean = normalized.iter().map(|r| r[0]).sum::<f64>() / 1024.0;
        let variance = normalized.iter().map(|r| r[0] * r[0]).sum::<f64>() / 1024.0;
        assert!(mean.abs() < 1e-12 && (variance - 1.0).abs() < 1e-12);
        assert!(normalized.iter().all(|r| r[1] == 0.0));
        assert!(group_timbres(&rows, 2).is_err());
        assert!(standardize_timbres(&[vec![f64::NAN]]).is_err());
        assert!(standardize_timbres(&[vec![1.0], vec![]]).is_err());
    }

    fn tone(gain: f32, anti_phase: bool) -> AudioBuffer {
        let mut audio = AudioBuffer::stereo(16_000, 16_000.0);
        for i in 0..16_000 {
            let value = gain * (std::f32::consts::TAU * 440.0 * i as f32 / 16_000.0).sin();
            audio.channel_mut(0)[i] = value;
            audio.channel_mut(1)[i] = if anti_phase { -value } else { value };
        }
        audio
    }

    #[test]
    fn descriptors_ignore_gain_and_stereo_polarity() {
        let a = timbre_features(&tone(0.5, false), 0.6).unwrap().unwrap();
        let b = timbre_features(&tone(0.05, true), 0.6).unwrap().unwrap();
        assert_eq!(a.len(), 93);
        assert!(distance(&a, &b) < 1e-6);
        assert!(
            timbre_features(&AudioBuffer::stereo(16_000, 16_000.0), 0.6)
                .unwrap()
                .is_none()
        );
        let mut invalid = tone(0.5, false);
        invalid.channel_mut(0)[2] = f32::NAN;
        assert!(timbre_features(&invalid, 0.6).is_err());
    }

    #[test]
    fn grouping_separates_clusters_repeatably() {
        let rows = vec![vec![0.0], vec![0.1], vec![10.0], vec![10.1]];
        let grouped = group_timbres(&rows, 2).unwrap();
        assert_eq!(grouped.clusters[0], grouped.clusters[1]);
        assert_eq!(grouped.clusters[2], grouped.clusters[3]);
        assert_ne!(grouped.clusters[0], grouped.clusters[2]);
        assert_eq!(grouped.clusters, group_timbres(&rows, 2).unwrap().clusters);
    }

    #[test]
    fn constant_empty_and_invalid_catalogues_are_handled() {
        assert!(group_timbres(&[], 4).unwrap().clusters.is_empty());
        let single = group_timbres(&[vec![1.0; 3]; 1], 5).unwrap();
        assert_eq!(single.standardized, vec![vec![0.0; 3]]);
        assert_eq!(single.clusters, vec![0]);
        assert!(group_timbres(&[vec![f64::INFINITY]], 1).is_err());
        assert!(group_timbres(&[vec![1.0], vec![]], 1).is_err());
        assert!(group_timbres_refined(&[], 12).unwrap().clusters.is_empty());
    }

    #[test]
    fn refinement_preserves_full_feature_similarity() {
        let rows: Vec<_> = (0..40)
            .map(|i| vec![(i as f64 * 0.2).sin(), (i as f64 * 0.4).cos()])
            .collect();
        let coarse = group_timbres(&rows, 6).unwrap();
        let refined = group_timbres_refined(&rows, 6).unwrap();
        assert_eq!(refined.standardized, coarse.standardized);
    }

    #[test]
    fn descriptors_separate_tone_from_noise_and_capture_decay() {
        let sustained = tone(0.5, false);
        let mut noise = sustained.clone();
        let mut decaying = sustained.clone();
        let mut seed = 73u32;
        for i in 0..16_000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let value = (seed as f64 / u32::MAX as f64 - 0.5) as f32;
            for channel in 0..2 {
                noise.channel_mut(channel)[i] = value;
                decaying.channel_mut(channel)[i] *= (-(i as f64) / 800.0).exp() as f32;
            }
        }
        let a = timbre_features(&sustained, 0.6).unwrap().unwrap();
        let b = timbre_features(&noise, 0.6).unwrap().unwrap();
        let c = timbre_features(&decaying, 0.6).unwrap().unwrap();
        assert!(distance(&a[..72], &b[..72]) > 10.0);
        assert!(
            a[91] > c[91] * 5.0,
            "energy duration distinguishes sustain from decay"
        );
        assert!(
            a[92] > c[92] + 0.5,
            "the sustained energy feature follows the envelope"
        );
    }
}
