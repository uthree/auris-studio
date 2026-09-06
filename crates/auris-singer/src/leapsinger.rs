//! LeapSinger's acoustic ONNX exports followed by the NHVSing ONNX vocoder.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use auris_vocal::{SILENCE, SingerFrames, SingerScore, is_voiceless};
use ort::session::Session;
use ort::tensor::TensorElementType;
use ort::value::{Tensor, ValueType};
use serde::Deserialize;

use crate::backend::{BackendKind, SingingBackend};
use crate::metadata::{FORMAT_VERSION, VoiceCard, VoiceInfo};
use crate::model::{Acceleration, open_session};
use crate::score::{MAX_CHUNK_FRAMES, chunk_ranges};
use crate::{SingError, validate_frames};

const NAME: &str = "LeapSinger";

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Variant {
    #[default]
    Full,
    Diffsinger,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    format_version: u32,
    name: String,
    acoustic: PathBuf,
    vocoder: PathBuf,
    phonemes: PathBuf,
    #[serde(default)]
    variant: Variant,
    #[serde(default = "sample_rate")]
    sample_rate: u32,
    #[serde(default = "hop_size")]
    hop_size: u32,
    #[serde(default = "mel_bins")]
    num_mel_bins: usize,
    #[serde(default)]
    speakers: Vec<Speaker>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Speaker {
    name: String,
    embedding: Vec<f32>,
}

fn sample_rate() -> u32 {
    44_100
}

fn hop_size() -> u32 {
    256
}

fn mel_bins() -> usize {
    128
}

pub(crate) struct LeapSingerBackend {
    acoustic: Session,
    vocoder: Session,
    config: Config,
    info: VoiceInfo,
    path: PathBuf,
    acceleration: Acceleration,
    on_gpu: bool,
}

impl LeapSingerBackend {
    pub(crate) fn load(path: &Path, acceleration: Acceleration) -> Result<Self, SingError> {
        let raw = std::fs::read_to_string(path).map_err(|error| load_error(path, error))?;
        let config: Config = serde_json::from_str(&raw)
            .map_err(|error| metadata(format!("invalid manifest: {error}")))?;
        validate_config(&config)?;
        let root = path.parent().unwrap_or_else(|| Path::new("."));
        let phonemes = root.join(&config.phonemes);
        let raw =
            std::fs::read_to_string(&phonemes).map_err(|error| load_error(&phonemes, error))?;
        let symbols = read_symbols(&raw)?;
        let (acoustic, acoustic_gpu) = open_session(&root.join(&config.acoustic), acceleration)?;
        let (vocoder, vocoder_gpu) = open_session(&root.join(&config.vocoder), acceleration)?;
        validate_models(&acoustic, &vocoder, &config)?;
        let speaker_to_id = if config.speakers.is_empty() {
            BTreeMap::from([(config.name.clone(), 0)])
        } else {
            config
                .speakers
                .iter()
                .enumerate()
                .map(|(id, speaker)| (speaker.name.clone(), id as u32))
                .collect()
        };
        let info = VoiceInfo {
            format_version: FORMAT_VERSION,
            sample_rate: config.sample_rate,
            hop_length: config.hop_size,
            inter_channels: 0,
            n_speakers: speaker_to_id.len() as u32,
            symbols,
            speaker_to_id,
            phoneme_durations: None,
            phoneme_levels: None,
            voice: Some(VoiceCard {
                name: config.name.clone(),
                description: "LeapSinger acoustic model with NHVSing vocoder".into(),
                ..VoiceCard::default()
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
        })
    }

    fn sing_chunk(
        &mut self,
        frames: &SingerFrames,
        range: Range<usize>,
        speaker: u32,
    ) -> Result<Vec<f32>, SingError> {
        let mut score = arrange(frames, range.clone(), &self.info.symbols)?;
        // The excitation STFT reflects 896 samples. Short previews still need that context.
        let minimum = if self.config.hop_size == 256 { 4 } else { 2 };
        score.pad_to(minimum);
        let count = score.f0.len();
        let bins = self.config.num_mel_bins;
        let hop = self.config.hop_size as usize;
        let mut inputs = ort::inputs![
            "tokens" => Tensor::from_array(([1, score.tokens.len()], score.tokens))?,
            "durations" => Tensor::from_array(([1, score.durations.len()], score.durations))?,
            "f0" => Tensor::from_array(([1, count], score.f0.clone()))?,
        ]
        .map_err(inference)?;
        if self.config.variant == Variant::Full {
            inputs.push((
                "uv".into(),
                Tensor::from_array(([1, count], score.voiced.clone()))
                    .map_err(inference)?
                    .into(),
            ));
        }
        if let Some(speaker) = self.config.speakers.get(speaker as usize) {
            inputs.push((
                "spk_embed".into(),
                Tensor::from_array(([1, speaker.embedding.len()], speaker.embedding.clone()))
                    .map_err(inference)?
                    .into(),
            ));
        }
        let acoustic = self.acoustic.run(inputs).map_err(inference)?;
        let (shape, values) = acoustic["mel"]
            .try_extract_raw_tensor::<f32>()
            .map_err(inference)?;
        let mut mel = vocoder_mel(shape, values, self.config.variant, count, bins)?;
        let mut unvoiced: Vec<f32> = score.voiced.iter().map(|value| 1.0 - value).collect();
        // NHVSing V3X interpolates T frames to 2*T-1 native frames. One extra frame lets us
        // crop to the exact score length without leaving a half-hop hole at every seam.
        let vocoder_count = if hop == 512 {
            mel.extend_from_within(mel.len() - bins..);
            score.f0.push(*score.f0.last().expect("padded score"));
            unvoiced.push(*unvoiced.last().expect("padded score"));
            count + 1
        } else {
            count
        };
        let inputs = ort::inputs![
            "mel" => Tensor::from_array(([1, vocoder_count, bins], mel))?,
            "f0" => Tensor::from_array(([1, 1, vocoder_count], score.f0))?,
            "uv" => Tensor::from_array(([1, 1, vocoder_count], unvoiced))?,
        ]
        .map_err(inference)?;
        let output = self.vocoder.run(inputs).map_err(inference)?;
        let (shape, samples) = output["waveform"]
            .try_extract_raw_tensor::<f32>()
            .map_err(inference)?;
        let expected = vocoder_count * hop - if hop == 512 { 256 } else { 0 };
        if shape != [1, 1, expected as i64] || samples.len() != expected {
            return Err(SingError::Inference(format!(
                "LeapSinger vocoder returned shape {shape:?}; expected [1, 1, {expected}] (check hop_size)"
            )));
        }
        if samples.iter().any(|value| !value.is_finite()) {
            return Err(SingError::Inference(
                "LeapSinger vocoder returned non-finite audio".into(),
            ));
        }
        let mut samples = samples[..range.len() * hop].to_vec();
        // Neither upstream graph consumes dynamics. Apply Auris' frame gain to the waveform,
        // interpolating between frames so an expression edit introduces no hop-sized steps.
        for (index, sample) in samples.iter_mut().enumerate() {
            let at = range.start + index / hop;
            let next = (at + 1).min(frames.len() - 1);
            let fraction = (index % hop) as f32 / hop as f32;
            let gain = frames.energy[at] + (frames.energy[next] - frames.energy[at]) * fraction;
            *sample *= gain;
        }
        Ok(samples)
    }
}

impl SingingBackend for LeapSingerBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::LeapSinger
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
        _score: Option<&SingerScore>,
        speaker: u32,
        _seed: u64,
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<Vec<f32>, SingError> {
        validate_frames(frames)?;
        validate_curves(frames)?;
        if speaker >= self.info.n_speakers {
            return Err(SingError::NoSuchSpeaker {
                speaker,
                count: self.info.n_speakers,
            });
        }
        let model_hop = self.info.hop_seconds();
        if !frames.hop_seconds.is_finite()
            || (frames.hop_seconds - model_hop).abs() > model_hop * 1e-6
        {
            return Err(SingError::HopMismatch {
                frames: frames.hop_seconds,
                model: model_hop,
            });
        }
        let hop = self.config.hop_size as usize;
        let length = frames
            .len()
            .checked_mul(hop)
            .ok_or_else(|| SingError::Inference("LeapSinger score is too long".into()))?;
        let chunks = chunk_ranges(frames, MAX_CHUNK_FRAMES);
        let total = chunks.len();
        if !progress(0, total) {
            return Err(SingError::Cancelled);
        }
        let mut samples = vec![0.0; length];
        for (index, range) in chunks.into_iter().enumerate() {
            let sung = match self.sing_chunk(frames, range.clone(), speaker) {
                Ok(sung) => sung,
                Err(error) if self.on_gpu && self.acceleration == Acceleration::Auto => {
                    log::warn!("the GPU refused LeapSinger ({error}); retrying on CPU");
                    let root = self.path.parent().unwrap_or_else(|| Path::new("."));
                    let (acoustic, _) =
                        open_session(&root.join(&self.config.acoustic), Acceleration::Cpu)?;
                    let (vocoder, _) =
                        open_session(&root.join(&self.config.vocoder), Acceleration::Cpu)?;
                    self.acoustic = acoustic;
                    self.vocoder = vocoder;
                    self.on_gpu = false;
                    self.sing_chunk(frames, range.clone(), speaker)?
                }
                Err(error) => return Err(error),
            };
            samples[range.start * hop..range.end * hop].copy_from_slice(&sung);
            if !progress(index + 1, total) {
                return Err(SingError::Cancelled);
            }
        }
        Ok(samples)
    }
}

fn validate_config(config: &Config) -> Result<(), SingError> {
    if config.format_version != 1 {
        return Err(metadata("manifest format_version must be 1"));
    }
    if config.name.trim().is_empty()
        || [&config.acoustic, &config.vocoder, &config.phonemes]
            .iter()
            .any(|path| path.as_os_str().is_empty())
    {
        return Err(metadata(
            "name, acoustic, vocoder, and phonemes must not be empty",
        ));
    }
    if config.sample_rate != 44_100
        || !matches!(config.hop_size, 256 | 512)
        || config.num_mel_bins == 0
        || config.num_mel_bins > 1024
        || (config.variant == Variant::Full && config.hop_size != 256)
    {
        return Err(metadata(
            "expected 44100 Hz, hop_size 256 (full) or 256/512 (diffsinger), and 1..=1024 mel bins",
        ));
    }
    let mut names = HashSet::new();
    if config.speakers.len() > 4096
        || config.speakers.iter().any(|speaker| {
            speaker.name.trim().is_empty()
                || !names.insert(&speaker.name)
                || speaker.embedding.is_empty()
                || speaker.embedding.len() > 16_384
                || speaker.embedding.iter().any(|value| !value.is_finite())
        })
    {
        return Err(metadata(
            "speakers require unique nonempty names and finite, nonempty embeddings",
        ));
    }
    if config
        .speakers
        .windows(2)
        .any(|pair| pair[0].embedding.len() != pair[1].embedding.len())
    {
        return Err(metadata("speaker embeddings must have the same length"));
    }
    Ok(())
}

fn read_symbols(raw: &str) -> Result<Vec<String>, SingError> {
    let mut seen = HashSet::new();
    let mut symbols = Vec::new();
    for line in raw.lines() {
        let symbol = line.split('#').next().unwrap_or("").trim();
        if symbol.is_empty() {
            continue;
        }
        if symbol.split_whitespace().count() != 1 || !seen.insert(symbol) {
            return Err(metadata(format!("invalid or duplicate phoneme `{symbol}`")));
        }
        symbols.push(symbol.to_owned());
    }
    if symbols.first().is_none_or(|symbol| symbol != "pau") {
        return Err(metadata(
            "phonemes must start with the silence token pau (id 0)",
        ));
    }
    Ok(symbols)
}

fn validate_models(
    acoustic: &Session,
    vocoder: &Session,
    config: &Config,
) -> Result<(), SingError> {
    use TensorElementType::{Float32, Int64};
    let mut acoustic_inputs = vec![
        ("tokens", Int64, vec![1, -1]),
        ("durations", Int64, vec![1, -1]),
        ("f0", Float32, vec![1, -1]),
    ];
    if config.variant == Variant::Full {
        acoustic_inputs.push(("uv", Float32, vec![1, -1]));
    }
    if let Some(speaker) = config.speakers.first() {
        acoustic_inputs.push((
            "spk_embed",
            Float32,
            vec![1, speaker.embedding.len() as i64],
        ));
    }
    check_inputs(acoustic, "acoustic", &acoustic_inputs)?;
    let bins = config.num_mel_bins as i64;
    check_inputs(
        vocoder,
        "vocoder",
        &[
            ("mel", Float32, vec![1, -1, bins]),
            ("f0", Float32, vec![1, 1, -1]),
            ("uv", Float32, vec![1, 1, -1]),
        ],
    )?;
    // Some upstream exports label the wrong mel axis dynamic; the actual layout is checked
    // after every inference, rather than trusting those symbolic dimension labels.
    for (session, name, rank) in [(acoustic, "mel", 3), (vocoder, "waveform", 3)] {
        let output = session
            .outputs
            .iter()
            .find(|output| output.name == name)
            .ok_or_else(|| metadata(format!("model has no `{name}` output")))?;
        match &output.output_type {
            ValueType::Tensor {
                ty: Float32,
                dimensions,
                ..
            } if dimensions.len() == rank => {}
            _ => {
                return Err(metadata(format!(
                    "`{name}` output must be a rank-{rank} float32 tensor"
                )));
            }
        }
    }
    Ok(())
}

fn check_inputs(
    session: &Session,
    stage: &str,
    expected: &[(&str, TensorElementType, Vec<i64>)],
) -> Result<(), SingError> {
    if session.inputs.len() != expected.len() {
        let names: Vec<_> = session
            .inputs
            .iter()
            .map(|input| input.name.as_str())
            .collect();
        return Err(metadata(format!(
            "{stage} inputs {names:?} do not match the variant and speakers in the manifest"
        )));
    }
    for (name, expected_type, shape) in expected {
        let input = session
            .inputs
            .iter()
            .find(|input| input.name == *name)
            .ok_or_else(|| metadata(format!("{stage} has no `{name}` input")))?;
        match &input.input_type {
            ValueType::Tensor { ty, dimensions, .. }
                if ty == expected_type
                    && dimensions.len() == shape.len()
                    && dimensions.iter().zip(shape).all(|(actual, expected)| {
                        *actual == -1 || (*expected != -1 && actual == expected)
                    }) => {}
            _ => {
                return Err(metadata(format!(
                    "{stage} `{name}` must be {expected_type:?} {shape:?} with dynamic timeline axes"
                )));
            }
        }
    }
    Ok(())
}

fn validate_curves(frames: &SingerFrames) -> Result<(), SingError> {
    if frames
        .inventory
        .first()
        .is_none_or(|symbol| symbol != SILENCE)
        || frames
            .phonemes
            .iter()
            .any(|id| *id as usize >= frames.inventory.len())
    {
        return Err(SingError::Inference(
            "LeapSinger frames need a silence-first inventory and valid phoneme IDs".into(),
        ));
    }
    if frames.f0_hz.iter().any(|f0| !f0.is_finite() || *f0 < 0.0)
        || frames
            .energy
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err(SingError::Inference(
            "LeapSinger pitch must be finite and nonnegative; energy must be in 0..=1".into(),
        ));
    }
    Ok(())
}

