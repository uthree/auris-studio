use std::hint::black_box;
use std::time::{Duration, Instant};

use auris_core::AudioBuffer;
use rustfft::{FftPlanner, num_complex::Complex};

use super::*;

fn reference_measure_spectrum(audio: &AudioBuffer, start: usize, end: usize) -> DrumSpectrum {
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

fn reference_analyze(audio: &AudioBuffer) -> Result<DrumAcoustics, &'static str> {
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
    let spectrum = reference_measure_spectrum(audio, start, frames);
    let attack = reference_measure_spectrum(audio, start, split(0.06));
    let body = reference_measure_spectrum(audio, split(0.06), split(0.2));
    let tail = reference_measure_spectrum(audio, split(0.2), frames);
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
        fitness: std::collections::BTreeMap::new(),
    };
    result.fitness = drum_role_fitness(&result);
    Ok(result)
}

fn fixture(rate: f64, seconds: f64, channels: usize) -> AudioBuffer {
    let frames = (rate * seconds) as usize;
    let data = (0..channels)
        .map(|channel| {
            (0..frames)
                .map(|frame| {
                    let t = frame as f64 / rate;
                    (0.4 * (std::f64::consts::TAU * (80.0 + channel as f64 * 37.0) * t).sin()
                        * (-t / 0.9).exp()) as f32
                })
                .collect()
        })
        .collect();
    AudioBuffer::from_planar(data, rate).expect("fixture")
}

#[test]
fn workspace_matches_original_measurement_exactly() {
    for rate in [8_000.0, 24_000.0, 48_000.0, 192_000.0] {
        for channels in [1, 2, 8] {
            let audio = fixture(rate, 0.5, channels);
            assert_eq!(analyze_drum_audio(&audio), reference_analyze(&audio));
        }
    }
    let mut nonfinite = fixture(48_000.0, 0.5, 2);
    nonfinite.channel_mut(0)[3] = f32::NAN;
    assert_eq!(
        analyze_drum_audio(&nonfinite),
        reference_analyze(&nonfinite)
    );
    assert_eq!(
        analyze_drum_audio(&AudioBuffer::new(2, 0, 48_000.0)),
        reference_analyze(&AudioBuffer::new(2, 0, 48_000.0))
    );
    for audio in [
        AudioBuffer::from_planar(vec![vec![0.5; 7]], 48_000.0).unwrap(),
        AudioBuffer::new(1, 7, 48_000.0),
        AudioBuffer::new(2, 256, 48_000.0),
        AudioBuffer::new(1, 128, f64::NAN),
        AudioBuffer::new(1, 128, 7_999.0),
        AudioBuffer::new(1, 128, 192_001.0),
    ] {
        assert_eq!(analyze_drum_audio(&audio), reference_analyze(&audio));
    }
    let mut state = 0x9e37_79b9_u32;
    let noise = (0..2048)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state as f32 / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect();
    let noise = AudioBuffer::from_planar(vec![noise], 48_000.0).expect("noise fixture");
    assert_eq!(analyze_drum_audio(&noise), reference_analyze(&noise));
}

fn median(values: &mut [Duration]) -> Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

fn benchmark(seconds: f64) {
    let audio = fixture(48_000.0, seconds, 2);
    assert_eq!(analyze_drum_audio(&audio), reference_analyze(&audio));
    let mut reference = Vec::with_capacity(9);
    let mut optimized = Vec::with_capacity(9);
    for trial in 0..9 {
        let reference_first = trial % 2 == 0;
        for is_reference in [reference_first, !reference_first] {
            let start = Instant::now();
            let _ = black_box(if is_reference {
                reference_analyze(&audio)
            } else {
                analyze_drum_audio(&audio)
            });
            if is_reference {
                reference.push(start.elapsed());
            } else {
                optimized.push(start.elapsed());
            }
        }
    }
    eprintln!(
        "drum analysis {seconds}s: reference {:?}, workspace {:?}",
        median(&mut reference),
        median(&mut optimized)
    );
}

#[test]
#[ignore = "release-only performance measurement"]
fn workspace_benchmark_one_second() {
    benchmark(1.0);
}

#[test]
#[ignore = "release-only performance measurement"]
fn workspace_benchmark_ten_seconds() {
    benchmark(10.0);
}
