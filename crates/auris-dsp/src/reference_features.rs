//! Fixed, level-independent descriptors for comparing the sound of whole mixes.
//!
//! These are acoustic statistics, not a learned aesthetic score or an assertion that two songs
//! have the same melody. Spectrum, envelope, stereo and onset distributions do not align sample
//! positions or require equal durations. Every candidate uses the same coordinates and scales;
//! neither the reference nor the search candidates fit a projection or normalization model.

use auris_core::AudioBuffer;
use rustfft::{FftPlanner, num_complex::Complex};

const BANDS: usize = 24;
const LOW_HZ: f64 = 40.0;
const HIGH_HZ: f64 = 12_000.0;
const QUANTILES: [f64; 6] = [0.1, 0.25, 0.5, 0.75, 0.9, 0.95];

/// Bounded acoustic differences, with zero meaning identical measured statistics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReferenceDistance {
    /// Hellinger distance squared between normalized broad-band energy distributions.
    pub spectrum: f64,
    /// Relative envelope and crest differences, using a fixed 24 dB comparison scale.
    pub dynamics: f64,
    /// Differences in stereo balance and side-energy statistics.
    pub stereo: f64,
    /// Differences in positive spectral-flux statistics, without beat or timeline alignment.
    pub rhythm: f64,
}

impl ReferenceDistance {
    /// Fixed weighted distance in 0..=1; lower values are closer to the reference.
    pub fn total(self) -> f64 {
        0.45 * self.spectrum + 0.25 * self.dynamics + 0.15 * self.stereo + 0.15 * self.rhythm
    }
}

/// A fixed acoustic summary of finite, audible mono or stereo PCM.
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceFeatures {
    spectrum: [f64; BANDS],
    dynamics: [f64; 7],
    stereo: [f64; 6],
    rhythm: [f64; 6],
}

impl ReferenceFeatures {
    /// Measures a whole mix on the CPU without models, I/O, or changes to its samples.
    ///
    /// Accepts 0.5 seconds to thirty minutes at 8–192 kHz. Rejects non-finite samples and
    /// silence instead of giving them an apparently valid distance. Positive or negative shared
    /// gain leaves the features unchanged above the silence floor. Stereo powers are summed
    /// after the FFT, so opposite-phase channels never cancel out of the timbre measurement.
    ///
    /// A single RMS normalization precedes extraction. Dynamics retain the envelope relative to
    /// that RMS; absolute loudness is deliberately absent. The 50 ms envelope and 10 ms spectral
    /// hop describe articulation and transient activity, not a recovered musical beat grid.
    pub fn analyze(audio: &AudioBuffer) -> Result<Self, &'static str> {
        let rate = audio.sample_rate();
        let frames = audio.frame_count();
        if !rate.is_finite() || !(8_000.0..=192_000.0).contains(&rate) {
            return Err("reference comparison requires a sample rate between 8000 and 192000 Hz");
        }
        if !(1..=2).contains(&audio.channel_count()) {
            return Err("reference comparison requires mono or stereo audio");
        }
        if !(0.5..=1800.0).contains(&(frames as f64 / rate)) {
            return Err("reference comparison requires between 0.5 seconds and thirty minutes");
        }
        let mut energy = 0.0;
        let mut peak = 0.0_f64;
        for sample in audio.channels().iter().flatten() {
            if !sample.is_finite() {
                return Err("reference comparison rejects non-finite audio");
            }
            let value = f64::from(*sample);
            energy += value * value;
            peak = peak.max(value.abs());
        }
        let rms = (energy / (frames * audio.channel_count()) as f64).sqrt();
        if rms < 1e-9 {
            return Err("reference comparison requires audible audio");
        }

