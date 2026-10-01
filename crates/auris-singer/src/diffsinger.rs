//! OpenUtau-compatible DiffSinger acoustic-model and vocoder inference.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use auris_vocal::{SILENCE, SingerFrames, SingerScore};
use ort::session::Session;
use ort::value::Tensor;
use ort::value::ValueType;
use serde::Deserialize;

use crate::backend::{BackendKind, SingingBackend};
use crate::limits::{
    MAX_COLLECTION_ITEMS, MAX_MEL_BINS, MAX_NAME_BYTES, MAX_PATH_BYTES, MAX_TOKEN_BYTES,
    automatic_descendant_path, checked_product, checked_sample_count, read_text_file, try_copy_f32,
    try_zeroed_f32, validate_audio_dimensions,
};
use crate::metadata::{VoiceCard, VoiceInfo};
use crate::runtime::{Acceleration, open_session_with_optimization};
use crate::score::{MAX_CHUNK_FRAMES, chunk_ranges};
use crate::{SingError, validate_frames};

const NAME: &str = "DiffSinger";
const DEFAULT_STEPS: i64 = 20;

fn open_session(path: &Path, acceleration: Acceleration) -> Result<(Session, bool), SingError> {
    // ORT 1.20's extended optimizer crashes on the diffusion graphs' control flow.
    // Basic optimization loads the original trained acoustic and variance exports safely.
    open_session_with_optimization(path, acceleration, true)
}

mod variance;
use variance::{Resources, VarianceBackend};

#[derive(Debug, Deserialize)]
#[serde(default)]
struct DsConfig {
    phonemes: String,
    acoustic: String,
    vocoder: String,
    sample_rate: u32,
    hop_size: u32,
    num_mel_bins: usize,
    mel_base: String,
    use_continuous_acceleration: bool,
    #[serde(alias = "use_shallow_diffusion")]
    use_variable_depth: bool,
    use_key_shift_embed: bool,
    use_speed_embed: bool,
    use_energy_embed: bool,
    use_breathiness_embed: bool,
    use_voicing_embed: bool,
    use_tension_embed: bool,
    use_lang_id: bool,
    languages: String,
    hidden_size: usize,
    max_depth: Option<f32>,
    linguistic: String,
    variance: String,
    speakers: Option<Vec<String>>,
}

impl Default for DsConfig {
    fn default() -> Self {
        Self {
            phonemes: "phonemes.txt".into(),
            acoustic: String::new(),
            vocoder: String::new(),
            sample_rate: 44_100,
            hop_size: 512,
            num_mel_bins: 128,
            mel_base: "10".into(),
            use_continuous_acceleration: false,
            use_variable_depth: false,
            use_key_shift_embed: false,
            use_speed_embed: false,
            use_energy_embed: false,
            use_breathiness_embed: false,
            use_voicing_embed: false,
            use_tension_embed: false,
            use_lang_id: false,
            languages: String::new(),
            hidden_size: 256,
            max_depth: None,
            linguistic: String::new(),
            variance: String::new(),
            speakers: None,
        }
    }
}

impl DsConfig {
    fn variance_names(&self) -> Vec<&'static str> {
        [
            ("energy", self.use_energy_embed),
            ("breathiness", self.use_breathiness_embed),
            ("voicing", self.use_voicing_embed),
            ("tension", self.use_tension_embed),
        ]
        .into_iter()
        .filter_map(|(name, enabled)| enabled.then_some(name))
        .collect()
    }

    fn depth(&self) -> f32 {
        self.max_depth
            .unwrap_or(if self.use_continuous_acceleration {
                1.0
            } else {
                1000.0
            })
    }
}

fn control_shape(session: &Session, name: &str) -> Result<Vec<usize>, SingError> {
    match session
        .inputs
        .iter()
        .find(|input| input.name == name)
        .map(|input| &input.input_type)
    {
        Some(ValueType::Tensor { dimensions, .. }) if dimensions.is_empty() => Ok(vec![]),
        Some(ValueType::Tensor { dimensions, .. }) if dimensions == &[1] => Ok(vec![1]),
        _ => Err(SingError::Metadata(format!(
            "DiffSinger {name} input must be scalar or [1]"
        ))),
    }
}

