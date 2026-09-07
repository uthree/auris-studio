//! Optional Google YAMNet inference on CPU, never on the audio callback.
//!
//! The export includes Google's preprocessing; Rust only averages channels, pads and
//! advances overlapping waveform windows. Labels follow the pinned AudioSet vocabulary.
//! This is multi-label event tagging, not source separation or note-to-instrument assignment.

use crate::{AnalysisControl, AnalysisError};
use auris_core::AudioBuffer;
use ort::{execution_providers::CPUExecutionProvider, session::Session, value::Tensor};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path, sync::LazyLock};

/// Required PCM sample rate; the session owns resampling.
pub const INSTRUMENT_RATE: f64 = 16_000.0;
const WINDOW: usize = 15_600;
const HOP: usize = 7_680;
const CLASSES: usize = 521;
const MAX_MODEL: u64 = 32 * 1024 * 1024;
const WEIGHTS: &str = "13c3308955bbfaef262f175ac9c40e47b134573a93984f009220dd7cc12a1744";
// Google AudioSet class names, transformed from the pinned CSV; see NOTICE-YAMNET.md.
static LABELS: LazyLock<Vec<String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("yamnet-labels.json")).expect("embedded YAMNet vocabulary")
});

/// An independently scored AudioSet class, not an exclusive or calibrated probability.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InstrumentCandidate {
    /// Stable index in the pinned 521-class YAMNet vocabulary.
    pub class_index: usize,
    /// Original model label in English.
    pub label: String,
    /// Sigmoid score, or arithmetic mean of sigmoid scores for a clip summary.
    pub score: f32,
}

/// One overlapping 975-ms classification window, padded only at the source end.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InstrumentWindow {
    /// Window start in source seconds.
    pub start: f64,
    /// Observed end in source seconds, excluding synthetic padding.
    pub end: f64,
    /// Up to ten instrument/voice labels above the requested score threshold.
    /// Empty means unknown, not proof that no instrument is present.
    pub candidates: Vec<InstrumentCandidate>,
    /// Five strongest labels across all event classes, including silence and non-music.
    pub raw_top: Vec<InstrumentCandidate>,
}

/// Read-only local instrument hypotheses; no notes or project edits are produced.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InstrumentAnalysis {
    /// Algorithm and preprocessing contract.
    pub algorithm: &'static str,
    /// Exact local ONNX file digest for reproducibility.
    pub model_sha256: String,
    /// Original Google weights digest carried by the export.
    pub weights_sha256: String,
    /// Analyzed duration before padding, in seconds.
    pub seconds: f64,
    /// Minimum score for displayed instrument hypotheses; not a confidence guarantee.
    pub threshold: f32,
    /// Instrument labels ranked by mean window score, with the same threshold.
    pub candidates: Vec<InstrumentCandidate>,
    /// Five highest mean event scores, including non-instrument labels.
    pub raw_top: Vec<InstrumentCandidate>,
    /// Overlapping per-window hypotheses, including transient instruments.
    pub windows: Vec<InstrumentWindow>,
}

fn model_error(error: impl ToString) -> AnalysisError {
    AnalysisError::Model(error.to_string())
}

fn is_instrument(index: usize) -> bool {
    // Singing includes choir, humming and rapping; speech and music genres are excluded.
    matches!(index, 24..=32 | 134..=178 | 180..=197 | 203..=209)
}

fn ranked(scores: &[f32], instruments: bool, threshold: f32) -> Vec<InstrumentCandidate> {
    let mut indices: Vec<_> = (0..CLASSES)
        .filter(|&i| (!instruments || is_instrument(i)) && scores[i] >= threshold)
        .collect();
    indices.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
    indices.truncate(if instruments { 10 } else { 5 });
    indices
        .into_iter()
        .map(|i| InstrumentCandidate {
            class_index: i,
            label: LABELS[i].clone(),
            score: scores[i],
        })
        .collect()
}