        let n = ((rate * 0.04).round() as usize).next_power_of_two();
        let hop = (rate * 0.01).round() as usize;
        let fft = FftPlanner::<f64>::new().plan_fft_forward(n);
        let mut bins = vec![Complex::default(); n];
        let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        let window: Vec<_> = (0..n)
            .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
            .collect();
        let band_for_bin: Vec<_> = (0..=n / 2)
            .map(|i| {
                let hz = i as f64 * rate / n as f64;
                (LOW_HZ..HIGH_HZ).contains(&hz).then(|| {
                    ((hz / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln() * BANDS as f64).floor() as usize
                })
            })
            .collect();
        let mut spectrum = [0.0; BANDS];
        let mut previous = [0.0_f64; BANDS];
        let mut fluxes = Vec::with_capacity(frames.div_ceil(hop));
        for centre in (0..frames).step_by(hop) {
            let mut powers = [0.0_f64; BANDS];
            for channel in audio.channels() {
                for (i, bin) in bins.iter_mut().enumerate() {
                    let frame = centre as isize + i as isize - (n / 2) as isize;
                    let value = usize::try_from(frame)
                        .ok()
                        .and_then(|frame| channel.get(frame))
                        .copied()
                        .unwrap_or(0.0);
                    *bin = Complex::new(f64::from(value) / rms * window[i], 0.0);
                }
                fft.process_with_scratch(&mut bins, &mut scratch);
                for (bin, band) in bins.iter().zip(&band_for_bin) {
                    if let Some(band) = band {
                        powers[*band] += bin.norm_sqr();
                    }
                }
            }
            let mut positive = 0.0;
            let mut amplitude = 0.0;
            for band in 0..BANDS {
                spectrum[band] += powers[band];
                let value = powers[band].sqrt();
                positive += (value - previous[band]).max(0.0);
                amplitude += value + previous[band];
                previous[band] = value;
            }
            fluxes.push(if amplitude > 1e-12 {
                positive / amplitude
            } else {
                0.0
            });
        }
        let total = spectrum.iter().sum::<f64>();
        if total <= 1e-12 || !total.is_finite() {
            return Err("reference comparison requires energy between 40 and 12000 Hz");
        }
        for band in &mut spectrum {
            *band /= total;
        }

        let block = (rate * 0.05).round() as usize;
        let left = audio.channel(0);
        let right = if audio.channel_count() == 2 {
            audio.channel(1)
        } else {
            left
        };
        let mut envelopes = Vec::with_capacity(frames.div_ceil(block));
        let mut balances = Vec::new();
        let mut widths = Vec::new();
        let mut left_total = 0.0;
        let mut right_total = 0.0;
        let mut side_total = 0.0;
        for start in (0..frames).step_by(block) {
            let end = (start + block).min(frames);
            let mut le = 0.0;
            let mut re = 0.0;
            let mut side = 0.0;
            for (l, r) in left[start..end].iter().zip(&right[start..end]) {
                let (l, r) = (f64::from(*l) / rms, f64::from(*r) / rms);
                le += l * l;
                re += r * r;
                side += (l - r).powi(2);
            }
            let power = (le + re) / (2 * (end - start)) as f64;
            envelopes.push((10.0 * power.max(1e-6).log10()).clamp(-60.0, 60.0));
            left_total += le;
            right_total += re;
            side_total += side;
            if power > 1e-8 {
                balances.push(re / (le + re));
                widths.push((side / (2.0 * (le + re))).clamp(0.0, 1.0));
            }
        }
        sort(&mut envelopes);
        sort(&mut balances);
        sort(&mut widths);
        let mut dynamics = [0.0; 7];
        for (value, q) in dynamics.iter_mut().zip(QUANTILES) {
            *value = quantile(&envelopes, q);
        }
        dynamics[6] = 20.0 * (peak / rms).log10();
        let stereo = [
            right_total / (left_total + right_total),
            (side_total / (2.0 * (left_total + right_total))).clamp(0.0, 1.0),
            quantile(&balances, 0.1),
            quantile(&balances, 0.9),
            quantile(&widths, 0.1),
            quantile(&widths, 0.9),
        ];
        let active_onsets =
            fluxes.iter().filter(|flux| **flux > 0.08).count() as f64 / fluxes.len() as f64;
        sort(&mut fluxes);
        let rhythm = [
            quantile(&fluxes, 0.5),
            quantile(&fluxes, 0.75),
            quantile(&fluxes, 0.9),
            quantile(&fluxes, 0.95),
            quantile(&fluxes, 0.99),
            active_onsets,
        ];
        Ok(Self {
            spectrum,
            dynamics,
            stereo,
            rhythm,
        })
    }

    /// Compares fixed feature groups without refitting scales or rewarding shared gain.
    pub fn distance(&self, reference: &Self) -> ReferenceDistance {
        ReferenceDistance {
            spectrum: (0.5
                * self
                    .spectrum
                    .iter()
                    .zip(reference.spectrum)
                    .map(|(a, b)| (a.sqrt() - b.sqrt()).powi(2))
                    .sum::<f64>())
            .clamp(0.0, 1.0),
            dynamics: mean_distance(&self.dynamics, &reference.dynamics, 24.0),
            stereo: mean_distance(&self.stereo, &reference.stereo, 1.0),
            rhythm: mean_distance(&self.rhythm, &reference.rhythm, 1.0),
        }
    }
}

