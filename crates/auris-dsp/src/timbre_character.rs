//! Fixed-scale descriptors for browsing and language-based timbre discovery.

use auris_core::AudioBuffer;
use serde::{Deserialize, Serialize};

/// Gain-independent reference-note descriptors, all on fixed 0..=1 scales.
/// These are acoustic proxies, not calibrated perceptual ratings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TimbreCharacter {
    /// Log spectral centroid, mapped from 100 Hz (0) to 8 kHz (1).
    pub brightness: f64,
    /// Speed of reaching 90% of the loudest held 10 ms RMS window; 1 is immediate.
    pub attack: f64,
    /// Last fifth of held-note power divided by the loudest held window power.
    pub sustain: f64,
    /// Time after note-off until the last window above -40 dB, divided by capture length.
    pub release: f64,
    /// Mean spectral flatness of the attack, body and tail.
    pub noise: f64,
    /// The captured tail ends above -40 dB; release is a lower bound.
    pub release_censored: bool,
}

impl TimbreCharacter {
    /// Fixed order: brightness, attack speed, sustain, captured release and noise.
    pub fn values(self) -> [f64; 5] {
        [
            self.brightness,
            self.attack,
            self.sustain,
            self.release,
            self.noise,
        ]
    }

    /// Average matched reference-note conditions without catalogue normalization.
    pub fn mean(values: &[Self]) -> Self {
        let mut mean = [0.; 5];
        for value in values {
            for (sum, v) in mean.iter_mut().zip(value.values()) {
                *sum += v / values.len().max(1) as f64;
            }
        }
        Self {
            brightness: mean[0],
            attack: mean[1],
            sustain: mean[2],
            release: mean[3],
            noise: mean[4],
            release_censored: values.iter().any(|v| v.release_censored),
        }
    }
}

/// Measure descriptors from a reference buffer and its unstandardized 93 DSP features.
/// Validation and silence rejection belong to `timbre_features`, called first.
pub fn timbre_character(audio: &AudioBuffer, note_off: f64, features: &[f64]) -> TimbreCharacter {
    let block = (audio.sample_rate() * 0.01).round().max(1.) as usize;
    let mut envelope = vec![0.; audio.frame_count().div_ceil(block)];
    for channel in audio.channels() {
        for (i, samples) in channel.chunks(block).enumerate() {
            envelope[i] += samples.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>()
                / samples.len() as f64
                / audio.channel_count().max(1) as f64;
        }
    }
    let held = ((note_off * audio.sample_rate()) as usize / block).min(envelope.len());
    let peak = envelope[..held]
        .iter()
        .copied()
        .fold(0., f64::max)
        .max(1e-30);
    let attack = envelope[..held]
        .iter()
        .position(|p| *p >= peak * 0.81)
        .unwrap_or(held);
    let sustain = &envelope[held * 4 / 5..held];
    let tail = &envelope[held..];
    let release = tail
        .iter()
        .rposition(|p| *p > peak * 0.0001)
        .map_or(0., |i| (i + 1) as f64 / tail.len().max(1) as f64);
    let centroid = features.get(81).copied().unwrap_or(0.).exp_m1();
    TimbreCharacter {
        brightness: ((centroid.max(100.) / 100.).ln() / 80f64.ln()).clamp(0., 1.),
        attack: (1. - attack as f64 / held.max(1) as f64).clamp(0., 1.),
        sustain: (sustain.iter().sum::<f64>() / sustain.len().max(1) as f64 / peak).clamp(0., 1.),
        release,
        noise: [76, 82, 88]
            .iter()
            .map(|i| features.get(*i).copied().unwrap_or(0.))
            .sum::<f64>()
            / 3.,
        release_censored: tail.last().is_some_and(|p| *p > peak * 0.0001),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timbre::timbre_features;

    fn reference(frequency: f64, gain: f64, slow: bool, decay: bool, release: f64) -> AudioBuffer {
        let rate = 16_000.;
        let mut audio = AudioBuffer::new(2, rate as usize, rate);
        for i in 0..rate as usize {
            let t = i as f64 / rate;
            let attack = if slow { (t / 0.5).min(1.) } else { 1. };
            let held = if decay { (-t / 0.06).exp() } else { 1. };
            let tail = if t < 0.6 {
                1.
            } else if release == 0. {
                0.
            } else {
                (-(t - 0.6) / release).exp()
            };
            let v = (gain * (std::f64::consts::TAU * frequency * t).sin() * attack * held * tail)
                as f32;
            audio.channel_mut(0)[i] = v;
            audio.channel_mut(1)[i] = -v;
        }
        audio
    }

    fn measure(audio: &AudioBuffer) -> TimbreCharacter {
        timbre_character(audio, 0.6, &timbre_features(audio, 0.6).unwrap().unwrap())
    }

    #[test]
    fn character_scales_follow_frequency_attack_sustain_and_captured_release() {
        let short = measure(&reference(220., 0.5, false, false, 0.));
        let bright = measure(&reference(3200., 0.5, false, false, 0.));
        let slow = measure(&reference(220., 0.5, true, false, 0.));
        let pluck = measure(&reference(220., 0.5, false, true, 0.));
        let long = measure(&reference(220., 0.5, false, false, 1.));
        assert!(bright.brightness > short.brightness + 0.4);
        assert!(short.attack > slow.attack + 0.5);
        assert!(short.sustain > pluck.sustain + 0.8);
        assert_eq!(short.release, 0.);
        assert!(!short.release_censored);
        assert_eq!(long.release, 1.);
        assert!(long.release_censored);
    }

    #[test]
    fn character_is_gain_and_stereo_polarity_independent() {
        let a = measure(&reference(440., 0.5, true, false, 0.02));
        let b = measure(&reference(440., 0.02, true, false, 0.02));
        for (a, b) in a.values().into_iter().zip(b.values()) {
            assert!((a - b).abs() < 1e-5, "{a} != {b}");
        }
        assert_eq!(a.release_censored, b.release_censored);
    }
}