struct Score {
    tokens: Vec<i64>,
    durations: Vec<i64>,
    f0: Vec<f32>,
    voiced: Vec<f32>,
}

impl Score {
    fn pad_to(&mut self, minimum: usize) {
        if self.f0.len() >= minimum {
            return;
        }
        let extra = minimum - self.f0.len();
        self.tokens.push(0);
        self.durations.push(extra as i64);
        self.f0.resize(minimum, *self.f0.last().unwrap_or(&1.0));
        self.voiced.resize(minimum, 0.0);
    }
}

fn arrange(
    frames: &SingerFrames,
    range: Range<usize>,
    symbols: &[String],
) -> Result<Score, SingError> {
    let mut tokens = Vec::new();
    let mut durations = Vec::new();
    let mut voiced = Vec::with_capacity(range.len());
    for at in range.clone() {
        let symbol = &frames.inventory[frames.phonemes[at] as usize];
        let token = symbol_id(symbol, symbols).ok_or_else(|| {
            SingError::Inference(format!(
                "LeapSinger dictionary does not contain `{symbol}` or its Japanese alias"
            ))
        })? as i64;
        if tokens.last() == Some(&token) {
            *durations.last_mut().expect("paired with tokens") += 1;
        } else {
            tokens.push(token);
            durations.push(1);
        }
        let mapped = &symbols[token as usize];
        let unvoiced = is_voiceless(symbol)
            || is_voiceless(mapped)
            || matches!(
                mapped.as_str(),
                "pau"
                    | "A"
                    | "I"
                    | "U"
                    | "E"
                    | "O"
                    | "cl"
                    | "GlottalStop"
                    | "br"
                    | "h_end"
                    | "cl_end"
                    | "sh"
                    | "ch"
                    | "ky"
                    | "ty"
                    | "py"
                    | "hy"
                    | "fy"
            );
        voiced.push(if !unvoiced && frames.f0_hz[at] > 1.0 {
            1.0
        } else {
            0.0
        });
    }
    Ok(Score {
        tokens,
        durations,
        f0: continuous_f0(&frames.f0_hz[range]),
        voiced,
    })
}

