//! Optional MuScriptor Small ONNX inference and streaming note decoding on CPU.
//! The export is user-prepared external data; see NOTICE-MUSCRIPTOR.md for provenance.

mod tokens;

use crate::{AnalysisControl, AnalysisError};
use auris_core::AudioBuffer;
use ort::{execution_providers::CPUExecutionProvider, session::Session, value::Tensor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

/// Notice required before every invocation, including model-facing tools.
pub const MUSCRIPTOR_NOTICE: &str = "MuScriptor model weights are licensed under CC BY-NC 4.0 for noncommercial use only. Do not use this model for commercial work without separate permission from its rights holders. This optional model does not change Auris Studio's Apache-2.0 license. Acknowledgement does not grant commercial rights.";

/// One unquantized, instrument-labeled note hypothesis.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MixtureNote {
    /// MIDI pitch, or percussion pitch for drums.
    pub pitch: u8,
    /// Source-second onset.
    pub start: f64,
    /// Exclusive source-second offset.
    pub end: f64,
    /// Model instrument group, not a recovered playback patch.
    pub instrument: String,
}

/// Editable mixture transcription with explicit model provenance.
#[derive(Clone, Debug, Serialize)]
pub struct MixtureAnalysis {
    /// Runtime and decoding contract.
    pub algorithm: &'static str,
    /// Restrictions on use of the external model.
    pub model_license: &'static str,
    /// Exact decoder ONNX file hash.
    pub model_sha256: String,
    /// Original safetensors checkpoint hash retained by the converter.
    pub checkpoint_sha256: String,
    /// Source duration before padding/stretch.
    pub seconds: f64,
    /// Potentially overlapping instrument-labeled note hypotheses.
    pub notes: Vec<MixtureNote>,
}

#[derive(Deserialize)]
struct Manifest {
    format: String,
    license: String,
    source_package: String,
    checkpoint_sha256: String,
    files: BTreeMap<String, String>,
    vocabulary: Vec<tokens::VocabularyEntry>,
    programs: Vec<String>,
}