// DiffSinger conditions silence on the neighbouring sung pitch, just as OpenUtau does.
fn continuous_f0(raw: &[f32]) -> Vec<f32> {
    let Some(first) = raw.iter().position(|f0| *f0 > 0.0) else {
        return vec![1.0; raw.len()];
    };
    let mut out = vec![raw[first]; raw.len()];
    let mut previous = first;
    for next in first + 1..raw.len() {
        if raw[next] <= 0.0 {
            continue;
        }
        for (offset, value) in out[previous..=next].iter_mut().enumerate() {
            let fraction = offset as f32 / (next - previous) as f32;
            *value = (raw[previous].log2() + (raw[next].log2() - raw[previous].log2()) * fraction)
                .exp2();
        }
        previous = next;
    }
    out[previous..].fill(raw[previous]);
    out
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct VocoderConfig {
    model: String,
    sample_rate: u32,
    hop_size: u32,
    num_mel_bins: usize,
    mel_base: String,
}

impl Default for VocoderConfig {
    fn default() -> Self {
        Self {
            model: "model.onnx".into(),
            sample_rate: 44_100,
            hop_size: 512,
            num_mel_bins: 128,
            mel_base: "10".into(),
        }
    }
}

/// The two-stage ONNX pipeline used by a DiffSinger voicebank.
pub(crate) struct DiffSingerBackend {
    acoustic: Session,
    vocoder: Session,
    config: DsConfig,
    info: VoiceInfo,
    path: PathBuf,
    acceleration: Acceleration,
    on_gpu: bool,
    mel_factor: f32,
    automatic_access_safe: bool,
    resources: Resources,
    variance: Option<VarianceBackend>,
}

impl DiffSingerBackend {
    pub(crate) fn load(
        path: &Path,
        acceleration: Acceleration,
        automatic: bool,
    ) -> Result<Self, SingError> {
        let root = path
            .parent()
            .ok_or_else(|| load_error("dsconfig.yaml has no parent folder"))?;
        let raw = read_text_file(path, "DiffSinger dsconfig.yaml")?;
        let config: DsConfig =
            serde_yaml_ng::from_str(&raw).map_err(|error| load_error(error.to_string()))?;
        validate_config(&config)?;
        let checked_phonemes = automatic_descendant_path(root, Path::new(&config.phonemes));
        let checked_acoustic = automatic_descendant_path(root, Path::new(&config.acoustic));
        let primary_access_safe = checked_phonemes.is_some() && checked_acoustic.is_some();
        if automatic && !primary_access_safe {
            return Err(unsafe_access(
                "DiffSinger manifest paths must resolve inside the voicebank folder",
            ));
        }
        // Automatic work opens the canonical paths that passed the containment check. Explicit
        // loading retains historical support for absolute and parent-relative manifests.
        let phonemes_path = if automatic {
            checked_phonemes
                .clone()
                .expect("automatic path safety was checked above")
        } else {
            root.join(&config.phonemes)
        };
        let acoustic_path = if automatic {
            checked_acoustic
                .clone()
                .expect("automatic path safety was checked above")
        } else {
            root.join(&config.acoustic)
        };
        let symbols = read_lines(&phonemes_path)?;
        if !symbols.iter().any(|symbol| symbol == "SP") {
            return Err(SingError::Metadata(
                "DiffSinger phonemes.txt has no SP silence token".into(),
            ));
        }
        let mut resources = Resources::load(root, root, &config, automatic)?;
        let variance = if config.variance_names().is_empty() {
            None
        } else {
            Some(VarianceBackend::load(
                root,
                &config,
                acceleration,
                automatic,
            )?)
        };
        if let Some(variance) = &variance {
            resources.safe &= variance.safe();
        }

        let bundled_vocoder = Path::new("dsvocoder/vocoder.yaml");
        let bundled_candidate = root.join(bundled_vocoder);
        let use_bundled = bundled_candidate.is_file();
        let configured_vocoder = Path::new(&config.vocoder).join("vocoder.yaml");
        let checked_vocoder_config = if use_bundled {
            automatic_descendant_path(root, bundled_vocoder)
        } else {
            automatic_descendant_path(root, &configured_vocoder)
        };
        if automatic && checked_vocoder_config.is_none() {
            return Err(unsafe_access(
                "DiffSinger vocoder config must resolve inside the voicebank folder",
            ));
        }
        let vocoder_config_path = if automatic {
            checked_vocoder_config
                .clone()
                .expect("automatic path safety was checked above")
        } else if use_bundled {
            bundled_candidate
        } else {
            root.join(&configured_vocoder)
        };
        let vocoder_root = vocoder_config_path
            .parent()
            .ok_or_else(|| load_error("vocoder.yaml has no parent folder"))?;
        let vocoder_raw = read_text_file(&vocoder_config_path, "DiffSinger vocoder.yaml")?;
        let vocoder_config: VocoderConfig =
            serde_yaml_ng::from_str(&vocoder_raw).map_err(|error| load_error(error.to_string()))?;
        let mel_factor = validate_vocoder(&config, &vocoder_config)?;
        let checked_vocoder_model =
            automatic_descendant_path(vocoder_root, Path::new(&vocoder_config.model));
        if automatic && checked_vocoder_model.is_none() {
            return Err(unsafe_access(
                "DiffSinger vocoder model must resolve inside the vocoder folder",
            ));
        }

        let vocoder_path = if automatic {
            checked_vocoder_model
                .clone()
                .expect("automatic path safety was checked above")
        } else {
            vocoder_root.join(&vocoder_config.model)
        };
        let (acoustic, acoustic_gpu) = open_session(&acoustic_path, acceleration)?;
        let (vocoder, vocoder_gpu) = open_session(&vocoder_path, acceleration)?;
        let display_name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| NAME.into());
        let mut speaker_to_id = BTreeMap::new();
        if resources.speakers.is_empty() {
            speaker_to_id.insert(display_name.clone(), 0);
        } else {
            for (id, name) in resources.speakers.iter().enumerate() {
                speaker_to_id.insert(name.clone(), id as u32);
            }
        }
        let info = VoiceInfo {
            sample_rate: config.sample_rate,
            hop_length: config.hop_size,
            n_speakers: speaker_to_id.len() as u32,
            symbols,
            speaker_to_id,
            voice: Some(VoiceCard {
                name: display_name,
                description: "OpenUtau-compatible DiffSinger voicebank".into(),
                version: String::new(),
                license: String::new(),
                credits: Vec::new(),
                url: String::new(),
            }),
        };
        Ok(Self {
            acoustic,
            vocoder,
            config,
            info,
            path: path.to_path_buf(),
            acceleration,
            on_gpu: acoustic_gpu
                || vocoder_gpu
                || variance.as_ref().is_some_and(VarianceBackend::on_gpu),
            mel_factor,
            automatic_access_safe: primary_access_safe
                && checked_vocoder_config.is_some()
                && checked_vocoder_model.is_some()
                && resources.safe,
            resources,
            variance,
        })
    }

    fn sing_chunk(
        &mut self,
        frames: &SingerFrames,
        range: std::ops::Range<usize>,
        speaker: u32,
    ) -> Result<Vec<f32>, SingError> {
        let score = arrange(frames, range.clone(), &self.info.symbols)?;
        let token_count = score.tokens.len();
        let frame_count = score.f0.len();
        let refused = |error: ort::Error| SingError::Inference(error.to_string());
        let mut inputs = ort::inputs![
            "tokens" => Tensor::from_array(([1, token_count], score.tokens.clone()))?,
            "durations" => Tensor::from_array(([1, token_count], score.durations.clone()))?,
            "f0" => Tensor::from_array(([1, frame_count], score.f0.clone()))?,
        ]
        .map_err(refused)?;
        if let Some(variance) = &mut self.variance {
            let speaker_name = self
                .resources
                .speakers
                .get(speaker as usize)
                .map(String::as_str);
            let curves = variance.predict(frames, range.clone(), speaker_name)?;
            for name in self.config.variance_names() {
                let values = curves.get(name).ok_or_else(|| {
                    SingError::Inference(format!(
                        "DiffSinger variance predictor has no {name} output"
                    ))
                })?;
                inputs.push((
                    name.into(),
                    Tensor::from_array(([1, frame_count], values.clone()))
                        .map_err(refused)?
                        .into(),
                ));
            }
        }
        self.resources
            .add_languages(&mut inputs, &score, &self.info.symbols)?;
        self.resources.add_inputs(&mut inputs, &score, speaker)?;
        if self.config.use_continuous_acceleration {
            inputs.push((
                "steps".into(),
                Tensor::from_array((control_shape(&self.acoustic, "steps")?, vec![DEFAULT_STEPS]))
                    .map_err(refused)?
                    .into(),
            ));
            if self.config.use_variable_depth {
                inputs.push((
                    "depth".into(),
                    Tensor::from_array((
                        control_shape(&self.acoustic, "depth")?,
                        vec![self.config.depth()],
                    ))
                    .map_err(refused)?
                    .into(),
                ));
            }
        } else {
            let mut speedup = (1_000 / DEFAULT_STEPS).max(1);
            while 1_000 % speedup != 0 && speedup > 1 {
                speedup -= 1;
            }
            inputs.push((
                "speedup".into(),
                Tensor::from_array((control_shape(&self.acoustic, "speedup")?, vec![speedup]))
                    .map_err(refused)?
                    .into(),
            ));
            if self.config.use_variable_depth {
                inputs.push((
                    "depth".into(),
                    Tensor::from_array((
                        control_shape(&self.acoustic, "depth")?,
                        vec![self.config.depth() as i64],
                    ))
                    .map_err(refused)?
                    .into(),
                ));
            }
        }
        if self.config.use_key_shift_embed {
            inputs.push((
                "gender".into(),
                Tensor::from_array(([1, frame_count], vec![0.0_f32; frame_count]))
                    .map_err(refused)?
                    .into(),
            ));
        }
        if self.config.use_speed_embed {
            inputs.push((
                "velocity".into(),
                Tensor::from_array(([1, frame_count], vec![1.0_f32; frame_count]))
                    .map_err(refused)?
                    .into(),
            ));
        }
        let acoustic = self.acoustic.run(inputs).map_err(refused)?;
        let (mel_shape, raw_mel) = acoustic[0]
            .try_extract_raw_tensor::<f32>()
            .map_err(refused)?;
        let mel_shape: Vec<usize> = mel_shape
            .iter()
            .map(|dimension| {
                usize::try_from(*dimension).map_err(|_| {
                    SingError::Inference(
                        "DiffSinger acoustic model returned a negative mel axis".into(),
                    )
                })
            })
            .collect::<Result<_, _>>()?;
        let mel_count = checked_product(
            frame_count,
            self.config.num_mel_bins,
            "DiffSinger mel output",
            (MAX_CHUNK_FRAMES + 2 * crate::score::CHUNK_PAD_FRAMES) * MAX_MEL_BINS,
        )?;
        let shape_count = mel_shape
            .iter()
            .try_fold(1usize, |product, dimension| product.checked_mul(*dimension));
        if mel_shape != [1, frame_count, self.config.num_mel_bins]
            || shape_count != Some(mel_count)
            || raw_mel.len() != mel_count
            || raw_mel.iter().any(|value| !value.is_finite())
        {
            return Err(SingError::Inference(format!(
                "DiffSinger acoustic model answered {} mel values where {mel_count} finite values were expected",
                raw_mel.len()
            )));
        }
        let mut mel = Vec::new();
        mel.try_reserve_exact(mel_count)
            .map_err(|_| SingError::Allocation {
                resource: "DiffSinger mel input",
            })?;
        mel.extend(raw_mel.iter().map(|value| value * self.mel_factor));
        let vocoder_inputs = ort::inputs![
            "mel" => Tensor::from_array((mel_shape, mel))?,
            "f0" => Tensor::from_array(([1, frame_count], score.f0))?,
        ]
        .map_err(refused)?;
        let output = self.vocoder.run(vocoder_inputs).map_err(refused)?;
        let (_, samples) = output[0].try_extract_raw_tensor::<f32>().map_err(refused)?;
        let expected = checked_sample_count(
            frame_count,
            self.config.hop_size as usize,
            "DiffSinger chunk audio",
        )?;
        if samples.len() != expected || samples.iter().any(|sample| !sample.is_finite()) {
            return Err(SingError::Inference(format!(
                "DiffSinger vocoder answered {} samples where {expected} finite samples were expected",
                samples.len()
            )));
        }
        let mut samples = try_copy_f32(samples, "DiffSinger chunk audio")?;
        let hop = self.config.hop_size as usize;
        // Dynamics are musical controls; predicted acoustic energy keeps its own scale.
        for (index, sample) in samples.iter_mut().enumerate() {
            let at = range.start + index / hop;
            let next = (at + 1).min(range.end - 1);
            let fraction = (index % hop) as f32 / hop as f32;
            *sample *= frames.energy[at] + (frames.energy[next] - frames.energy[at]) * fraction;
        }
        Ok(samples)
    }
}

