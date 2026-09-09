//! Local CLAP audio/text embeddings from the versioned Auris ONNX export.
//!
//! This is the learned audio representation, unrelated to the CLAP plugin format.
//! Loading checks the converter's complete file manifest before creating CPU sessions.
//! Model preparation is explicit; inference never downloads files or invokes Python.

use crate::AnalysisError;
use ort::{
    execution_providers::CPUExecutionProvider,
    session::Session,
    tensor::TensorElementType,
    value::{Tensor, ValueType},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, path::Path};
use tokenizers::Tokenizer;

/// Number of log-mel frames in one centered, ten-second CLAP input.
pub const AUDIO_FRAMES: usize = 1001;
/// Number of Slaney-normalized mel bands in one CLAP frame.
pub const MEL_BANDS: usize = 64;
/// Shared dimension of the CLAP audio and text embedding space.
pub const EMBEDDING_SIZE: usize = 512;
/// Fixed text sequence length, including start/end tokens and padding.
pub const TEXT_TOKENS: usize = 77;
/// Periodic Hann window followed by the row-major 513-by-64 Slaney mel matrix.
pub const PREPROCESSING_COEFFICIENTS: usize = 1024 + 513 * MEL_BANDS;

const FORMAT: &str = "auris-clap-htsat-unfused-v1";
const MODEL_ID: &str = "laion/clap-htsat-unfused";
const FILES: [&str; 4] = [
    "audio.onnx",
    "text.onnx",
    "tokenizer.json",
    "preprocess.bin",
];
const MAX_PROMPT_BYTES: usize = 16_384;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    model_id: String,
    source_revision: String,
    license: String,
    files: BTreeMap<String, String>,
}

fn error(message: impl ToString) -> AnalysisError {
    AnalysisError::Model(message.to_string())
}

fn lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Manifest {
    fn validate(&self) -> Result<(), AnalysisError> {
        if self.format != FORMAT
            || self.model_id != MODEL_ID
            || self.license != "Apache-2.0"
            || !lowercase_hex(&self.source_revision, 40)
            || self.files.len() != FILES.len()
            || FILES.iter().any(|name| {
                !self
                    .files
                    .get(*name)
                    .is_some_and(|hash| lowercase_hex(hash, 64))
            })
        {
            return Err(error(
                "incompatible CLAP export manifest; run export_clap.py",
            ));
        }
        Ok(())
    }
}

