//! Local CLAP audio/text objectives over the same PCM that the optimizer renders.
//!
//! Model loading, tokenization, resampling and inference run on a worker. No Python process or
//! network access is involved. A prepared evaluator fixes the model and target for an entire
//! search; similarity measures agreement with that target, not musical quality.

use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use auris_analysis::clap::ClapModel;
use auris_core::AudioBuffer;
use auris_dsp::clap_features::{CLAP_SAMPLES, ClapFrontend};

use crate::audio_evaluation::{AudioEvaluation, AudioEvaluator, AudioMetric};

/// The immutable target embedded once when a CLAP search is prepared.
#[derive(Clone)]
pub enum ClapTarget {
    /// A short text prompt; the model works best with English descriptions of sound.
    Text(String),
    /// The selected reference excerpt, with the same segmentation policy as candidate PCM.
    Audio(Arc<AudioBuffer>),
}

/// CPU ONNX inference for cosine similarity to a fixed reference or prompt.
///
/// Audio is downmixed, resampled to 48 kHz and split into consecutive ten-second windows.
/// Each window is embedded and normalized; their duration-weighted mean is normalized again.
/// The final short window uses the model's repeat-and-pad policy. Waveform gain is preserved.
/// Fitness rounds cosine similarity to six decimal places so insignificant inference noise
/// cannot win; the diagnostic retains the raw cosine in -1..=1.
pub struct ClapAudioEvaluator {
    model: ClapModel,
    frontend: ClapFrontend,
    target: Vec<f32>,
    cancel: Arc<AtomicBool>,
    description: String,
}

impl ClapAudioEvaluator {
    /// Verifies and loads an exported bundle and captures the target embedding on a worker.
    ///
    /// Cancellation is observed around model loading and between bounded frontend/inference
    /// steps. A single ONNX call completes before cancellation takes effect.
    pub fn load(
        directory: &Path,
        target: ClapTarget,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        check_cancelled(&cancel)?;
        // Validate PCM before reading large model files.
        if let ClapTarget::Audio(audio) = &target {
            validate_audio(audio)?;
        }
        let model = ClapModel::load(directory).map_err(|error| error.to_string())?;
        check_cancelled(&cancel)?;
        let frontend = ClapFrontend::new(model.preprocessing_coefficients())?;
        let description = format!(
            "CLAP {} @ {}: {}; cosine similarity (-1 to 1), higher is closer",
            model.model_id(),
            model.source_revision(),
            match &target {
                ClapTarget::Text(prompt) => format!("text {prompt:?}"),
                ClapTarget::Audio(_) => "reference audio".to_owned(),
            },
        );
        let target = match target {
            ClapTarget::Text(prompt) => model
                .text_embedding(&prompt)
                .map_err(|error| error.to_string())?,
            ClapTarget::Audio(audio) => embed_audio(&model, &frontend, &audio, &cancel)?,
        };
        check_cancelled(&cancel)?;
        Ok(Self {
            model,
            frontend,
            target,
            cancel,
            description,
        })
    }
}

impl AudioEvaluator for ClapAudioEvaluator {
    fn evaluate(&self, audio: &AudioBuffer) -> Result<AudioEvaluation, String> {
        let embedding = embed_audio(&self.model, &self.frontend, audio, &self.cancel)?;
        let cosine = cosine_similarity(&embedding, &self.target)?;
        let evaluation = AudioEvaluation {
            fitness: (cosine * 1_000_000.0).round() / 1_000_000.0,
            metrics: vec![AudioMetric {
                name: "clap_cosine_similarity".into(),
                value: cosine,
            }],
        };
        evaluation.validate()?;
        Ok(evaluation)
    }

    fn description(&self) -> String {
        self.description.clone()
    }
}

fn check_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("CLAP evaluation cancelled".into())
    } else {
        Ok(())
    }
}