impl SingingBackend for DiffSingerBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::DiffSinger
    }
    fn info(&self) -> &VoiceInfo {
        &self.info
    }
    fn acceleration(&self) -> Acceleration {
        self.acceleration
    }
    fn on_gpu(&self) -> bool {
        self.on_gpu
    }
    fn path(&self) -> &Path {
        &self.path
    }
    fn automatic_access_safe(&self) -> bool {
        self.automatic_access_safe
    }

    fn sing_with(
        &mut self,
        frames: &SingerFrames,
        _score: Option<&SingerScore>,
        speaker: u32,
        _seed: u64,
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<Vec<f32>, SingError> {
        validate_frames(frames)?;
        if speaker >= self.info.n_speakers {
            return Err(SingError::NoSuchSpeaker {
                speaker,
                count: self.info.n_speakers,
            });
        }
        if frames.inventory.first().map(String::as_str) != Some(SILENCE) {
            return Err(SingError::Inference(
                "DiffSinger frame inventory must start with silence".into(),
            ));
        }
        let model_hop = self.info.hop_seconds();
        if (frames.hop_seconds - model_hop).abs() > model_hop * 1e-6 {
            return Err(SingError::HopMismatch {
                frames: frames.hop_seconds,
                model: model_hop,
            });
        }
        let hop = self.config.hop_size as usize;
        let length = checked_sample_count(frames.len(), hop, "DiffSinger rendered audio")?;
        let mut out = try_zeroed_f32(length, "DiffSinger rendered audio")?;
        let chunks = chunk_ranges(frames, MAX_CHUNK_FRAMES);
        let total = chunks.len();
        for (index, range) in chunks.into_iter().enumerate() {
            if !progress(index, total) {
                return Err(SingError::Cancelled);
            }
            let samples = match self.sing_chunk(frames, range.clone(), speaker) {
                Ok(samples) => samples,
                Err(error) if self.on_gpu && self.acceleration == Acceleration::Auto => {
                    log::warn!("the GPU refused DiffSinger ({error}); retrying on CPU");
                    let mut cpu =
                        Self::load(&self.path, Acceleration::Cpu, self.automatic_access_safe)?;
                    cpu.acceleration = Acceleration::Auto;
                    *self = cpu;
                    self.sing_chunk(frames, range.clone(), speaker)?
                }
                Err(error) => return Err(error),
            };
            let expected = checked_sample_count(range.len(), hop, "DiffSinger chunk audio")?;
            if samples.len() != expected {
                return Err(SingError::Inference(format!(
                    "DiffSinger vocoder answered {} samples where {expected} were expected",
                    samples.len()
                )));
            }
            let start = checked_sample_count(range.start, hop, "DiffSinger render offset")?;
            let end = checked_sample_count(range.end, hop, "DiffSinger render offset")?;
            out[start..end].copy_from_slice(&samples);
        }
        if !progress(total, total) {
            return Err(SingError::Cancelled);
        }
        Ok(out)
    }
}