/// Keep trained dictionary IDs intact while translating Auris' Japanese IPA spelling.
fn symbol_id(symbol: &str, symbols: &[String]) -> Option<usize> {
    let alias = match symbol {
        SILENCE => "pau",
        "ɯ" => "u",
        "ḁ" => "A",
        "i̥" => "I",
        "ɯ̥" => "U",
        "e̥" => "E",
        "o̥" => "O",
        "ɴ" => "N",
        "ʔ" => "cl",
        "ɾ" => "r",
        "ɸ" => "f",
        "ɸʲ" => "fy",
        "ɕ" => "sh",
        "tɕ" => "ch",
        "dʑ" => "j",
        "dz" => "z",
        "ç" => "hy",
        "kʲ" => "ky",
        "gʲ" => "gy",
        "tʲ" => "ty",
        "dʲ" => "dy",
        "nʲ" | "ɲ" => "ny",
        "mʲ" => "my",
        "ɾʲ" => "ry",
        "bʲ" => "by",
        "pʲ" => "py",
        other => other,
    };
    // Silence must always be model id zero, even if a custom dictionary also contains `sil`.
    if symbol == SILENCE {
        return Some(0);
    }
    // IPA /j/ is や's consonant; LeapSinger's OpenJTalk `j` is じゃ's /dʑ/. The
    // spelling collides, so the Japanese `y` token takes precedence when it exists.
    if symbol == "j"
        && let Some(id) = symbols.iter().position(|known| known == "y")
    {
        return Some(id);
    }
    symbols
        .iter()
        .position(|known| known == symbol)
        .or_else(|| symbols.iter().position(|known| known == alias))
}

