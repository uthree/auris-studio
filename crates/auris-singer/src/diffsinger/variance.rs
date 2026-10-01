//! DiffSinger linguistic and variance predictors and their deployment resources.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use auris_vocal::{SingerFrames, is_syllabic};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use serde::Deserialize;

use super::open_session;
use super::{
    DEFAULT_STEPS, DiffScore, DsConfig, arrange, control_shape, load_error, read_lines,
    unsafe_access,
};
use crate::limits::{
    MAX_COLLECTION_ITEMS, MAX_NAME_BYTES, MAX_PATH_BYTES, checked_product, read_text_file,
};
use crate::score::MAX_CHUNK_FRAMES;
use crate::{Acceleration, SingError};

type Inputs<'a> = Vec<(Cow<'a, str>, SessionInputValue<'a>)>;

fn inference(error: ort::Error) -> SingError {
    SingError::Inference(error.to_string())
}

pub(super) struct Resources {
    pub(super) speakers: Vec<String>,
    embeddings: Vec<Vec<f32>>,
    languages: Option<BTreeMap<String, i64>>,
    pub(super) safe: bool,
}

// Auxiliary configs commonly use ../ to refer to siblings inside the selected voicebank.
// The boundary stays the whole voicebank, including canonical symlink targets.
fn resource_path(
    boundary: &Path,
    owner: &Path,
    child: &str,
    automatic: bool,
    safe: &mut bool,
) -> Result<PathBuf, SingError> {
    if child.is_empty() || child.len() > MAX_PATH_BYTES {
        return Err(SingError::Metadata(
            "DiffSinger resource path is empty or too long".into(),
        ));
    }
    let path = owner.join(child);
    let checked = boundary
        .canonicalize()
        .ok()
        .zip(path.canonicalize().ok())
        .and_then(|(root, target)| target.starts_with(root).then_some(target));
    let contained = !Path::new(child).is_absolute()
        && !child.contains('\\')
        && !child.contains(':')
        && checked.is_some();
    *safe &= contained;
    if automatic && !contained {
        return Err(unsafe_access(format!(
            "DiffSinger resource must resolve inside the voicebank: {child}"
        )));
    }
    if automatic {
        checked.ok_or_else(|| unsafe_access("DiffSinger resource containment check failed"))
    } else {
        Ok(path)
    }
}

impl Resources {
    pub(super) fn load(
        boundary: &Path,
        owner: &Path,
        config: &DsConfig,
        automatic: bool,
    ) -> Result<Self, SingError> {
        let mut safe = true;
        let speakers = config.speakers.clone().unwrap_or_default();
        if speakers.len() > MAX_COLLECTION_ITEMS
            || !(1..=MAX_COLLECTION_ITEMS).contains(&config.hidden_size)
        {
            return Err(SingError::Metadata(
                "DiffSinger speaker dimensions exceed their limit".into(),
            ));
        }
        let mut embeddings = Vec::new();
        for speaker in &speakers {
            if speaker.trim().is_empty() || speaker.len() > MAX_NAME_BYTES {
                return Err(SingError::Metadata(
                    "DiffSinger speaker name is empty or too long".into(),
                ));
            }
            let path = resource_path(
                boundary,
                owner,
                &format!("{speaker}.emb"),
                automatic,
                &mut safe,
            )?;
            let expected = config.hidden_size * 4;
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .map_err(|error| load_error(format!("{}: {error}", path.display())))?
                .take(expected as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| load_error(error.to_string()))?;
            if bytes.len() != expected {
                return Err(SingError::Metadata(format!(
                    "DiffSinger {speaker}.emb must contain {} little-endian float32 values",
                    config.hidden_size
                )));
            }
            let values: Vec<_> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes))
                .collect();
            if values.iter().any(|value| !value.is_finite()) {
                return Err(SingError::Metadata(
                    "DiffSinger speaker embedding contains a nonfinite value".into(),
                ));
            }
            embeddings.push(values);
        }
        if speakers
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != speakers.len()
        {
            return Err(SingError::Metadata(
                "DiffSinger speaker names must be unique".into(),
            ));
        }
        let languages = if config.use_lang_id {
            let path = resource_path(boundary, owner, &config.languages, automatic, &mut safe)?;
            let languages: BTreeMap<String, i64> =
                serde_json::from_str(&read_text_file(&path, "DiffSinger languages")?)
                    .map_err(|error| SingError::Metadata(error.to_string()))?;
            if languages.is_empty()
                || languages.len() > MAX_COLLECTION_ITEMS
                || languages.iter().any(|(name, id)| {
                    name.len() > MAX_NAME_BYTES || !(0..MAX_COLLECTION_ITEMS as i64).contains(id)
                })
            {
                return Err(SingError::Metadata(
                    "DiffSinger language IDs or names exceed their limits".into(),
                ));
            }
            Some(languages)
        } else {
            None
        };
        Ok(Self {
            speakers,
            embeddings,
            languages,
            safe,
        })
    }

    pub(super) fn add_languages(
        &self,
        inputs: &mut Inputs<'_>,
        score: &DiffScore,
        symbols: &[String],
    ) -> Result<(), SingError> {
        if let Some(languages) = &self.languages {
            let ids: Vec<_> = score
                .tokens
                .iter()
                .map(|id| {
                    let language = symbols[*id as usize]
                        .split_once('/')
                        .map_or("", |(language, _)| language);
                    languages.get(language).copied().unwrap_or(0)
                })
                .collect();
            inputs.push((
                "languages".into(),
                Tensor::from_array(([1, ids.len()], ids))
                    .map_err(inference)?
                    .into(),
            ));
        }
        Ok(())
    }

    pub(super) fn add_inputs(
        &self,
        inputs: &mut Inputs<'_>,
        score: &DiffScore,
        speaker: u32,
    ) -> Result<(), SingError> {
        if let Some(vector) = self.embeddings.get(speaker as usize) {
            let count = checked_product(
                score.f0.len(),
                vector.len(),
                "DiffSinger speaker tensor",
                (MAX_CHUNK_FRAMES + 2 * crate::score::CHUNK_PAD_FRAMES) * MAX_COLLECTION_ITEMS,
            )?;
            let values: Vec<_> = vector.iter().copied().cycle().take(count).collect();
            inputs.push((
                "spk_embed".into(),
                Tensor::from_array(([1, score.f0.len(), vector.len()], values))
                    .map_err(inference)?
                    .into(),
            ));
        }
        Ok(())
    }
}

