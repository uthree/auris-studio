//! Real ONNX execution against small arithmetic contracts, independent of voice weights.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use auris_singer::{Acceleration, MAX_CHUNK_FRAMES, SingError, VoiceModel};
use auris_vocal::{SILENCE, SingerFrames};
use serde_json::{Value, json};

struct Bank {
    root: PathBuf,
    config: Value,
}

impl Bank {
    fn new(legacy: bool, word_mode: bool) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "auris-diffsinger-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("dsvocoder")).unwrap();
        std::fs::create_dir(root.join("dsvariance")).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diffsinger");
        for name in [
            "modern.onnx",
            "legacy.onnx",
            "speaker.onnx",
            "variance.onnx",
            "linguistic_word.onnx",
            "linguistic_phone.onnx",
        ] {
            std::fs::copy(fixtures.join(name), root.join(name)).unwrap();
        }
        std::fs::copy(
            fixtures.join("vocoder.onnx"),
            root.join("dsvocoder/vocoder.onnx"),
        )
        .unwrap();
        std::fs::write(
            root.join("dsvocoder/vocoder.yaml"),
            "model: vocoder.onnx\nnum_mel_bins: 2\n",
        )
        .unwrap();
        std::fs::write(root.join("phonemes.json"), r#"{"SP":4,"a":5,"k":35}"#).unwrap();
        std::fs::write(root.join("dsvariance/dsconfig.yaml"), format!(
            "phonemes: ../phonemes.json\nlinguistic: ../linguistic_{}.onnx\nvariance: ../variance.onnx\nhidden_size: 2\nuse_continuous_acceleration: true\n",
            if word_mode { "word" } else { "phone" }
        )).unwrap();
        Self {
            root,
            config: json!({
                "phonemes": "phonemes.json", "acoustic": if legacy { "legacy.onnx" } else { "modern.onnx" },
                "vocoder": "dsvocoder", "num_mel_bins": 2, "hidden_size": 2,
                "use_continuous_acceleration": !legacy, "use_variable_depth": true,
                "max_depth": if legacy { 600.0 } else { 0.6 },
                "use_key_shift_embed": !legacy, "use_speed_embed": !legacy,
                "use_energy_embed": !legacy, "use_breathiness_embed": !legacy,
                "use_voicing_embed": !legacy, "use_tension_embed": !legacy,
            }),
        }
    }

    fn entry(&self) -> PathBuf {
        let path = self.root.join("dsconfig.yaml");
        std::fs::write(&path, serde_json::to_vec_pretty(&self.config).unwrap()).unwrap();
        path
    }

    fn load(&self) -> Result<VoiceModel, SingError> {
        VoiceModel::load(&self.entry(), Acceleration::Cpu)
    }
}

