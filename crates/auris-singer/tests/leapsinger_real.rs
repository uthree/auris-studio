//! Opt-in synthesis through trained LeapSinger acoustic weights and an NHVSing vocoder.

use std::path::PathBuf;

use auris_singer::{Acceleration, BackendKind, VoiceModel};
use auris_vocal::{SILENCE, SingerFrames};

/// Estimate the fundamental near the written note, using normalized autocorrelation.
fn pitch(samples: &[f32], rate: u32, expected: f32) -> f64 {
    let minimum = (rate as f64 / (expected as f64 * 1.06)).floor() as usize;
    let maximum = (rate as f64 / (expected as f64 / 1.06)).ceil() as usize;
    let correlations: Vec<f64> = (minimum..=maximum)
        .map(|lag| {
            let (mut cross, mut left, mut right) = (0.0, 0.0, 0.0);
            for (&a, &b) in samples.iter().zip(&samples[lag..]) {
                let (a, b) = (a as f64, b as f64);
                cross += a * b;
                left += a * a;
                right += b * b;
            }
            cross / (left * right).sqrt()
        })
        .collect();
    let (index, &correlation) = correlations
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .unwrap();
    assert!(
        correlation > 0.6,
        "the vowel must be periodic: {correlation}"
    );
    assert!(index > 0 && index + 1 < correlations.len());
    let (a, b, c) = (
        correlations[index - 1],
        correlation,
        correlations[index + 1],
    );
    let offset = 0.5 * (a - c) / (a - 2.0 * b + c);
    rate as f64 / (minimum as f64 + index as f64 + offset)
}

#[test]
#[ignore = "set AURIS_LEAPSINGER_TEST_MODEL to a trained voice's .leapsinger.json"]
fn trained_leapsinger_speakers_render_a_pitched_phrase() {
    let path = PathBuf::from(std::env::var_os("AURIS_LEAPSINGER_TEST_MODEL").expect("voice path"));
    let acceleration = match std::env::var("AURIS_LEAPSINGER_TEST_ACCELERATION").as_deref() {
        Ok("auto") => Acceleration::Auto,
        Ok("gpu") => Acceleration::Gpu,
        Ok("cpu") | Err(_) => Acceleration::Cpu,
        Ok(other) => panic!("unknown acceleration: {other}"),
    };
    let mut voice = VoiceModel::load(&path, acceleration).expect("load trained voice");
    assert_eq!(voice.backend_kind(), BackendKind::LeapSinger);
    let pad = (0.1 / voice.info().hop_seconds()).ceil() as usize;
    let note = (0.4 / voice.info().hop_seconds()).ceil() as usize;
    let hop = voice.info().hop_length as usize;
    let pitches = [261.625_58, 329.627_56, 391.995_42];
    let mut frames = SingerFrames {
        hop_seconds: voice.info().hop_seconds(),
        inventory: vec![SILENCE.into(), "a".into()],
        phonemes: vec![0; pad],
        f0_hz: vec![0.0; pad],
        energy: vec![0.0; pad],
    };
    for f0 in pitches {
        frames.phonemes.extend(vec![1; note]);
        frames.f0_hz.extend(vec![f0; note]);
        frames.energy.extend(vec![0.8; note]);
    }
    frames.phonemes.extend(vec![0; pad]);
    frames.f0_hz.extend(vec![0.0; pad]);
    frames.energy.extend(vec![0.0; pad]);
    let output = std::env::var_os("AURIS_LEAPSINGER_TEST_WAV").map(PathBuf::from);
    if let Some(output) = &output {
        std::fs::write(
            output.with_extension("frames.json"),
            serde_json::to_vec_pretty(&frames).unwrap(),
        )
        .unwrap();
    }
    for speaker in 0..voice.info().n_speakers {
        let samples = voice
            .sing(&frames, speaker, 7)
            .expect("acoustic + vocoder inference");
        assert_eq!(samples.len(), frames.len() * hop);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        let peak = samples
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
            / samples.len() as f32)
            .sqrt();
        assert!(peak > 0.01 && rms > 0.001, "peak={peak}, RMS={rms}");
        let measured: Vec<f64> = pitches
            .into_iter()
            .enumerate()
            .map(|(index, expected)| {
                let start = (pad + index * note + note / 4) * hop;
                let end = (pad + index * note + note * 3 / 4) * hop;
                let measured = pitch(&samples[start..end], voice.info().sample_rate, expected);
                let cents = 1200.0 * (measured / expected as f64).log2();
                assert!(
                    cents.abs() < 30.0,
                    "speaker {speaker}: {measured} Hz, {cents} cents"
                );
                measured
            })
            .collect();
        eprintln!(
            "LeapSinger speaker {speaker}: {} Hz, {} samples, GPU={}, peak={peak:.6}, RMS={rms:.6}, pitches={measured:.2?}",
            voice.info().sample_rate,
            samples.len(),
            voice.on_gpu(),
        );
        if let Some(output) = &output {
            let output = if speaker == 0 {
                output.clone()
            } else {
                output.with_file_name(format!(
                    "{}-speaker-{speaker}.wav",
                    output.file_stem().unwrap().to_string_lossy()
                ))
            };
            let mut wav = hound::WavWriter::create(
                output,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: voice.info().sample_rate,
                    bits_per_sample: 32,
                    sample_format: hound::SampleFormat::Float,
                },
            )
            .unwrap();
            for sample in samples {
                wav.write_sample(sample).unwrap();
            }
            wav.finalize().unwrap();
        }
    }
}
