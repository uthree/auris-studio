//! Deterministic end-to-end tests of the LeapSinger acoustic/NHVSing tensor contract.
//!
//! The checked-in ONNX graphs encode their inputs in a waveform instead of requiring a
//! downloaded voice. `fixtures/leapsinger/generate.py` documents and regenerates them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use auris_singer::{Acceleration, BackendKind, MAX_CHUNK_FRAMES, SingError, VoiceModel};
use auris_vocal::{SILENCE, SingerFrames};
use serde_json::{Value, json};

const HOP: usize = 256;

/// Each test owns its manifest so invalid configurations never change shared fixtures.
struct Bank {
    root: PathBuf,
    config: Value,
}

impl Bank {
    fn new(full: bool) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "auris-leapsinger-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/leapsinger");
        for name in [
            "ja.phonemes",
            "diffsinger.onnx",
            "full.onnx",
            "diffsinger_speaker.onnx",
            "invalid_acoustic.onnx",
            "vocoder.onnx",
            "vocoder_v3x.onnx",
            "wrong_length_vocoder.onnx",
        ] {
            std::fs::copy(fixtures.join(name), root.join(name)).unwrap();
        }
        Self {
            root,
            config: json!({
                "format_version": 1,
                "name": "LeapSinger contract fixture",
                "acoustic": if full { "full.onnx" } else { "diffsinger.onnx" },
                "vocoder": "vocoder.onnx",
                "phonemes": "ja.phonemes",
                "sample_rate": 44_100,
                "hop_size": HOP,
                "num_mel_bins": 128,
                "variant": if full { "full" } else { "diffsinger" },
            }),
        }
    }

    fn load(&self) -> Result<VoiceModel, SingError> {
        let path = self.root.join("fixture.leapsinger.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&self.config).unwrap()).unwrap();
        VoiceModel::load(&path, Acceleration::Cpu)
    }
}