#[derive(Debug, PartialEq)]
struct DiffScore {
    tokens: Vec<i64>,
    durations: Vec<i64>,
    f0: Vec<f32>,
}

fn arrange(
    frames: &SingerFrames,
    range: std::ops::Range<usize>,
    symbols: &[String],
) -> Result<DiffScore, SingError> {
    let silence = symbols
        .iter()
        .position(|symbol| symbol == "SP")
        .expect("validated") as i64;
    let ids: Vec<Option<i64>> = frames
        .inventory
        .iter()
        .map(|symbol| {
            if symbol == SILENCE {
                Some(silence)
            } else {
                diffsinger_symbol(symbol, symbols).map(|id| id as i64)
            }
        })
        .collect();
    let mut tokens = Vec::new();
    let mut durations = Vec::new();
    let mut f0 = Vec::with_capacity(range.len());
    for at in range {
        let entry = frames.phonemes[at] as usize;
        let token = ids.get(entry).copied().flatten().ok_or_else(|| {
            let symbol = frames
                .inventory
                .get(entry)
                .map_or("<invalid>", String::as_str);
            SingError::Inference(format!(
                "DiffSinger phonemes.txt does not contain `{symbol}` or its Japanese alias"
            ))
        })?;
        if tokens.last() == Some(&token) {
            *durations
                .last_mut()
                .expect("tokens and durations are paired") += 1;
        } else {
            tokens.push(token);
            durations.push(1);
        }
        f0.push(frames.f0_hz[at]);
    }
    Ok(DiffScore {
        tokens,
        durations,
        f0: continuous_f0(&f0),
    })
}