fn window_count(samples: usize) -> usize {
    1 + samples.saturating_sub(WINDOW).div_ceil(HOP)
}

/// Tags prepared PCM with an explicitly supplied local ONNX export on the CPU.
///
/// At most thirty minutes and eight channels; channel averaging can cancel out-of-phase
/// stereo. Cancellation is checked before loading and between bounded 975-ms inference
/// calls. No runtime download, GPU provider, subprocess or network request is used.
pub fn analyze_instruments(
    audio: &AudioBuffer,
    model: &Path,
    threshold: f32,
    control: &AnalysisControl,
) -> Result<InstrumentAnalysis, AnalysisError> {
    control.check(0.0)?;
    if !threshold.is_finite()
        || !(0.0..=1.0).contains(&threshold)
        || audio.sample_rate() != INSTRUMENT_RATE
        || audio.frame_count() == 0
        || audio.frame_count() > 28_800_000
        || !(1..=8).contains(&audio.channel_count())
        || audio.iter_channels().flatten().any(|s| !s.is_finite())
    {
        return Err(AnalysisError::Invalid(
            "expected finite 16 kHz PCM (up to 30 minutes / 8 channels) and threshold 0..1",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(model)
        .map_err(model_error)?
        .take(MAX_MODEL + 1)
        .read_to_end(&mut bytes)
        .map_err(model_error)?;
    if bytes.len() as u64 > MAX_MODEL {
        return Err(AnalysisError::Invalid("YAMNet model exceeds 32 MiB"));
    }
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let session = Session::builder()
        .map_err(model_error)?
        .with_intra_threads(2)
        .map_err(model_error)?
        .with_execution_providers([CPUExecutionProvider::default().build().error_on_failure()])
        .map_err(model_error)?
        .commit_from_memory(&bytes)
        .map_err(model_error)?;
    let metadata = session.metadata().map_err(model_error)?;
    if metadata
        .custom("auris.yamnet")
        .map_err(model_error)?
        .as_deref()
        != Some("waveform-15600-v1")
        || metadata
            .custom("weights_sha256")
            .map_err(model_error)?
            .as_deref()
            != Some(WEIGHTS)
        || session.inputs.len() != 1
        || session.inputs[0].name != "waveform"
        || session.outputs.len() != 1
        || session.outputs[0].name != "scores"
    {
        return Err(AnalysisError::Invalid(
            "use the YAMNet export from tools/music-models/export_yamnet.py",
        ));
    }
    let count = window_count(audio.frame_count());
    let mut windows = Vec::with_capacity(count);
    let mut mean = vec![0.0_f64; CLASSES];
    for index in 0..count {
        control.check(index as f32 / count as f32)?;
        let from = index * HOP;
        let to = (from + WINDOW).min(audio.frame_count());
        let mut waveform = vec![0.0_f32; WINDOW];
        for channel in audio.iter_channels() {
            for (target, sample) in waveform.iter_mut().zip(&channel[from..to]) {
                *target += sample / audio.channel_count() as f32;
            }
        }
        let tensor = Tensor::from_array(([WINDOW], waveform)).map_err(model_error)?;
        let inputs = ort::inputs!["waveform" => tensor].map_err(model_error)?;
        let outputs = session.run(inputs).map_err(model_error)?;
        let (shape, scores) = outputs["scores"]
            .try_extract_raw_tensor::<f32>()
            .map_err(model_error)?;
        if shape != [1, CLASSES as i64]
            || scores.len() != CLASSES
            || scores
                .iter()
                .any(|s| !s.is_finite() || !(0.0..=1.0).contains(s))
        {
            return Err(AnalysisError::Invalid(
                "YAMNet returned invalid class scores",
            ));
        }
        for (sum, score) in mean.iter_mut().zip(scores) {
            *sum += f64::from(*score);
        }
        windows.push(InstrumentWindow {
            start: from as f64 / INSTRUMENT_RATE,
            end: to as f64 / INSTRUMENT_RATE,
            candidates: ranked(scores, true, threshold),
            raw_top: ranked(scores, false, 0.0),
        });
    }
    control.check(1.0)?;
    let mean: Vec<_> = mean.iter().map(|s| (s / count as f64) as f32).collect();
    Ok(InstrumentAnalysis {
        algorithm: "yamnet-onnx-cpu-v1",
        model_sha256: hash,
        weights_sha256: WEIGHTS.into(),
        seconds: audio.frame_count() as f64 / INSTRUMENT_RATE,
        threshold,
        candidates: ranked(&mean, true, threshold),
        raw_top: ranked(&mean, false, 0.0),
        windows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocabulary_and_multilabel_unknown_are_preserved() {
        assert_eq!(LABELS.len(), CLASSES);
        assert_eq!(LABELS[148], "Piano");
        assert_eq!(LABELS[137], "Bass guitar");
        let mut scores = vec![0.0; CLASSES];
        scores[148] = 0.8;
        scores[137] = 0.7;
        scores[0] = 0.9;
        let result = ranked(&scores, true, 0.2);
        assert_eq!(
            result.iter().map(|c| c.class_index).collect::<Vec<_>>(),
            [148, 137]
        );
        assert_eq!(ranked(&scores, false, 0.0)[0].class_index, 0);
        assert!(ranked(&scores, true, 0.95).is_empty());
    }

    #[test]
    fn padding_matches_google_patch_count() {
        for (samples, count) in [
            (1, 1),
            (15600, 1),
            (15601, 2),
            (23280, 2),
            (23281, 3),
            (48000, 6),
        ] {
            assert_eq!(window_count(samples), count);
        }
    }

    #[test]
    fn invalid_requests_and_cancellation_do_not_open_a_model() {
        let pcm = AudioBuffer::from_planar(vec![vec![0.0; 16]], INSTRUMENT_RATE).unwrap();
        let missing = Path::new("no-such-model.onnx");
        assert!(matches!(
            analyze_instruments(&pcm, missing, f32::NAN, &Default::default()),
            Err(AnalysisError::Invalid(_))
        ));
        let control = AnalysisControl::default();
        control.cancel();
        assert!(matches!(
            analyze_instruments(&pcm, missing, 0.2, &control),
            Err(AnalysisError::Cancelled)
        ));
    }

    #[test]
    #[ignore = "requires explicitly prepared AURIS_YAMNET_MODEL; runs real CPU inference"]
    fn real_model_matches_export_reference() {
        let path = std::env::var_os("AURIS_YAMNET_MODEL").expect("set AURIS_YAMNET_MODEL");
        let reference: serde_json::Value = serde_json::from_slice(
            &std::fs::read(Path::new(&path).with_extension("verification.json")).unwrap(),
        )
        .unwrap();
        for sine in [false, true] {
            let samples = (0..WINDOW)
                .map(|i| {
                    if sine {
                        ((std::f64::consts::TAU * 440.0 * i as f64 / INSTRUMENT_RATE).sin() * 0.4)
                            as f32
                    } else {
                        0.0
                    }
                })
                .collect();
            let pcm = AudioBuffer::from_planar(vec![samples], INSTRUMENT_RATE).unwrap();
            let result =
                analyze_instruments(&pcm, Path::new(&path), 0.2, &Default::default()).unwrap();
            let expected = &reference["checks"][if sine { "sine" } else { "silence" }];
            assert_eq!(
                result.raw_top[0].class_index,
                expected["top_index"].as_u64().unwrap() as usize
            );
            for candidate in &result.raw_top {
                assert!(
                    (f64::from(candidate.score)
                        - expected["scores"][candidate.class_index].as_f64().unwrap())
                    .abs()
                        < 2e-4
                );
            }
            assert_eq!(result.windows.len(), 1);
            assert_eq!(result.windows.last().unwrap().end, 0.975);
            assert!(result.candidates.is_empty());
        }
    }
}