fn read(path: &Path, limit: u64) -> Result<Vec<u8>, AnalysisError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| error(format!("cannot open CLAP file {}: {e}", path.display())))?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > limit {
        return Err(error(format!(
            "CLAP file exceeds its size limit: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

fn read_verified(
    root: &Path,
    name: &str,
    limit: u64,
    manifest: &Manifest,
) -> Result<Vec<u8>, AnalysisError> {
    let bytes = read(&root.join(name), limit)?;
    if format!("{:x}", Sha256::digest(&bytes)) != manifest.files[name] {
        return Err(error(format!(
            "CLAP checksum mismatch for {name}; run export_clap.py again"
        )));
    }
    Ok(bytes)
}

fn tensor_matches(value: &ValueType, element: TensorElementType, shape: &[i64]) -> bool {
    matches!(value, ValueType::Tensor { ty, dimensions, .. } if *ty == element && dimensions == shape)
}

fn load_session(root: &Path, role: &str, manifest: &Manifest) -> Result<Session, AnalysisError> {
    let bytes = read_verified(root, &format!("{role}.onnx"), 800 * 1024 * 1024, manifest)?;
    let session = Session::builder()
        .map_err(error)?
        .with_intra_threads(2)
        .map_err(error)?
        .with_execution_providers([CPUExecutionProvider::default().build()])
        .map_err(error)?
        .commit_from_memory(&bytes)
        .map_err(error)?;
    let metadata = session.metadata().map_err(error)?;
    if metadata.custom("auris.clap").map_err(error)?.as_deref() != Some("htsat-unfused-v1")
        || metadata.custom("role").map_err(error)?.as_deref() != Some(role)
        || metadata.custom("model_id").map_err(error)?.as_deref()
            != Some(manifest.model_id.as_str())
        || metadata
            .custom("source_revision")
            .map_err(error)?
            .as_deref()
            != Some(manifest.source_revision.as_str())
    {
        return Err(error(format!("incompatible CLAP {role} ONNX metadata")));
    }
    let inputs: &[(&str, TensorElementType, &[i64])] = if role == "audio" {
        &[(
            "input_features",
            TensorElementType::Float32,
            &[1, 1, AUDIO_FRAMES as i64, MEL_BANDS as i64],
        )]
    } else {
        &[
            (
                "input_ids",
                TensorElementType::Int64,
                &[1, TEXT_TOKENS as i64],
            ),
            (
                "attention_mask",
                TensorElementType::Int64,
                &[1, TEXT_TOKENS as i64],
            ),
        ]
    };
    if session.inputs.len() != inputs.len()
        || inputs.iter().any(|(name, element, shape)| {
            !session.inputs.iter().any(|input| {
                input.name == *name && tensor_matches(&input.input_type, *element, shape)
            })
        })
        || session.outputs.len() != 1
        || session.outputs[0].name != "embedding"
        || !tensor_matches(
            &session.outputs[0].output_type,
            TensorElementType::Float32,
            &[1, EMBEDDING_SIZE as i64],
        )
    {
        return Err(error(format!(
            "incompatible CLAP {role} ONNX tensor names, types or shapes"
        )));
    }
    drop(metadata);
    Ok(session)
}

fn coefficients(bytes: &[u8]) -> Result<Vec<f64>, AnalysisError> {
    if bytes.len() != PREPROCESSING_COEFFICIENTS * size_of::<f64>() {
        return Err(error("incompatible CLAP preprocessing coefficient count"));
    }
    let result: Vec<_> = bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| f64::from_le_bytes(*chunk))
        .collect();
    if result.iter().any(|v| !v.is_finite() || *v < 0.0)
        || result[..1024].iter().all(|v| *v == 0.0)
        || result[1024..].iter().all(|v| *v == 0.0)
    {
        return Err(error("invalid CLAP preprocessing coefficients"));
    }
    Ok(result)
}

fn configure_tokenizer(bytes: &[u8]) -> Result<Tokenizer, AnalysisError> {
    let mut tokenizer = Tokenizer::from_bytes(bytes).map_err(error)?;
    if tokenizer.token_to_id("<s>") != Some(0)
        || tokenizer.token_to_id("<pad>") != Some(1)
        || tokenizer.token_to_id("</s>") != Some(2)
        || tokenizer.token_to_id("<unk>") != Some(3)
    {
        return Err(error("incompatible CLAP tokenizer special tokens"));
    }
    // Serialized Hugging Face settings must not silently cut off a user's objective.
    tokenizer.with_truncation(None).map_err(error)?;
    tokenizer.with_padding(None);
    Ok(tokenizer)
}

fn tokenize(tokenizer: &Tokenizer, prompt: &str) -> Result<(Vec<i64>, Vec<i64>), AnalysisError> {
    if prompt.trim().is_empty() {
        return Err(AnalysisError::Invalid("CLAP text prompt is empty"));
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        return Err(AnalysisError::Invalid(
            "CLAP text prompt exceeds 16,384 UTF-8 bytes",
        ));
    }
    let encoded = tokenizer.encode(prompt, true).map_err(error)?;
    let ids = encoded.get_ids();
    if ids.len() > TEXT_TOKENS {
        return Err(AnalysisError::Invalid(
            "CLAP text prompt exceeds 75 content tokens; shorten the prompt",
        ));
    }
    if ids.len() < 2
        || ids.first() != Some(&0)
        || ids.last() != Some(&2)
        || ids.iter().any(|id| *id >= 50_265)
    {
        return Err(error("CLAP tokenizer returned an incompatible sequence"));
    }
    let mut mask = vec![0_i64; TEXT_TOKENS];
    mask[..ids.len()].fill(1);
    let mut padded: Vec<_> = ids.iter().copied().map(i64::from).collect();
    padded.resize(TEXT_TOKENS, 1);
    Ok((padded, mask))
}

fn normalized_embedding(shape: &[i64], values: &[f32]) -> Result<Vec<f32>, AnalysisError> {
    if shape != [1, EMBEDDING_SIZE as i64]
        || values.len() != EMBEDDING_SIZE
        || values.iter().any(|v| !v.is_finite())
    {
        return Err(error("CLAP returned an invalid embedding"));
    }
    let norm = values
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    if !(0.99..=1.01).contains(&norm) {
        return Err(error("CLAP returned a non-normalized or zero embedding"));
    }
    Ok(values
        .iter()
        .map(|v| (f64::from(*v) / norm) as f32)
        .collect())
}

/// Validated CLAP audio and text encoders for use on an offline worker.
///
/// Sessions run on the CPU with two intra-op threads. A model is reusable throughout a
/// search, so weights are loaded once and each target embedding can be cached by its caller.
pub struct ClapModel {
    audio: Session,
    text: Session,
    tokenizer: Tokenizer,
    coefficients: Vec<f64>,
    manifest: Manifest,
}

impl ClapModel {
    /// Loads a model directory produced by `tools/music-models/export_clap.py`.
    ///
    /// The manifest, checksums, preprocessing coefficients, tokenizer, graph metadata and
    /// fixed tensor contracts are checked before a model can evaluate any candidate.
    pub fn load(root: &Path) -> Result<Self, AnalysisError> {
        let manifest: Manifest =
            serde_json::from_slice(&read(&root.join("manifest.json"), 64 * 1024)?)
                .map_err(error)?;
        manifest.validate()?;
        let coefficients = coefficients(&read_verified(
            root,
            "preprocess.bin",
            (PREPROCESSING_COEFFICIENTS * 8) as u64,
            &manifest,
        )?)?;
        let tokenizer = configure_tokenizer(&read_verified(
            root,
            "tokenizer.json",
            16 * 1024 * 1024,
            &manifest,
        )?)?;
        let audio = load_session(root, "audio", &manifest)?;
        let text = load_session(root, "text", &manifest)?;
        Ok(Self {
            audio,
            text,
            tokenizer,
            coefficients,
            manifest,
        })
    }

    /// Encodes a nonempty prompt without truncation and returns a unit-length embedding.
    ///
    /// The fixed export accepts at most 75 content tokens plus BOS and EOS. Padding is
    /// applied on the right with the official RoBERTa token and attention mask.
    pub fn text_embedding(&self, prompt: &str) -> Result<Vec<f32>, AnalysisError> {
        let (ids, mask) = tokenize(&self.tokenizer, prompt)?;
        let ids = Tensor::from_array(([1, TEXT_TOKENS], ids)).map_err(error)?;
        let mask = Tensor::from_array(([1, TEXT_TOKENS], mask)).map_err(error)?;
        let inputs = ort::inputs![
            "input_ids" => ids,
            "attention_mask" => mask,
        ]
        .map_err(error)?;
        let outputs = self.text.run(inputs).map_err(error)?;
        let (shape, embedding) = outputs["embedding"]
            .try_extract_raw_tensor::<f32>()
            .map_err(error)?;
        normalized_embedding(shape, embedding)
    }

    /// Encodes one row-major 1001-by-64 log-mel input and returns a unit-length embedding.
    ///
    /// Features must use this export's preprocessing coefficients and 48 kHz mono audio.
    /// Long excerpts are split and combined by the caller under one fixed search policy.
    pub fn audio_embedding(&self, features: &[f32]) -> Result<Vec<f32>, AnalysisError> {
        if features.len() != AUDIO_FRAMES * MEL_BANDS || features.iter().any(|f| !f.is_finite()) {
            return Err(AnalysisError::Invalid(
                "CLAP expects finite 1001-by-64 audio features",
            ));
        }
        let features = Tensor::from_array(([1, 1, AUDIO_FRAMES, MEL_BANDS], features.to_vec()))
            .map_err(error)?;
        let inputs = ort::inputs![
            "input_features" => features,
        ]
        .map_err(error)?;
        let outputs = self.audio.run(inputs).map_err(error)?;
        let (shape, embedding) = outputs["embedding"]
            .try_extract_raw_tensor::<f32>()
            .map_err(error)?;
        normalized_embedding(shape, embedding)
    }

    /// Exact periodic Hann and Slaney mel coefficients retained by the exporter.
    pub fn preprocessing_coefficients(&self) -> &[f64] {
        &self.coefficients
    }

    /// Official checkpoint identifier from the validated export manifest.
    pub fn model_id(&self) -> &str {
        &self.manifest.model_id
    }

    /// Immutable source repository commit recorded by the exporter.
    pub fn source_revision(&self) -> &str {
        &self.manifest.source_revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenizers::{
        models::wordlevel::WordLevel, pre_tokenizers::whitespace::Whitespace,
        processors::template::TemplateProcessing,
    };

    fn manifest() -> Manifest {
        Manifest {
            format: FORMAT.into(),
            model_id: MODEL_ID.into(),
            source_revision: "a".repeat(40),
            license: "Apache-2.0".into(),
            files: FILES
                .into_iter()
                .map(|name| (name.into(), "b".repeat(64)))
                .collect(),
        }
    }

    fn tokenizer() -> Tokenizer {
        let model = WordLevel::builder()
            .vocab(
                [
                    ("<s>", 0),
                    ("<pad>", 1),
                    ("</s>", 2),
                    ("<unk>", 3),
                    ("music", 4),
                ]
                .into_iter()
                .map(|(word, id)| (word.into(), id))
                .collect(),
            )
            .unk_token("<unk>".into())
            .build()
            .unwrap();
        let mut tokenizer = Tokenizer::new(model);
        tokenizer.with_pre_tokenizer(Some(Whitespace));
        tokenizer.with_post_processor(Some(
            TemplateProcessing::builder()
                .try_single("<s> $A </s>")
                .unwrap()
                .special_tokens(vec![("<s>", 0), ("</s>", 2)])
                .build()
                .unwrap(),
        ));
        configure_tokenizer(tokenizer.to_string(false).unwrap().as_bytes()).unwrap()
    }

    #[test]
    fn manifest_rejects_wrong_model_revision_and_file_substitution() {
        manifest().validate().unwrap();
        let mut value = manifest();
        value.source_revision = "main".into();
        assert!(value.validate().is_err());
        value = manifest();
        value.model_id = "other/clap".into();
        assert!(value.validate().is_err());
        value = manifest();
        value.files.remove("text.onnx");
        value.files.insert("../text.onnx".into(), "b".repeat(64));
        assert!(value.validate().is_err());
        value = manifest();
        value.files.insert("audio.onnx".into(), "g".repeat(64));
        assert!(value.validate().is_err());
    }

    #[test]
    fn text_preserves_start_end_and_masks_only_right_padding() {
        let (ids, mask) = tokenize(&tokenizer(), "music music").unwrap();
        assert_eq!(&ids[..4], &[0, 4, 4, 2]);
        assert_eq!(ids.len(), TEXT_TOKENS);
        assert!(ids[4..].iter().all(|v| *v == 1));
        assert_eq!(&mask[..4], &[1, 1, 1, 1]);
        assert!(mask[4..].iter().all(|v| *v == 0));
    }

    #[test]
    fn text_length_is_checked_without_truncation() {
        let tokenizer = tokenizer();
        let accepted = std::iter::repeat_n("music", 75)
            .collect::<Vec<_>>()
            .join(" ");
        let (ids, mask) = tokenize(&tokenizer, &accepted).unwrap();
        assert_eq!(ids[76], 2);
        assert!(mask.iter().all(|v| *v == 1));
        assert!(tokenize(&tokenizer, &(accepted + " music")).is_err());
        assert!(tokenize(&tokenizer, " \n\t").is_err());
        assert!(tokenize(&tokenizer, &"a".repeat(MAX_PROMPT_BYTES + 1)).is_err());
    }

    #[test]
    fn serialized_padding_and_truncation_cannot_change_the_prompt() {
        let mut serialized = tokenizer();
        serialized
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: 3,
                ..Default::default()
            }))
            .unwrap();
        serialized.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::Fixed(100),
            ..Default::default()
        }));
        let loaded = configure_tokenizer(serialized.to_string(false).unwrap().as_bytes()).unwrap();
        let (ids, mask) = tokenize(&loaded, "music music music").unwrap();
        assert_eq!(&ids[..5], &[0, 4, 4, 4, 2]);
        assert_eq!(ids.len(), TEXT_TOKENS);
        assert_eq!(mask.iter().sum::<i64>(), 5);
    }

    #[test]
    fn tensor_contract_rejects_dynamic_or_wrong_types() {
        let tensor = ValueType::Tensor {
            ty: TensorElementType::Float32,
            dimensions: vec![1, 512],
            dimension_symbols: vec![None, None],
        };
        assert!(tensor_matches(
            &tensor,
            TensorElementType::Float32,
            &[1, 512]
        ));
        assert!(!tensor_matches(
            &tensor,
            TensorElementType::Float32,
            &[-1, 512]
        ));
        assert!(!tensor_matches(
            &tensor,
            TensorElementType::Int64,
            &[1, 512]
        ));
    }

    #[test]
    fn embeddings_must_be_finite_unit_vectors_of_expected_size() {
        let mut values = vec![0.0; EMBEDDING_SIZE];
        assert!(normalized_embedding(&[1, 512], &values).is_err());
        values[0] = 1.000_01;
        assert_eq!(normalized_embedding(&[1, 512], &values).unwrap()[0], 1.0);
        values[0] = f32::NAN;
        assert!(normalized_embedding(&[1, 512], &values).is_err());
        values[0] = 1.0;
        assert!(normalized_embedding(&[512], &values).is_err());
        assert!(normalized_embedding(&[1, 512], &values[..511]).is_err());
    }

    #[test]
    fn coefficients_reject_nonfinite_missing_and_empty_filter_banks() {
        let mut values = vec![0.0_f64; PREPROCESSING_COEFFICIENTS];
        let bytes = |values: &[f64]| {
            values
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>()
        };
        assert!(coefficients(&bytes(&values)).is_err());
        values[1] = 0.5;
        values[1024] = 0.001;
        assert_eq!(coefficients(&bytes(&values)).unwrap(), values);
        assert!(coefficients(&bytes(&values)[8..]).is_err());
        values[1025] = f64::INFINITY;
        assert!(coefficients(&bytes(&values)).is_err());
    }

    #[test]
    fn model_is_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<ClapModel>();
    }

    #[test]
    #[ignore = "requires an exported checkpoint in AURIS_CLAP_TEST_MODEL"]
    fn real_export_matches_python_embeddings_and_tokens() {
        let path = std::env::var_os("AURIS_CLAP_TEST_MODEL")
            .expect("set AURIS_CLAP_TEST_MODEL to the exported CLAP model directory");
        let root = Path::new(&path);
        let model = ClapModel::load(root).unwrap();
        #[derive(Deserialize)]
        struct TextParity {
            prompt: String,
            input_ids: Vec<i64>,
            attention_mask: Vec<i64>,
            embedding: Vec<f32>,
        }
        let reference: TextParity =
            serde_json::from_slice(&std::fs::read(root.join("parity/text.json")).unwrap()).unwrap();
        let (ids, mask) = tokenize(&model.tokenizer, &reference.prompt).unwrap();
        assert_eq!(ids, reference.input_ids);
        assert_eq!(mask, reference.attention_mask);
        let text = model.text_embedding(&reference.prompt).unwrap();
        let floats = |path: &str| {
            std::fs::read(root.join(path))
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes))
                .collect::<Vec<_>>()
        };
        let audio = model
            .audio_embedding(&floats("parity/features.f32"))
            .unwrap();
        for (actual, expected) in [
            (&text, &reference.embedding),
            (&audio, &floats("parity/audio_embedding.f32")),
        ] {
            assert_eq!(actual.len(), expected.len());
            let maximum = actual
                .iter()
                .zip(expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(maximum < 0.0002, "CLAP embedding parity error {maximum}");
        }
    }
}
