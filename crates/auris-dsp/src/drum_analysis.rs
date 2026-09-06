//! Offline, label-free measurements of a triggered percussion sound.
//!
//! Only PCM and its sample rate enter this module. A note address, author, instrument name,
//! or sample label cannot influence a measurement. Fitness is an explicit acoustic criterion,
//! not a learned class probability. Computation allocates and must stay off the audio thread.

use std::collections::BTreeMap;

use auris_core::AudioBuffer;
use auris_core::project::DrumRole;
use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};

/// Coarse description of the measured spectrum, independent of musical use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcousticCharacter {
    /// No measurable signal.
    Silent,
    /// Most energy occupies a narrow frequency range.
    Tonal,
    /// Broad, noise-like spectrum.
    Noisy,
    /// Both narrow and broad spectral components.
    Mixed,
}

/// Energy-normalized spectral measurements over an elapsed-time region.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DrumSpectrum {
    /// Fraction of spectral energy below 250 Hz.
    pub low: f64,
    /// Fraction between 250 Hz and 2 kHz.
    pub body: f64,
    /// Fraction above 4 kHz.
    pub high: f64,
    /// Power-weighted mean frequency in Hz.
    pub centroid_hz: f64,
    /// Geometric/arithmetic mean of power between 40 Hz and Nyquist; 0 tonal, 1 flat.
    pub flatness: f64,
    /// Energy in the strongest spectral bin and its two neighbors, divided by total energy.
    pub concentration: f64,
    /// Frequency of the strongest bin between 35 and 800 Hz, if any.
    pub low_peak_hz: f64,
}

/// Measurements and acoustic fitness for one triggered audio buffer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DrumAcoustics {
    /// Coarse acoustic description, never a musical assignment.
    pub character: AcousticCharacter,
    /// Peak absolute sample value across every channel.
    pub peak: f64,
    /// Root mean square across every channel.
    pub rms: f64,
    /// Delay before the first frame reaching one percent of the peak.
    pub onset_seconds: f64,
    /// Elapsed time from onset until 90 percent of measured energy has sounded.
    pub energy_duration_seconds: f64,
    /// Last 20 percent mean energy divided by the loudest 10 ms mean energy.
    pub sustained_energy: f64,
    /// Full-sound spectral statistics.
    pub spectrum: DrumSpectrum,
    /// First 60 ms after the measured onset.
    pub attack: DrumSpectrum,
    /// 60 to 200 ms after the measured onset.
    pub body: DrumSpectrum,
    /// Spectrum after 200 ms.
    pub tail: DrumSpectrum,
    /// Low-frequency spectral-peak fall from attack to body, in semitones.
    /// Meaningful for a tonal low sound; broadband noise can have arbitrary local peaks.
    pub pitch_fall_semitones: f64,
    /// Independent acoustic fitness in 0..=1. These values are not probabilities.
    pub fitness: BTreeMap<DrumRole, f64>,
}