impl Drop for Bank {
    fn drop(&mut self) {
        // The voice is declared after its bank, so ONNX sessions close before cleanup.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn phrase() -> SingerFrames {
    SingerFrames {
        hop_seconds: HOP as f64 / 44_100.0,
        inventory: [SILENCE, "a", "k", "ɯ", "i̥"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        phonemes: vec![1, 1, 2, 2, 3, 3, 4, 1],
        f0_hz: vec![220.0, 220.0, 220.0, 220.0, 440.0, 440.0, 440.0, 440.0],
        energy: vec![1.0; 8],
    }
}

fn expected(token: u32, duration: usize, f0: f32, voiced: bool, full: bool, speaker: f32) -> f32 {
    token as f32 * 0.01
        + duration as f32 * 0.02
        + f0 * 0.00002
        + if full && voiced { 0.3 } else { 0.0 }
        + if voiced { 0.0 } else { 0.05 }
        + speaker * 0.004
}

fn assert_frames(samples: &[f32], expected: &[f32], hop: usize) {
    assert_eq!(samples.len(), expected.len() * hop);
    for (frame, (samples, expected)) in samples.chunks_exact(hop).zip(expected).enumerate() {
        for (within_frame, sample) in samples.iter().enumerate() {
            assert!(
                (sample - expected).abs() < 1e-6,
                "frame {frame}, sample {within_frame}: expected {expected}, received {sample}"
            );
        }
    }
}

fn phrase_expectations(full: bool, speaker: f32) -> Vec<f32> {
    let frames = phrase();
    let durations = [2, 2, 2, 2, 2, 2, 1, 1];
    let voiced = [true, true, false, false, true, true, false, true];
    (0..frames.len())
        .map(|at| {
            expected(
                frames.phonemes[at],
                durations[at],
                frames.f0_hz[at],
                voiced[at],
                full,
                speaker,
            )
        })
        .collect()
}

#[test]
fn both_acoustic_layouts_reach_nhvsing_with_the_right_pitch_and_voicing() {
    for full in [false, true] {
        let bank = Bank::new(full);
        let mut voice = bank.load().expect("the synthetic voice loads");
        assert_eq!(voice.backend_kind(), BackendKind::LeapSinger);
        assert!(voice.backend_kind().capabilities().manual_phonemes);
        assert!(voice.backend_kind().capabilities().phoneme_timing);
        assert_eq!(voice.info().sample_rate, 44_100);
        assert_eq!(voice.info().hop_length, HOP as u32);
        assert!(!voice.on_gpu());
        let samples = voice.sing(&phrase(), 0, 7).unwrap();
        assert_frames(&samples, &phrase_expectations(full, 0.0), HOP);
        assert_eq!(samples, voice.sing(&phrase(), 0, 7).unwrap());
    }
}

#[test]
fn manual_phoneme_and_duration_changes_reach_the_acoustic_model() {
    let bank = Bank::new(false);
    let mut voice = bank.load().unwrap();
    let mut frames = phrase();
    frames.phonemes[1] = 2;
    let samples = voice.sing(&frames, 0, 0).unwrap();
    let mut expected_frames = phrase_expectations(false, 0.0);
    expected_frames[0] = expected(1, 1, 220.0, true, false, 0.0);
    expected_frames[1..4].fill(expected(2, 3, 220.0, false, false, 0.0));
    assert_frames(&samples, &expected_frames, HOP);
}

#[test]
fn unvoiced_pitch_gaps_are_interpolated_in_log_frequency() {
    let bank = Bank::new(true);
    let mut voice = bank.load().unwrap();
    let mut frames = phrase();
    frames.phonemes = vec![1; 4];
    frames.f0_hz = vec![220.0, 0.0, 0.0, 440.0];
    frames.energy = vec![1.0; 4];
    let expected_frames: Vec<_> = (0..4)
        .map(|at| {
            expected(
                1,
                4,
                220.0 * 2.0_f32.powf(at as f32 / 3.0),
                at == 0 || at == 3,
                true,
                0.0,
            )
        })
        .collect();
    assert_frames(&voice.sing(&frames, 0, 0).unwrap(), &expected_frames, HOP);
}

#[test]
fn speaker_embeddings_and_expression_change_the_returned_waveform() {
    let mut bank = Bank::new(false);
    bank.config["acoustic"] = json!("diffsinger_speaker.onnx");
    bank.config["speakers"] = json!([
        {"name": "First", "embedding": [1.0, 2.0]},
        {"name": "Second", "embedding": [4.0, 5.0]},
    ]);
    let mut voice = bank.load().unwrap();
    assert_eq!(voice.info().n_speakers, 2);
    assert_eq!(voice.info().speaker_to_id["Second"], 1);
    assert_frames(
        &voice.sing(&phrase(), 0, 0).unwrap(),
        &phrase_expectations(false, 3.0),
        HOP,
    );
    let mut frames = phrase();
    frames.energy = vec![0.5; frames.len()];
    let expected_frames: Vec<_> = phrase_expectations(false, 9.0)
        .into_iter()
        .map(|sample| sample * 0.5)
        .collect();
    assert_frames(&voice.sing(&frames, 1, 0).unwrap(), &expected_frames, HOP);
    assert!(matches!(
        voice.sing(&frames, 2, 0),
        Err(SingError::NoSuchSpeaker {
            speaker: 2,
            count: 2
        })
    ));
}

#[test]
fn nhvsing_v3x_tail_and_short_phrases_keep_the_requested_sample_count() {
    for hop in [256, 512] {
        let mut bank = Bank::new(false);
        bank.config["hop_size"] = json!(hop);
        if hop == 512 {
            bank.config["vocoder"] = json!("vocoder_v3x.onnx");
        }
        let mut voice = bank.load().unwrap();
        for length in [1, 2, 8] {
            let frames = SingerFrames {
                hop_seconds: hop as f64 / 44_100.0,
                inventory: vec![SILENCE.into(), "a".into()],
                phonemes: vec![1; length],
                f0_hz: vec![220.0; length],
                energy: vec![1.0; length],
            };
            // Acoustic padding is silence and must not extend the vowel's duration.
            let expected_frames = vec![expected(1, length, 220.0, true, false, 0.0); length];
            assert_frames(&voice.sing(&frames, 0, 0).unwrap(), &expected_frames, hop);
        }
    }
}

#[test]
fn chunk_progress_can_cancel_before_inference() {
    let bank = Bank::new(false);
    let mut voice = bank.load().unwrap();
    let len = MAX_CHUNK_FRAMES + 1;
    let frames = SingerFrames {
        hop_seconds: HOP as f64 / 44_100.0,
        inventory: vec![SILENCE.into(), "a".into()],
        phonemes: vec![1; len],
        f0_hz: vec![220.0; len],
        energy: vec![1.0; len],
    };
    let mut calls = Vec::new();
    let error = voice
        .sing_with(&frames, 0, 0, |done, total| {
            calls.push((done, total));
            false
        })
        .unwrap_err();
    assert!(matches!(error, SingError::Cancelled));
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, 0);
    assert!(calls[0].1 > 1);
}

#[test]
fn incompatible_frames_are_rejected_before_running_onnx() {
    let bank = Bank::new(false);
    let mut voice = bank.load().unwrap();
    let mut frames = phrase();
    frames.hop_seconds *= 2.0;
    assert!(matches!(
        voice.sing(&frames, 0, 0),
        Err(SingError::HopMismatch { .. })
    ));
    frames = phrase();
    frames.energy.pop();
    assert!(matches!(
        voice.sing(&frames, 0, 0),
        Err(SingError::InvalidFrames { .. })
    ));
    frames = phrase();
    frames.inventory[1] = "not-in-the-dictionary".into();
    assert!(matches!(
        voice.sing(&frames, 0, 0),
        Err(SingError::Inference(_))
    ));
}

#[test]
fn incompatible_models_and_vocoder_lengths_are_reported() {
    let mut bank = Bank::new(false);
    bank.config["acoustic"] = json!("invalid_acoustic.onnx");
    assert!(
        bank.load().is_err(),
        "an unknown required input must fail at load time"
    );

    bank.config["acoustic"] = json!("diffsinger.onnx");
    bank.config["vocoder"] = json!("wrong_length_vocoder.onnx");
    let mut voice = bank.load().unwrap();
    assert!(matches!(
        voice.sing(&phrase(), 0, 0),
        Err(SingError::Inference(_))
    ));
}

#[test]
fn malformed_manifests_and_missing_speaker_embeddings_are_rejected() {
    let mut bank = Bank::new(false);
    bank.config["format_version"] = json!(2);
    assert!(bank.load().is_err());
    bank.config["format_version"] = json!(1);
    bank.config["sample_rate"] = json!(0);
    assert!(bank.load().is_err());
    bank.config["sample_rate"] = json!(44_100);
    bank.config["acoustic"] = json!("diffsinger_speaker.onnx");
    assert!(
        bank.load().is_err(),
        "a model requiring spk_embed needs named embeddings"
    );
    bank.config["speakers"] = json!([{ "name": "Wrong width", "embedding": [1.0] }]);
    assert!(
        bank.load().is_err(),
        "speaker width must match the model input"
    );
}

#[test]
fn malformed_curves_and_inventory_are_rejected() {
    let bank = Bank::new(false);
    let mut voice = bank.load().unwrap();
    for invalid in [-1.0, f32::NAN, f32::INFINITY] {
        let mut frames = phrase();
        frames.f0_hz[0] = invalid;
        assert!(matches!(
            voice.sing(&frames, 0, 0),
            Err(SingError::Inference(_))
        ));
    }
    for invalid in [-0.1, 1.1, f32::NAN, f32::INFINITY] {
        let mut frames = phrase();
        frames.energy[0] = invalid;
        assert!(matches!(
            voice.sing(&frames, 0, 0),
            Err(SingError::Inference(_))
        ));
    }
    let mut frames = phrase();
    frames.hop_seconds = f64::NAN;
    assert!(matches!(
        voice.sing(&frames, 0, 0),
        Err(SingError::HopMismatch { .. })
    ));
    frames = phrase();
    frames.phonemes[0] = frames.inventory.len() as u32;
    assert!(matches!(
        voice.sing(&frames, 0, 0),
        Err(SingError::Inference(_))
    ));
    frames = phrase();
    frames.inventory[0] = "a".into();
    assert!(matches!(
        voice.sing(&frames, 0, 0),
        Err(SingError::Inference(_))
    ));
}

#[test]
fn empty_and_silent_scores_skip_inference() {
    let mut bank = Bank::new(false);
    // This graph would fail the waveform length check if either score reached inference.
    bank.config["vocoder"] = json!("wrong_length_vocoder.onnx");
    let mut voice = bank.load().unwrap();
    for len in [0, MAX_CHUNK_FRAMES + 1] {
        let frames = SingerFrames {
            hop_seconds: HOP as f64 / 44_100.0,
            inventory: vec![SILENCE.into()],
            phonemes: vec![0; len],
            f0_hz: vec![0.0; len],
            energy: vec![0.0; len],
        };
        let mut calls = Vec::new();
        let samples = voice
            .sing_with(&frames, 0, 0, |done, total| {
                calls.push((done, total));
                true
            })
            .unwrap();
        assert_eq!(samples, vec![0.0; len * HOP]);
        assert_eq!(calls, [(0, 0)]);
    }
}

#[test]
fn cancelling_after_a_chunk_stops_before_the_next_inference() {
    let bank = Bank::new(false);
    let mut voice = bank.load().unwrap();
    let len = MAX_CHUNK_FRAMES + 1;
    let frames = SingerFrames {
        hop_seconds: HOP as f64 / 44_100.0,
        inventory: vec![SILENCE.into(), "a".into()],
        phonemes: vec![1; len],
        f0_hz: vec![220.0; len],
        energy: vec![1.0; len],
    };
    let mut calls = Vec::new();
    let error = voice
        .sing_with(&frames, 0, 0, |done, total| {
            calls.push((done, total));
            done == 0
        })
        .unwrap_err();
    assert!(matches!(error, SingError::Cancelled));
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, 0);
    assert_eq!(calls[1].0, 1);
    assert!(calls[1].1 > calls[1].0);
}