/// Resolves Auris' IPA spelling to a voicebank token, preserving an exact match first.
fn diffsinger_symbol(symbol: &str, symbols: &[String]) -> Option<usize> {
    let alias = match symbol {
        "ɯ" | "ɯ̥" => "u",
        "ḁ" => "a",
        "i̥" => "i",
        "e̥" => "e",
        "o̥" => "o",
        "ɴ" => "N",
        "ɾ" => "r",
        "ɸ" => "f",
        "ɸʲ" => "fy",
        "ɕ" => "sh",
        "tɕ" => "ch",
        "dʑ" => "j",
        "ç" => "hy",
        "kʲ" => "ky",
        "gʲ" => "gy",
        "tʲ" => "ty",
        "dʲ" => "dy",
        "nʲ" => "ny",
        "mʲ" => "my",
        "ɾʲ" => "ry",
        "bʲ" => "by",
        "pʲ" => "py",
        other => other,
    };
    symbols
        .iter()
        .position(|known| known == symbol)
        .or_else(|| symbols.iter().position(|known| known == alias))
        .or_else(|| {
            symbols
                .iter()
                .position(|known| known.strip_prefix("ja/") == Some(alias))
        })
}

fn read_lines(path: &Path) -> Result<Vec<String>, SingError> {
    let text = read_text_file(path, "DiffSinger phoneme dictionary")?;
    let lines: Vec<String> = if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
    {
        let tokens: BTreeMap<String, usize> = serde_json::from_str(&text).map_err(|error| {
            SingError::Metadata(format!("invalid DiffSinger phoneme JSON: {error}"))
        })?;
        let maximum = tokens.values().copied().max().unwrap_or(0);
        if tokens.is_empty() || maximum >= MAX_COLLECTION_ITEMS {
            return Err(SingError::Metadata(
                "DiffSinger phoneme IDs must be in 0..4096".into(),
            ));
        }
        // Zero is reserved padding in modern exports. Sparse IDs must never be renumbered.
        let mut symbols = vec![String::new(); maximum + 1];
        for (symbol, id) in tokens {
            if symbol.is_empty() || !symbols[id].is_empty() {
                return Err(SingError::Metadata(
                    "DiffSinger phoneme IDs must be unique and tokens nonempty".into(),
                ));
            }
            symbols[id] = symbol;
        }
        symbols
    } else {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    };
    if lines.is_empty()
        || lines.len() > MAX_COLLECTION_ITEMS
        || lines.iter().any(|line| {
            line.len() > MAX_TOKEN_BYTES
                || (!line.is_empty() && line.split_whitespace().count() != 1)
        })
    {
        Err(SingError::Metadata(
            "DiffSinger phoneme dictionary must contain 1..=4096 tokens of at most 256 UTF-8 bytes"
                .into(),
        ))
    } else {
        Ok(lines)
    }
}

