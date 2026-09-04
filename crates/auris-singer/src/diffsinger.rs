//! OpenUtau-compatible DiffSinger acoustic-model and vocoder inference.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use auris_vocal::{SILENCE, SingerFrames};
use ort::session::Session;
use ort::value::Tensor;
use serde::Deserialize;

use crate::backend::{BackendKind, SingingBackend};
use crate::metadata::{FORMAT_VERSION, VoiceCard, VoiceInfo};
use crate::model::{Acceleration, open_session};
use crate::score::{MAX_CHUNK_FRAMES, chunk_ranges};
use crate::{SingError, validate_frames};

const NAME: &str = "DiffSinger";
const DEFAULT_STEPS: i64 = 20;

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
            speakers: None,
        }
    }
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
}

impl DiffSingerBackend {
    pub(crate) fn load(path: &Path, acceleration: Acceleration) -> Result<Self, SingError> {
        let root = path
            .parent()
            .ok_or_else(|| load_error("dsconfig.yaml has no parent folder"))?;
        let raw = std::fs::read_to_string(path).map_err(|error| load_error(error.to_string()))?;
        let config: DsConfig =
            serde_yaml_ng::from_str(&raw).map_err(|error| load_error(error.to_string()))?;
        validate_config(&config)?;

        let symbols = read_lines(&root.join(&config.phonemes))?;
        if !symbols.iter().any(|symbol| symbol == "SP") {
            return Err(SingError::Metadata(
                "DiffSinger phonemes.txt has no SP silence token".into(),
            ));
        }

        let vocoder_root = if root.join("dsvocoder/vocoder.yaml").is_file() {
            root.join("dsvocoder")
        } else {
            root.join(&config.vocoder)
        };
        let vocoder_config_path = vocoder_root.join("vocoder.yaml");
        let vocoder_raw = std::fs::read_to_string(&vocoder_config_path)
            .map_err(|error| load_error(format!("{}: {error}", vocoder_config_path.display())))?;
        let vocoder_config: VocoderConfig =
            serde_yaml_ng::from_str(&vocoder_raw).map_err(|error| load_error(error.to_string()))?;
        let mel_factor = validate_vocoder(&config, &vocoder_config)?;

        let acoustic_path = root.join(&config.acoustic);
        let vocoder_path = vocoder_root.join(&vocoder_config.model);
        let (acoustic, acoustic_gpu) = open_session(&acoustic_path, acceleration)?;
        let (vocoder, vocoder_gpu) = open_session(&vocoder_path, acceleration)?;
        let display_name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| NAME.into());
        let mut speaker_to_id = BTreeMap::new();
        speaker_to_id.insert(display_name.clone(), 0);
        let info = VoiceInfo {
            format_version: FORMAT_VERSION,
            sample_rate: config.sample_rate,
            hop_length: config.hop_size,
            inter_channels: 0,
            n_speakers: 1,
            symbols,
            speaker_to_id,
            phoneme_durations: None,
            phoneme_levels: None,
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
            on_gpu: acoustic_gpu || vocoder_gpu,
            mel_factor,
        })
    }

    fn sing_chunk(
        &mut self,
        frames: &SingerFrames,
        range: std::ops::Range<usize>,
    ) -> Result<Vec<f32>, SingError> {
        let score = arrange(frames, range, &self.info.symbols)?;
        let token_count = score.tokens.len();
        let frame_count = score.f0.len();
        let refused = |error: ort::Error| SingError::Inference(error.to_string());
        let mut inputs = ort::inputs![
            "tokens" => Tensor::from_array(([1, token_count], score.tokens))?,
            "durations" => Tensor::from_array(([1, token_count], score.durations))?,
            "f0" => Tensor::from_array(([1, frame_count], score.f0.clone()))?,
        ]
        .map_err(refused)?;
        if self.config.use_continuous_acceleration {
            inputs.push((
                "steps".into(),
                Tensor::from_array(([1], vec![DEFAULT_STEPS]))
                    .map_err(refused)?
                    .into(),
            ));
            if self.config.use_variable_depth {
                inputs.push((
                    "depth".into(),
                    Tensor::from_array(([1], vec![1.0_f32]))
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
                Tensor::from_array(([1], vec![speedup]))
                    .map_err(refused)?
                    .into(),
            ));
            if self.config.use_variable_depth {
                inputs.push((
                    "depth".into(),
                    Tensor::from_array(([1], vec![1_000_i64]))
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
            .map(|dimension| *dimension as usize)
            .collect();
        let mel: Vec<f32> = raw_mel
            .iter()
            .map(|value| value * self.mel_factor)
            .collect();
        let vocoder_inputs = ort::inputs![
            "mel" => Tensor::from_array((mel_shape, mel))?,
            "f0" => Tensor::from_array(([1, frame_count], score.f0))?,
        ]
        .map_err(refused)?;
        let output = self.vocoder.run(vocoder_inputs).map_err(refused)?;
        let (_, samples) = output[0].try_extract_raw_tensor::<f32>().map_err(refused)?;
        Ok(samples.to_vec())
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

    fn sing_with(
        &mut self,
        frames: &SingerFrames,
        speaker: u32,
        _seed: u64,
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<Vec<f32>, SingError> {
        validate_frames(frames)?;
        if speaker != 0 {
            return Err(SingError::NoSuchSpeaker { speaker, count: 1 });
        }
        let model_hop = self.info.hop_seconds();
        if (frames.hop_seconds - model_hop).abs() > model_hop * 1e-6 {
            return Err(SingError::HopMismatch {
                frames: frames.hop_seconds,
                model: model_hop,
            });
        }
        let hop = self.config.hop_size as usize;
        let mut out = vec![0.0; frames.len() * hop];
        let chunks = chunk_ranges(frames, MAX_CHUNK_FRAMES);
        let total = chunks.len();
        for (index, range) in chunks.into_iter().enumerate() {
            if !progress(index, total) {
                return Err(SingError::Cancelled);
            }
            let samples = self.sing_chunk(frames, range.clone())?;
            let expected = range.len() * hop;
            if samples.len() != expected {
                return Err(SingError::Inference(format!(
                    "DiffSinger vocoder answered {} samples where {expected} were expected",
                    samples.len()
                )));
            }
            out[range.start * hop..range.end * hop].copy_from_slice(&samples);
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
        f0,
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
}

fn read_lines(path: &Path) -> Result<Vec<String>, SingError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| load_error(format!("{}: {error}", path.display())))?;
    let lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if lines.is_empty() {
        Err(SingError::Metadata(
            "DiffSinger phonemes.txt is empty".into(),
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
    if config.sample_rate == 0 || config.hop_size == 0 || config.num_mel_bins == 0 {
        return Err(SingError::Metadata(
            "DiffSinger audio dimensions must be positive".into(),
        ));
    }
    let unsupported = config.use_energy_embed
        || config.use_breathiness_embed
        || config.use_voicing_embed
        || config.use_tension_embed
        || config.use_lang_id
        || config
            .speakers
            .as_ref()
            .is_some_and(|speakers| !speakers.is_empty());
    if unsupported {
        return Err(SingError::Unsupported {
            backend: NAME,
            reason: "language, speaker, and variance embeddings require auxiliary models".into(),
        });
    }
    Ok(())
}

fn validate_vocoder(acoustic: &DsConfig, vocoder: &VocoderConfig) -> Result<f32, SingError> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn auxiliary_model_voicebanks_are_refused_at_load_time() {
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
        assert!(matches!(
            validate_config(&config),
            Err(SingError::Unsupported { .. })
        ));
    }
}
