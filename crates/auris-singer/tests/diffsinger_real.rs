//! Opt-in synthesis through a trained DiffSinger deployment, never a contract fixture.

use std::path::PathBuf;

use auris_singer::{Acceleration, BackendKind, VoiceModel};
use auris_vocal::{SILENCE, SingerFrames};

#[test]
#[ignore = "set AURIS_DIFFSINGER_TEST_CONFIG to a trained voicebank's dsconfig.yaml"]
fn trained_diffsinger_renders_a_vowel_phrase() {
    let path =
        PathBuf::from(std::env::var_os("AURIS_DIFFSINGER_TEST_CONFIG").expect("voicebank path"));
    let acceleration = match std::env::var("AURIS_DIFFSINGER_TEST_ACCELERATION").as_deref() {
        Ok("auto") => Acceleration::Auto,
        Ok("gpu") => Acceleration::Gpu,
        Ok("cpu") | Err(_) => Acceleration::Cpu,
        Ok(other) => panic!("unknown acceleration: {other}"),
    };
    let mut voice = VoiceModel::load(&path, acceleration).expect("load trained voicebank");
    assert_eq!(voice.backend_kind(), BackendKind::DiffSinger);
    let mut frames = SingerFrames {
        hop_seconds: voice.info().hop_seconds(),
        inventory: vec![SILENCE.into(), "a".into()],
        phonemes: vec![0; 25],
        f0_hz: vec![0.0; 25],
        energy: vec![0.0; 25],
    };
    for pitch in [261.625_58, 329.627_56, 391.995_42] {
        frames.phonemes.extend(vec![1; 40]);
        frames.f0_hz.extend(vec![pitch; 40]);
        frames.energy.extend(vec![0.8; 40]);
    }
    frames.phonemes.extend(vec![0; 25]);
    frames.f0_hz.extend(vec![0.0; 25]);
    frames.energy.extend(vec![0.0; 25]);
    let samples = voice
        .sing(&frames, 0, 42)
        .expect("trained acoustic + variance + vocoder inference");
    assert_eq!(
        samples.len(),
        frames.len() * voice.info().hop_length as usize
    );
    assert!(samples.iter().all(|sample| sample.is_finite()));
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    let rms =
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt();
    assert!(peak > 0.01 && rms > 0.001, "peak={peak}, RMS={rms}");
    if let Some(output) = std::env::var_os("AURIS_DIFFSINGER_TEST_WAV") {
        let output = PathBuf::from(output);
        std::fs::write(
            output.with_extension("frames.json"),
            serde_json::to_vec_pretty(&frames).unwrap(),
        )
        .unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: voice.info().sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut wav = hound::WavWriter::create(output, spec).unwrap();
        for sample in samples {
            wav.write_sample(sample).unwrap();
        }
        wav.finalize().unwrap();
    }
    eprintln!(
        "DiffSinger: {} Hz, {} frames, peak={peak:.6}, RMS={rms:.6}",
        voice.info().sample_rate,
        frames.len()
    );
}
