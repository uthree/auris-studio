//! VOICEVOX Engine's score-query and frame-synthesis HTTP pipeline.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use auris_vocal::{SingerFrames, SingerNote, SingerScore};
use serde::Deserialize;
use serde_json::{Value, json};
use unicode_normalization::UnicodeNormalization;

use crate::backend::{BackendKind, SingingBackend};
use crate::metadata::{FORMAT_VERSION, VoiceCard, VoiceInfo};
use crate::{
    Acceleration, CurveGenerator, CurvePrediction, CurveSource, CurveSources, SingError,
    validate_frames,
};

const NAME: &str = "VOICEVOX";

/// Gives consonants room before a first-beat note and the decoder context at both boundaries.
const BOUNDARY_SECONDS: f64 = 1.0;

pub(crate) fn validate_lyrics(score: &SingerScore) -> Result<(), SingError> {
    padded_score(score, 2).map(|_| ())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VoicevoxConfig {
    format_version: u32,
    #[serde(default = "default_name")]
    name: String,
    #[serde(default = "default_url")]
    url: String,
    #[serde(default = "default_sample_rate")]
    sample_rate: u32,
    #[serde(default = "default_frame_rate")]
    frame_rate: f64,
    styles: Vec<VoicevoxStyle>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VoicevoxStyle {
    name: String,
    query_style_id: u32,
    decode_style_id: u32,
}

fn default_name() -> String {
    NAME.to_string()
}

fn default_url() -> String {
    "http://127.0.0.1:50021".to_string()
}

fn default_sample_rate() -> u32 {
    24_000
}

fn default_frame_rate() -> f64 {
    93.75
}

/// Extends boundary rests instead of leaving a one-frame rest immediately before a consonant.
/// VOICEVOX 0.25.2 rounds that consonant down to zero frames and fails its score query.
fn padded_score(score: &SingerScore, padding: u32) -> Result<SingerScore, SingError> {
    let mut score = score.clone();
    let origins = split_lyrics(&mut score)?;
    normalize_long_vowels(&mut score).map_err(|error| match error {
        SingError::InvalidLyric {
            event,
            lyric,
            issue,
        } => SingError::InvalidLyric {
            event: origins[event],
            lyric,
            issue,
        },
        error => error,
    })?;
    bridge_short_rests(&mut score)?;
    validate_score(&score)?;
    let rest = || SingerNote {
        key: None,
        frame_length: padding,
        lyric: String::new(),
    };
    let extend = |note: &mut SingerNote| -> Result<(), SingError> {
        note.frame_length = note
            .frame_length
            .checked_add(padding)
            .ok_or_else(|| SingError::Inference("VOICEVOX boundary rest is too long".into()))?;
        Ok(())
    };
    match score.notes.first_mut() {
        Some(note) if note.key.is_none() && note.lyric.is_empty() => extend(note)?,
        _ => score.notes.insert(0, rest()),
    }
    match score.notes.last_mut() {
        Some(note) if note.key.is_none() && note.lyric.is_empty() => extend(note)?,
        _ => score.notes.push(rest()),
    }
    Ok(score)
}

/// A score query accepts one mora per note, even when the editor holds a whole word.
fn split_lyrics(score: &mut SingerScore) -> Result<Vec<usize>, SingError> {
    let mut notes = Vec::new();
    let mut origins = Vec::new();
    for (index, note) in score.notes.iter().enumerate() {
        let lyric: String = note.lyric.trim().nfkc().collect();
        let moras = auris_vocal::split_kana_lyric(&lyric).unwrap_or_default();
        if note.key.is_none() || moras.len() <= 1 {
            notes.push(note.clone());
            origins.push(index);
            continue;
        }
        let count = u32::try_from(moras.len())
            .map_err(|_| SingError::Inference("VOICEVOX lyric is too long".into()))?;
        if note.frame_length < count {
            return Err(SingError::InvalidLyric {
                event: index,
                lyric,
                issue: crate::LyricIssue::TooShort,
            });
        }
        for (at, (lyric, _)) in moras.into_iter().enumerate() {
            origins.push(index);
            notes.push(SingerNote {
                key: note.key,
                frame_length: note.frame_length / count
                    + u32::from((at as u32) < note.frame_length % count),
                lyric,
            });
        }
    }
    score.notes = notes;
    Ok(origins)
}

/// Give the Engine's consonant predictor two frames before every consonant.
/// Record only the inserted frames so the prediction can return to the original clock.
fn query_score(score: &SingerScore, padding: u32) -> Result<(SingerScore, Vec<usize>), SingError> {
    let mut score = padded_score(score, padding)?;
    let mut removed = Vec::new();
    let mut at = 0usize;
    for index in 0..score.notes.len() {
        if score.notes[index].frame_length == 1
            && score
                .notes
                .get(index + 1)
                .is_some_and(starts_with_consonant)
        {
            score.notes[index].frame_length = 2;
            removed.push(at + 1);
        }
        at = at
            .checked_add(score.notes[index].frame_length as usize)
            .ok_or_else(|| SingError::Inference("VOICEVOX score is too long".into()))?;
    }
    Ok((score, removed))
}

/// Validate all three Engine timelines before decoding, then remove query-only frames.
fn restore_query_timing(
    query: &mut Value,
    expected: usize,
    removed: &[usize],
) -> Result<(), SingError> {
    let invalid = || {
        SingError::Inference(
            "VOICEVOX query has inconsistent phoneme, f0 or volume frame lengths".into(),
        )
    };
    if removed.windows(2).any(|pair| pair[0] >= pair[1])
        || removed.last().is_some_and(|last| *last >= expected)
    {
        return Err(invalid());
    }
    for field in ["f0", "volume"] {
        if query[field]
            .as_array()
            .is_none_or(|values| values.len() != expected)
        {
            return Err(invalid());
        }
    }
    let phonemes = query["phonemes"].as_array_mut().ok_or_else(invalid)?;
    let mut at = 0usize;
    for phoneme in phonemes.iter_mut() {
        if phoneme["phoneme"].as_str().is_none_or(str::is_empty) {
            return Err(invalid());
        }
        let length = phoneme["frame_length"]
            .as_u64()
            .and_then(|length| usize::try_from(length).ok())
            .ok_or_else(invalid)?;
        let end = at
            .checked_add(length)
            .filter(|end| *end <= expected)
            .ok_or_else(invalid)?;
        let omitted = removed.partition_point(|frame| *frame < end)
            - removed.partition_point(|frame| *frame < at);
        phoneme["frame_length"] = json!(length - omitted);
        at = end;
    }
    if at != expected {
        return Err(invalid());
    }
    phonemes.retain(|phoneme| phoneme["frame_length"].as_u64() != Some(0));
    for field in ["f0", "volume"] {
        let values = query[field].as_array_mut().ok_or_else(invalid)?;
        let mut at = 0;
        let mut omitted = removed.iter().copied().peekable();
        values.retain(|_| {
            let keep = omitted.peek() != Some(&at);
            if !keep {
                omitted.next();
            }
            at += 1;
            keep
        });
    }
    Ok(())
}

/// The Engine accepts vowel kana, but rejects a prolonged-sound mark as a standalone lyric.
/// Keep the note boundaries and the stored score intact; only the outgoing spelling changes.
fn normalize_long_vowels(score: &mut SingerScore) -> Result<(), SingError> {
    let mut vowel: Option<&str> = None;
    for (index, note) in score.notes.iter_mut().enumerate() {
        note.lyric = note.lyric.trim().nfkc().collect();
        if note.key.is_none() {
            continue;
        }
        if note.lyric.trim() == "ー" {
            note.lyric = vowel
                .ok_or_else(|| SingError::InvalidLyric {
                    event: index,
                    lyric: note.lyric.clone(),
                    issue: crate::LyricIssue::MissingVowel,
                })?
                .to_string();
        }
        if auris_vocal::kana_phonemes(&note.lyric).is_none() || note.lyric.is_empty() {
            return Err(SingError::InvalidLyric {
                event: index,
                lyric: note.lyric.clone(),
                issue: crate::LyricIssue::Unreadable,
            });
        }
        // Auris accepts phonetic spellings such as シァ and フゥ; the Engine only
        // accepts their standard mora spellings (しゃ and ふ).
        if let Some(canonical) = auris_vocal::kana_phonemes(&note.lyric)
            .and_then(|phonemes| auris_vocal::kana::phonemes_to_kana(&phonemes))
        {
            let hiragana: String = note
                .lyric
                .chars()
                .map(|c| match c {
                    'ァ'..='ヶ' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
                    _ => c,
                })
                .collect();
            if canonical != hiragana {
                note.lyric = canonical;
            }
        }
        vowel = auris_vocal::kana_phonemes(note.lyric.trim()).and_then(|phonemes| {
            match phonemes.last().map(String::as_str) {
                Some("a") => Some("ア"),
                Some("i") => Some("イ"),
                Some("ɯ") => Some("ウ"),
                Some("e") => Some("エ"),
                Some("o") => Some("オ"),
                _ => None,
            }
        });
    }
    Ok(())
}

/// Bridge quantized one-frame gaps into the preceding note only for the Engine query.
/// The next onset and total frame count stay fixed, as do the document and host curves.
fn bridge_short_rests(score: &mut SingerScore) -> Result<(), SingError> {
    let mut index = 1;
    while index + 1 < score.notes.len() {
        let note = &score.notes[index];
        if note.key.is_none()
            && note.lyric.is_empty()
            && note.frame_length == 1
            && score.notes[index - 1].key.is_some()
            && score.notes[index - 1].frame_length > 0
            && starts_with_consonant(&score.notes[index + 1])
        {
            let previous = &mut score.notes[index - 1];
            previous.frame_length = previous.frame_length.checked_add(1).ok_or_else(|| {
                SingError::Inference("VOICEVOX note before a short rest is too long".into())
            })?;
            score.notes.remove(index);
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn starts_with_consonant(note: &SingerNote) -> bool {
    note.key.is_some()
        && auris_vocal::kana_phonemes(&note.lyric).is_some_and(|phonemes| phonemes.len() > 1)
}

/// Reject malformed events before adapting the Engine's temporal constraints.
fn validate_score(score: &SingerScore) -> Result<(), SingError> {
    for (index, note) in score.notes.iter().enumerate() {
        if note.frame_length == 0
            || note.key.is_some_and(|key| key > 127)
            || note.key.is_none() != note.lyric.is_empty()
        {
            return Err(SingError::Inference(format!(
                "VOICEVOX: score event {} needs a positive duration and either a MIDI key (0–127) with a kana lyric, or a rest with no lyric",
                index + 1
            )));
        }
    }
    Ok(())
}

/// The host's frame grid must represent an exact number of output samples.
fn output_hop(sample_rate: u32, frame_rate: f64) -> Result<u32, SingError> {
    let hop = f64::from(sample_rate) / frame_rate;
    if !hop.is_finite()
        || hop < 1.0
        || hop > f64::from(u32::MAX)
        || (hop - hop.round()).abs() > 1.0e-8
    {
        return Err(SingError::Metadata(
            "VOICEVOX sample_rate / frame_rate must be an integer; use 24000 or 48000 Hz with the standard 93.75 fps Engine".into(),
        ));
    }
    Ok(hop.round() as u32)
}

/// Keep the Engine's explanation: a bare HTTP 400 hides which lyric it refused.
fn request_error(endpoint: &str, error: ureq::Error) -> SingError {
    let reason = match error {
        ureq::Error::Status(status, response) => {
            let mut body = String::new();
            let _ = response.into_reader().take(8192).read_to_string(&mut body);
            let detail = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|body| body.get("detail").cloned())
                .map(|detail| {
                    detail
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| detail.to_string())
                })
                .unwrap_or_else(|| body.trim().to_string());
            format!("HTTP {status}: {detail}")
        }
        error => error.to_string(),
    };
    SingError::Inference(format!("VOICEVOX {endpoint}: {reason}"))
}

pub(crate) struct VoicevoxBackend {
    config: VoicevoxConfig,
    info: VoiceInfo,
    path: PathBuf,
    acceleration: Acceleration,
}

impl VoicevoxBackend {
    pub(crate) fn load(path: &Path, acceleration: Acceleration) -> Result<Self, SingError> {
        let raw = std::fs::read_to_string(path).map_err(|error| SingError::Load {
            reason: error.to_string(),
        })?;
        let mut config: VoicevoxConfig =
            serde_json::from_str(&raw).map_err(|error| SingError::Metadata(error.to_string()))?;
        if config.format_version != 1 {
            return Err(SingError::Metadata(format!(
                "VOICEVOX connection format {} is unsupported; this build reads 1",
                config.format_version
            )));
        }
        config.url = config.url.trim_end_matches('/').to_string();
        if !config.url.starts_with("http://") && !config.url.starts_with("https://") {
            return Err(SingError::Metadata(
                "VOICEVOX url must begin with http:// or https://".into(),
            ));
        }
        if config.sample_rate == 0 || !config.frame_rate.is_finite() || config.frame_rate <= 0.0 {
            return Err(SingError::Metadata(
                "VOICEVOX sample_rate and frame_rate must be positive".into(),
            ));
        }
        if config.styles.is_empty() {
            return Err(SingError::Metadata(
                "VOICEVOX connection has no singing styles".into(),
            ));
        }
        let hop_length = output_hop(config.sample_rate, config.frame_rate)?;
        let speaker_to_id: BTreeMap<String, u32> = config
            .styles
            .iter()
            .enumerate()
            .map(|(id, style)| (style.name.clone(), id as u32))
            .collect();
        if speaker_to_id.len() != config.styles.len() {
            return Err(SingError::Metadata(
                "VOICEVOX style names must be unique".into(),
            ));
        }
        let info = VoiceInfo {
            format_version: FORMAT_VERSION,
            sample_rate: config.sample_rate,
            hop_length,
            inter_channels: 1,
            n_speakers: config.styles.len() as u32,
            symbols: vec!["<sil>".into(), "<unk>".into()],
            speaker_to_id,
            phoneme_durations: None,
            phoneme_levels: None,
            voice: Some(VoiceCard {
                name: config.name.clone(),
                description: "VOICEVOX Engine singing connection".into(),
                ..VoiceCard::default()
            }),
        };
        Ok(Self {
            config,
            info,
            path: path.to_path_buf(),
            acceleration,
        })
    }

    fn post_json(&self, endpoint: &str, style: u32, body: Value) -> Result<Value, SingError> {
        let url = format!("{}{endpoint}?speaker={style}", self.config.url);
        let response = ureq::post(&url)
            .send_json(body)
            .map_err(|error| request_error(endpoint, error))?;
        response
            .into_json()
            .map_err(|error| SingError::Inference(format!("VOICEVOX {endpoint}: {error}")))
    }

    fn synthesize(&self, style: u32, query: Value) -> Result<Vec<u8>, SingError> {
        let url = format!("{}/frame_synthesis?speaker={style}", self.config.url);
        let response = ureq::post(&url)
            .send_json(query)
            .map_err(|error| request_error("/frame_synthesis", error))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|error| SingError::Inference(format!("VOICEVOX audio response: {error}")))?;
        Ok(bytes)
    }

    fn decode_wav(&self, bytes: Vec<u8>) -> Result<Vec<f32>, SingError> {
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).map_err(|error| {
            SingError::Inference(format!("VOICEVOX returned invalid WAV: {error}"))
        })?;
        let spec = reader.spec();
        if spec.channels != 1 || spec.sample_rate != self.info.sample_rate {
            return Err(SingError::Inference(format!(
                "VOICEVOX returned {} channel(s) at {} Hz; expected mono at {} Hz",
                spec.channels, spec.sample_rate, self.info.sample_rate
            )));
        }
        match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| SingError::Inference(format!("VOICEVOX WAV: {error}"))),
            hound::SampleFormat::Int => {
                let scale = (1_u64 << spec.bits_per_sample.saturating_sub(1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|sample| sample.map(|value| value as f32 / scale))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| SingError::Inference(format!("VOICEVOX WAV: {error}")))
            }
        }
    }
}

