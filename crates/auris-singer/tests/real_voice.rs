//! Tests against a real exported voice, named by `AURIS_SINGER_TEST_MODEL`.
//!
//! The model is a couple of hundred megabytes and lives wherever the developer keeps it, so
//! these tests skip silently on machines (and CI runners) that do not set the variable, and
//! measure the whole pipeline — frames, chunking, inference, stitching — where it is set:
//!
//! ```text
//! AURIS_SINGER_TEST_MODEL=/path/to/voice.onnx cargo test -p auris-singer --test real_voice
//! ```

use std::path::PathBuf;

use auris_singer::VoiceModel;
use auris_vocal::{SILENCE, SingerFrames};

/// The model under test, or `None` on a machine that keeps no model around.
fn model_path() -> Option<PathBuf> {
    std::env::var_os("AURIS_SINGER_TEST_MODEL").map(PathBuf::from)
}

/// A second of か at 440 Hz between two stretches of silence, at the model's usual hop.
fn ka() -> SingerFrames {
    let len = 150;
    let mut phonemes = vec![0u32; len];
    let mut f0_hz = vec![0.0f32; len];
    let mut energy = vec![0.0f32; len];
    for at in 25..125 {
        phonemes[at] = if at < 31 { 1 } else { 2 };
        f0_hz[at] = 440.0;
        energy[at] = 0.75;
    }
    SingerFrames {
        hop_seconds: 0.010,
        inventory: vec![SILENCE.to_string(), "k".to_string(), "a".to_string()],
        phonemes,
        f0_hz,
        energy,
    }
}

#[test]
fn the_real_voice_sings_a_note_reproducibly() {
    let Some(path) = model_path() else {
        eprintln!("AURIS_SINGER_TEST_MODEL not set; skipping the real-voice test");
        return;
    };
    let mut voice = VoiceModel::load(&path).expect("the named model loads");
    assert_eq!(
        voice.info().hop_seconds(),
        0.010,
        "the shipped voices sing at 10 ms"
    );
    assert!(!voice.info().symbols.is_empty());
    let hop = voice.info().hop_length as usize;

    let frames = ka();
    let first = voice.sing(&frames, 7).expect("a second of か renders");
    assert_eq!(first.len(), frames.len() * hop);
    let sung = &first[40 * hop..110 * hop];
    let rms = (sung.iter().map(|s| s * s).sum::<f32>() / sung.len() as f32).sqrt();
    assert!(rms > 0.01, "the note should be audible, rms was {rms}");
    let peak = first.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    assert!(peak < 1.0, "the render must not clip, peak was {peak}");
    let head = &first[..20 * hop];
    let head_peak = head.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    assert!(
        head_peak < 0.01,
        "the lead-in is silence, peak was {head_peak}"
    );

    // Same seed, same take; another seed, another take.
    let again = voice.sing(&frames, 7).expect("the same take renders again");
    assert_eq!(first, again, "a seed names a take");
    let other = voice.sing(&frames, 8).expect("another take renders");
    assert_ne!(first, other, "a different seed is a different take");
}

#[test]
fn a_cancelled_render_stops_between_chunks() {
    let Some(path) = model_path() else {
        eprintln!("AURIS_SINGER_TEST_MODEL not set; skipping the real-voice test");
        return;
    };
    let mut voice = VoiceModel::load(&path).expect("the named model loads");
    let error = voice
        .sing_with(&ka(), 0, |_, _| false)
        .expect_err("refusing the first chunk cancels the render");
    assert!(matches!(error, auris_singer::SingError::Cancelled));
}