fn validate_audio(audio: &AudioBuffer) -> Result<(), String> {
    if !(1..=2).contains(&audio.channel_count())
        || !audio.sample_rate().is_finite()
        || !(8_000.0..=192_000.0).contains(&audio.sample_rate())
        || audio.frame_count() == 0
        || audio.duration_seconds() > 30.0 + 1.0 / audio.sample_rate()
        || audio
            .iter_channels()
            .flatten()
            .any(|sample| !sample.is_finite())
    {
        return Err("CLAP expects finite mono/stereo audio, 8–192 kHz, up to 30 seconds".into());
    }
    Ok(())
}

fn mono_48k(audio: &AudioBuffer) -> Result<AudioBuffer, String> {
    validate_audio(audio)?;
    let scale = 1.0 / audio.channel_count() as f64;
    let mut mono = vec![0.0; audio.frame_count()];
    for (index, sample) in mono.iter_mut().enumerate() {
        *sample = (audio
            .iter_channels()
            .map(|channel| channel[index] as f64)
            .sum::<f64>()
            * scale) as f32;
    }
    let mono = AudioBuffer::from_planar(vec![mono], audio.sample_rate())
        .map_err(|error| error.to_string())?;
    let resampled =
        auris_io::resample_buffer(&mono, 48_000.0).map_err(|error| error.to_string())?;
    if resampled
        .iter_channels()
        .flatten()
        .any(|sample| !sample.is_finite())
    {
        return Err("CLAP resampling produced non-finite audio".into());
    }
    Ok(resampled)
}

fn embed_audio(
    model: &ClapModel,
    frontend: &ClapFrontend,
    audio: &AudioBuffer,
    cancel: &AtomicBool,
) -> Result<Vec<f32>, String> {
    check_cancelled(cancel)?;
    let mono = mono_48k(audio)?;
    check_cancelled(cancel)?;
    let mut sum = vec![0.0; 512];
    for chunk in mono.channel(0).chunks(CLAP_SAMPLES) {
        let features = frontend.extract(chunk, cancel)?;
        check_cancelled(cancel)?;
        let embedding = model
            .audio_embedding(&features)
            .map_err(|error| error.to_string())?;
        check_cancelled(cancel)?;
        add_weighted_embedding(&mut sum, &embedding, chunk.len() as f64)?;
    }
    normalize(&sum)
}

fn add_weighted_embedding(sum: &mut [f64], embedding: &[f32], weight: f64) -> Result<(), String> {
    if sum.len() != embedding.len() || !weight.is_finite() || weight <= 0.0 {
        return Err("invalid CLAP embedding aggregation".into());
    }
    let unit = normalize(&embedding.iter().map(|&x| x as f64).collect::<Vec<_>>())?;
    for (sum, value) in sum.iter_mut().zip(unit) {
        *sum += value as f64 * weight;
    }
    Ok(())
}