fn prediction_from_query(
    query: Value,
    padding: usize,
) -> Result<CurvePrediction<Value>, SingError> {
    let read = |name: &str| -> Result<Vec<f64>, SingError> {
        serde_json::from_value(query[name].clone())
            .map_err(|_| SingError::Inference(format!("VOICEVOX query has invalid {name} frames")))
    };
    let pitch_hz = read("f0")?;
    let mut energy = read("volume")?;
    // Engine 0.25.2 can predict small negative volumes around silence. They are
    // amplitude undershoot, not negative energy; retain every positive prediction.
    // Nonfinite values remain invalid and are rejected by the shared curve validation.
    for value in &mut energy {
        if value.is_finite() && *value < 0.0 {
            *value = 0.0;
        }
    }
    Ok(CurvePrediction {
        pitch_hz: Some(pitch_hz),
        energy: Some(energy),
        leading_frames: padding,
        trailing_frames: padding,
        context: query,
    })
}

impl CurveGenerator for VoicevoxBackend {
    type Context = Value;

    const SOURCES: CurveSources = CurveSources {
        pitch: CurveSource::Backend,
        energy: CurveSource::Backend,
    };

    fn generate_curves(
        &mut self,
        _frames: &SingerFrames,
        score: &SingerScore,
        speaker: u32,
        _seed: u64,
    ) -> Result<CurvePrediction<Value>, SingError> {
        let style = self
            .config
            .styles
            .get(speaker as usize)
            .ok_or(SingError::NoSuchSpeaker {
                speaker,
                count: self.info.n_speakers,
            })?;
        let padding = (self.config.frame_rate * BOUNDARY_SECONDS).ceil() as u32;
        let (padded, removed) = query_score(score, padding)?;
        let expected = padded
            .notes
            .iter()
            .map(|note| note.frame_length as usize)
            .sum();
        let mut query = self.post_json(
            "/sing_frame_audio_query",
            style.query_style_id,
            json!({ "notes": padded.notes }),
        )?;
        restore_query_timing(&mut query, expected, &removed)?;
        prediction_from_query(query, padding as usize)
    }
}