fn validate_config(config: &DsConfig) -> Result<(), SingError> {
    if config.phonemes.is_empty() || config.acoustic.is_empty() || config.vocoder.is_empty() {
        return Err(SingError::Metadata(
            "DiffSinger dsconfig.yaml must name phonemes, acoustic, and vocoder".into(),
        ));
    }
    if [
        config.phonemes.as_str(),
        config.acoustic.as_str(),
        config.vocoder.as_str(),
    ]
    .iter()
    .any(|value| value.len() > MAX_PATH_BYTES)
        || config.mel_base.len() > MAX_NAME_BYTES
        || config.speakers.as_ref().is_some_and(|speakers| {
            speakers.len() > MAX_COLLECTION_ITEMS
                || speakers
                    .iter()
                    .any(|speaker| speaker.trim().is_empty() || speaker.len() > MAX_NAME_BYTES)
        })
    {
        return Err(SingError::Metadata(
            "DiffSinger config strings or lists exceed their practical limits".into(),
        ));
    }
    validate_audio_dimensions(config.sample_rate, config.hop_size, "DiffSinger")?;
    if !(1..=MAX_MEL_BINS).contains(&config.num_mel_bins) {
        return Err(SingError::Metadata(format!(
            "DiffSinger num_mel_bins must be 1..={MAX_MEL_BINS}"
        )));
    }
    if !(1..=MAX_COLLECTION_ITEMS).contains(&config.hidden_size)
        || config.max_depth.is_some_and(|depth| {
            !depth.is_finite()
                || depth <= 0.0
                || depth
                    > if config.use_continuous_acceleration {
                        1.0
                    } else {
                        1000.0
                    }
        })
    {
        return Err(SingError::Metadata(
            "DiffSinger hidden_size or max_depth is outside its valid range".into(),
        ));
    }
    Ok(())
}