pub(super) struct VarianceBackend {
    linguistic: Session,
    predictor: Session,
    symbols: Vec<String>,
    resources: Resources,
    continuous: bool,
    on_gpu: bool,
    safe: bool,
    channels: Vec<String>,
    vowels: Option<BTreeSet<String>>,
}

#[derive(Deserialize)]
struct Dictionary {
    symbols: Vec<DictionarySymbol>,
}

#[derive(Deserialize)]
struct DictionarySymbol {
    symbol: String,
    #[serde(rename = "type")]
    kind: String,
}

impl VarianceBackend {
    pub(super) fn load(
        root: &Path,
        acoustic: &DsConfig,
        acceleration: Acceleration,
        automatic: bool,
    ) -> Result<Self, SingError> {
        let relative = if root.join("dsvariance/dsconfig.yaml").is_file() {
            "dsvariance/dsconfig.yaml"
        } else {
            "dsconfig.yaml"
        };
        let path = root.join(relative);
        let mut safe = true;
        let checked = resource_path(root, root, relative, automatic, &mut safe)?;
        let config: DsConfig =
            serde_yaml_ng::from_str(&read_text_file(&checked, "DiffSinger variance config")?)
                .map_err(|error| load_error(error.to_string()))?;
        if config.sample_rate != acoustic.sample_rate || config.hop_size != acoustic.hop_size {
            return Err(SingError::Metadata(
                "DiffSinger variance and acoustic frame clocks must match".into(),
            ));
        }
        let owner = path
            .parent()
            .ok_or_else(|| load_error("variance config has no parent"))?;
        let phonemes = resource_path(root, owner, &config.phonemes, automatic, &mut safe)?;
        let linguistic_path = resource_path(root, owner, &config.linguistic, automatic, &mut safe)?;
        let predictor_path = resource_path(root, owner, &config.variance, automatic, &mut safe)?;
        let symbols = read_lines(&phonemes)?;
        if !symbols.iter().any(|symbol| symbol == "SP") {
            return Err(SingError::Metadata(
                "DiffSinger variance vocabulary has no SP token".into(),
            ));
        }
        let resources = Resources::load(root, owner, &config, automatic)?;
        safe &= resources.safe;
        let vowels = if owner.join("dsdict.yaml").is_file() {
            let dictionary_path = resource_path(root, owner, "dsdict.yaml", automatic, &mut safe)?;
            let dictionary: Dictionary = serde_yaml_ng::from_str(&read_text_file(
                &dictionary_path,
                "DiffSinger vowel dictionary",
            )?)
            .map_err(|error| SingError::Metadata(error.to_string()))?;
            if dictionary.symbols.len() > MAX_COLLECTION_ITEMS
                || dictionary.symbols.iter().any(|symbol| {
                    symbol.symbol.len() > crate::limits::MAX_TOKEN_BYTES
                        || symbol.kind.len() > MAX_NAME_BYTES
                })
            {
                return Err(SingError::Metadata(
                    "DiffSinger vowel dictionary exceeds its limits".into(),
                ));
            }
            Some(
                dictionary
                    .symbols
                    .into_iter()
                    .filter(|symbol| symbol.kind == "vowel")
                    .map(|symbol| symbol.symbol)
                    .collect(),
            )
        } else {
            None
        };
        let (linguistic, linguistic_gpu) = open_session(&linguistic_path, acceleration)?;
        let (predictor, predictor_gpu) = open_session(&predictor_path, acceleration)?;
        let channels: Vec<_> = ["energy", "breathiness", "voicing", "tension"]
            .into_iter()
            .filter(|name| predictor.inputs.iter().any(|input| input.name == *name))
            .map(str::to_owned)
            .collect();
        for name in acoustic.variance_names() {
            if !channels.iter().any(|channel| channel == name)
                || !predictor
                    .outputs
                    .iter()
                    .any(|output| output.name == format!("{name}_pred"))
            {
                return Err(SingError::Metadata(format!(
                    "DiffSinger variance model cannot predict required {name}"
                )));
            }
        }
        Ok(Self {
            linguistic,
            predictor,
            symbols,
            resources,
            continuous: config.use_continuous_acceleration,
            on_gpu: linguistic_gpu || predictor_gpu,
            safe,
            channels,
            vowels,
        })
    }

