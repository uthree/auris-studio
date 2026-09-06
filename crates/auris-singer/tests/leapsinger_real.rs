//! Optional smoke test of a real LeapSinger acoustic model and NHVSing vocoder.
//!
//! Set `AURIS_LEAPSINGER_TEST_MODEL` to a `.leapsinger.json` manifest to run it.

use std::path::PathBuf;

use auris_singer::{Acceleration, BackendKind, VoiceModel};
use auris_vocal::{SILENCE, SingerFrames};

#[test]
fn a_real_leapsinger_voice_sings_a_held_vowel() {
    let Some(path) = std::env::var_os("AURIS_LEAPSINGER_TEST_MODEL") else {
        eprintln!("AURIS_LEAPSINGER_TEST_MODEL not set; skipping the real LeapSinger voice test");
        return;
    };
    let mut voice = VoiceModel::load(&PathBuf::from(path), Acceleration::Cpu)
        .expect("the configured LeapSinger voice loads on CPU");
    assert_eq!(voice.backend_kind(), BackendKind::LeapSinger);
    let hop = voice.info().hop_seconds();
    let hop_samples = voice.info().hop_length as usize;
    let pad = (0.1 / hop).ceil() as usize;
    let vowel = (0.5 / hop).ceil() as usize;
    let length = pad * 2 + vowel;
    let mut frames = SingerFrames {
        hop_seconds: hop,
        inventory: vec![SILENCE.into(), "a".into()],
        phonemes: vec![0; length],
        f0_hz: vec![0.0; length],
        energy: vec![0.0; length],
    };
    frames.phonemes[pad..pad + vowel].fill(1);
    frames.f0_hz[pad..pad + vowel].fill(261.625_55);
    frames.energy[pad..pad + vowel].fill(0.8);

    let samples = voice.sing(&frames, 0, 7).expect("the held あ renders");
    assert_eq!(samples.len(), length * hop_samples);
    assert!(samples.iter().all(|sample| sample.is_finite()));
    let middle = &samples[(pad + vowel / 4) * hop_samples..(pad + vowel * 3 / 4) * hop_samples];
    let rms =
        (middle.iter().map(|sample| sample * sample).sum::<f32>() / middle.len() as f32).sqrt();
    assert!(rms > 1e-4, "the vowel should be audible; RMS was {rms}");
}