fn validate_vocoder(acoustic: &DsConfig, vocoder: &VocoderConfig) -> Result<f32, SingError> {
    if vocoder.model.is_empty()
        || vocoder.model.len() > MAX_PATH_BYTES
        || vocoder.mel_base.len() > MAX_NAME_BYTES
    {
        return Err(SingError::Metadata(
            "DiffSinger vocoder config strings exceed their practical limits".into(),
        ));
    }
    validate_audio_dimensions(vocoder.sample_rate, vocoder.hop_size, "DiffSinger vocoder")?;
    if !(1..=MAX_MEL_BINS).contains(&vocoder.num_mel_bins) {
        return Err(SingError::Metadata(format!(
            "DiffSinger vocoder num_mel_bins must be 1..={MAX_MEL_BINS}"
        )));
    }
    if acoustic.sample_rate != vocoder.sample_rate
        || acoustic.hop_size != vocoder.hop_size
        || acoustic.num_mel_bins != vocoder.num_mel_bins
    {
        return Err(SingError::Metadata(
            "DiffSinger acoustic model and vocoder audio dimensions do not match".into(),
        ));
    }
    match (acoustic.mel_base.as_str(), vocoder.mel_base.as_str()) {
        ("10", "10") | ("e", "e") => Ok(1.0),
        ("10", "e") => Ok(std::f32::consts::LN_10),
        ("e", "10") => Ok(std::f32::consts::LOG10_E),
        _ => Err(SingError::Metadata(
            "DiffSinger mel_base must be either `10` or `e`".into(),
        )),
    }
}

fn load_error(reason: impl Into<String>) -> SingError {
    SingError::Load {
        reason: reason.into(),
    }
}