fn error(e: impl ToString) -> AnalysisError {
    AnalysisError::Model(e.to_string())
}
fn read(path: &Path, limit: u64) -> Result<Vec<u8>, AnalysisError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(error)?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > limit {
        return Err(error("MuScriptor file exceeds its size limit"));
    }
    Ok(bytes)
}
fn sha_valid(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Manifest {
    fn validate(&self) -> Result<(), AnalysisError> {
        if self.format != "auris-muscriptor-small-v1"
            || self.license != "CC-BY-NC-4.0"
            || self.source_package != "muscriptor==0.3.0"
            || !sha_valid(&self.checkpoint_sha256)
            || self.files.len() != 2
            || ["audio.onnx", "decoder.onnx"]
                .iter()
                .any(|n| !self.files.get(*n).is_some_and(|s| sha_valid(s)))
            || self.programs.len() != 130
            || self
                .programs
                .iter()
                .any(|p| p.is_empty() || p.len() > 96 || p.chars().any(char::is_control))
            || !tokens::valid_vocabulary(&self.vocabulary)
        {
            return Err(error("incompatible MuScriptor export manifest"));
        }
        Ok(())
    }
}

fn load(root: &Path, name: &str, manifest: &Manifest) -> Result<Session, AnalysisError> {
    let bytes = read(&root.join(format!("{name}.onnx")), 512 * 1024 * 1024)?;
    if format!("{:x}", Sha256::digest(&bytes)) != manifest.files[&format!("{name}.onnx")] {
        return Err(error(
            "MuScriptor ONNX checksum mismatch; run the converter again",
        ));
    }
    let session = Session::builder()
        .map_err(error)?
        .with_intra_threads(2)
        .map_err(error)?
        .with_execution_providers([CPUExecutionProvider::default().build()])
        .map_err(error)?
        .commit_from_memory(&bytes)
        .map_err(error)?;
    let meta = session.metadata().map_err(error)?;
    if meta.custom("auris.muscriptor").map_err(error)?.as_deref() != Some("small-v1")
        || meta.custom("role").map_err(error)?.as_deref() != Some(name)
        || meta.custom("license").map_err(error)?.as_deref() != Some("CC-BY-NC-4.0")
    {
        return Err(error("incompatible MuScriptor ONNX metadata"));
    }
    let (inputs, outputs): (&[&str], &[&str]) = if name == "audio" {
        (&["waveform"], &["prefix"])
    } else {
        (&["tokens", "prefix", "past"], &["logits", "present"])
    };
    if session
        .inputs
        .iter()
        .map(|v| v.name.as_str())
        .ne(inputs.iter().copied())
        || session
            .outputs
            .iter()
            .map(|v| v.name.as_str())
            .ne(outputs.iter().copied())
    {
        return Err(error("incompatible MuScriptor ONNX inputs/outputs"));
    }
    drop(meta);
    Ok(session)
}

/// Validates model note events before they cross the document boundary.
pub fn validate_notes(notes: &mut [MixtureNote], seconds: f64) -> Result<(), AnalysisError> {
    if !seconds.is_finite()
        || seconds <= 0.0
        || seconds > 600.0
        || notes.len() > 200_000
        || notes.iter().any(|n| {
            n.pitch > 127
                || !n.start.is_finite()
                || !n.end.is_finite()
                || n.start < 0.0
                || n.start >= seconds
                || n.end <= n.start
                || n.end > seconds + 10.0
                || n.instrument.is_empty()
                || n.instrument.len() > 96
                || n.instrument.chars().any(char::is_control)
        })
    {
        return Err(error("invalid or excessive MuScriptor note events"));
    }
    for n in notes.iter_mut() {
        n.end = n.end.min(seconds);
    }
    notes.sort_by(|a, b| {
        a.start
            .total_cmp(&b.start)
            .then(a.instrument.cmp(&b.instrument))
            .then(a.pitch.cmp(&b.pitch))
    });
    Ok(())
}

/// Runs a user-converted Small ONNX package without Python, downloads or GPU providers.
///
/// `model` selects `decoder.onnx` beside `audio.onnx` and `muscriptor.json`.
/// Explicit per-run noncommercial acknowledgement is required before any file is opened.
/// Input is bounded to ten minutes / eight channels at 16 kHz. Generation uses five-second
/// chunks, at most 2,000 tokens each, and a thirty-minute wall limit. Cancellation is observed
/// between ONNX calls; a call already executing finishes before its session is released.
pub fn transcribe(
    audio: &AudioBuffer,
    model: &Path,
    acknowledge_noncommercial: bool,
    control: &AnalysisControl,
) -> Result<MixtureAnalysis, AnalysisError> {
    if !acknowledge_noncommercial {
        return Err(error(MUSCRIPTOR_NOTICE));
    }
    control.check(0.0)?;
    if audio.sample_rate() != 16000.0
        || audio.frame_count() == 0
        || audio.frame_count() > 9_600_000
        || !(1..=8).contains(&audio.channel_count())
        || audio.iter_channels().flatten().any(|s| !s.is_finite())
    {
        return Err(error(
            "expected finite 16 kHz audio, at most ten minutes / eight channels",
        ));
    }
    if model.file_name().and_then(|s| s.to_str()) != Some("decoder.onnx") {
        return Err(error("select the converted decoder.onnx"));
    }
    let root = model
        .parent()
        .ok_or_else(|| error("missing model directory"))?;
    let manifest: Manifest =
        serde_json::from_slice(&read(&root.join("muscriptor.json"), 256 * 1024)?).map_err(error)?;
    manifest.validate()?;
    let audio_model = load(root, "audio", &manifest)?;
    control.check(0.0)?;
    let decoder = load(root, "decoder", &manifest)?;
    let started = Instant::now();
    let count = audio.frame_count().div_ceil(80000);
    let mut tracker = tokens::Tracker::new(&manifest.programs);
    // ort 2.0-rc.9 rejects zero dimensions in raw tuples; ndarray supports empty caches.
    let empty_prefix =
        Tensor::from_array(ndarray::Array3::<f32>::zeros((1, 0, 768))).map_err(error)?;
    for chunk in 0..count {
        control.check(chunk as f32 / count as f32)?;
        tracker.boundary(
            chunk as f64 * 5.0,
            (chunk + 1 < count).then_some((chunk + 1) as f64 * 5.0),
        );
        let mut prompt = if chunk == 0 {
            Vec::new()
        } else {
            tracker.prompt()
        };
        if prompt.len() >= 2000 {
            return Err(error("too many sustained notes in MuScriptor prologue"));
        }
        for &token in &prompt {
            tracker.feed(token)?;
        }
        let forced = prompt.len();
        prompt.insert(0, 1393);
        let mut waveform = vec![0.0_f32; 80000];
        let end = (chunk * 80000 + 80000).min(audio.frame_count());
        for channel in audio.iter_channels() {
            for (out, sample) in waveform.iter_mut().zip(&channel[chunk * 80000..end]) {
                *out += sample / audio.channel_count() as f32;
            }
        }
        let wav = Tensor::from_array(([1, 80000], waveform)).map_err(error)?;
        let mut prefix_output = audio_model
            .run(ort::inputs!["waveform" => wav].map_err(error)?)
            .map_err(error)?;
        let prefix = prefix_output
            .remove("prefix")
            .ok_or_else(|| error("missing audio prefix"))?;
        let (shape, data) = prefix.try_extract_raw_tensor::<f32>().map_err(error)?;
        if shape != [1, 503, 768] || data.iter().any(|v| !v.is_finite()) {
            return Err(error("invalid audio prefix"));
        }
        let mut past = Tensor::from_array(ndarray::ArrayD::<f32>::zeros(ndarray::IxDyn(&[
            14, 2, 1, 12, 0, 64,
        ])))
        .map_err(error)?
        .into_dyn();
        let mut cache_length = 0;
        let mut finished = false;
        for step in forced..2000 {
            control.check((chunk as f32 + (step as f32 / 2000.0)) / count as f32)?;
            if started.elapsed() > Duration::from_secs(1800) {
                return Err(error("MuScriptor exceeded thirty minutes"));
            }
            let first = step == forced;
            cache_length += prompt.len() + if first { 503 } else { 0 };
            let input = Tensor::from_array(([1, prompt.len()], std::mem::take(&mut prompt)))
                .map_err(error)?;
            let current_prefix = if first {
                prefix.view()
            } else {
                empty_prefix.view().into_dyn()
            };
            let mut outputs = decoder
                .run(vec![
                    ("tokens", ort::session::SessionInputValue::from(input)),
                    ("prefix", current_prefix.into()),
                    ("past", past.view().into()),
                ])
                .map_err(error)?;
            let (shape, logits) = outputs["logits"]
                .try_extract_raw_tensor::<f32>()
                .map_err(error)?;
            if shape != [1, 1393] || logits.iter().any(|v| !v.is_finite()) {
                return Err(error("invalid decoder logits"));
            }
            let token = logits
                .iter()
                .enumerate()
                .max_by(|(a, x), (b, y)| x.total_cmp(y).then(b.cmp(a)))
                .unwrap()
                .0 as i64;
            past = outputs
                .remove("present")
                .ok_or_else(|| error("missing decoder cache"))?;
            let (shape, _) = past.try_extract_raw_tensor::<f32>().map_err(error)?;
            if shape != [14, 2, 1, 12, cache_length as i64, 64] {
                return Err(error("invalid decoder cache shape"));
            }
            if token == 1 {
                finished = true;
                break;
            }
            tracker.feed(token)?;
            prompt.push(token);
        }
        if !finished {
            return Err(error(
                "MuScriptor chunk did not emit EOS within 2,000 tokens",
            ));
        }
    }
    let mut notes = tracker.finish();
    let seconds = audio.frame_count() as f64 / 16000.0;
    validate_notes(&mut notes, seconds)?;
    control.check(1.0)?;
    Ok(MixtureAnalysis {
        algorithm: "muscriptor-small-onnx-cpu-v1",
        model_license: "CC-BY-NC-4.0",
        model_sha256: manifest.files["decoder.onnx"].clone(),
        checkpoint_sha256: manifest.checkpoint_sha256,
        seconds,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_and_cancellation_precede_model_io() {
        let audio = AudioBuffer::from_planar(vec![vec![0.0; 160]], 16000.0).unwrap();
        let missing = Path::new("absent/decoder.onnx");
        assert!(
            transcribe(&audio, missing, false, &Default::default())
                .unwrap_err()
                .to_string()
                .contains("CC BY-NC 4.0")
        );
        let control = AnalysisControl::default();
        control.cancel();
        assert!(matches!(
            transcribe(&audio, missing, true, &control),
            Err(AnalysisError::Cancelled)
        ));
    }

    #[test]
    fn changed_model_is_rejected_before_onnx_parsing() {
        let folder =
            std::env::temp_dir().join(format!("auris-onnx-integrity-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("audio.onnx");
        std::fs::write(&path, b"changed model; must not reach ORT").unwrap();
        let manifest = Manifest {
            format: String::new(),
            license: String::new(),
            source_package: String::new(),
            checkpoint_sha256: String::new(),
            files: BTreeMap::from([("audio.onnx".into(), "0".repeat(64))]),
            vocabulary: Vec::new(),
            programs: Vec::new(),
        };
        let result = load(&folder, "audio", &manifest);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
        assert!(
            matches!(result, Err(AnalysisError::Model(message)) if message.contains("checksum mismatch"))
        );
    }
}