    pub(super) fn safe(&self) -> bool {
        self.safe
    }
    pub(super) fn on_gpu(&self) -> bool {
        self.on_gpu
    }

    pub(super) fn predict(
        &mut self,
        frames: &SingerFrames,
        range: std::ops::Range<usize>,
        speaker: Option<&str>,
    ) -> Result<BTreeMap<String, Vec<f32>>, SingError> {
        let score = arrange(frames, range, &self.symbols)?;
        let n = score.tokens.len();
        let count = score.f0.len();
        let mut inputs =
            ort::inputs!["tokens" => Tensor::from_array(([1, n], score.tokens.clone()))?]
                .map_err(inference)?;
        if self
            .linguistic
            .inputs
            .iter()
            .any(|input| input.name == "word_div")
        {
            let (div, dur) = word_groups(&score, &self.symbols, self.vowels.as_ref());
            inputs.push((
                "word_div".into(),
                Tensor::from_array(([1, div.len()], div))
                    .map_err(inference)?
                    .into(),
            ));
            inputs.push((
                "word_dur".into(),
                Tensor::from_array(([1, dur.len()], dur))
                    .map_err(inference)?
                    .into(),
            ));
        } else {
            inputs.push((
                "ph_dur".into(),
                Tensor::from_array(([1, n], score.durations.clone()))
                    .map_err(inference)?
                    .into(),
            ));
        }
        self.resources
            .add_languages(&mut inputs, &score, &self.symbols)?;
        let encoded = self.linguistic.run(inputs).map_err(inference)?;
        let encoder = encoded.get("encoder_out").ok_or_else(|| {
            SingError::Inference("DiffSinger linguistic model has no encoder_out".into())
        })?;
        let (shape, values) = encoder.try_extract_raw_tensor::<f32>().map_err(inference)?;
        if shape.len() != 3
            || shape[0] != 1
            || shape[1] != n as i64
            || !(1..=MAX_COLLECTION_ITEMS as i64).contains(&shape[2])
            || values.iter().any(|value| !value.is_finite())
        {
            return Err(SingError::Inference(
                "DiffSinger linguistic output has invalid dimensions or values".into(),
            ));
        }
        let pitch: Vec<_> = score
            .f0
            .iter()
            .map(|f0| 69.0 + 12.0 * (f0 / 440.0).log2())
            .collect();
        let mut inputs = ort::inputs![
            "encoder_out" => Tensor::from_array((shape.to_vec(), values.to_vec()))?,
            "ph_dur" => Tensor::from_array(([1, n], score.durations.clone()))?,
            "pitch" => Tensor::from_array(([1, count], pitch))?,
            "retake" => Tensor::from_array(([1, count, self.channels.len()], vec![true; count * self.channels.len()]))?,
        ].map_err(inference)?;
        for name in &self.channels {
            inputs.push((
                name.clone().into(),
                Tensor::from_array(([1, count], vec![0.0_f32; count]))
                    .map_err(inference)?
                    .into(),
            ));
        }
        let (name, value) = if self.continuous {
            ("steps", DEFAULT_STEPS)
        } else {
            ("speedup", 50)
        };
        inputs.push((
            name.into(),
            Tensor::from_array((control_shape(&self.predictor, name)?, vec![value]))
                .map_err(inference)?
                .into(),
        ));
        if !self.resources.speakers.is_empty() {
            let speaker = matching_speaker(&self.resources.speakers, speaker)?;
            self.resources
                .add_inputs(&mut inputs, &score, speaker as u32)?;
        }
        let outputs = self.predictor.run(inputs).map_err(inference)?;
        let mut curves = BTreeMap::new();
        for name in &self.channels {
            let output_name = format!("{name}_pred");
            let output = outputs.get(&output_name).ok_or_else(|| {
                SingError::Inference(format!("DiffSinger variance model has no {output_name}"))
            })?;
            let (shape, values) = output.try_extract_raw_tensor::<f32>().map_err(inference)?;
            if shape != [1, count as i64]
                || values.len() != count
                || values.iter().any(|value| !value.is_finite())
            {
                return Err(SingError::Inference(format!(
                    "DiffSinger {output_name} must contain {count} finite frames"
                )));
            }
            curves.insert(name.clone(), values.to_vec());
        }
        Ok(curves)
    }
}