/// Measures stereo channel powers separately, so opposite-phase channels never cancel.
///
/// Non-finite PCM, unsupported rates and excessively long inputs are rejected, not sanitized
/// into apparently valid evidence. Inputs are limited to ten seconds and eight channels.
pub fn analyze_drum_audio(audio: &AudioBuffer) -> Result<DrumAcoustics, &'static str> {
    let rate = audio.sample_rate();
    if !rate.is_finite() || !(8_000.0..=192_000.0).contains(&rate) {
        return Err("drum analysis requires a sample rate between 8000 and 192000 Hz");
    }
    if audio.channel_count() > 8 || audio.frame_count() > (rate * 10.0) as usize {
        return Err("drum analysis accepts at most eight channels and ten seconds");
    }
    if audio.channels().iter().flatten().any(|v| !v.is_finite()) {
        return Err("the instrument produced non-finite audio");
    }
    let frames = audio.frame_count();
    let mut envelope = vec![0.0; frames];
    let mut peak = 0.0f64;
    for channel in audio.channels() {
        for (frame, &sample) in channel.iter().enumerate() {
            peak = peak.max(f64::from(sample).abs());
            envelope[frame] += f64::from(sample).powi(2) / audio.channel_count() as f64;
        }
    }
    let energy: f64 = envelope.iter().sum();
    let rms = (energy / frames.max(1) as f64).sqrt();
    if peak < 1e-7 || energy == 0.0 {
        return Ok(DrumAcoustics {
            character: AcousticCharacter::Silent,
            peak,
            rms,
            onset_seconds: 0.0,
            energy_duration_seconds: 0.0,
            sustained_energy: 0.0,
            spectrum: DrumSpectrum::default(),
            attack: DrumSpectrum::default(),
            body: DrumSpectrum::default(),
            tail: DrumSpectrum::default(),
            pitch_fall_semitones: 0.0,
            fitness: DrumRole::ALL.into_iter().map(|role| (role, 0.0)).collect(),
        });
    }
    let start = envelope
        .iter()
        .position(|e| *e >= peak.powi(2) * 0.0001)
        .unwrap_or(0);
    let mut cumulative = 0.0;
    let end90 = envelope
        .iter()
        .position(|e| {
            cumulative += e;
            cumulative >= energy * 0.9
        })
        .unwrap_or(start);
    let duration = end90.saturating_sub(start) as f64 / rate;
    let block = (rate * 0.01) as usize;
    let max_energy = envelope
        .chunks(block)
        .map(|c| c.iter().sum::<f64>() / c.len() as f64)
        .fold(0.0f64, f64::max);
    let end_region = &envelope[frames * 4 / 5..];
    let sustained =
        (end_region.iter().sum::<f64>() / end_region.len().max(1) as f64 / max_energy.max(1e-30))
            .clamp(0.0, 1.0);
    let split = |seconds: f64| (start + (rate * seconds) as usize).min(frames);
    let spectrum = measure_spectrum(audio, start, frames);
    let attack = measure_spectrum(audio, start, split(0.06));
    let body = measure_spectrum(audio, split(0.06), split(0.2));
    let tail = measure_spectrum(audio, split(0.2), frames);
    let character = if peak < 1e-7 || energy == 0.0 {
        AcousticCharacter::Silent
    } else if spectrum.concentration > 0.55 {
        AcousticCharacter::Tonal
    } else if spectrum.concentration < 0.08 {
        AcousticCharacter::Noisy
    } else {
        AcousticCharacter::Mixed
    };
    let pitch_fall = if attack.low_peak_hz > 0.0 && body.low_peak_hz > 0.0 {
        (12.0 * (attack.low_peak_hz / body.low_peak_hz).log2()).clamp(-96.0, 96.0)
    } else {
        0.0
    };
    let mut result = DrumAcoustics {
        character,
        peak,
        rms,
        onset_seconds: start as f64 / rate,
        energy_duration_seconds: duration,
        sustained_energy: sustained,
        spectrum,
        attack,
        body,
        tail,
        pitch_fall_semitones: pitch_fall,
        fitness: BTreeMap::new(),
    };
    result.fitness = drum_role_fitness(&result);
    Ok(result)
}