impl Drop for Bank {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn phrase() -> SingerFrames {
    SingerFrames {
        hop_seconds: 512.0 / 44100.0,
        inventory: vec![SILENCE.into(), "a".into(), "k".into()],
        phonemes: vec![1, 1, 2, 2, 1, 1],
        f0_hz: vec![220.0; 6],
        energy: vec![1.0; 6],
    }
}

fn expected(word_mode: bool) -> f32 {
    let encoder = 15.0 + if word_mode { 0.09 } else { 0.06 };
    let variances = 57.0 * 0.1 + encoder * 0.04;
    220.0 * 0.0001 + 45.0 * 0.01 + 6.0 * 0.001 + 20.0 * 0.001 + 0.6 * 0.1 + 0.01 + variances * 0.1
}

#[test]
fn trained_export_tensor_contracts_cover_word_and_phone_encoders() {
    for word_mode in [true, false] {
        let bank = Bank::new(false, word_mode);
        let mut voice =
            VoiceModel::load_for_automatic_access(&bank.entry(), Acceleration::Cpu).unwrap();
        assert!(voice.automatic_access_safe());
        let samples = voice.sing(&phrase(), 0, 0).unwrap();
        assert_eq!(samples.len(), 6 * 512);
        assert!(
            samples
                .iter()
                .all(|sample| (sample - expected(word_mode)).abs() < 1e-5)
        );
    }
}

#[test]
fn legacy_rank_one_controls_keep_the_configured_integer_depth() {
    let bank = Bank::new(true, false);
    let mut voice = bank.load().unwrap();
    let samples = voice.sing(&phrase(), 0, 0).unwrap();
    let expected = 0.022 + 0.45 + 0.006 + 0.05 + 0.06;
    assert!(
        samples
            .iter()
            .all(|sample| (sample - expected).abs() < 1e-6)
    );
}

#[test]
fn speaker_vectors_and_per_token_language_ids_reach_the_acoustic_graph() {
    let mut bank = Bank::new(false, true);
    bank.config["acoustic"] = json!("speaker.onnx");
    bank.config["speakers"] = json!(["first", "second"]);
    bank.config["use_lang_id"] = json!(true);
    bank.config["languages"] = json!("languages.json");
    std::fs::write(
        bank.root.join("phonemes.json"),
        r#"{"SP":4,"ja/a":5,"ja/k":35}"#,
    )
    .unwrap();
    std::fs::write(bank.root.join("languages.json"), r#"{"ja":3}"#).unwrap();
    for (name, vector) in [("first", [1.0_f32, 2.0]), ("second", [4.0, 8.0])] {
        std::fs::write(
            bank.root.join(format!("{name}.emb")),
            vector
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    }
    let mut frames = phrase();
    frames.inventory[1] = "ja/a".into();
    frames.inventory[2] = "ja/k".into();
    let mut voice = bank.load().unwrap();
    assert_eq!(voice.info().speaker_to_id["second"], 1);
    for (speaker, vector_sum) in [(0, 3.0), (1, 12.0)] {
        let samples = voice.sing(&frames, speaker, 0).unwrap();
        let expected = expected(true) + 9.0 * 0.01 + vector_sum * 0.001;
        assert!(
            samples
                .iter()
                .all(|sample| (sample - expected).abs() < 1e-5)
        );
    }
    assert!(matches!(
        voice.sing(&frames, 2, 0),
        Err(SingError::NoSuchSpeaker { .. })
    ));
}

#[test]
fn frame_dynamics_scale_the_waveform_smoothly() {
    let bank = Bank::new(false, true);
    let mut voice = bank.load().unwrap();
    let mut frames = phrase();
    frames.energy = vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0];
    let samples = voice.sing(&frames, 0, 0).unwrap();
    for (at, sample) in samples.iter().enumerate() {
        let frame = at / 512;
        let next = (frame + 1).min(5);
        let gain = frames.energy[frame]
            + (frames.energy[next] - frames.energy[frame]) * (at % 512) as f32 / 512.0;
        assert!((sample - expected(true) * gain).abs() < 1e-5);
    }
}

#[test]
fn silence_and_cancellation_skip_inference() {
    let bank = Bank::new(false, true);
    let mut voice = bank.load().unwrap();
    let mut frames = phrase();
    frames.phonemes.fill(0);
    assert_eq!(voice.sing(&frames, 0, 0).unwrap(), vec![0.0; 6 * 512]);
    frames = phrase();
    assert!(matches!(
        voice.sing_with(&frames, 0, 0, |_, _| false),
        Err(SingError::Cancelled)
    ));
    let count = MAX_CHUNK_FRAMES + 10;
    frames.phonemes = vec![1; count];
    frames.f0_hz = vec![220.0; count];
    frames.energy = vec![1.0; count];
    let mut calls = Vec::new();
    let error = voice
        .sing_with(&frames, 0, 0, |done, total| {
            calls.push((done, total));
            done == 0
        })
        .unwrap_err();
    assert!(matches!(error, SingError::Cancelled));
    assert_eq!(calls, [(0, 2), (1, 2)]);
}

#[test]
fn invalid_embeddings_and_auxiliary_path_escapes_fail_before_inference() {
    let mut bank = Bank::new(false, true);
    bank.config["speakers"] = json!(["broken"]);
    std::fs::write(bank.root.join("broken.emb"), [0; 3]).unwrap();
    assert!(matches!(bank.load(), Err(SingError::Metadata(_))));
    bank.config["speakers"] = json!([]);
    std::fs::write(bank.root.join("dsvariance/dsconfig.yaml"), "phonemes: ../../outside.json\nlinguistic: ../linguistic_word.onnx\nvariance: ../variance.onnx\n").unwrap();
    assert!(matches!(
        VoiceModel::load_for_automatic_access(&bank.entry(), Acceleration::Cpu),
        Err(SingError::UnsafeAutomaticAccess { .. })
    ));
}