fn matching_speaker(speakers: &[String], wanted: Option<&str>) -> Result<usize, SingError> {
    if let Some(wanted) = wanted {
        if let Some(index) = speakers.iter().position(|name| name == wanted) {
            return Ok(index);
        }
        // Exporters prefix embedding files with the acoustic/variance model's distinct name.
        fn suffix(name: &str) -> &str {
            name.rsplit('/')
                .next()
                .unwrap_or(name)
                .rsplit('.')
                .next()
                .unwrap_or(name)
        }
        let wanted = suffix(wanted);
        let mut matches = speakers
            .iter()
            .enumerate()
            .filter(|(_, name)| suffix(name) == wanted);
        if let Some((index, _)) = matches.next() {
            if matches.next().is_none() {
                return Ok(index);
            }
            return Err(SingError::Metadata(
                "DiffSinger variance speaker suffix is ambiguous".into(),
            ));
        }
    }
    if speakers.len() == 1 {
        return Ok(0);
    }
    Err(SingError::Metadata(
        "DiffSinger acoustic speaker has no matching variance embedding".into(),
    ))
}

fn word_groups(
    score: &DiffScore,
    symbols: &[String],
    vowels: Option<&BTreeSet<String>>,
) -> (Vec<i64>, Vec<i64>) {
    let mut starts = vec![0];
    for (at, id) in score.tokens.iter().enumerate().skip(1) {
        let token = symbols[*id as usize].rsplit('/').next().unwrap_or("");
        let vowel = vowels.map_or_else(
            || is_syllabic(token) || ["u", "N", "cl"].contains(&token),
            |vowels| vowels.contains(&symbols[*id as usize]) || vowels.contains(token),
        );
        if vowel {
            starts.push(at);
        }
    }
    starts.push(score.tokens.len());
    let div = starts
        .windows(2)
        .map(|pair| (pair[1] - pair[0]) as i64)
        .collect();
    let dur = starts
        .windows(2)
        .map(|pair| score.durations[pair[0]..pair[1]].iter().sum())
        .collect();
    (div, dur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auxiliary_speakers_match_export_names_without_guessing_ambiguous_suffixes() {
        let speakers = ["variance.bob", "variance.alice"].map(str::to_owned);
        assert_eq!(
            matching_speaker(&speakers, Some("acoustic.alice")).unwrap(),
            1
        );
        assert_eq!(
            matching_speaker(&speakers, Some("variance.bob")).unwrap(),
            0
        );
        assert!(matching_speaker(&speakers, Some("acoustic.unknown")).is_err());
        let ambiguous = ["one.alice", "two.alice"].map(str::to_owned);
        assert!(matching_speaker(&ambiguous, Some("acoustic.alice")).is_err());
    }

    #[test]
    fn word_mode_uses_dictionary_vowels_and_preserves_both_totals() {
        let symbols = ["SP", "k", "ai", "sh", "u"].map(str::to_owned);
        let score = DiffScore {
            tokens: vec![0, 1, 2, 3, 4, 0],
            durations: vec![25, 5, 40, 5, 40, 25],
            f0: vec![],
        };
        let vowels = BTreeSet::from(["ai".into(), "u".into()]);
        assert_eq!(
            word_groups(&score, &symbols, Some(&vowels)),
            (vec![2, 2, 2], vec![30, 45, 65])
        );
        assert_eq!(
            word_groups(&score, &symbols, None),
            (vec![4, 2], vec![75, 65])
        );
    }
}