fn measure_spectrum(audio: &AudioBuffer, start: usize, end: usize) -> DrumSpectrum {
    if end <= start {
        return DrumSpectrum::default();
    }
    let size = 2048;
    let rate = audio.sample_rate();
    let fft = FftPlanner::new().plan_fft_forward(size);
    let mut data = vec![Complex::new(0.0f64, 0.0); size];
    let mut powers = vec![0.0; size / 2 + 1];
    for channel in audio.channels() {
        for offset in (start..end).step_by(size / 2) {
            data.fill(Complex::new(0.0, 0.0));
            for (i, value) in data.iter_mut().enumerate().take((end - offset).min(size)) {
                let window = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / size as f64).cos();
                value.re = f64::from(channel[offset + i]) * window;
            }
            fft.process(&mut data);
            for (power, value) in powers.iter_mut().zip(&data) {
                *power += value.norm_sqr();
            }
        }
    }
    // DC and sub-audible drift do not identify a drum.
    for (i, power) in powers.iter_mut().enumerate() {
        if i as f64 * rate / (size as f64) < 20.0 {
            *power = 0.0;
        }
    }
    let total: f64 = powers.iter().sum();
    if total <= 1e-30 {
        return DrumSpectrum::default();
    }
    let mut result = DrumSpectrum::default();
    let mut logs = 0.0;
    let mut arithmetic = 0.0;
    let mut count = 0;
    let mut strongest = 0usize;
    let mut strongest_low = 0.0;
    for (i, &power) in powers.iter().enumerate() {
        let hz = i as f64 * rate / size as f64;
        let fraction = power / total;
        result.centroid_hz += hz * fraction;
        if hz < 250.0 {
            result.low += fraction;
        }
        if (250.0..2000.0).contains(&hz) {
            result.body += fraction;
        }
        if hz >= 4000.0 {
            result.high += fraction;
        }
        if (35.0..=800.0).contains(&hz) && power > strongest_low {
            strongest_low = power;
            result.low_peak_hz = hz;
        }
        if power > powers[strongest] {
            strongest = i;
        }
        if hz >= 40.0 {
            logs += fraction.max(1e-30).ln();
            arithmetic += fraction;
            count += 1;
        }
    }
    result.flatness =
        ((logs / count as f64).exp() / (arithmetic / count as f64).max(1e-30)).clamp(0.0, 1.0);
    result.concentration = powers[strongest.saturating_sub(1)..(strongest + 2).min(powers.len())]
        .iter()
        .sum::<f64>()
        / total;
    result
}

/// Computes the documented acoustic criteria from measured features, without a note address.
///
/// Also used to verify that an imported report's scores still agree with its measurements.
/// The temporal memberships use the time containing 90 percent of energy, not sample length or
/// the last audible tail: for an exponential amplitude decay, that energy duration is one
/// quarter of the time to -40 dB. Overlapping memberships deliberately allow ambiguous uses.
pub fn drum_role_fitness(audio: &DrumAcoustics) -> BTreeMap<DrumRole, f64> {
    let mut result: BTreeMap<_, _> = DrumRole::ALL.into_iter().map(|r| (r, 0.0)).collect();
    if audio.character == AcousticCharacter::Silent {
        return result;
    }
    let spectrum = &audio.spectrum;
    let transient = (1.0 - audio.sustained_energy / 0.3).clamp(0.0, 1.0);
    let noisy = (1.0 - spectrum.concentration / 0.3).clamp(0.0, 1.0);
    let tonal = (spectrum.concentration / 0.45).clamp(0.0, 1.0);
    let duration = audio.energy_duration_seconds;
    let short = 1.0 - rise(duration, 0.04, 0.10);
    let open = rise(duration, 0.035, 0.09) * (1.0 - rise(duration, 0.18, 0.40));
    let long = rise(duration, 0.12, 0.30);
    let pitched_low = (spectrum.low + spectrum.body * 0.65).clamp(0.0, 1.0);
    // Low-band energy alone cannot separate a bass drum from a resonant floor tom. A deep
    // centroid or a substantial pitch fall into the bass provides foundation evidence; a
    // higher, more stable pitched resonance provides fill-voice evidence. These are musical
    // fitness criteria, so a deep, stable tom may still serve as a kick.
    let deep = 1.0 - rise(spectrum.centroid_hz, 85.0, 140.0);
    let falling = rise(audio.pitch_fall_semitones, 4.0, 9.0)
        * (1.0 - rise(audio.body.low_peak_hz, 100.0, 200.0));
    let stable_pitch = 1.0 - rise(audio.pitch_fall_semitones.abs(), 4.0, 10.0);
    let resonant_register = rise(spectrum.centroid_hz, 80.0, 140.0);
    // The 2–4 kHz noise body matters as much as 250 Hz–2 kHz: inspecting only `body` would
    // reject a backbeat merely because its wire spectrum is centered around 3 kHz.
    let midrange = (1.0 - spectrum.low - spectrum.high).clamp(0.0, 1.0);
    result.insert(
        DrumRole::Kick,
        spectrum.low * (0.55 + 0.45 * tonal) * deep.max(falling) * transient,
    );
    result.insert(
        DrumRole::Snare,
        (midrange / 0.45).min(1.0) * noisy * transient,
    );
    result.insert(
        DrumRole::ClosedHat,
        spectrum.high * noisy * short * transient,
    );
    result.insert(DrumRole::OpenHat, spectrum.high * noisy * open * transient);
    result.insert(DrumRole::Crash, spectrum.high * noisy * long * transient);
    result.insert(
        DrumRole::Tom,
        pitched_low * tonal * transient * stable_pitch * (0.25 + 0.75 * resonant_register),
    );
    result
}