fn unsafe_access(reason: impl Into<String>) -> SingError {
    SingError::UnsafeAutomaticAccess {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn valid_config() -> DsConfig {
        DsConfig {
            phonemes: "phonemes.txt".into(),
            acoustic: "acoustic.onnx".into(),
            vocoder: "vocoder".into(),
            ..DsConfig::default()
        }
    }

    fn temp_root() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        crate::limits::test_temp_dir().join(format!(
            "auris-diffsinger-policy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn frames_are_run_length_encoded_in_the_diffsinger_vocabulary() {
        let frames = SingerFrames {
            hop_seconds: 0.01,
            inventory: vec![SILENCE.into(), "a".into(), "missing".into()],
            phonemes: vec![0, 1, 1, 2],
            f0_hz: vec![0.0, 220.0, 220.0, 220.0],
            energy: vec![0.0; 4],
        };
        let error = arrange(&frames, 0..4, &["SP".into(), "a".into()]).unwrap_err();
        assert!(error.to_string().contains("missing"));

        let score = arrange(&frames, 0..3, &["SP".into(), "a".into()]).unwrap();
        assert_eq!(score.tokens, [0, 1]);
        assert_eq!(score.durations, [1, 2]);
        assert_eq!(score.f0.len(), 3);
    }

    #[test]
    fn json_phonemes_preserve_explicit_nonzero_ids() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("phonemes.json");
        std::fs::write(&path, r#"{"a":5,"SP":4,"k":35}"#).unwrap();
        let symbols = read_lines(&path).unwrap();
        assert_eq!(symbols[4], "SP");
        assert_eq!(symbols[5], "a");
        assert_eq!(symbols[35], "k");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn japanese_ipa_uses_openutau_diffsinger_aliases() {
        let symbols = vec![
            "SP".into(),
            "u".into(),
            "N".into(),
            "sh".into(),
            "ry".into(),
        ];
        for (ipa, expected) in [("ɯ", 1), ("ɴ", 2), ("ɕ", 3), ("ɾʲ", 4)] {
            assert_eq!(diffsinger_symbol(ipa, &symbols), Some(expected));
        }
    }

    #[test]
    fn multilingual_japanese_tokens_preserve_exact_matches() {
        let symbols = vec!["SP".into(), "ja/u".into(), "ja/sh".into(), "ɕ".into()];
        assert_eq!(diffsinger_symbol("ɯ", &symbols), Some(1));
        assert_eq!(diffsinger_symbol("ɕ", &symbols), Some(3));
        assert_eq!(diffsinger_symbol("ja/sh", &symbols), Some(2));
    }

    #[test]
    fn pitch_gaps_are_continuous_in_semitone_space() {
        assert_eq!(
            continuous_f0(&[0.0, 220.0, 0.0, 880.0, 0.0]),
            [220.0, 220.0, 440.0, 880.0, 880.0]
        );
    }

    #[test]
    fn malformed_json_ids_and_diffusion_depth_are_rejected() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("phonemes.json");
        for text in [r#"{"a":4096}"#, r#"{"SP":4,"a":4}"#, r#"{"a":-1}"#, r#"{}"#] {
            std::fs::write(&path, text).unwrap();
            assert!(read_lines(&path).is_err(), "{text}");
        }
        for depth in [0.0, f32::NAN, 1.01] {
            let mut config = valid_config();
            config.use_continuous_acceleration = true;
            config.max_depth = Some(depth);
            assert!(validate_config(&config).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn variance_config_is_accepted_before_auxiliary_models_are_loaded() {
        let config = DsConfig {
            phonemes: "phonemes.txt".into(),
            acoustic: "acoustic.onnx".into(),
            vocoder: "vocoder".into(),
            sample_rate: 44_100,
            hop_size: 512,
            num_mel_bins: 128,
            use_energy_embed: true,
            ..DsConfig::default()
        };
        validate_config(&config).unwrap();
    }

    #[test]
    fn config_audio_and_collection_boundaries_are_explicit() {
        let mut config = valid_config();
        config.sample_rate = 8_000;
        config.hop_size = 8;
        config.num_mel_bins = MAX_MEL_BINS;
        validate_config(&config).expect("inclusive lower clock and mel upper bound");

        config.sample_rate = 192_000;
        config.hop_size = 19_200;
        validate_config(&config).expect("inclusive upper clock boundary");
        config.hop_size += 1;
        assert!(validate_config(&config).is_err());
        config = valid_config();
        config.num_mel_bins = MAX_MEL_BINS + 1;
        assert!(validate_config(&config).is_err());
        config = valid_config();
        config.acoustic = "x".repeat(MAX_PATH_BYTES + 1);
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn automatic_load_rejects_manifest_escape_before_opening_children() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("dsconfig.yaml");
        std::fs::write(
            &path,
            "phonemes: ../outside.txt\nacoustic: acoustic.onnx\nvocoder: vocoder\n",
        )
        .unwrap();
        let error = match crate::VoiceModel::load_for_automatic_access(&path, Acceleration::Cpu) {
            Err(error) => error,
            Ok(_) => panic!("parent traversal must be rejected before phonemes.txt is opened"),
        };
        assert!(matches!(error, SingError::UnsafeAutomaticAccess { .. }));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatic_load_rejects_nested_vocoder_escape_before_model_open() {
        let root = temp_root();
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("vocoder")).unwrap();
        std::fs::write(root.join("phonemes.txt"), "SP\na\n").unwrap();
        std::fs::write(
            root.join("dsconfig.yaml"),
            "phonemes: phonemes.txt\nacoustic: acoustic.onnx\nvocoder: vocoder\n",
        )
        .unwrap();
        std::fs::write(
            root.join("vocoder/vocoder.yaml"),
            "model: ../outside.onnx\n",
        )
        .unwrap();
        let error = match crate::VoiceModel::load_for_automatic_access(
            &root.join("dsconfig.yaml"),
            Acceleration::Cpu,
        ) {
            Err(error) => error,
            Ok(_) => panic!("vocoder traversal must be rejected before acoustic.onnx is opened"),
        };
        assert!(matches!(error, SingError::UnsafeAutomaticAccess { .. }));
        std::fs::remove_dir_all(root).unwrap();
    }
}