fn normalize(values: &[f64]) -> Result<Vec<f32>, String> {
    let norm = values.iter().map(|value| value * value).sum::<f64>().sqrt();
    if !norm.is_finite() || norm < 1e-12 {
        return Err("CLAP returned a non-finite or zero embedding".into());
    }
    Ok(values.iter().map(|value| (value / norm) as f32).collect())
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Result<f64, String> {
    if left.len() != right.len() {
        return Err("CLAP embedding dimensions differ".into());
    }
    let norm = |values: &[f32]| {
        values
            .iter()
            .map(|&value| (value as f64).powi(2))
            .sum::<f64>()
            .sqrt()
    };
    let denominator = norm(left) * norm(right);
    let dot: f64 = left
        .iter()
        .zip(right)
        .map(|(&a, &b)| a as f64 * b as f64)
        .sum();
    if !denominator.is_finite() || denominator < 1e-12 || !dot.is_finite() {
        return Err("CLAP returned a non-finite or zero embedding".into());
    }
    Ok((dot / denominator).clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_f32(path: &Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len() % 4, 0);
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|x| f32::from_le_bytes(*x))
            .collect()
    }

    #[test]
    #[ignore = "requires the real exported checkpoint in AURIS_CLAP_TEST_MODEL"]
    fn real_frontend_and_audio_text_objectives_match_the_exported_model() {
        let root = std::path::PathBuf::from(
            std::env::var_os("AURIS_CLAP_TEST_MODEL").expect("set AURIS_CLAP_TEST_MODEL"),
        );
        let model = ClapModel::load(&root).unwrap();
        let frontend = ClapFrontend::new(model.preprocessing_coefficients()).unwrap();
        let waveform = read_f32(&root.join("parity/waveform.f32"));
        let expected = read_f32(&root.join("parity/features.f32"));
        let cancel = Arc::new(AtomicBool::new(false));
        let actual = frontend.extract(&waveform, &cancel).unwrap();
        assert_eq!(actual.len(), expected.len());
        let max_error = actual
            .iter()
            .zip(&expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        eprintln!("Rust / official CLAP log-mel max error: {max_error}");
        assert!(max_error < 1e-4, "frontend error {max_error}");
        let audio = AudioBuffer::from_planar(vec![waveform], 48_000.0).unwrap();
        let embedding = embed_audio(&model, &frontend, &audio, &cancel).unwrap();
        let expected_embedding = read_f32(&root.join("parity/audio_embedding.f32"));
        assert!(cosine_similarity(&embedding, &expected_embedding).unwrap() > 0.99999);
        let text: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("parity/text.json")).unwrap()).unwrap();
        let prompt = text["prompt"].as_str().unwrap();
        let prompt_embedding = model.text_embedding(prompt).unwrap();
        let evaluator = ClapAudioEvaluator {
            model,
            frontend,
            target: embedding.clone(),
            cancel,
            description: "real test".into(),
        };
        assert!((evaluator.evaluate(&audio).unwrap().fitness - 1.0).abs() < 1e-6);
        let mut evaluator = evaluator;
        evaluator.target = prompt_embedding;
        let scored = evaluator.evaluate(&audio).unwrap();
        let expected_text: Vec<f32> = text["embedding"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        let expected_cosine = cosine_similarity(&expected_embedding, &expected_text).unwrap();
        assert!((scored.metrics[0].value - expected_cosine).abs() < 1e-4);
        assert_eq!(evaluator.evaluate(&audio).unwrap(), scored);
        evaluator.cancel.store(true, Ordering::Relaxed);
        assert!(
            evaluator
                .evaluate(&audio)
                .unwrap_err()
                .contains("cancelled")
        );
    }

    #[test]
    fn duration_weighted_aggregation_does_not_overweight_the_short_tail() {
        let mut sum = vec![0.0; 2];
        add_weighted_embedding(&mut sum, &[3.0, 0.0], 10.0).unwrap();
        add_weighted_embedding(&mut sum, &[0.0, 7.0], 2.0).unwrap();
        let mean = normalize(&sum).unwrap();
        assert!((mean[1] / mean[0] - 0.2).abs() < 1e-6);
        assert!((cosine_similarity(&mean, &mean).unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]).unwrap(), -1.0);
        assert!(cosine_similarity(&[f32::NAN], &[1.0]).is_err());
        assert!(normalize(&[0.0]).is_err());
    }

    #[test]
    fn downmix_resample_and_input_limits_are_checked_before_inference() {
        let left = vec![0.1; 24_000];
        let right = vec![-0.1; 24_000];
        let stereo = AudioBuffer::from_planar(vec![left, right], 24_000.0).unwrap();
        let mono = mono_48k(&stereo).unwrap();
        assert_eq!(mono.sample_rate(), 48_000.0);
        assert_eq!(mono.frame_count(), 48_000);
        assert_eq!(mono.channel_count(), 1);
        assert!(mono.channel(0).iter().all(|sample| *sample == 0.0));
        let too_long = AudioBuffer::from_planar(vec![vec![0.0; 31 * 8_000]], 8_000.0).unwrap();
        assert!(validate_audio(&too_long).is_err());
        let invalid = AudioBuffer::from_planar(vec![vec![f32::NAN]], 48_000.0).unwrap();
        assert!(validate_audio(&invalid).is_err());
        assert!(
            ClapAudioEvaluator::load(
                Path::new("missing"),
                ClapTarget::Text("test".into()),
                Arc::new(AtomicBool::new(true))
            )
            .err()
            .unwrap()
            .contains("cancelled")
        );
    }
}