fn rise(value: f64, low: f64, high: f64) -> f64 {
    ((value - low) / (high - low)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: f64, gain: f32) -> Vec<f32> {
        (0..rate as usize)
            .map(|i| {
                let t = i as f64 / rate;
                (gain as f64 * (std::f64::consts::TAU * 80.0 * t).sin() * (-t / 0.09).exp()) as f32
            })
            .collect()
    }

    #[test]
    fn gain_and_stereo_polarity_do_not_change_role_fitness() {
        let base = tone(48_000.0, 0.7);
        let mono = AudioBuffer::from_planar(vec![base.clone()], 48_000.0).unwrap();
        let stereo = AudioBuffer::from_planar(
            vec![base.clone(), base.iter().map(|x| -*x).collect()],
            48_000.0,
        )
        .unwrap();
        let quiet = AudioBuffer::from_planar(vec![tone(48_000.0, 0.07)], 48_000.0).unwrap();
        let a = analyze_drum_audio(&mono).unwrap();
        let b = analyze_drum_audio(&stereo).unwrap();
        let c = analyze_drum_audio(&quiet).unwrap();
        assert!(a.spectrum.low > 0.99);
        assert!(a.fitness[&DrumRole::Kick] > 0.95);
        for role in DrumRole::ALL {
            assert!((a.fitness[&role] - b.fitness[&role]).abs() < 1e-8);
            assert!((a.fitness[&role] - c.fitness[&role]).abs() < 1e-6);
        }
    }

    #[test]
    fn silence_has_no_candidates_and_nonfinite_audio_is_an_error() {
        let mut audio = AudioBuffer::stereo(1024, 48_000.0);
        let result = analyze_drum_audio(&audio).unwrap();
        assert_eq!(result.character, AcousticCharacter::Silent);
        assert!(result.fitness.values().all(|s| *s == 0.0));
        audio.channel_mut(0)[8] = f32::NAN;
        assert!(analyze_drum_audio(&audio).is_err());
    }

    #[test]
    fn natural_decay_is_measured_in_seconds_and_sustain_is_rejected() {
        for rate in [24_000.0, 48_000.0, 96_000.0] {
            let decaying =
                analyze_drum_audio(&AudioBuffer::from_planar(vec![tone(rate, 0.5)], rate).unwrap())
                    .unwrap();
            assert!((decaying.energy_duration_seconds - 0.09 * 10.0f64.ln() / 2.0).abs() < 0.003);
            assert!(decaying.fitness[&DrumRole::Kick] > 0.95);
            let held = (0..rate as usize)
                .map(|i| (std::f64::consts::TAU * 80.0 * i as f64 / rate).sin() as f32)
                .collect();
            let sustained =
                analyze_drum_audio(&AudioBuffer::from_planar(vec![held], rate).unwrap()).unwrap();
            assert!(sustained.fitness.values().all(|score| *score == 0.0));
        }
    }

    #[test]
    fn lowpass_noise_loses_hat_fitness() {
        let mut seed = 17u64;
        let mut low = 0.0;
        let mut bright = Vec::new();
        let mut dark = Vec::new();
        for i in 0..48_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let noise = (seed as i64 as f64 / i64::MAX as f64) as f32;
            low += 0.015 * (noise - low);
            let envelope = (-(i as f32) / 2400.0).exp();
            bright.push(noise * envelope);
            dark.push(low * envelope);
        }
        let bright =
            analyze_drum_audio(&AudioBuffer::from_planar(vec![bright], 48_000.0).unwrap()).unwrap();
        let dark =
            analyze_drum_audio(&AudioBuffer::from_planar(vec![dark], 48_000.0).unwrap()).unwrap();
        assert!(bright.spectrum.high > 0.7);
        assert!(dark.spectrum.high < 0.05);
        assert!(bright.fitness[&DrumRole::ClosedHat] > dark.fitness[&DrumRole::ClosedHat] + 0.5);
    }

    fn noise_with_envelope(decay: f64, midrange: bool) -> AudioBuffer {
        use crate::{Biquad, BiquadCoefficients};
        let rate = 48_000.0;
        let coefficients = if midrange {
            BiquadCoefficients::bandpass(rate, 2800.0, 0.9)
        } else {
            BiquadCoefficients::highpass(rate, 6000.0, 0.707)
        };
        let mut first = Biquad::new(coefficients);
        let mut second = Biquad::new(coefficients);
        let mut seed = 37u64;
        let samples = (0..72_000)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let noise = (seed as i64 as f64 / i64::MAX as f64) as f32;
                let filtered = second.process_sample(first.process_sample(noise));
                filtered * (-(i as f64) / rate / decay).exp() as f32
            })
            .collect();
        AudioBuffer::from_planar(vec![samples], rate).unwrap()
    }

    #[test]
    fn a_noise_body_above_two_kilohertz_still_has_backbeat_fitness() {
        let measured = analyze_drum_audio(&noise_with_envelope(0.055, true)).unwrap();
        let midrange = 1.0 - measured.spectrum.low - measured.spectrum.high;
        assert!(midrange > 0.7);
        assert!(measured.spectrum.centroid_hz > 2000.0);
        assert!(measured.fitness[&DrumRole::Snare] > 0.65);
        assert!(measured.fitness[&DrumRole::Snare] > measured.fitness[&DrumRole::ClosedHat]);
    }

    #[test]
    fn longer_bright_noise_moves_from_short_timekeeper_to_open_timekeeper_to_accent() {
        let short = analyze_drum_audio(&noise_with_envelope(0.015, false)).unwrap();
        let open = analyze_drum_audio(&noise_with_envelope(0.10, false)).unwrap();
        let long = analyze_drum_audio(&noise_with_envelope(0.40, false)).unwrap();
        assert!(short.energy_duration_seconds < open.energy_duration_seconds);
        assert!(open.energy_duration_seconds < long.energy_duration_seconds);
        assert!(short.fitness[&DrumRole::ClosedHat] > 0.7);
        assert!(short.fitness[&DrumRole::OpenHat] < 0.2);
        assert!(open.fitness[&DrumRole::OpenHat] > 0.7);
        assert!(open.fitness[&DrumRole::ClosedHat] < 0.1);
        assert!(long.fitness[&DrumRole::Crash] > 0.7);
        assert!(long.fitness[&DrumRole::Crash] > long.fitness[&DrumRole::OpenHat]);
    }

    #[test]
    fn deep_or_falling_tones_and_higher_stable_resonances_have_different_fitness() {
        let rate = 48_000.0;
        let render = |base: f64, drop: f64| {
            let mut phase = 0.0;
            let samples = (0..48_000)
                .map(|i| {
                    let t = i as f64 / rate;
                    phase += std::f64::consts::TAU * (base + drop * (-t / 0.02).exp()) / rate;
                    (phase.sin() * (-t / 0.10).exp()) as f32
                })
                .collect();
            analyze_drum_audio(&AudioBuffer::from_planar(vec![samples], rate).unwrap()).unwrap()
        };
        let deep = render(70.0, 0.0);
        let falling = render(60.0, 220.0);
        let higher = render(180.0, 0.0);
        assert!(deep.fitness[&DrumRole::Kick] > 0.95);
        assert!(higher.fitness[&DrumRole::Tom] > 0.9);
        assert!(higher.fitness[&DrumRole::Kick] < 0.1);
        assert!(falling.pitch_fall_semitones > 4.0);
        assert!(falling.fitness[&DrumRole::Kick] > falling.fitness[&DrumRole::Tom] + 0.4);
    }
}