/// Upstream feeds a gapless pitch contour: interpolate log2 Hz, extending the edge pitches.
fn continuous_f0(raw: &[f32]) -> Vec<f32> {
    let Some(first) = raw.iter().position(|f0| *f0 > 1.0) else {
        return vec![1.0; raw.len()];
    };
    let mut out = vec![raw[first]; raw.len()];
    let mut previous = first;
    for next in first + 1..raw.len() {
        if raw[next] <= 1.0 {
            continue;
        }
        let left = raw[previous].log2();
        let right = raw[next].log2();
        for (index, value) in out.iter_mut().enumerate().take(next).skip(previous + 1) {
            let fraction = (index - previous) as f32 / (next - previous) as f32;
            *value = (left + (right - left) * fraction).exp2();
        }
        out[next] = raw[next];
        previous = next;
    }
    out[previous..].fill(raw[previous]);
    out
}

fn vocoder_mel(
    shape: &[i64],
    values: &[f32],
    variant: Variant,
    count: usize,
    bins: usize,
) -> Result<Vec<f32>, SingError> {
    let expected = match variant {
        Variant::Full => [1, bins as i64, count as i64],
        Variant::Diffsinger => [1, count as i64, bins as i64],
    };
    if shape != expected || values.len() != count * bins || values.iter().any(|v| !v.is_finite()) {
        return Err(SingError::Inference(format!(
            "LeapSinger acoustic mel must be finite with shape {expected:?}; got {shape:?}"
        )));
    }
    if variant == Variant::Diffsinger {
        return Ok(values.to_vec());
    }
    let mut mel = vec![0.0; values.len()];
    for frame in 0..count {
        for bin in 0..bins {
            mel[frame * bins + bin] = values[bin * count + frame];
        }
    }
    Ok(mel)
}