impl SingingBackend for VoicevoxBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Voicevox
    }

    fn info(&self) -> &VoiceInfo {
        &self.info
    }

    fn acceleration(&self) -> Acceleration {
        self.acceleration
    }

    fn on_gpu(&self) -> bool {
        false
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn sing_with(
        &mut self,
        frames: &SingerFrames,
        score: Option<&SingerScore>,
        speaker: u32,
        seed: u64,
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<Vec<f32>, SingError> {
        self.sing_render_with(frames, score, speaker, seed, progress)
            .map(|render| render.samples)
    }

    fn sing_render_with(
        &mut self,
        frames: &SingerFrames,
        score: Option<&SingerScore>,
        speaker: u32,
        seed: u64,
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<crate::SingingRender, SingError> {
        validate_frames(frames)?;
        let score = score.ok_or_else(|| SingError::Unsupported {
            backend: NAME,
            reason: "a lyric-bearing note score is required; raw frame files cannot be sung".into(),
        })?;
        let style = self
            .config
            .styles
            .get(speaker as usize)
            .ok_or(SingError::NoSuchSpeaker {
                speaker,
                count: self.info.n_speakers,
            })?;
        let score_frames: usize = score
            .notes
            .iter()
            .map(|note| note.frame_length as usize)
            .sum();
        if score_frames != frames.len() {
            return Err(SingError::Inference(format!(
                "the note score covers {score_frames} frames but the curves cover {}",
                frames.len()
            )));
        }
        if frames.is_empty() {
            return Ok(crate::SingingRender::default());
        }
        if !progress(0, 2) {
            return Err(SingError::Cancelled);
        }
        let decode_style = style.decode_style_id;
        let prepared = self.prepare_curves(frames, score, speaker, seed)?;
        let padding = prepared.leading_frames;
        let query_frames = prepared.pitch_hz.len();
        let backend_pitch = auris_core::SingerPitch {
            hop_seconds: frames.hop_seconds,
            hz: prepared.pitch_hz[padding..padding + frames.len()].to_vec(),
        };
        let mut query = prepared.context;
        query["f0"] = json!(prepared.pitch_hz);
        query["volume"] = json!(prepared.energy);
        query["outputSamplingRate"] = json!(self.info.sample_rate);
        query["outputStereo"] = json!(false);
        if !progress(1, 2) {
            return Err(SingError::Cancelled);
        }
        let wav = self.synthesize(decode_style, query)?;
        if !progress(2, 2) {
            return Err(SingError::Cancelled);
        }
        let mut samples = self.decode_wav(wav)?;
        let hop = self.info.hop_length as usize;
        let expected = query_frames * hop;
        if samples.len() != expected {
            return Err(SingError::Inference(format!(
                "VOICEVOX returned {} samples for {query_frames} frames; expected {expected}",
                samples.len()
            )));
        }
        // The padding belongs to this adapter, never to the track's saved score or timeline.
        samples.drain(..padding * hop);
        samples.truncate(frames.len() * hop);
        Ok(crate::SingingRender {
            samples,
            backend_pitch: Some(backend_pitch),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use auris_vocal::{SingerFrames, SingerNote, SingerScore};

    use super::*;

    fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "client closed before the HTTP headers ended");
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(at) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while bytes.len() < header_end + length {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0, "client closed before the HTTP body ended");
            bytes.extend_from_slice(&buffer[..count]);
        }
        String::from_utf8(bytes).unwrap()
    }

    fn wav(samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&24_000_u32.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes
    }

    fn answer(stream: &mut TcpStream, content_type: &str, body: &[u8]) {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    }

    fn mock_round_trip(
        frame_rate: f64,
        padding: usize,
        opening_rest: bool,
        short_wave: bool,
    ) -> (Result<Vec<f32>, SingError>, Vec<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let hop = (24_000.0 / frame_rate).round() as usize;
        let query_frames = 2 * padding + 2;
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            let (mut query, _) = listener.accept().unwrap();
            requests.push(read_request(&mut query));
            let mut predicted_pitch = vec![0.0; query_frames];
            let mut predicted_volume = vec![0.0; query_frames];
            if !opening_rest {
                predicted_pitch[padding] = 220.0;
                predicted_volume[padding] = 1.0;
            }
            predicted_pitch[padding + 1] = 440.0;
            predicted_volume[padding + 1] = 1.0;
            let response = json!({
                "f0": predicted_pitch,
                "volume": predicted_volume,
                "phonemes": [{"phoneme": "a", "frame_length": query_frames}],
                "outputSamplingRate": 24000,
                "outputStereo": false,
            });
            answer(
                &mut query,
                "application/json",
                &serde_json::to_vec(&response).unwrap(),
            );
            let (mut synthesis, _) = listener.accept().unwrap();
            requests.push(read_request(&mut synthesis));
            let mut samples = vec![-16_384; query_frames * hop];
            samples[padding * hop..(padding + 1) * hop].fill(8_192);
            samples[(padding + 1) * hop..(padding + 2) * hop].fill(16_384);
            if short_wave {
                samples.pop();
            }
            answer(&mut synthesis, "audio/wav", &wav(&samples));
            requests
        });

        // A clock reading is not a unique ID: concurrent mocks can otherwise overwrite each
        // other's Engine URL and leave one server waiting for requests that went elsewhere.
        static NEXT_MOCK: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT_MOCK.fetch_add(1, Ordering::Relaxed);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "auris-voicevox-{}-{unique}-{sequence}.voicevox.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            format!(
                r#"{{"format_version":1,"name":"Mock singer","url":"http://{address}","sample_rate":24000,"frame_rate":{frame_rate},"styles":[{{"name":"Mock style","query_style_id":6000,"decode_style_id":3001}}]}}"#
            ),
        )
        .unwrap();
        let mut model = crate::VoiceModel::load(&path, Acceleration::Auto).unwrap();
        assert_eq!(model.backend_kind(), BackendKind::Voicevox);
        assert_eq!(model.info().speakers(), ["Mock style"]);
        let frames = SingerFrames {
            hop_seconds: hop as f64 / 24_000.0,
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: vec![0, 1],
            f0_hz: vec![if opening_rest { 0.0 } else { 220.0 }, 440.0],
            energy: vec![if opening_rest { 0.0 } else { 0.4 }, 0.8],
        };
        let score = SingerScore {
            notes: vec![
                SingerNote {
                    key: (!opening_rest).then_some(57),
                    frame_length: 1,
                    lyric: if opening_rest { "" } else { "ア" }.into(),
                },
                SingerNote {
                    key: Some(69),
                    frame_length: 1,
                    lyric: if opening_rest { "ラ" } else { "ア" }.into(),
                },
            ],
        };
        let samples = model
            .sing_render_with(&frames, &score, 0, 0, |_, _| true)
            .map(|render| {
                let pitch = render
                    .backend_pitch
                    .expect("the prediction travels with its audio");
                assert_eq!(pitch.hop_seconds, frames.hop_seconds);
                assert_eq!(
                    pitch.hz,
                    frames
                        .f0_hz
                        .iter()
                        .map(|hz| f64::from(*hz))
                        .collect::<Vec<_>>(),
                    "decoder padding is removed and unvoiced frames are preserved"
                );
                render.samples
            });
        let requests = server.join().unwrap();
        std::fs::remove_file(path).unwrap();
        (samples, requests)
    }

    fn request_body(request: &str) -> Value {
        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    #[test]
    fn predicted_volume_undershoot_is_silent_without_losing_articulation() {
        let query = json!({
            "f0": [0.0, 0.0, 440.0, 438.0, 0.0, 0.0],
            "volume": [-0.00038448721170425415, 0.04, 0.4, 0.2, -0.000013425946235656738, 0.0],
            "phonemes": [{"phoneme": "k", "frame_length": 2}, {"phoneme": "a", "frame_length": 4}]
        });
        let frames = SingerFrames {
            hop_seconds: 256.0 / 24000.0,
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: vec![1; 4],
            f0_hz: vec![880.0; 4],
            energy: vec![0.5, 0.5, 0.25, 0.5],
        };
        let score = SingerScore {
            notes: vec![SingerNote {
                key: Some(69),
                frame_length: 4,
                lyric: "カ".into(),
            }],
        };
        let prepared = prediction_from_query(query.clone(), 1)
            .unwrap()
            .apply_expression(&frames, &score, VoicevoxBackend::SOURCES)
            .unwrap();
        assert_eq!(prepared.pitch_hz, [0.0, 0.0, 880.0, 876.0, 0.0, 0.0]);
        assert_eq!(prepared.energy, [0.0, 0.02, 0.2, 0.05, 0.0, 0.0]);
        assert_eq!(
            prepared.context, query,
            "pronunciation context is untouched"
        );
    }

    fn test_score(events: &[(u32, &str)]) -> SingerScore {
        SingerScore {
            notes: events
                .iter()
                .map(|(length, lyric)| SingerNote {
                    key: (!lyric.is_empty()).then_some(60),
                    frame_length: *length,
                    lyric: (*lyric).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn short_note_queries_restore_the_original_frame_clock() {
        let original = test_score(&[(1, "ア"), (1, "キ"), (25, "カ")]);
        let (outgoing, removed) = query_score(&original, 94).unwrap();
        assert_eq!(removed, [95, 97]);
        assert_eq!(outgoing.notes[1].frame_length, 2);
        assert_eq!(outgoing.notes[2].frame_length, 2);
        let mut query = json!({
            "f0": [10.0, 20.0, 30.0, 40.0, 50.0],
            "volume": [1.0, 2.0, 3.0, 4.0, 5.0],
            "phonemes": [
                {"phoneme": "a", "frame_length": 2},
                {"phoneme": "k", "frame_length": 1},
                {"phoneme": "i", "frame_length": 2}
            ]
        });
        restore_query_timing(&mut query, 5, &[1, 2]).unwrap();
        assert_eq!(query["f0"], json!([10.0, 40.0, 50.0]));
        assert_eq!(query["volume"], json!([1.0, 4.0, 5.0]));
        assert_eq!(
            query["phonemes"],
            json!([
                {"phoneme": "a", "frame_length": 1},
                {"phoneme": "i", "frame_length": 2}
            ])
        );
    }

    #[test]
    fn multi_mora_notes_split_without_losing_lyrics_or_frames() {
        let (outgoing, _) = query_score(&test_score(&[(25, "こーひー")]), 94).unwrap();
        assert_eq!(
            &outgoing.notes[1..5],
            test_score(&[(7, "こ"), (6, "オ"), (6, "ひ"), (6, "イ")]).notes
        );
        assert!(query_score(&test_score(&[(1, "かな")]), 94).is_err());
    }

    #[test]
    fn malformed_engine_phoneme_lengths_are_rejected_before_synthesis() {
        for length in [json!(-1), json!(2), json!(1.5), json!(null)] {
            let mut query = json!({"f0": [100.0], "volume": [1.0], "phonemes": [{"phoneme": "a", "frame_length": length}]});
            assert!(restore_query_timing(&mut query, 1, &[]).is_err());
        }
    }

    #[test]
    fn lyric_errors_keep_original_event_indices_after_splitting() {
        let error =
            crate::validate_voicevox_score(&test_score(&[(1, ""), (25, "かな"), (25, "🙂")]))
                .unwrap_err();
        assert!(matches!(
            error,
            SingError::InvalidLyric {
                event: 2,
                issue: crate::LyricIssue::Unreadable,
                ..
            }
        ));
        let error =
            crate::validate_voicevox_score(&test_score(&[(25, "かん"), (25, "ー")])).unwrap_err();
        assert!(matches!(
            error,
            SingError::InvalidLyric {
                event: 1,
                issue: crate::LyricIssue::MissingVowel,
                ..
            }
        ));
    }

    #[test]
    fn kana_spelling_is_normalized_only_in_the_outgoing_score() {
        let original = test_score(&[(25, " ｶﾞ "), (25, "ｰ"), (25, "か\u{3099}"), (25, " ー ")]);
        let outgoing = padded_score(&original, 94).unwrap();
        assert_eq!(
            outgoing.notes[1..5]
                .iter()
                .map(|note| note.lyric.as_str())
                .collect::<Vec<_>>(),
            ["ガ", "ア", "が", "ア"]
        );
        assert_eq!(original.notes[0].lyric, " ｶﾞ ");
        assert_eq!(original.notes[1].lyric, "ｰ");
        assert_eq!(original.notes[2].lyric, "か\u{3099}");
    }

    #[test]
    fn tempo_changes_keep_short_gaps_singable_without_moving_note_onsets() {
        use auris_core::project::{ClipId, MidiClip, Note};
        use auris_core::{PluginState, SingerTrack, TempoMap, Ticks};

        // The ten-tick gap before "ひ" in the reported project crosses frame boundaries
        // differently as tempo changes.
        let mut clip = MidiClip::new(ClipId(1), "Verse", Ticks::ZERO, Ticks::from_beats(24.0));
        for (start, length, lyric) in [(9120, 470, "の"), (9600, 520, "ひ")] {
            let mut note = Note::new(64, Ticks(start), Ticks(length));
            note.lyric = lyric.into();
            clip.notes.push(note);
        }
        let track = SingerTrack {
            instrument_id: "auris.synth.vocal".into(),
            instrument_state: PluginState::empty(),
            clips: vec![clip],
            frame_hop: 1.0 / 93.75,
            voice: None,
            take: None,
        };
        let onsets = |score: &SingerScore| {
            let mut at = 0_u64;
            score
                .notes
                .iter()
                .filter_map(|note| {
                    let start = at;
                    at += u64::from(note.frame_length);
                    note.key.map(|key| (start, key, note.lyric.clone()))
                })
                .collect::<Vec<_>>()
        };
        for bpm in [60.0, 90.0, 120.0, 150.0, 180.0, 240.0] {
            let score = auris_vocal::render_score(&track, &TempoMap::constant(bpm));
            let outgoing = padded_score(&score, 94).unwrap();
            let expected = onsets(&score)
                .into_iter()
                .map(|(at, key, lyric)| (at + 94, key, lyric))
                .collect::<Vec<_>>();
            assert_eq!(onsets(&outgoing), expected, "tempo {bpm}");
            assert_eq!(
                outgoing
                    .notes
                    .iter()
                    .map(|n| u64::from(n.frame_length))
                    .sum::<u64>(),
                score
                    .notes
                    .iter()
                    .map(|n| u64::from(n.frame_length))
                    .sum::<u64>()
                    + 188
            );
        }
    }

    #[test]
    fn short_internal_notes_get_temporary_query_frames() {
        for events in [
            vec![(1, "ア"), (25, "カ")],
            vec![(25, "ア"), (1, "イ"), (25, "カ")],
        ] {
            let (_, removed) = query_score(&test_score(&events), 94).unwrap();
            assert_eq!(removed.len(), 1);
        }
        for events in [
            vec![(1, ""), (25, "カ")],
            vec![(2, "ア"), (25, "カ")],
            vec![(25, "ア"), (2, ""), (25, "カ")],
            vec![(1, "ア"), (25, "ア")],
            vec![(1, "ア"), (25, "ン")],
            vec![(1, "ア"), (25, "ッ")],
            vec![(1, "カ")],
            vec![(25, "")],
        ] {
            let (_, removed) = query_score(&test_score(&events), 94).unwrap();
            assert!(removed.is_empty());
        }
    }

    #[test]
    fn short_rests_are_bridged_only_before_consonants_in_the_outgoing_score() {
        let original = test_score(&[
            (25, "ア"),
            (1, ""),
            (25, "カ"),
            (1, ""),
            (25, "ア"),
            (2, ""),
            (25, "キ"),
        ]);
        let outgoing = padded_score(&original, 94).unwrap();
        let expected = test_score(&[
            (94, ""),
            (26, "ア"),
            (25, "カ"),
            (1, ""),
            (25, "ア"),
            (2, ""),
            (25, "キ"),
            (94, ""),
        ]);
        assert_eq!(outgoing, expected);
        assert_eq!(original.notes[0].frame_length, 25);
        assert_eq!(original.notes[1].frame_length, 1);
    }

    #[test]
    fn malformed_events_are_rejected_before_the_query() {
        let mut score = test_score(&[(0, "ア")]);
        assert!(padded_score(&score, 94).is_err());
        score.notes[0].frame_length = 25;
        score.notes[0].key = Some(128);
        assert!(padded_score(&score, 94).is_err());
        score.notes[0].key = None;
        assert!(padded_score(&score, 94).is_err());
        score.notes[0].key = Some(60);
        score.notes[0].lyric.clear();
        assert!(padded_score(&score, 94).is_err());
    }

    #[test]
    fn output_rates_must_fit_the_frame_grid_without_rounding() {
        assert_eq!(output_hop(24000, 93.75).unwrap(), 256);
        assert_eq!(output_hop(48000, 93.75).unwrap(), 512);
        assert_eq!(output_hop(24000, 100.0).unwrap(), 240);
        for (rate, frames) in [
            (44100, 93.75),
            (24000, 48000.0),
            (24000, 0.0),
            (0, 93.75),
            (24000, 1.0e-20),
        ] {
            assert!(output_hop(rate, frames).is_err());
        }
    }

    #[test]
    fn prolonged_vowels_keep_notes_rests_and_the_original_score() {
        for (mora, vowel) in [
            ("ラ", "ア"),
            ("ひ", "イ"),
            ("シュ", "ウ"),
            ("て", "エ"),
            ("きょ", "オ"),
        ] {
            let notes = [mora, "ー", "", "ー"]
                .into_iter()
                .enumerate()
                .map(|(index, lyric)| SingerNote {
                    key: (!lyric.is_empty()).then_some(60 + index as u8),
                    frame_length: 20 + index as u32,
                    lyric: lyric.into(),
                })
                .collect();
            let original = SingerScore { notes };
            let padded = padded_score(&original, 94).unwrap();
            assert_eq!(original.notes[1].lyric, "ー");
            for (source, outgoing) in original.notes.iter().zip(&padded.notes[1..5]) {
                assert_eq!(outgoing.key, source.key);
                assert_eq!(outgoing.frame_length, source.frame_length);
                assert_eq!(
                    outgoing.lyric,
                    if source.lyric == "ー" {
                        vowel
                    } else {
                        &source.lyric
                    }
                );
            }
        }
        for lyrics in [vec!["ー"], vec!["か", "ン", "ー"], vec!["か", "ッ", "ー"]] {
            let mut score = SingerScore {
                notes: lyrics
                    .into_iter()
                    .map(|lyric| SingerNote {
                        key: Some(60),
                        frame_length: 20,
                        lyric: lyric.into(),
                    })
                    .collect(),
            };
            assert!(
                normalize_long_vowels(&mut score)
                    .unwrap_err()
                    .to_string()
                    .contains("needs a preceding vowel")
            );
        }
    }

    #[test]
    fn rejected_queries_report_the_engines_lyric_detail() {
        let body = serde_json::to_string(&json!({"detail": "lyricが不正です: ー"})).unwrap();
        let response = ureq::Response::new(400, "Bad Request", &body).unwrap();
        let error = request_error(
            "/sing_frame_audio_query",
            ureq::Error::Status(400, response),
        )
        .to_string();
        assert!(error.contains("HTTP 400"));
        assert!(error.contains("lyricが不正です: ー"));
    }

    #[test]
    fn score_query_and_frame_synthesis_preserve_timeline_after_boundary_padding() {
        for (frame_rate, padding, hop) in [(93.75, 94, 256), (100.0, 100, 240)] {
            let (samples, requests) = mock_round_trip(frame_rate, padding, true, false);
            let samples = samples.unwrap();
            assert_eq!(samples.len(), 2 * hop);
            assert_eq!(samples[..hop], vec![0.25; hop]);
            assert_eq!(samples[hop..], vec![0.5; hop]);
            assert!(requests[0].starts_with("POST /sing_frame_audio_query?speaker=6000 "));
            assert!(requests[1].starts_with("POST /frame_synthesis?speaker=3001 "));
            let score = request_body(&requests[0]);
            assert_eq!(
                score["notes"],
                json!([
                    {"key": null, "frame_length": padding + 1, "lyric": ""},
                    {"key": 69, "frame_length": 1, "lyric": "ラ"},
                    {"key": null, "frame_length": padding, "lyric": ""},
                ])
            );
            let query = request_body(&requests[1]);
            assert_eq!(query["outputSamplingRate"], 24000);
            assert_eq!(query["outputStereo"], false);
            let mut pitch = vec![0.0; 2 * padding + 2];
            let mut energy = pitch.clone();
            pitch[padding + 1] = 440.0;
            energy[padding + 1] = f64::from(0.8_f32);
            assert_eq!(query["f0"], json!(pitch));
            assert_eq!(query["volume"], json!(energy));
        }
    }

    #[test]
    fn a_score_without_boundary_rests_keeps_its_first_pitch_and_energy() {
        let (samples, requests) = mock_round_trip(93.75, 94, false, false);
        assert_eq!(samples.unwrap().len(), 2 * 256);
        let score = request_body(&requests[0]);
        assert_eq!(
            score["notes"][0],
            json!({"key": null, "frame_length": 94, "lyric": ""})
        );
        assert_eq!(score["notes"][1]["lyric"], "ア");
        let query = request_body(&requests[1]);
        assert_eq!(query["f0"][94], json!(220.0_f32));
        assert_eq!(query["volume"][94], json!(f64::from(0.4_f32)));
    }

    #[test]
    fn boundary_padding_merges_existing_rests_without_rewriting_the_score() {
        let score = SingerScore {
            notes: vec![
                SingerNote {
                    key: None,
                    frame_length: 1,
                    lyric: String::new(),
                },
                SingerNote {
                    key: Some(60),
                    frame_length: 46,
                    lyric: "か".into(),
                },
                SingerNote {
                    key: None,
                    frame_length: 5,
                    lyric: String::new(),
                },
                SingerNote {
                    key: Some(62),
                    frame_length: 46,
                    lyric: "え".into(),
                },
                SingerNote {
                    key: None,
                    frame_length: 1,
                    lyric: String::new(),
                },
            ],
        };
        let padded = padded_score(&score, 94).unwrap();
        assert_eq!(padded.notes.len(), score.notes.len());
        assert_eq!(padded.notes[0].frame_length, 95);
        assert_eq!(padded.notes[4].frame_length, 95);
        assert_eq!(padded.notes[1..4], score.notes[1..4]);
        assert_eq!(score.notes[0].frame_length, 1);
        assert_eq!(score.notes[4].frame_length, 1);
    }

    #[test]
    fn truncated_audio_is_rejected_instead_of_cropping_away_the_last_note() {
        let (samples, _) = mock_round_trip(93.75, 94, true, true);
        let error = samples.unwrap_err().to_string();
        assert!(error.contains("48639 samples"), "{error}");
        assert!(error.contains("expected 48640"), "{error}");
    }

    #[test]
    fn first_beat_consonants_sing_through_a_running_voicevox_engine() {
        let Ok(url) = std::env::var("AURIS_VOICEVOX_TEST_URL") else {
            return;
        };
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "auris-voicevox-live-{}-{unique}.voicevox.json",
            std::process::id()
        ));
        let config = json!({
            "format_version": 1,
            "name": "Live test singer",
            "url": url,
            "sample_rate": 24000,
            "frame_rate": 93.75,
            "styles": [{
                "name": "Live test style",
                "query_style_id": 6000,
                "decode_style_id": 3001,
            }],
        });
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let mut model = crate::VoiceModel::load(&path, Acceleration::Auto).unwrap();
        let mut frames = SingerFrames {
            hop_seconds: 256.0 / 24000.0,
            inventory: vec!["<sil>".into(), "a".into()],
            phonemes: vec![0; 140],
            f0_hz: vec![0.0; 140],
            energy: vec![0.0; 140],
        };
        let rest = || SingerNote {
            key: None,
            frame_length: 1,
            lyric: String::new(),
        };
        let mut score = SingerScore {
            notes: vec![rest()],
        };
        for (index, (pitch, lyric)) in [(60_u8, "か"), (62, "え"), (64, "る")]
            .into_iter()
            .enumerate()
        {
            score.notes.push(SingerNote {
                key: Some(pitch),
                frame_length: 46,
                lyric: lyric.into(),
            });
            let range = 1 + index * 46..1 + (index + 1) * 46;
            frames.phonemes[range.clone()].fill(1);
            frames.f0_hz[range.clone()]
                .fill(440.0 * 2.0_f32.powf((f32::from(pitch) - 69.0) / 12.0));
            frames.energy[range].fill(0.15);
        }
        score.notes.push(rest());
        let result = model.sing_score(&frames, &score, 0, 0);
        std::fs::remove_file(path).unwrap();
        let samples = result.unwrap();
        assert_eq!(samples.len(), 140 * 256);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        let rms = (samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt();
        assert!(rms > 1.0e-5, "the live Engine returned silence: RMS {rms}");
        eprintln!(
            "VOICEVOX live score: {} samples, RMS {rms:.6}",
            samples.len()
        );
        for events in [
            vec![(1, "ア"), (25, "カ")],
            vec![(1, "カ"), (1, "キ"), (1, "ク"), (25, "ケ")],
            vec![(25, "カ"), (1, ""), (25, "キ")],
            vec![(25, "こーひー")],
            vec![(25, "シァ"), (25, "フゥ"), (25, "じぃ")],
            vec![(2, "かな")],
            vec![(25, "ひ"), (25, "か"), (25, "り")],
        ] {
            let score = test_score(&events);
            let count = score
                .notes
                .iter()
                .map(|note| note.frame_length as usize)
                .sum();
            let frames = SingerFrames {
                hop_seconds: 256.0 / 24000.0,
                inventory: vec!["a".into()],
                phonemes: vec![0; count],
                f0_hz: vec![261.62555; count],
                energy: vec![0.15; count],
            };
            let render = model
                .sing_render_with(&frames, &score, 0, 0, |_, _| true)
                .unwrap_or_else(|error| panic!("{events:?}: {error}"));
            assert_eq!(render.samples.len(), count * 256, "{events:?}");
            assert!(render.samples.iter().all(|sample| sample.is_finite()));
            assert_eq!(render.backend_pitch.unwrap().hz.len(), count);
            eprintln!("VOICEVOX live regression: {events:?}, {count} frames");
        }
    }
}