fn sort(values: &mut [f64]) {
    values.sort_by(f64::total_cmp);
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    sorted
        .get(((sorted.len().saturating_sub(1)) as f64 * q).round() as usize)
        .copied()
        .unwrap_or(0.0)
}

fn mean_distance(a: &[f64], b: &[f64], scale: f64) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| ((a - b).abs() / scale).min(1.0))
        .sum::<f64>()
        / a.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, envelope: impl Fn(f64) -> f64, right_gain: f64) -> AudioBuffer {
        let rate = 24_000.0;
        let left: Vec<f32> = (0..48_000)
            .map(|frame| {
                let time = frame as f64 / rate;
                (0.2 * envelope(time) * (std::f64::consts::TAU * hz * time).sin()) as f32
            })
            .collect();
        let right = left
            .iter()
            .map(|sample| (*sample as f64 * right_gain) as f32)
            .collect();
        AudioBuffer::from_planar(vec![left, right], rate).unwrap()
    }

    #[test]
    fn identical_audio_has_zero_distance_and_shared_gain_cannot_improve_it() {
        let audio = tone(440.0, |t| 0.2 + 0.8 * (t * 3.0).sin().abs(), 0.4);
        let features = ReferenceFeatures::analyze(&audio).unwrap();
        assert_eq!(features.distance(&features).total(), 0.0);
        for gain in [0.01, -0.4, 3.0] {
            let mut scaled = audio.clone();
            for sample in scaled.channels_mut().iter_mut().flatten() {
                *sample *= gain;
            }
            let changed = ReferenceFeatures::analyze(&scaled).unwrap();
            let distance = features.distance(&changed);
            assert!(distance.total() < 1e-6, "gain {gain}: {distance:?}");
        }
    }

    #[test]
    fn spectrum_distinguishes_dark_and_bright_audio_at_equal_rms() {
        let dark = ReferenceFeatures::analyze(&tone(180.0, |_| 1.0, 1.0)).unwrap();
        let bright = ReferenceFeatures::analyze(&tone(5000.0, |_| 1.0, 1.0)).unwrap();
        let distance = dark.distance(&bright);
        assert!(distance.spectrum > 0.9, "{distance:?}");
        assert!(distance.total() > 0.4);
    }

    #[test]
    fn stereo_distinguishes_position_and_width_without_erasing_opposite_phase_audio() {
        let mono = ReferenceFeatures::analyze(&tone(440.0, |_| 1.0, 1.0)).unwrap();
        let left = ReferenceFeatures::analyze(&tone(440.0, |_| 1.0, 0.0)).unwrap();
        let wide = ReferenceFeatures::analyze(&tone(440.0, |_| 1.0, -1.0)).unwrap();
        assert!(mono.distance(&left).stereo > 0.4);
        assert!(mono.distance(&wide).stereo > 0.4);
        assert!(mono.distance(&wide).spectrum < 1e-12);
        assert!(mono.distance(&wide).dynamics < 1e-12);
    }

    #[test]
    fn envelope_and_onsets_distinguish_held_and_detached_performances() {
        let held = ReferenceFeatures::analyze(&tone(440.0, |_| 1.0, 1.0)).unwrap();
        let detached = ReferenceFeatures::analyze(&tone(
            440.0,
            |t| {
                if (t * 4.0).fract() < 0.35 { 1.0 } else { 0.0 }
            },
            1.0,
        ))
        .unwrap();
        let distance = held.distance(&detached);
        assert!(distance.dynamics > 0.2, "{distance:?}");
        assert!(distance.rhythm > 0.05, "{distance:?}");
        assert!(distance.total() > 0.05);
    }

    #[test]
    fn invalid_silent_and_too_short_inputs_are_rejected() {
        assert!(ReferenceFeatures::analyze(&AudioBuffer::stereo(48_000, 48_000.0)).is_err());
        assert!(ReferenceFeatures::analyze(&AudioBuffer::stereo(10, 48_000.0)).is_err());
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut audio = tone(440.0, |_| 1.0, 1.0);
            audio.channel_mut(0)[1] = invalid;
            assert!(ReferenceFeatures::analyze(&audio).is_err());
        }
    }
}