fn metadata(reason: impl std::fmt::Display) -> SingError {
    SingError::Metadata(format!("{NAME}: {reason}"))
}

fn load_error(path: &Path, error: impl std::fmt::Display) -> SingError {
    SingError::Load {
        reason: format!("{NAME} {}: {error}", path.display()),
    }
}

fn inference(error: ort::Error) -> SingError {
    SingError::Inference(format!("{NAME}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_preserves_export_ids_and_rejects_ambiguous_tables() {
        assert_eq!(
            read_symbols("# vocabulary\npau # silence\n\na\nI\n").unwrap(),
            ["pau", "a", "I"]
        );
        for raw in ["", "a\npau", "pau\na\na", "pau\na b"] {
            assert!(read_symbols(raw).is_err());
        }
    }

    #[test]
    fn ipa_aliases_preserve_devoiced_vowels_and_closures() {
        let symbols = read_symbols("pau\nu\nU\nI\ncl\nsh\nɕ\nry").unwrap();
        for (symbol, id) in [
            (SILENCE, 0),
            ("ɯ", 1),
            ("ɯ̥", 2),
            ("i̥", 3),
            ("ʔ", 4),
            ("ɕ", 6),
            ("ɾʲ", 7),
        ] {
            assert_eq!(symbol_id(symbol, &symbols), Some(id));
        }
        assert_eq!(symbol_id("absent", &symbols), None);
    }

    #[test]
    fn f0_gaps_interpolate_in_pitch_space_and_extend_the_edges() {
        let f0 = continuous_f0(&[0.0, 220.0, 0.0, 880.0, 0.0]);
        for (actual, expected) in f0.iter().zip([220.0, 220.0, 440.0, 880.0, 880.0]) {
            assert!((actual - expected).abs() < 0.001);
        }
        assert_eq!(continuous_f0(&[0.0, 0.0]), [1.0, 1.0]);
    }

    #[test]
    fn japanese_lyrics_keep_palatal_nasal_affricates_and_glides_distinct() {
        let symbols = read_symbols("pau\na\nu\nny\nz\nj\ny").unwrap();
        for (lyric, expected) in [
            ("にゃ", vec![3, 1]),
            ("ざ", vec![4, 1]),
            ("じゃ", vec![5, 1]),
            ("や", vec![6, 1]),
            ("ゆ", vec![6, 2]),
        ] {
            let phonemes = auris_vocal::kana_phonemes(lyric).unwrap();
            let ids: Vec<_> = phonemes
                .iter()
                .map(|symbol| symbol_id(symbol, &symbols).unwrap())
                .collect();
            assert_eq!(ids, expected, "{lyric}");
        }
    }

    #[test]
    fn durations_and_voicing_follow_phonemes_with_continuous_pitch() {
        let symbols = read_symbols("pau\nk\na\nU\ncl").unwrap();
        let frames = SingerFrames {
            hop_seconds: 256.0 / 44100.0,
            inventory: vec![
                SILENCE.into(),
                "k".into(),
                "a".into(),
                "ɯ̥".into(),
                "ʔ".into(),
            ],
            phonemes: vec![0, 1, 1, 2, 3, 4, 0],
            f0_hz: vec![0.0, 220.0, 220.0, 220.0, 220.0, 220.0, 0.0],
            energy: vec![1.0; 7],
        };
        let score = arrange(&frames, 0..7, &symbols).unwrap();
        assert_eq!(score.tokens, [0, 1, 2, 3, 4, 0]);
        assert_eq!(score.durations, [1, 2, 1, 1, 1, 1]);
        assert_eq!(score.voiced, [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(score.f0, [220.0; 7]);
    }

    #[test]
    fn full_mel_is_transposed_and_invalid_outputs_are_refused() {
        assert_eq!(
            vocoder_mel(&[1, 2, 3], &[1., 2., 3., 4., 5., 6.], Variant::Full, 3, 2).unwrap(),
            [1., 4., 2., 5., 3., 6.]
        );
        assert!(vocoder_mel(&[1, 3, 2], &[0.; 6], Variant::Full, 3, 2).is_err());
        assert!(vocoder_mel(&[1, 2, 3], &[f32::NAN; 6], Variant::Full, 3, 2).is_err());
    }
}
